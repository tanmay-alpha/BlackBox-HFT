use super::*;
use crate::{
    config::{
        ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig, RecordConfig, RiskConfig,
        StrategyConfig, StrategyKind, TouchMakerParams,
    },
    dataset::{
        DataLevel, DatasetKind, EventCounts, InstrumentMeta, LocalTsProvenance, QualityReport,
        SourceInfo, TimestampInfo,
    },
    time::{NS_PER_MS, NS_PER_S, TimeUnit},
};

const DATASET: &str = "synthetic-demo";

fn base() -> RunConfig {
    RunConfig {
        dataset_id: DATASET.into(),
        strategy: StrategyConfig {
            interval_ns: 10 * NS_PER_MS,
            kind: StrategyKind::TouchMaker(TouchMakerParams {
                order_qty: 1.0,
                max_position: 5.0,
                offset_ticks: 0,
                skew_ticks_per_unit: 0.0,
            }),
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::NoPartialFill,
            queue_model: QueueModelConfig::RiskAdverse,
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: NS_PER_MS,
                response_ns: NS_PER_MS,
            },
            feed_latency_offset_ns: 0,
            think_time_ns: 0,
            fees: FeeConfig {
                maker: 0.0,
                taker: 0.0002,
            },
        },
        risk: RiskConfig {
            max_position: 5.0,
            max_order_qty: 1.0,
            max_open_orders: 4,
            max_drawdown: None,
        },
        record: RecordConfig {
            sample_every_ticks: 10,
            ..RecordConfig::default()
        },
        seed: 1,
    }
}

/// A 120 s dataset of `rows` engine rows.
fn manifest(rows: u64) -> DatasetManifest {
    DatasetManifest {
        contract_version: crate::CONTRACT_VERSION,
        id: DATASET.into(),
        title: "demo".into(),
        kind: DatasetKind::Synthetic,
        level: DataLevel::L2MbpWithTrades,
        source: SourceInfo {
            vendor: "blackbox-synthetic".into(),
            exchange: "synthetic".into(),
            symbol: "SYNTH".into(),
            inputs: vec![],
            license_note: String::new(),
            exch_ts_semantics: String::new(),
        },
        instrument: InstrumentMeta {
            tick_size: 0.1,
            lot_size: 0.001,
            contract_size: 1.0,
        },
        origin_ns: "1700000000000000000".into(),
        start: 0,
        end: 120 * NS_PER_S,
        timestamps: TimestampInfo {
            input_unit: TimeUnit::Nanos,
            local_ts: LocalTsProvenance::Synthetic,
            local_ts_shift_ns: 0,
            note: String::new(),
        },
        counts: EventCounts {
            rows,
            ..EventCounts::default()
        },
        events_file: "events.npz".into(),
        events_sha256: "ab".repeat(32),
        events_bytes: 1,
        generator: None,
        quality: QualityReport {
            checks: vec![],
            feed_latency_ns: None,
            spread_ticks: None,
            inter_event_gap_ns: None,
            book_valid_fraction: 1.0,
        },
    }
}

fn objective() -> Objective {
    Objective {
        metric: ObjectiveMetric::NetPnl,
        direction: Direction::Maximize,
    }
}

fn grid(axes: Vec<GridAxis>) -> SweepSpec {
    SweepSpec {
        base: base(),
        design: SweepDesign::Grid { axes },
        partition: None,
        objective: objective(),
        cscv_blocks: None,
    }
}

fn random(samples: u32, seed: u64, params: Vec<RandomParam>) -> SweepSpec {
    SweepSpec {
        design: SweepDesign::Random {
            samples,
            seed,
            params,
        },
        ..grid(vec![])
    }
}

fn latency_set(values: &[i64]) -> RandomParam {
    RandomParam::OrderLatencyNs {
        domain: IntDomain::Set {
            values: values.to_vec(),
        },
    }
}

fn queue_set() -> RandomParam {
    RandomParam::QueueModel {
        values: vec![
            QueueModelConfig::RiskAdverse,
            QueueModelConfig::LogProb,
            QueueModelConfig::PowerProb { n: 2.0 },
        ],
    }
}

fn maker_uniform() -> RandomParam {
    RandomParam::MakerFee {
        domain: FloatDomain::Uniform {
            min: -0.0001,
            max: 0.0001,
        },
    }
}

fn values(cells: &[SweepCell], param: &str) -> Vec<serde_json::Value> {
    cells
        .iter()
        .map(|c| {
            c.params
                .iter()
                .find(|p| p.param == param)
                .expect("param present")
                .value
                .clone()
        })
        .collect()
}

fn partition(is: (i64, i64), oos: (i64, i64)) -> Partition {
    Partition {
        in_sample: TimeWindow {
            start: is.0 * NS_PER_S,
            end: is.1 * NS_PER_S,
        },
        out_of_sample: TimeWindow {
            start: oos.0 * NS_PER_S,
            end: oos.1 * NS_PER_S,
        },
    }
}

#[test]
fn grid_cells_are_row_major_and_apply_the_axes() {
    let x = GridAxis::OrderLatencyNs {
        values: vec![NS_PER_MS, 5 * NS_PER_MS, 20 * NS_PER_MS],
    };
    let y = GridAxis::QueueModel {
        values: vec![QueueModelConfig::RiskAdverse, QueueModelConfig::LogProb],
    };
    let spec = grid(vec![x.clone(), y.clone()]);
    assert!(spec.validate().is_empty(), "{:?}", spec.validate());
    let cells = spec.cells().unwrap();
    assert_eq!(cells.len(), 6);
    for (i, c) in cells.iter().enumerate() {
        let (xi, yi) = (i % 3, i / 3);
        assert_eq!(c.index as usize, i);
        assert_eq!(c.coords, vec![xi as u32, yi as u32]);
        assert_eq!(c.config, y.apply(&x.apply(&base(), xi), yi));
        assert_eq!(c.params[0].label, x.value_label(xi));
        assert_eq!(c.params[1].label, y.value_label(yi));
        assert_eq!(
            c.runs,
            vec![SweepRun {
                segment: Segment::Full,
                window: None
            }]
        );
    }
    assert_eq!(cells[1].params[0].value, serde_json::json!(5_000_000));
    assert_eq!(
        cells[3].params[1].value,
        serde_json::json!({"type": "log_prob"})
    );
}

#[test]
fn grid_rejects_repeats_overlaps_and_bad_shapes_before_generating() {
    let lat = |v: Vec<i64>| GridAxis::OrderLatencyNs { values: v };
    let repeated = grid(vec![lat(vec![NS_PER_MS, NS_PER_MS])]);
    assert!(repeated.validate()[0].contains("repeats a value"));

    let overlap = grid(vec![
        lat(vec![NS_PER_MS]),
        GridAxis::EntryLatencyNs {
            values: vec![NS_PER_MS],
        },
    ]);
    assert!(
        overlap
            .validate()
            .iter()
            .any(|e| e.contains("both set the entry latency"))
    );
    assert!(grid(vec![]).validate()[0].contains("one or two axes"));

    // 10^6 values: refused by the cell count alone, as the only message.
    let huge = grid(vec![lat((0..1_000_000).collect())]);
    assert_eq!(huge.cells().unwrap_err().len(), 1);
    assert!(huge.validate()[0].contains("1000000 configurations"));

    // An empty axis makes the product zero; the other axis is never scanned value by value.
    let empty = grid(vec![lat((0..1_000_000).collect()), lat(vec![])]);
    assert!(empty.validate()[0].contains("at least one value"));
}

#[test]
fn random_design_is_deterministic_and_seed_sensitive() {
    let params = || {
        vec![
            latency_set(&[0, NS_PER_MS, 2 * NS_PER_MS]),
            queue_set(),
            maker_uniform(),
        ]
    };
    let a = random(12, 42, params()).cells().unwrap();
    let b = random(12, 42, params()).cells().unwrap();
    assert_eq!(a, b);
    assert_eq!(
        random(12, 42, params()).fingerprint(),
        random(12, 42, params()).fingerprint()
    );
    let c = random(12, 43, params()).cells().unwrap();
    assert_ne!(values(&a, "maker_fee"), values(&c, "maker_fee"));
    assert_ne!(
        random(12, 42, params()).fingerprint(),
        random(12, 43, params()).fingerprint()
    );
    for cell in &a {
        assert!(cell.coords.is_empty());
        let fee = cell.config.execution.fees.maker;
        assert!((-0.0001..0.0001).contains(&fee), "{fee}");
    }
    // Pinned: the first draws of seed 42 never change (the generator is the project's own).
    let first: Vec<i64> = values(&a, "order_latency_ns")
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    let again: Vec<i64> = values(&b, "order_latency_ns")
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(first, again);
    let ms = NS_PER_MS;
    assert_eq!(
        first,
        vec![
            0,
            ms,
            0,
            ms,
            ms,
            2 * ms,
            2 * ms,
            2 * ms,
            0,
            2 * ms,
            2 * ms,
            0
        ]
    );
    assert_eq!(
        values(&a, "maker_fee")[0],
        serde_json::json!(0.00004296663243763721)
    );
}

#[test]
fn adding_a_continuous_parameter_keeps_the_discrete_draws() {
    let discrete = || {
        vec![
            latency_set(&[0, NS_PER_MS, 2 * NS_PER_MS, 3 * NS_PER_MS]),
            queue_set(),
        ]
    };
    let without = random(8, 7, discrete()).cells().unwrap();
    let mut with_fee = discrete();
    with_fee.push(maker_uniform());
    let with = random(8, 7, with_fee).cells().unwrap();
    assert_eq!(
        values(&without, "order_latency_ns"),
        values(&with, "order_latency_ns")
    );
    assert_eq!(
        values(&without, "queue_model"),
        values(&with, "queue_model")
    );
}

#[test]
fn discrete_combinations_are_drawn_without_replacement() {
    // 2 latencies x 3 queue models = 6 combinations; 6 samples must be all of them.
    let spec = random(6, 9, vec![latency_set(&[0, NS_PER_MS]), queue_set()]);
    let cells = spec.cells().unwrap();
    let combos: BTreeSet<String> = cells
        .iter()
        .map(|c| format!("{:?}", (&c.params[0].value, &c.params[1].value)))
        .collect();
    assert_eq!(combos.len(), 6);

    // A seventh would repeat one.
    let too_many = random(7, 9, vec![latency_set(&[0, NS_PER_MS]), queue_set()]);
    assert!(too_many.validate()[0].contains("only 6 combinations"));

    // Integer ranges count their grid points: 0, 1, 2 ms.
    let range = RandomParam::FeedLatencyOffsetNs {
        domain: IntDomain::Range {
            min: 0,
            max: 2 * NS_PER_MS + 5,
            step: NS_PER_MS,
        },
    };
    let cells = random(3, 1, vec![range.clone()]).cells().unwrap();
    let mut got: Vec<i64> = values(&cells, "feed_latency_offset_ns")
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    got.sort_unstable();
    assert_eq!(got, vec![0, NS_PER_MS, 2 * NS_PER_MS]);
    assert!(random(4, 1, vec![range]).validate()[0].contains("only 3 combinations"));
}

#[test]
fn with_a_continuous_parameter_every_combination_is_used_before_any_repeats() {
    let spec = random(
        7,
        3,
        vec![latency_set(&[0, NS_PER_MS, 2 * NS_PER_MS]), maker_uniform()],
    );
    let cells = spec.cells().unwrap();
    let lat: Vec<i64> = values(&cells, "order_latency_ns")
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    for pass in [&lat[0..3], &lat[3..6]] {
        let set: BTreeSet<i64> = pass.iter().copied().collect();
        assert_eq!(set.len(), 3, "{lat:?}");
    }
    let fees: BTreeSet<u64> = cells
        .iter()
        .map(|c| c.config.execution.fees.maker.to_bits())
        .collect();
    assert_eq!(fees.len(), 7);
}

#[test]
fn random_domains_are_validated() {
    let bad = random(
        2,
        1,
        vec![
            RandomParam::EntryLatencyNs {
                domain: IntDomain::Range {
                    min: 5,
                    max: 1,
                    step: 0,
                },
            },
            RandomParam::TakerFee {
                domain: FloatDomain::Uniform { min: 1.0, max: 1.0 },
            },
            RandomParam::QueueModel {
                values: vec![QueueModelConfig::LogProb, QueueModelConfig::LogProb],
            },
        ],
    );
    let e = bad.validate();
    assert!(
        e.iter().any(|m| m.contains("step must be positive")),
        "{e:?}"
    );
    assert!(
        e.iter().any(|m| m.contains("min must not exceed max")),
        "{e:?}"
    );
    assert!(e.iter().any(|m| m.contains("finite min < max")), "{e:?}");
    assert!(e.iter().any(|m| m.contains("must be distinct")), "{e:?}");

    let twice = random(2, 1, vec![queue_set(), queue_set()]);
    assert!(twice.validate().iter().any(|m| m.contains("swept twice")));

    // Out-of-range draws are caught by the cell's own RunConfig validation.
    let fee = random(
        3,
        1,
        vec![RandomParam::MakerFee {
            domain: FloatDomain::Set {
                values: vec![0.0, 0.5],
            },
        }],
    );
    let e = random(2, 1, fee_param(vec![0.0, 0.5])).validate();
    assert!(e.iter().any(|m| m.starts_with("cell ")), "{e:?}");
    assert!(fee.validate()[0].contains("only 2 combinations"));
}

fn fee_param(values: Vec<f64>) -> Vec<RandomParam> {
    vec![RandomParam::MakerFee {
        domain: FloatDomain::Set { values },
    }]
}

#[test]
fn partition_forbids_look_ahead_and_overlap() {
    let lat = GridAxis::OrderLatencyNs {
        values: vec![NS_PER_MS, 2 * NS_PER_MS],
    };
    let with = |p: Partition| SweepSpec {
        partition: Some(p),
        ..grid(vec![lat.clone()])
    };
    let ok = with(partition((0, 80), (80, 120)));
    assert!(ok.validate().is_empty(), "{:?}", ok.validate());
    let cells = ok.cells().unwrap();
    assert_eq!(
        cells[0].runs,
        vec![
            SweepRun {
                segment: Segment::InSample,
                window: Some(ok.partition.unwrap().in_sample),
            },
            SweepRun {
                segment: Segment::OutOfSample,
                window: Some(ok.partition.unwrap().out_of_sample),
            },
        ]
    );
    // A gap (embargo) between the two is allowed.
    assert!(with(partition((0, 60), (70, 120))).validate().is_empty());

    for (is, oos) in [
        ((0, 80), (79, 120)),
        ((60, 120), (0, 60)),
        ((0, 120), (0, 120)),
    ] {
        let e = with(partition(is, oos)).validate();
        assert!(
            e.iter()
                .any(|m| m.contains("out_of_sample must start at or after")),
            "{is:?} {oos:?}: {e:?}"
        );
    }
    let e = with(partition((10, 10), (20, 30))).validate();
    assert!(e[0].contains("in_sample must have start < end"));
}

#[test]
fn admission_plans_windowed_runs_on_derived_datasets() {
    let spec = SweepSpec {
        partition: Some(partition((0, 80), (80, 120))),
        ..grid(vec![GridAxis::OrderLatencyNs {
            values: vec![NS_PER_MS, 2 * NS_PER_MS, 3 * NS_PER_MS],
        }])
    };
    let parent = manifest(60_000);
    let plan = spec.admit(&parent, &SweepLimits::hosted()).unwrap();
    assert_eq!(plan.fingerprint, spec.fingerprint());
    assert_eq!(plan.cells.len(), 3);
    assert_eq!(plan.runs.len(), 6);
    let is = spec.partition.unwrap().in_sample;
    let oos = spec.partition.unwrap().out_of_sample;
    for (i, run) in plan.runs.iter().enumerate() {
        assert_eq!(run.cell as usize, i / 2);
        let w = if i % 2 == 0 { is } else { oos };
        assert_eq!(run.window, Some(w));
        assert_eq!(
            run.config.dataset_id,
            w.slice_dataset_id(&parent.id, &parent.events_sha256)
        );
        // Everything but the dataset is the cell's configuration.
        let mut back = run.config.clone();
        back.dataset_id = DATASET.into();
        assert_eq!(back, plan.cells[i / 2].config);
        assert_eq!(
            run.estimate.ticks,
            (w.len_ns() / (10 * NS_PER_MS)) as u64 + 1
        );
    }
    assert_eq!(plan.total.runs, 6);
    assert_eq!(
        plan.total.ticks,
        plan.runs.iter().map(|r| r.estimate.ticks).sum::<u64>()
    );

    // Windows must lie inside the dataset.
    let outside = SweepSpec {
        partition: Some(partition((0, 80), (80, 121))),
        ..spec.clone()
    };
    match outside.admit(&parent, &SweepLimits::hosted()) {
        Err(SweepRefusal::Invalid(e)) => assert!(e[0].contains("outside"), "{e:?}"),
        other => panic!("{other:?}"),
    }
    // And the sweep must be admitted on its own dataset.
    let other = DatasetManifest {
        id: "other".into(),
        ..parent.clone()
    };
    assert!(matches!(
        spec.admit(&other, &SweepLimits::hosted()),
        Err(SweepRefusal::Invalid(_))
    ));
}

#[test]
fn admission_refuses_sweeps_over_budget() {
    let lat = |n: i64| GridAxis::OrderLatencyNs {
        values: (1..=n).map(|i| i * NS_PER_MS).collect(),
    };
    let parent = manifest(60_000);

    // 30 cells: over the hosted cap of 25, under the absolute cap of 100.
    let wide = grid(vec![lat(30)]);
    assert!(wide.validate().is_empty());
    let refusal = wide.admit(&parent, &SweepLimits::hosted()).unwrap_err();
    assert_eq!(
        refusal,
        SweepRefusal::TooManyCells {
            cells: 30,
            limit: 25
        }
    );
    assert_eq!(
        refusal.budget_details(),
        Some(("sweep_cells".to_string(), 25, 30))
    );
    assert!(wide.admit(&parent, &SweepLimits::local()).is_ok());

    // One run over the per-run limit: 600,000 rows exceed hosted max_dataset_rows.
    let big = manifest(600_000);
    match grid(vec![lat(2)]).admit(&big, &SweepLimits::hosted()) {
        Err(SweepRefusal::RunOverBudget {
            cell: 0,
            segment: Segment::Full,
            violation,
        }) => assert_eq!(violation.what, "dataset_rows"),
        other => panic!("{other:?}"),
    }

    // Every run within its limit, the sum over the sweep total. 100,000 rows: a touch maker run
    // predicts 100,000·5/4 + 12,001/2 + 64 = 131,064 tape events (limit 150,000), three runs
    // 393,192 against a total of 2 × 150,000.
    let busy = manifest(100_000);
    let tight = SweepLimits::from_execution(ExecutionLimits::hosted(), 2);
    let three = SweepLimits {
        max_cells: 3,
        ..tight
    };
    match grid(vec![lat(3)]).admit(&busy, &three) {
        Err(refusal @ SweepRefusal::TotalOverBudget { .. }) => {
            let (what, limit, predicted) = refusal.budget_details().unwrap();
            assert_eq!(what, "sweep_total_tape_events");
            assert_eq!(limit, 2 * ExecutionLimits::hosted().max_tape_events);
            assert!(predicted > limit);
            assert!(refusal.to_string().contains("sweep limit"), "{refusal}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn cscv_blocks_need_an_additive_objective_and_long_enough_blocks() {
    let lat = GridAxis::OrderLatencyNs {
        values: vec![NS_PER_MS, 2 * NS_PER_MS],
    };
    let spec = |blocks: u32, metric: ObjectiveMetric, p: Option<Partition>| SweepSpec {
        cscv_blocks: Some(blocks),
        partition: p,
        objective: Objective {
            metric,
            direction: Direction::Maximize,
        },
        ..grid(vec![lat.clone()])
    };
    assert!(
        spec(
            8,
            ObjectiveMetric::NetPnl,
            Some(partition((0, 80), (80, 120)))
        )
        .validate()
        .is_empty()
    );
    for blocks in [2, 5, 18] {
        assert!(spec(blocks, ObjectiveMetric::NetPnl, None).validate()[0].contains("even"));
    }
    let e = spec(8, ObjectiveMetric::MaxDrawdown, None).validate();
    assert!(e[0].contains("time-additive"), "{e:?}");

    // Equity is sampled every 10 ticks of 10 ms = 100 ms; 16 blocks of 1 s windows are 62.5 ms.
    let short = Partition {
        in_sample: TimeWindow {
            start: 0,
            end: NS_PER_S,
        },
        out_of_sample: TimeWindow {
            start: NS_PER_S,
            end: 2 * NS_PER_S,
        },
    };
    let e = spec(16, ObjectiveMetric::NetPnl, Some(short)).validate();
    assert!(e[0].contains("shorter than one equity sample"), "{e:?}");
}

#[test]
fn spec_json_round_trips_and_fingerprint_is_sensitive() {
    let spec = SweepSpec {
        partition: Some(partition((0, 80), (80, 120))),
        cscv_blocks: Some(8),
        ..random(5, 11, vec![queue_set(), maker_uniform()])
    };
    let json = serde_json::to_value(&spec).unwrap();
    let back: SweepSpec = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, spec);
    assert_eq!(back.fingerprint(), spec.fingerprint());
    assert_eq!(spec.fingerprint().len(), 64);
    assert_eq!(json["design"]["type"], "random");
    assert_eq!(json["design"]["params"][1]["domain"]["kind"], "uniform");

    let mut changed = spec.clone();
    changed.objective.direction = Direction::Minimize;
    assert_ne!(changed.fingerprint(), spec.fingerprint());

    // Optional fields may be omitted; unknown fields are refused.
    let mut minimal = json.clone();
    minimal.as_object_mut().unwrap().remove("partition");
    minimal.as_object_mut().unwrap().remove("cscv_blocks");
    let parsed: SweepSpec = serde_json::from_value(minimal.clone()).unwrap();
    assert_eq!(parsed.partition, None);
    minimal["typo"] = serde_json::json!(1);
    assert!(serde_json::from_value::<SweepSpec>(minimal).is_err());
}

#[test]
fn windows_split_exactly_and_name_their_slices() {
    let w = TimeWindow { start: 10, end: 33 };
    let blocks = w.blocks(4);
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0].start, 10);
    assert_eq!(blocks[3].end, 33);
    for pair in blocks.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }
    assert_eq!(
        blocks.iter().map(TimeWindow::len_ns).collect::<Vec<_>>(),
        vec![5, 6, 6, 6]
    );

    let id = w.slice_dataset_id("synthetic-demo", &"ab".repeat(32));
    assert_eq!(id, w.slice_dataset_id("synthetic-demo", &"ab".repeat(32)));
    assert_ne!(id, w.slice_dataset_id("synthetic-demo", &"cd".repeat(32)));
    assert!(id.starts_with("synthetic-demo-w-"));
    let long = w.slice_dataset_id(&"x".repeat(64), "00");
    assert!(long.len() <= 64);
    assert!(
        long.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    );
}

#[test]
fn objective_metrics_extract_and_orient() {
    let o = Objective {
        metric: ObjectiveMetric::MaxDrawdown,
        direction: Direction::Minimize,
    };
    assert_eq!(o.oriented(2.0), -2.0);
    assert!(!ObjectiveMetric::PnlPerTurnoverBps.is_time_additive());
    assert!(ObjectiveMetric::NetPnl.is_time_additive());
    let sample = EquitySample {
        ts: 0,
        tick_no: 0,
        mid_tick: None,
        position: 0.0,
        known_position: 0.0,
        cash: 0.0,
        fees: 0.5,
        realized_pnl: 2.0,
        unrealized_pnl: 1.0,
        equity: 2.5,
        open_orders: 0,
    };
    assert_eq!(ObjectiveMetric::NetPnl.cumulative(&sample), Some(2.5));
    assert_eq!(ObjectiveMetric::Fees.cumulative(&sample), Some(0.5));
    assert_eq!(ObjectiveMetric::FillCount.cumulative(&sample), None);
}
