//! Reference reconciliation against the unwrapped engine (audit AT-13).
//!
//! An adapter run's request stream (the order intents that passed the risk checks and every
//! cancel, read back from its tape) is replayed open-loop through the stock hftbacktest engine at
//! the same ticks. The engine must report as many trades as the adapter recorded fills: the
//! adapter observes the engine, it does not change what the engine matches. This is the check
//! `examples/bench.rs` makes on every benchmark run, run here on every `cargo test`.
//!
//! Only the no-partial-fill exchange is compared: the stock `PartialFillExchange` aborts on an
//! exact completion (pinned in `engine_defects.rs`), which the adapter works around.

#[path = "support/raw_engine.rs"]
mod raw_engine;

use bb_core::config::{
    AvellanedaStoikovParams, ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig,
    QueueModelConfig, RecordConfig, RiskConfig, RunConfig, StrategyConfig, StrategyKind,
    TouchMakerParams,
};
use bb_data::synthetic::{SyntheticConfig, generate};
use bb_sim::{SimInput, simulate};
use raw_engine::{RawSetup, actions_of, data_end, raw_order_replay};

const LATENCY_NS: i64 = 1_000_000;
const MAKER_FEE: f64 = 0.0002;
const TAKER_FEE: f64 = 0.0004;
const INTERVAL_NS: i64 = 100_000_000;
const POWER_N: f64 = 3.0;

fn config(kind: StrategyKind, lot: f64) -> RunConfig {
    RunConfig {
        dataset_id: "parity".into(),
        strategy: StrategyConfig {
            interval_ns: INTERVAL_NS,
            kind,
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::NoPartialFill,
            queue_model: QueueModelConfig::PowerProb3 { n: POWER_N },
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
            max_position: 100.0 * lot,
            max_order_qty: 50.0 * lot,
            max_open_orders: 8,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 1,
    }
}

#[test]
fn raw_engine_on_the_same_request_stream_reports_the_same_trades() {
    let built = generate(&SyntheticConfig {
        id: "parity".into(),
        duration_ns: 120_000_000_000,
        ..SyntheticConfig::default()
    })
    .expect("synthetic dataset generates");
    let inst = built.manifest.instrument;
    let origin_ns = built.manifest.origin().expect("origin parses");
    let (lot, tick) = (inst.lot_size, inst.tick_size);

    let strategies = [
        (
            "touch_maker",
            StrategyKind::TouchMaker(TouchMakerParams {
                order_qty: 10.0 * lot,
                max_position: 100.0 * lot,
                offset_ticks: 0,
                skew_ticks_per_unit: 2.0 / (100.0 * lot),
            }),
        ),
        (
            "avellaneda_stoikov",
            StrategyKind::AvellanedaStoikov(AvellanedaStoikovParams {
                order_qty: 10.0 * lot,
                max_position: 100.0 * lot,
                gamma: 0.1 / tick,
                kappa: 0.5 / tick,
                horizon_s: 1.0,
                vol_halflife_ticks: 50,
            }),
        ),
    ];
    let setup = RawSetup {
        events: &built.events,
        instrument: inst,
        latency_ns: LATENCY_NS,
        maker_fee: MAKER_FEE,
        taker_fee: TAKER_FEE,
    };
    for (name, kind) in strategies {
        let cfg = config(kind, lot);
        let out = simulate(
            &SimInput {
                events: &built.events,
                instrument: inst,
                origin_ns,
                config: &cfg,
            },
            bb_strategies::build(&cfg.strategy, &inst),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let actions = actions_of(&out, inst);
        assert!(
            !actions.is_empty(),
            "{name}: the adapter issued no requests"
        );
        assert!(
            !out.fills.is_empty(),
            "{name}: the adapter recorded no fills"
        );

        let (raw_ticks, raw_trades) = raw_order_replay(
            &setup,
            POWER_N,
            INTERVAL_NS,
            data_end(&built.events),
            &actions,
        );
        assert_eq!(raw_ticks, out.ticks, "{name}: tick counts differ");
        assert_eq!(
            raw_trades,
            out.fills.len() as i64,
            "{name}: the stock engine reports {raw_trades} trades for the adapter's request \
             stream, the adapter recorded {} fills",
            out.fills.len()
        );
    }
}
