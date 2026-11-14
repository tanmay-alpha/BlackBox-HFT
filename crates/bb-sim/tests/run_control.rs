//! Run control: progress, cooperative cancellation and the runtime budget never change a completed
//! run, report monotonically, stop within one check interval, and decide a budget breach the same
//! way whatever the check cadence.

use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use bb_core::{
    config::{
        ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig, QueueModelConfig,
        RecordConfig, RiskConfig, RunConfig, StrategyConfig, StrategyKind, TouchMakerParams,
    },
    hash::sha256_json,
};
use bb_data::{
    BuiltDataset,
    synthetic::{SyntheticConfig, generate},
};
use bb_sim::{
    NoControl, Progress, RunBudget, RunControl, SimError, SimInput, SimOutput, simulate,
    simulate_with,
};

fn dataset() -> BuiltDataset {
    generate(&SyntheticConfig {
        id: "control".into(),
        duration_ns: 60_000_000_000,
        ..SyntheticConfig::default()
    })
    .expect("synthetic dataset generates")
}

fn config(ds: &BuiltDataset) -> RunConfig {
    let lot = ds.manifest.instrument.lot_size;
    RunConfig {
        dataset_id: "control".into(),
        strategy: StrategyConfig {
            interval_ns: 100_000_000,
            kind: StrategyKind::TouchMaker(TouchMakerParams {
                order_qty: 10.0 * lot,
                max_position: 100.0 * lot,
                offset_ticks: 0,
                skew_ticks_per_unit: 2.0 / (100.0 * lot),
            }),
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::NoPartialFill,
            queue_model: QueueModelConfig::PowerProb3 { n: 3.0 },
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: 1_000_000,
                response_ns: 1_000_000,
            },
            think_time_ns: 0,
            feed_latency_offset_ns: 0,
            fees: FeeConfig {
                maker: 0.0002,
                taker: 0.0004,
            },
        },
        risk: RiskConfig {
            max_position: 100.0 * lot,
            max_order_qty: 50.0 * lot,
            max_open_orders: 8,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 1,
    }
}

fn run(
    ds: &BuiltDataset,
    cfg: &RunConfig,
    budget: &RunBudget,
    control: &dyn RunControl,
) -> Result<SimOutput, SimError> {
    let inst = ds.manifest.instrument;
    simulate_with(
        &SimInput {
            events: &ds.events,
            instrument: inst,
            origin_ns: ds.manifest.origin().unwrap(),
            config: cfg,
        },
        bb_strategies::build(&cfg.strategy, &inst),
        budget,
        control,
    )
}

fn hash(out: &SimOutput) -> String {
    sha256_json(&(&out.tape, &out.fills, &out.equity, out.ticks))
}

/// Records every progress report; cancels once `cancel_after` reports have been seen.
struct Recorder {
    reports: Mutex<Vec<Progress>>,
    cancel_after: u64,
    polls: AtomicU64,
}

impl Recorder {
    fn new(cancel_after: u64) -> Self {
        Self {
            reports: Mutex::new(Vec::new()),
            cancel_after,
            polls: AtomicU64::new(0),
        }
    }
}

impl RunControl for Recorder {
    fn on_progress(&self, p: &Progress) {
        self.reports.lock().unwrap().push(*p);
    }

    fn should_cancel(&self) -> bool {
        self.polls.fetch_add(1, Ordering::Relaxed) + 1 >= self.cancel_after
    }
}

fn budget(every: u64) -> RunBudget {
    RunBudget {
        progress_every_ticks: every,
        ..RunBudget::local()
    }
}

#[test]
fn a_controlled_run_is_bit_identical_and_reports_monotonic_progress() {
    let ds = dataset();
    let cfg = config(&ds);
    let plain = {
        let inst = ds.manifest.instrument;
        simulate(
            &SimInput {
                events: &ds.events,
                instrument: inst,
                origin_ns: ds.manifest.origin().unwrap(),
                config: &cfg,
            },
            bb_strategies::build(&cfg.strategy, &inst),
        )
        .unwrap()
    };
    assert!(plain.ticks > 100 && !plain.fills.is_empty());

    for every in [1, 7, 1_000, u64::MAX] {
        let rec = Recorder::new(u64::MAX);
        let out = run(&ds, &cfg, &budget(every), &rec).unwrap();
        assert_eq!(hash(&out), hash(&plain), "cadence {every} changed the run");

        let reports = rec.reports.into_inner().unwrap();
        let expected = plain.ticks / every.max(1) + 1;
        assert_eq!(reports.len() as u64, expected, "cadence {every}");
        // Strictly increasing, except that the closing report repeats the last in-loop one when
        // the tick count is a multiple of the cadence.
        let (body, _) = reports.split_at(reports.len() - 1);
        for pair in body.windows(2) {
            assert!(pair[1].ticks_done > pair[0].ticks_done);
        }
        for pair in reports.windows(2) {
            assert!(pair[1].ticks_done >= pair[0].ticks_done);
            assert!(pair[1].tape_events >= pair[0].tape_events);
        }
        for p in &reports {
            assert!(p.ticks_done <= p.ticks_total, "{p:?}");
        }
        let last = reports.last().unwrap();
        assert_eq!(last.ticks_done, plain.ticks);
        assert_eq!(last.ticks_total, plain.ticks);
        assert_eq!(last.tape_events, plain.tape.len() as u64);
    }
    // The prediction used in intermediate reports is close to the real count.
    let rec = Recorder::new(u64::MAX);
    run(&ds, &cfg, &budget(10), &rec).unwrap();
    let first = rec.reports.lock().unwrap()[0];
    assert!(
        first.ticks_total >= plain.ticks && first.ticks_total <= plain.ticks + 2,
        "predicted {} ticks, ran {}",
        first.ticks_total,
        plain.ticks
    );

    let out = run(&ds, &cfg, &RunBudget::local(), &NoControl).unwrap();
    assert_eq!(hash(&out), hash(&plain));
}

#[test]
fn cancellation_stops_the_run_within_one_check_interval() {
    let ds = dataset();
    let cfg = config(&ds);
    let every = 25;
    let rec = Recorder::new(3);
    let err = run(&ds, &cfg, &budget(every), &rec).unwrap_err();
    assert!(matches!(err, SimError::Cancelled), "{err}");
    let reports = rec.reports.into_inner().unwrap();
    // Asked to cancel at the third poll: exactly three reports, the last at 3 intervals.
    assert_eq!(reports.len(), 3);
    assert_eq!(reports.last().unwrap().ticks_done, 3 * every);
}

#[test]
fn budget_breaches_abort_regardless_of_cadence() {
    let ds = dataset();
    let cfg = config(&ds);
    let full = run(&ds, &cfg, &RunBudget::local(), &NoControl).unwrap();
    let tape = full.tape.len() as u64;
    let samples = full.equity.len() as u64;

    for every in [1, 13, u64::MAX] {
        // Exactly at the limit is allowed.
        let at_limit = RunBudget {
            max_tape_events: tape,
            max_equity_samples: samples,
            progress_every_ticks: every,
        };
        let out = run(&ds, &cfg, &at_limit, &NoControl).unwrap();
        assert_eq!(hash(&out), hash(&full));

        let over_tape = RunBudget {
            max_tape_events: tape - 1,
            ..at_limit
        };
        match run(&ds, &cfg, &over_tape, &NoControl) {
            Err(SimError::BudgetExceeded { what, limit }) => {
                assert_eq!((what, limit), ("tape_events", tape - 1))
            }
            other => panic!("cadence {every}: expected a tape budget breach, got {other:?}"),
        }
        let over_samples = RunBudget {
            max_equity_samples: samples - 1,
            ..at_limit
        };
        match run(&ds, &cfg, &over_samples, &NoControl) {
            Err(SimError::BudgetExceeded { what, limit }) => {
                assert_eq!((what, limit), ("equity_samples", samples - 1))
            }
            other => panic!("cadence {every}: expected a sample budget breach, got {other:?}"),
        }
    }

    // Mid-run: a small budget stops the run long before its end.
    let rec = Recorder::new(u64::MAX);
    let small = RunBudget {
        max_tape_events: tape / 10,
        max_equity_samples: u64::MAX,
        progress_every_ticks: 10,
    };
    assert!(matches!(
        run(&ds, &cfg, &small, &rec),
        Err(SimError::BudgetExceeded {
            what: "tape_events",
            ..
        })
    ));
    let reports = rec.reports.into_inner().unwrap();
    assert!(
        reports.last().unwrap().ticks_done < full.ticks / 2,
        "{reports:?}"
    );
}
