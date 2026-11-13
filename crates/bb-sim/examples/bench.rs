//! Wall-clock benchmark of the BlackBox adapter against the stock hftbacktest engine.
//!
//! ```text
//! cargo run --release -p bb-sim --example bench -- [--reps N] [--duration-s S] [--interval-ms MS]
//!                                                  [--dataset ID]... [--no-synthetic] [--no-sweep]
//! ```
//!
//! Every measurement is one untimed warm-up followed by `--reps` timed repetitions (default 5);
//! min / median / max wall time are reported. A timed repetition covers engine construction
//! (which copies the event rows into the engine's buffer), the full replay and, for the adapter,
//! tape finalisation. Dataset generation/loading and output hashing are not timed.
//!
//! Workloads per dataset:
//!
//! * `raw engine`: the stock engine built with `L2AssetBuilder` exactly as
//!   `tests/engine_defects.rs` does (`NoPartialFillExchange`, `RiskAdverseQueueModel`,
//!   `ConstantLatency`), elapsing through the whole dataset in steps of the strategy interval
//!   with no orders. The baseline.
//! * `adapter idle`: `bb_sim::simulate` with the same execution models and a strategy that never
//!   emits anything. Isolates harness + tap overhead.
//! * `adapter <strategy> <exchange>`: `TouchMaker` and `AvellanedaStoikov`, both exchange models,
//!   `PowerProb3 { n: 3 }` queue model. Every repetition (warm-up included) is hashed with
//!   `sha256_json(&(tape, fills, equity))` and all hashes must be identical.
//! * Interval sweep (unless `--no-sweep`): raw vs idle adapter at 10 / 100 / 1000 ms, separating
//!   per-tick from per-event overhead.
//! * Queue-probe micro-benchmark: the cost of one `QueuePos` read via `Debug` formatting (what
//!   `taps.rs` does for probabilistic queue models) vs the `f64` downcast used for
//!   `RiskAdverseQueueModel`.
//!
//! Run from the repository root: stored datasets are looked up in `data/datasets/<id>`.

use std::{
    collections::BTreeSet,
    hint::black_box,
    path::Path,
    time::{Duration, Instant},
};

use bb_core::{
    config::{
        AvellanedaStoikovParams, ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig,
        QueueModelConfig, RecordConfig, RiskConfig, RunConfig, ScriptedParams, StrategyConfig,
        StrategyKind, TouchMakerParams,
    },
    dataset::InstrumentMeta,
    flight::FlightBody,
    hash::sha256_json,
    strategy::{Intent, LocalView, Strategy},
    ReqKind,
};
use bb_data::synthetic::{generate, SyntheticConfig};
use bb_sim::{simulate, Event, SimInput, SimOutput};
use hftbacktest::{
    backtest::{
        assettype::LinearAsset,
        data::{Data, DataSource},
        models::{
            CommonFees, ConstantLatency, PowerProbQueueFunc3, ProbQueueModel, QueueModel,
            QueuePos, RiskAdverseQueueModel, TradingValueFeeModel,
        },
        Backtest, ExchangeKind, L2AssetBuilder,
    },
    depth::{HashMapMarketDepth, L2MarketDepth},
    types::{Bot, ElapseResult, OrdType, Order, Side as HSide, TimeInForce, LOCAL_EVENT},
};

type Depth = HashMapMarketDepth;

const MS: i64 = 1_000_000;
/// Order latency each way, as in the bb-app presets.
const LATENCY_NS: i64 = MS;
const MAKER_FEE: f64 = 0.0002;
const TAKER_FEE: f64 = 0.0004;
/// Strategy sizing in lots, as in the bb-app presets (10 / 100 / 50 lots).
const ORDER_LOTS: f64 = 10.0;
const MAX_POSITION_LOTS: f64 = 100.0;
const MAX_ORDER_LOTS: f64 = 50.0;

// ------------------------------------------------------------------------------------------------
// CLI
// ------------------------------------------------------------------------------------------------

struct Args {
    reps: usize,
    duration_s: i64,
    interval_ms: i64,
    datasets: Vec<String>,
    synthetic: bool,
    sweep: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        reps: 5,
        duration_s: 600,
        interval_ms: 100,
        datasets: Vec::new(),
        synthetic: true,
        sweep: true,
    };
    let mut it = std::env::args().skip(1);
    let usage = "usage: bench [--reps N] [--duration-s S] [--interval-ms MS] [--dataset ID]... \
                 [--no-synthetic] [--no-sweep]";
    while let Some(flag) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .unwrap_or_else(|| panic!("{name} needs a value\n{usage}"))
        };
        match flag.as_str() {
            "--reps" => a.reps = value("--reps").parse().expect("--reps: integer"),
            "--duration-s" => {
                a.duration_s = value("--duration-s").parse().expect("--duration-s: integer")
            }
            "--interval-ms" => {
                a.interval_ms = value("--interval-ms").parse().expect("--interval-ms: integer")
            }
            "--dataset" => a.datasets.push(value("--dataset")),
            "--no-synthetic" => a.synthetic = false,
            "--no-sweep" => a.sweep = false,
            "-h" | "--help" => {
                println!("{usage}");
                std::process::exit(0);
            }
            other => panic!("unknown argument `{other}`\n{usage}"),
        }
    }
    assert!(a.reps >= 1, "--reps must be at least 1");
    assert!(a.duration_s >= 1, "--duration-s must be at least 1");
    assert!(a.interval_ms >= 1, "--interval-ms must be at least 1");
    a
}

// ------------------------------------------------------------------------------------------------
// Timing
// ------------------------------------------------------------------------------------------------

struct Stats {
    min: Duration,
    median: Duration,
    max: Duration,
}

impl Stats {
    fn from(mut v: Vec<Duration>) -> Stats {
        v.sort();
        let n = v.len();
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2
        };
        Stats {
            min: v[0],
            median,
            max: v[n - 1],
        }
    }

    /// (max - min) / median, in percent.
    fn spread_pct(&self) -> f64 {
        (self.max.as_secs_f64() - self.min.as_secs_f64()) / self.median.as_secs_f64() * 100.0
    }
}

/// One untimed warm-up, then `reps` timed runs. `inspect` sees every result (warm-up included,
/// index 0) outside the timed region.
fn measure<R>(reps: usize, mut run: impl FnMut() -> R, mut inspect: impl FnMut(usize, &R)) -> Stats {
    let warm = run();
    inspect(0, &warm);
    drop(warm);
    let mut times = Vec::with_capacity(reps);
    for i in 1..=reps {
        let t = Instant::now();
        let r = run();
        times.push(t.elapsed());
        inspect(i, &black_box(r));
    }
    Stats::from(times)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

// ------------------------------------------------------------------------------------------------
// Datasets
// ------------------------------------------------------------------------------------------------

struct Dataset {
    name: String,
    events: Vec<Event>,
    instrument: InstrumentMeta,
    origin_ns: i64,
    /// Exchange-clock span of the data, seconds.
    span_s: f64,
}

fn synthetic(duration_s: i64) -> Dataset {
    let cfg = SyntheticConfig {
        id: "bench-synthetic".into(),
        duration_ns: duration_s * 1_000_000_000,
        ..SyntheticConfig::default()
    };
    let t = Instant::now();
    let built = generate(&cfg).expect("synthetic dataset generates");
    eprintln!(
        "generated synthetic dataset ({} s, seed {}) in {:.1} ms",
        duration_s,
        cfg.seed,
        ms(t.elapsed())
    );
    let origin_ns = built.manifest.origin().expect("synthetic origin parses");
    Dataset {
        name: format!("synthetic {duration_s} s (seed {})", cfg.seed),
        span_s: span_s(&built.events),
        instrument: built.manifest.instrument,
        events: built.events,
        origin_ns,
    }
}

fn stored(id: &str) -> Option<Dataset> {
    let root = Path::new("data/datasets");
    if !root.join(id).is_dir() {
        eprintln!("dataset `{id}` not found under data/datasets; skipped");
        return None;
    }
    let t = Instant::now();
    let ds = bb_data::store::load(root, id).unwrap_or_else(|e| panic!("loading `{id}`: {e}"));
    eprintln!("loaded dataset `{id}` in {:.1} ms (hash-verified)", ms(t.elapsed()));
    let events: Vec<Event> = ds.events.as_ref().clone();
    Some(Dataset {
        name: id.to_string(),
        span_s: span_s(&events),
        instrument: ds.manifest.instrument,
        events,
        origin_ns: ds.origin_ns,
    })
}

fn span_s(events: &[Event]) -> f64 {
    let first = events.iter().map(|e| e.exch_ts).min().unwrap_or(0);
    let last = events.iter().map(|e| e.exch_ts).max().unwrap_or(0);
    (last - first) as f64 / 1e9
}

// ------------------------------------------------------------------------------------------------
// Workloads
// ------------------------------------------------------------------------------------------------

/// The stock engine, assembled exactly as in `tests/engine_defects.rs` (no BlackBox wrapper).
fn raw_engine_with<QM>(ds: &Dataset, queue_model: QM) -> Backtest<Depth>
where
    QM: QueueModel<Depth> + 'static,
{
    let inst = ds.instrument;
    let asset = L2AssetBuilder::new()
        .data(vec![DataSource::Data(Data::from_data(&ds.events))])
        .latency_model(ConstantLatency::new(LATENCY_NS, LATENCY_NS))
        .asset_type(LinearAsset::new(inst.contract_size))
        .fee_model(TradingValueFeeModel::new(CommonFees::new(MAKER_FEE, TAKER_FEE)))
        .exchange(ExchangeKind::NoPartialFillExchange)
        .queue_model(queue_model)
        .depth(move || Depth::new(inst.tick_size, inst.lot_size))
        .build()
        .expect("asset builds");
    Backtest::builder()
        .add_asset(asset)
        .build()
        .expect("backtest builds")
}

/// Elapses the raw engine (`RiskAdverseQueueModel`) to the end of data with no orders; returns
/// the number of `elapse` calls.
fn raw_replay(ds: &Dataset, interval_ns: i64) -> u64 {
    let mut hbt = raw_engine_with(ds, RiskAdverseQueueModel::<Depth>::new());
    let mut steps = 0u64;
    loop {
        steps += 1;
        if hbt.elapse(interval_ns).expect("raw elapse") == ElapseResult::EndOfData {
            return steps;
        }
    }
}

/// One request the adapter actually issued, keyed by the strategy tick it was issued at.
#[derive(Clone, Copy)]
enum Action {
    Place {
        tick: u64,
        id: u64,
        side: bb_core::Side,
        price: f64,
        qty: f64,
        tif: TimeInForce,
        ord_type: OrdType,
    },
    Cancel {
        tick: u64,
        id: u64,
    },
}

impl Action {
    fn tick(&self) -> u64 {
        match self {
            Action::Place { tick, .. } | Action::Cancel { tick, .. } => *tick,
        }
    }
}

/// The adapter's request stream, read back from its tape: every order intent that passed the
/// risk checks and every cancel intent, in tape (= issue) order.
fn actions_of(out: &SimOutput, inst: InstrumentMeta) -> Vec<Action> {
    let mut actions = Vec::new();
    for (i, e) in out.tape.iter().enumerate() {
        let id = e.order_id.unwrap_or(0);
        match &e.body {
            FlightBody::OrderIntent {
                tick_no,
                side,
                price_tick,
                qty,
                tif,
                ord_type,
                ..
            } => {
                let blocked = matches!(
                    out.tape.get(i + 1).map(|n| &n.body),
                    Some(FlightBody::RiskBlocked { .. })
                );
                if blocked {
                    continue;
                }
                actions.push(Action::Place {
                    tick: *tick_no,
                    id,
                    side: *side,
                    price: inst.price(*price_tick),
                    qty: *qty,
                    tif: match tif {
                        bb_core::Tif::Gtc => TimeInForce::GTC,
                        bb_core::Tif::Gtx => TimeInForce::GTX,
                        bb_core::Tif::Ioc => TimeInForce::IOC,
                        bb_core::Tif::Fok => TimeInForce::FOK,
                    },
                    ord_type: match ord_type {
                        bb_core::OrdType::Limit => OrdType::Limit,
                        bb_core::OrdType::Market => OrdType::Market,
                    },
                });
            }
            FlightBody::CancelIntent { tick_no, .. } => {
                actions.push(Action::Cancel { tick: *tick_no, id })
            }
            _ => {}
        }
    }
    // Tape order is already issue order; a stable sort by tick only guards that assumption.
    actions.sort_by_key(Action::tick);
    actions
}

/// Absolute local time after which the adapter stops evaluating ticks (its `data_end`).
fn data_end(ds: &Dataset) -> i64 {
    ds.events
        .iter()
        .filter(|e| e.ev & LOCAL_EVENT != 0)
        .map(|e| e.local_ts)
        .max()
        .expect("dataset has local events")
}

/// The stock engine (`NoPartialFillExchange`, `ProbQueueModel<PowerProbQueueFunc3(3)>`) driven
/// open-loop by a minimal loop that issues exactly the adapter's recorded request stream at the
/// same ticks, calling `clear_inactive_orders` each tick as any hftbacktest strategy loop does.
/// This is the engine's own cost for the same order flow, with no views, recording or risk
/// checks. Returns (ticks, engine-reported trade count).
fn raw_order_replay(ds: &Dataset, interval_ns: i64, end: i64, actions: &[Action]) -> (u64, i64) {
    let mut hbt = raw_engine_with(
        ds,
        ProbQueueModel::<_, Depth>::new(PowerProbQueueFunc3::new(3.0)),
    );
    let mut tick = 0u64;
    let mut next = 0usize;
    loop {
        if hbt.elapse(interval_ns).expect("raw elapse") == ElapseResult::EndOfData {
            break;
        }
        if hbt.current_timestamp() > end {
            break;
        }
        hbt.clear_inactive_orders(Some(0));
        while let Some(a) = actions.get(next).filter(|a| a.tick() == tick) {
            // Errors (e.g. cancelling an order that just filled) are what the adapter records as
            // refusals; the request stream is replayed regardless.
            let _ = match *a {
                Action::Place {
                    id,
                    side,
                    price,
                    qty,
                    tif,
                    ord_type,
                    ..
                } => match side {
                    bb_core::Side::Buy => {
                        hbt.submit_buy_order(0, id, price, qty, tif, ord_type, false)
                    }
                    bb_core::Side::Sell => {
                        hbt.submit_sell_order(0, id, price, qty, tif, ord_type, false)
                    }
                },
                Action::Cancel { id, .. } => hbt.cancel(0, id, false),
            };
            next += 1;
        }
        tick += 1;
    }
    (tick, hbt.state_values(0).num_trades)
}

/// Never places or cancels anything.
struct Idle;

impl Strategy for Idle {
    fn on_tick(&mut self, _view: &LocalView<'_>, _out: &mut Vec<Intent>) {}
}

fn config(
    ds: &Dataset,
    interval_ns: i64,
    kind: StrategyKind,
    exchange_model: ExchangeModel,
    queue_model: QueueModelConfig,
) -> RunConfig {
    let lot = ds.instrument.lot_size;
    RunConfig {
        dataset_id: "bench".into(),
        strategy: StrategyConfig { interval_ns, kind },
        execution: ExecutionConfig {
            exchange_model,
            queue_model,
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: LATENCY_NS,
                response_ns: LATENCY_NS,
            },
            feed_latency_offset_ns: 0,
            fees: FeeConfig {
                maker: MAKER_FEE,
                taker: TAKER_FEE,
            },
        },
        risk: RiskConfig {
            max_position: MAX_POSITION_LOTS * lot,
            max_order_qty: MAX_ORDER_LOTS * lot,
            max_open_orders: 8,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 1,
    }
}

fn idle_config(ds: &Dataset, interval_ns: i64) -> RunConfig {
    config(
        ds,
        interval_ns,
        StrategyKind::Scripted(ScriptedParams { orders: vec![] }),
        ExchangeModel::NoPartialFill,
        QueueModelConfig::RiskAdverse,
    )
}

fn touch_maker(ds: &Dataset) -> StrategyKind {
    let lot = ds.instrument.lot_size;
    StrategyKind::TouchMaker(TouchMakerParams {
        order_qty: ORDER_LOTS * lot,
        max_position: MAX_POSITION_LOTS * lot,
        offset_ticks: 0,
        skew_ticks_per_unit: 2.0 / (MAX_POSITION_LOTS * lot),
    })
}

fn avellaneda_stoikov(ds: &Dataset) -> StrategyKind {
    let lot = ds.instrument.lot_size;
    let tick = ds.instrument.tick_size;
    StrategyKind::AvellanedaStoikov(AvellanedaStoikovParams {
        order_qty: ORDER_LOTS * lot,
        max_position: MAX_POSITION_LOTS * lot,
        gamma: 0.1 / tick,
        kappa: 0.5 / tick,
        horizon_s: 1.0,
        vol_halflife_ticks: 50,
    })
}

fn run_adapter(ds: &Dataset, cfg: &RunConfig, strategy: Box<dyn Strategy>) -> SimOutput {
    simulate(
        &SimInput {
            events: &ds.events,
            instrument: ds.instrument,
            origin_ns: ds.origin_ns,
            config: cfg,
        },
        strategy,
    )
    .unwrap_or_else(|e| panic!("simulate: {e}"))
}

/// What one loaded run produced.
#[derive(Default, Clone)]
struct Shape {
    ticks: u64,
    tape: usize,
    fills: usize,
    intents: usize,
    sent: usize,
    exch_queued: usize,
    queue_updates: usize,
    queue_dropped: u64,
}

impl Shape {
    fn of(out: &SimOutput) -> Shape {
        let mut s = Shape {
            ticks: out.ticks,
            tape: out.tape.len(),
            fills: out.fills.len(),
            ..Shape::default()
        };
        for e in &out.tape {
            match &e.body {
                FlightBody::OrderIntent { .. } => s.intents += 1,
                FlightBody::RequestSent {
                    req: ReqKind::New, ..
                } => s.sent += 1,
                FlightBody::ExchQueued { .. } => s.exch_queued += 1,
                FlightBody::QueueUpdate { .. } => s.queue_updates += 1,
                FlightBody::QueueTruncated { dropped } => s.queue_dropped += dropped,
                _ => {}
            }
        }
        s
    }

    /// Exact number of queue-probe reads: one per `new_order`, two per `trade`/`depth` callback
    /// (before and after), whether the resulting update is kept or dropped by the cap.
    fn probe_reads(&self) -> u64 {
        self.exch_queued as u64 + 2 * (self.queue_updates as u64 + self.queue_dropped)
    }
}

fn output_hash(out: &SimOutput) -> String {
    sha256_json(&(&out.tape, &out.fills, &out.equity))
}

// ------------------------------------------------------------------------------------------------
// Queue-probe micro-benchmark
// ------------------------------------------------------------------------------------------------

/// Same parsing as `taps::parse_front_qty` (crate-private there).
fn parse_front_qty(debug: &str) -> Option<f64> {
    let rest = debug.split_once("front_q_qty: ")?.1;
    let end = rest.find([',', ' ', '}']).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Nanoseconds per read for the Debug-format probe and for the `f64` downcast probe.
fn probe_microbench() -> (f64, f64) {
    const N: u32 = 2_000_000;
    let mut depth = Depth::new(0.1, 0.001);
    depth.update_bid_depth(30_000.0, 2.347, 0);
    let make = || Order::new(1, 300_000, 0.1, 0.01, HSide::Buy, OrdType::Limit, TimeInForce::GTC);

    let mut prob_order = make();
    let prob = ProbQueueModel::<_, Depth>::new(PowerProbQueueFunc3::new(3.0));
    prob.new_order(&mut prob_order, &depth);
    prob.trade(&mut prob_order, 0.123, &depth);
    let read_debug = |o: &Order| {
        let pos = (*o.q).as_any().downcast_ref::<QueuePos>()?;
        parse_front_qty(&format!("{pos:?}"))
    };
    assert_eq!(read_debug(&prob_order), Some(2.347 - 0.123));

    let mut ra_order = make();
    RiskAdverseQueueModel::<Depth>::new().new_order(&mut ra_order, &depth);
    let read_f64 = |o: &Order| (*o.q).as_any().downcast_ref::<f64>().copied();
    assert_eq!(read_f64(&ra_order), Some(2.347));

    let time = |f: &dyn Fn(&Order) -> Option<f64>, o: &Order| {
        let mut best = f64::INFINITY;
        for _ in 0..5 {
            let t = Instant::now();
            let mut acc = 0.0;
            for _ in 0..N {
                acc += f(black_box(o)).unwrap_or(0.0);
            }
            black_box(acc);
            best = best.min(t.elapsed().as_nanos() as f64 / N as f64);
        }
        best
    };
    (time(&read_debug, &prob_order), time(&read_f64, &ra_order))
}

// ------------------------------------------------------------------------------------------------
// Report
// ------------------------------------------------------------------------------------------------

struct SameFlow {
    name: String,
    raw: Stats,
    raw_ticks: u64,
    raw_trades: i64,
    adapter_fills: usize,
    adapter_median: Duration,
    /// Median total time spent inside the strategy's `on_tick`.
    strategy: Duration,
    probe_ms: f64,
}

/// Accumulates the wall time spent inside the wrapped strategy.
struct Timed {
    inner: Box<dyn Strategy>,
    total: std::rc::Rc<std::cell::Cell<Duration>>,
}

impl Strategy for Timed {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        let t = Instant::now();
        self.inner.on_tick(view, out);
        self.total.set(self.total.get() + t.elapsed());
    }
}

/// Median (over `reps` extra runs, not part of any timed row) of the total time the strategy
/// spends in `on_tick` during one adapter run.
fn strategy_time(ds: &Dataset, cfg: &RunConfig, reps: usize) -> Duration {
    let times = (0..reps)
        .map(|_| {
            let total = std::rc::Rc::new(std::cell::Cell::new(Duration::ZERO));
            let strategy = Timed {
                inner: bb_strategies::build(&cfg.strategy, &ds.instrument),
                total: total.clone(),
            };
            drop(run_adapter(ds, cfg, Box::new(strategy)));
            total.get()
        })
        .collect();
    Stats::from(times).median
}

struct Row {
    name: String,
    stats: Stats,
    ticks: u64,
    shape: Option<Shape>,
    hash: Option<String>,
}

fn bench_dataset(ds: &Dataset, args: &Args, probe_ns: f64, sweep: bool) {
    let interval_ns = args.interval_ms * MS;
    let n = ds.events.len() as f64;
    println!("\n## {}\n", ds.name);
    println!(
        "{} engine rows, {:.1} s of exchange time, tick {}, lot {}, strategy interval {} ms, \
         {} timed reps after 1 warm-up\n",
        ds.events.len(),
        ds.span_s,
        ds.instrument.tick_size,
        ds.instrument.lot_size,
        args.interval_ms,
        args.reps
    );

    let mut rows: Vec<Row> = Vec::new();

    let mut steps = 0;
    let stats = measure(args.reps, || raw_replay(ds, interval_ns), |_, s| steps = *s);
    rows.push(Row {
        name: "raw engine (no orders)".into(),
        stats,
        ticks: steps,
        shape: None,
        hash: None,
    });

    let idle_cfg = idle_config(ds, interval_ns);
    let mut idle_ticks = 0;
    let stats = measure(
        args.reps,
        || run_adapter(ds, &idle_cfg, Box::new(Idle)),
        |_, out| {
            assert!(out.tape.is_empty() && out.fills.is_empty(), "idle run produced output");
            idle_ticks = out.ticks;
        },
    );
    rows.push(Row {
        name: "adapter, idle strategy".into(),
        stats,
        ticks: idle_ticks,
        shape: None,
        hash: None,
    });

    let loaded = [
        ("TouchMaker", touch_maker(ds)),
        ("AvellanedaStoikov", avellaneda_stoikov(ds)),
    ];
    let end = data_end(ds);
    // `simulate` scans every row once for the last local timestamp before it starts.
    let prescan = measure(args.reps, || data_end(black_box(ds)), |_, e| assert_eq!(*e, end));
    println!(
        "Adapter pre-scan for the last local timestamp (same scan as `simulate`): median {:.2} ms \
         ({:.2} ns/row)\n",
        ms(prescan.median),
        prescan.median.as_nanos() as f64 / n
    );
    let mut same_flow: Vec<SameFlow> = Vec::new();
    for (label, kind) in &loaded {
        for (ex_label, ex) in [
            ("no-partial", ExchangeModel::NoPartialFill),
            ("partial", ExchangeModel::PartialFill),
        ] {
            let cfg = config(ds, interval_ns, kind.clone(), ex, QueueModelConfig::PowerProb3 {
                n: 3.0,
            });
            let mut hashes: BTreeSet<String> = BTreeSet::new();
            let mut runs = 0;
            let mut shape = Shape::default();
            let mut actions: Vec<Action> = Vec::new();
            let stats = measure(
                args.reps,
                || run_adapter(ds, &cfg, bb_strategies::build(&cfg.strategy, &ds.instrument)),
                |i, out| {
                    hashes.insert(output_hash(out));
                    runs += 1;
                    shape = Shape::of(out);
                    if i == 0 {
                        actions = actions_of(out, ds.instrument);
                    }
                },
            );
            assert_eq!(
                hashes.len(),
                1,
                "{label} {ex_label}: {runs} runs produced {} distinct output hashes",
                hashes.len()
            );
            rows.push(Row {
                name: format!("adapter, {label}, {ex_label}, PowerProb3(n=3)"),
                stats,
                ticks: shape.ticks,
                shape: Some(shape.clone()),
                hash: Some(format!(
                    "{} (identical across {runs} runs)",
                    hashes.into_iter().next().unwrap()
                )),
            });
            // The raw engine with the identical request stream. Only for the no-partial model:
            // the stock `PartialFillExchange` aborts on an exact completion (engine_defects.rs).
            if ex == ExchangeModel::NoPartialFill {
                let mut raw = (0, 0);
                let raw_stats = measure(
                    args.reps,
                    || raw_order_replay(ds, interval_ns, end, &actions),
                    |_, r| raw = *r,
                );
                same_flow.push(SameFlow {
                    name: format!("{label}, no-partial, PowerProb3(n=3)"),
                    raw: raw_stats,
                    raw_ticks: raw.0,
                    raw_trades: raw.1,
                    adapter_fills: shape.fills,
                    adapter_median: rows.last().unwrap().stats.median,
                    strategy: strategy_time(ds, &cfg, args.reps),
                    probe_ms: shape.probe_reads() as f64 * probe_ns / 1e6,
                });
            }
        }
    }

    let raw_median = rows[0].stats.median.as_secs_f64();
    println!(
        "| workload | min ms | median ms | max ms | spread % | Mevents/s (median) | vs raw | ticks |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for r in &rows {
        let med = r.stats.median.as_secs_f64();
        println!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.2} | {:+.1}% | {} |",
            r.name,
            ms(r.stats.min),
            ms(r.stats.median),
            ms(r.stats.max),
            r.stats.spread_pct(),
            n / med / 1e6,
            (med / raw_median - 1.0) * 100.0,
            r.ticks
        );
    }

    println!(
        "\n| loaded workload | intents | orders sent | fills | tape events | tape Mevents/s \
         (median) | queue probe reads | est. probe ms (share of median) |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for r in rows.iter().filter(|r| r.shape.is_some()) {
        let s = r.shape.as_ref().unwrap();
        let med = r.stats.median.as_secs_f64();
        let probe_ms = s.probe_reads() as f64 * probe_ns / 1e6;
        println!(
            "| {} | {} | {} | {} | {} | {:.2} | {} | {:.1} ({:.1}%) |",
            r.name,
            s.intents,
            s.sent,
            s.fills,
            s.tape,
            s.tape as f64 / med / 1e6,
            s.probe_reads(),
            probe_ms,
            probe_ms / (med * 1e3) * 100.0
        );
    }

    println!("\nDeterminism (sha256_json(&(tape, fills, equity))):\n");
    for r in rows.iter().filter(|r| r.hash.is_some()) {
        println!("- {}: `{}`", r.name, r.hash.as_ref().unwrap());
    }

    println!(
        "\nSame request stream through the raw engine (open-loop replay of the adapter's issued \
         requests, no views / recording / risk checks):\n"
    );
    println!(
        "| workload | raw min ms | raw median ms | raw max ms | raw ticks | raw trades | adapter \
         fills | adapter median ms | adapter overhead | strategy on_tick ms | est. queue probe ms \
         | rest of adapter ms |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for f in &same_flow {
        let (raw, adapter) = (ms(f.raw.median), ms(f.adapter_median));
        let strategy = ms(f.strategy);
        println!(
            "| {} | {:.1} | {raw:.1} | {:.1} | {} | {} | {} | {adapter:.1} | {:+.1}% | {strategy:.1} \
             | {:.1} | {:.1} |",
            f.name,
            ms(f.raw.min),
            ms(f.raw.max),
            f.raw_ticks,
            f.raw_trades,
            f.adapter_fills,
            (adapter / raw - 1.0) * 100.0,
            f.probe_ms,
            adapter - raw - strategy - f.probe_ms
        );
    }

    if sweep {
        println!("\nInterval sweep, idle adapter vs raw engine (median ms):\n");
        println!("| interval ms | raw ticks | raw ms | idle ticks | idle ms | overhead ms | overhead % |");
        println!("|---:|---:|---:|---:|---:|---:|---:|");
        for interval_ms in [10_i64, 100, 1_000] {
            let iv = interval_ms * MS;
            let mut raw_steps = 0;
            let raw = measure(args.reps, || raw_replay(ds, iv), |_, s| raw_steps = *s);
            let cfg = idle_config(ds, iv);
            let mut ticks = 0;
            let idle = measure(
                args.reps,
                || run_adapter(ds, &cfg, Box::new(Idle)),
                |_, out| ticks = out.ticks,
            );
            let (r, i) = (ms(raw.median), ms(idle.median));
            println!(
                "| {interval_ms} | {raw_steps} | {r:.1} | {ticks} | {i:.1} | {:.1} | {:+.1}% |",
                i - r,
                (i / r - 1.0) * 100.0
            );
        }
    }
}

fn main() {
    let args = parse_args();
    println!("# BlackBox bench\n");
    println!(
        "build: {} profile, hftbacktest 0.9.4, logical CPUs reported by std: {}",
        if cfg!(debug_assertions) { "debug" } else { "release" },
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );

    let (debug_ns, f64_ns) = probe_microbench();
    println!(
        "\nQueue probe micro-benchmark (best of 5 x 2,000,000 reads): Debug-format `QueuePos` \
         read {debug_ns:.1} ns, `f64` downcast read {f64_ns:.2} ns"
    );
    let timer_ns = {
        const N: u32 = 1_000_000;
        let t = Instant::now();
        for _ in 0..N {
            black_box(Instant::now());
        }
        t.elapsed().as_nanos() as f64 / N as f64
    };
    println!(
        "`Instant::now()` cost: {timer_ns:.1} ns (the strategy timing wrapper reads the clock \
         twice per tick)"
    );

    if args.synthetic {
        let ds = synthetic(args.duration_s);
        bench_dataset(&ds, &args, debug_ns, args.sweep);
    }
    for id in &args.datasets {
        if let Some(ds) = stored(id) {
            bench_dataset(&ds, &args, debug_ns, args.sweep);
        }
    }
}
