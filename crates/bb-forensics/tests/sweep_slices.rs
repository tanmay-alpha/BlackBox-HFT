//! A partitioned sweep end to end on generated data: admission, slicing into in-sample and
//! out-of-sample datasets, simulation of every planned run, per-block objective values and the
//! registry report.

use bb_core::{
    compare::GridAxis,
    config::{
        ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig, QueueModelConfig,
        RecordConfig, RiskConfig, RunConfig, StrategyConfig, StrategyKind, TouchMakerParams,
    },
    sweep::{
        Direction, Objective, ObjectiveMetric, Partition, Segment, SweepDesign, SweepLimits,
        SweepSpec, TimeWindow,
    },
};
use bb_data::{
    slice::{SliceProvenance, slice},
    store,
    synthetic::{SyntheticConfig, generate},
};
use bb_forensics::{
    Availability, SweepCellOutcome, analyze_sweep, build_orders, compute_metrics, objective_blocks,
};
use bb_sim::{SimInput, simulate};

const MS: i64 = 1_000_000;
const BLOCKS: u32 = 4;

fn base(dataset_id: &str) -> RunConfig {
    RunConfig {
        dataset_id: dataset_id.into(),
        strategy: StrategyConfig {
            interval_ns: 10 * MS,
            kind: StrategyKind::TouchMaker(TouchMakerParams {
                order_qty: 0.01,
                max_position: 0.05,
                offset_ticks: 0,
                skew_ticks_per_unit: 0.0,
            }),
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::PartialFill,
            queue_model: QueueModelConfig::RiskAdverse,
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: MS,
                response_ns: MS,
            },
            feed_latency_offset_ns: 0,
            fees: FeeConfig {
                maker: 0.0,
                taker: 0.0005,
            },
        },
        risk: RiskConfig {
            max_position: 0.05,
            max_order_qty: 0.01,
            max_open_orders: 4,
            max_drawdown: None,
        },
        record: RecordConfig {
            sample_every_ticks: 10,
            ..RecordConfig::default()
        },
        seed: 3,
    }
}

#[test]
fn partitioned_sweep_runs_on_slices_and_reports() {
    let root = std::env::temp_dir().join(format!("bb-sweep-slices-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let built = generate(&SyntheticConfig {
        duration_ns: 6_000 * MS,
        ..Default::default()
    })
    .unwrap();
    let saved = store::save(&root, &built).unwrap();
    let parent = store::load(&root, &saved.id).unwrap();
    let m = &parent.manifest;

    let in_sample = TimeWindow {
        start: 0,
        end: 4_000 * MS,
    };
    let out_of_sample = TimeWindow {
        start: 4_000 * MS,
        end: m.end + 1,
    };
    let spec = SweepSpec {
        base: base(&m.id),
        design: SweepDesign::Grid {
            axes: vec![
                GridAxis::OrderLatencyNs {
                    values: vec![MS, 5 * MS, 20 * MS],
                },
                GridAxis::QueueModel {
                    values: vec![QueueModelConfig::RiskAdverse, QueueModelConfig::LogProb],
                },
            ],
        },
        partition: Some(Partition {
            in_sample,
            out_of_sample,
        }),
        objective: Objective {
            metric: ObjectiveMetric::NetPnl,
            direction: Direction::Maximize,
        },
        cscv_blocks: Some(BLOCKS),
    };
    let plan = spec.admit(m, &SweepLimits::local()).unwrap();
    assert_eq!(plan.runs.len(), 12);

    let is_slice = slice(&parent, in_sample).unwrap();
    let oos_slice = slice(&parent, out_of_sample).unwrap();
    for (s, w) in [(&is_slice, in_sample), (&oos_slice, out_of_sample)] {
        assert_eq!(SliceProvenance::of(&s.manifest).unwrap().window, w);
    }

    let mut outcomes: Vec<SweepCellOutcome> = plan
        .cells
        .iter()
        .map(|c| SweepCellOutcome {
            cell: c.index,
            in_sample: None,
            out_of_sample: None,
            blocks: None,
            error: None,
        })
        .collect();
    for run in &plan.runs {
        let (data, window) = match run.segment {
            Segment::InSample => (&is_slice, in_sample),
            Segment::OutOfSample => (&oos_slice, out_of_sample),
            Segment::Full => unreachable!("partitioned"),
        };
        // The planned dataset id is the slice's own id.
        assert_eq!(run.config.dataset_id, data.manifest.id);
        let out = simulate(
            &SimInput {
                events: &data.events,
                instrument: data.manifest.instrument,
                origin_ns: parent.origin_ns,
                config: &run.config,
            },
            bb_strategies::build(&run.config.strategy, &data.manifest.instrument),
        )
        .expect("simulate slice");
        // Nothing the run recorded precedes its window: no look-ahead into earlier data, and
        // the out-of-sample run starts after the in-sample window ends.
        assert!(out.equity.iter().all(|e| e.ts >= window.start));
        assert!(out.tape.iter().all(|e| e.ts >= window.start));
        let orders = build_orders(&out.tape);
        let metrics = compute_metrics(
            &out.tape,
            &orders,
            &out.fills,
            &out.equity,
            &data.manifest.instrument,
        );
        let value = ObjectiveMetric::NetPnl.value(&metrics);
        let o = &mut outcomes[run.cell as usize];
        match run.segment {
            Segment::InSample => {
                let blocks =
                    objective_blocks(ObjectiveMetric::NetPnl, &out.equity, window, BLOCKS).unwrap();
                let sum: f64 = blocks.iter().sum();
                assert!(
                    (sum - metrics.net_pnl).abs() <= 1e-9 * metrics.net_pnl.abs().max(1.0),
                    "blocks {blocks:?} sum to {sum}, net_pnl {}",
                    metrics.net_pnl
                );
                o.in_sample = value;
                o.blocks = Some(blocks);
            }
            _ => o.out_of_sample = value,
        }
    }
    // The fixture produces fills: the cells differ, so the ranking is not a tie artefact.
    let distinct: std::collections::BTreeSet<u64> = outcomes
        .iter()
        .map(|o| o.in_sample.unwrap().to_bits())
        .collect();
    assert!(distinct.len() > 1, "{outcomes:?}");
    let report = analyze_sweep(&spec, &outcomes).unwrap();
    assert_eq!(report.configurations.tried, 6);
    assert_eq!(report.configurations.completed_in_sample, 6);
    assert_eq!(report.configurations.completed_out_of_sample, 6);
    let selected = report.selected.unwrap();
    assert!(selected.out_of_sample_rank.is_some());
    match &report.pbo {
        Availability::Available(p) => {
            assert_eq!((p.cells, p.blocks, p.combinations), (6, BLOCKS, 6));
            assert!((0.0..=1.0).contains(&p.pbo));
        }
        Availability::Unavailable { reason } => panic!("{reason}"),
    }
    // Same inputs, same report.
    assert_eq!(analyze_sweep(&spec, &outcomes).unwrap(), report);
    let _ = std::fs::remove_dir_all(&root);
}
