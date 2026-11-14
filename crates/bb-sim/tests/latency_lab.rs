//! Latency laboratory: seeded log-normal and heavy-tailed order latency, distinct cancel latency
//! and strategy think time, end to end through the adapter.
mod common;

use bb_core::{
    OrderStatus, ReqKind, Side, Tif,
    config::{CubicLeg, LogNormalLeg, OrderLatencyConfig, RunConfig, StrategyKind},
    flight::FlightBody,
    hash::sha256_json,
    strategy::order_id,
};
use bb_data::synthetic::{SyntheticConfig, generate};
use bb_sim::{SimInput, SimOutput, simulate};
use common::{
    Act, INTERVAL, MS, Mkt, RESPONSE, Script, buy, check_causality, config, find, of, run_with,
    tick_ts,
};

fn log_normal() -> OrderLatencyConfig {
    OrderLatencyConfig::LogNormal {
        entry: LogNormalLeg {
            median_ns: 2 * MS,
            sigma: 0.8,
        },
        response: LogNormalLeg {
            median_ns: MS,
            sigma: 0.4,
        },
        cancel: Some(LogNormalLeg {
            median_ns: MS / 2,
            sigma: 0.0,
        }),
    }
}

fn heavy_tail() -> OrderLatencyConfig {
    let leg = CubicLeg {
        min_ns: MS,
        jitter: 0.5,
        clip: 0.1,
        unit: 10.0,
    };
    OrderLatencyConfig::HeavyTail {
        entry: leg,
        response: leg,
        cancel: None,
    }
}

/// A busy 20 s synthetic market quoted by the touch maker every 10 ms.
fn busy_run(latency: OrderLatencyConfig, think_ns: i64, seed: u64) -> (SimOutput, RunConfig) {
    let built = generate(&SyntheticConfig {
        id: "latency-lab".into(),
        duration_ns: 20_000_000_000,
        ..SyntheticConfig::default()
    })
    .expect("synthetic dataset generates");
    let inst = built.manifest.instrument;
    let mut cfg = config();
    cfg.strategy.interval_ns = INTERVAL;
    cfg.strategy.kind = StrategyKind::TouchMaker(bb_core::config::TouchMakerParams {
        order_qty: 10.0 * inst.lot_size,
        max_position: 100.0 * inst.lot_size,
        offset_ticks: 0,
        skew_ticks_per_unit: 2.0 / (100.0 * inst.lot_size),
    });
    cfg.risk.max_position = 100.0 * inst.lot_size;
    cfg.risk.max_order_qty = 50.0 * inst.lot_size;
    cfg.risk.max_open_orders = 8;
    cfg.execution.order_latency = latency;
    cfg.execution.think_time_ns = think_ns;
    cfg.seed = seed;
    let out = simulate(
        &SimInput {
            events: &built.events,
            instrument: inst,
            origin_ns: built.manifest.origin().expect("origin"),
            config: &cfg,
        },
        bb_strategies::build(&cfg.strategy, &inst),
    )
    .expect("simulation succeeds");
    (out, cfg)
}

fn hash(out: &SimOutput) -> String {
    sha256_json(&(&out.tape, &out.fills, &out.equity))
}

/// Entry latencies drawn for new orders and for cancels.
fn entry_latencies(out: &SimOutput, kind: ReqKind) -> Vec<i64> {
    out.tape
        .iter()
        .filter_map(|e| match e.body {
            FlightBody::RequestSent {
                req,
                entry_latency_ns,
                ..
            } if req == kind => Some(entry_latency_ns),
            _ => None,
        })
        .collect()
}

#[test]
fn seeded_models_are_deterministic_and_seed_sensitive() {
    for latency in [log_normal(), heavy_tail()] {
        let (a, _) = busy_run(latency, 0, 5);
        let (b, _) = busy_run(latency, 0, 5);
        let (c, _) = busy_run(latency, 0, 6);
        assert!(!a.fills.is_empty(), "{latency:?}: no fills");
        assert_eq!(
            hash(&a),
            hash(&b),
            "{latency:?}: same seed, different result"
        );
        assert_ne!(
            entry_latencies(&a, ReqKind::New),
            entry_latencies(&c, ReqKind::New),
            "{latency:?}: the seed changed nothing"
        );
        assert_ne!(hash(&a), hash(&c));
    }
}

#[test]
fn every_run_is_causal_with_think_time_and_stochastic_latency() {
    for latency in [log_normal(), heavy_tail()] {
        for think in [0, 3 * MS] {
            let (out, cfg) = busy_run(latency, think, 21);
            assert!(!out.fills.is_empty());
            check_causality(&out, &cfg);
            for (seq, e) in out.tape.iter().enumerate() {
                assert_eq!(e.seq, seq as u64);
                assert_eq!(e.clock, e.body.clock());
                assert_eq!(e.source, e.body.source());
            }
        }
    }
}

/// The drawn latencies on a real run follow the configured legs: cancels use their own
/// (here constant 0.5 ms) leg, new orders the 2 ms-median entry leg, and the heavy tail stays
/// inside ABIDES' bounds.
#[test]
fn recorded_latencies_follow_the_configured_legs() {
    let (out, _) = busy_run(log_normal(), 0, 3);
    let cancels = entry_latencies(&out, ReqKind::Cancel);
    assert!(!cancels.is_empty());
    assert!(cancels.iter().all(|&l| l == MS / 2), "{cancels:?}");
    let mut news = entry_latencies(&out, ReqKind::New);
    assert!(news.len() > 200, "{} new orders", news.len());
    news.sort_unstable();
    let median = news[news.len() / 2] as f64;
    // Sample median of a log-normal with median 2 ms and sigma 0.8: within 15% at n > 200
    // (standard error about 1.25 * sigma / sqrt(n) on the log scale, under 7%).
    assert!(
        (median / (2.0 * MS as f64) - 1.0).abs() < 0.15,
        "median {median}"
    );

    let (out, _) = busy_run(heavy_tail(), 0, 3);
    for kind in [ReqKind::New, ReqKind::Cancel] {
        let l = entry_latencies(&out, kind);
        assert!(!l.is_empty());
        assert!(
            l.iter().all(|&l| (1_050_000..=51_000_000).contains(&l)),
            "{l:?}"
        );
    }
}

/// Think time moves the send, not the decision: the strategy is evaluated on the same grid and
/// shown the same market, and its order reaches the exchange `think + entry` after the tick.
/// With a trade-through 5 ms after the first tick, 2 ms of think time still arrives in time
/// (13 ms) and fills; 6 ms arrives at 17 ms and does not.
#[test]
fn think_time_delays_the_request_not_the_decision() {
    let mut m = Mkt::standard();
    m.trade(15 * MS, Side::Sell, 99.5, 1.0).keepalive(80 * MS);
    let mut baseline_views = None;
    for (think, fills) in [(0, 1), (2 * MS, 1), (6 * MS, 0)] {
        let mut cfg = config();
        cfg.execution.order_latency = OrderLatencyConfig::Constant {
            entry_ns: MS,
            response_ns: RESPONSE,
        };
        cfg.execution.think_time_ns = think;
        let script = Script::new(vec![(0, buy(0, 100.0, 1.0, Tif::Gtc))]);
        let seen = script.seen.clone();
        let out = run_with(&m, &cfg, script);
        let ev = of(&out, order_id(0, 0));
        assert_eq!(find(&ev, "order_intent")[0].ts, tick_ts(0));
        assert_eq!(find(&ev, "request_sent")[0].ts, tick_ts(0) + think);
        assert_eq!(find(&ev, "exch_queued")[0].ts, tick_ts(0) + think + MS);
        assert_eq!(out.fills.len(), fills, "think time {think}");
        check_causality(&out, &cfg);

        // Same grid, same information at every decision before the fill could be known.
        let views: Vec<_> = seen.borrow().iter().take(2).copied().collect();
        assert_eq!(views[0].1, tick_ts(0));
        assert_eq!(views[1].1, tick_ts(1));
        match &baseline_views {
            None => baseline_views = Some(views),
            Some(base) => assert_eq!(&views[0], &base[0], "think time {think}"),
        }
    }
}

/// A cancel decided on a tick also waits for the think time; meanwhile the order can fill, and
/// the cancel then loses the race.
#[test]
fn think_time_applies_to_cancels() {
    let mut m = Mkt::standard();
    // The bid rests from 11 ms; a sell sweeps it at 23 ms.
    m.trade(23 * MS, Side::Sell, 99.5, 10.0).keepalive(80 * MS);
    let acts = || {
        vec![
            (0, buy(0, 100.0, 0.1, Tif::Gtc)),
            (
                1,
                Act::Cancel {
                    order_id: order_id(0, 0),
                },
            ),
        ]
    };
    let outcome = |think: i64| {
        let mut cfg = config();
        cfg.execution.think_time_ns = think;
        let out = run_with(&m, &cfg, Script::new(acts()));
        check_causality(&out, &cfg);
        let ev = of(&out, order_id(0, 0));
        let cancel_sent = ev
            .iter()
            .find(|e| {
                matches!(
                    e.body,
                    FlightBody::RequestSent {
                        req: ReqKind::Cancel,
                        ..
                    }
                )
            })
            .map(|e| e.ts);
        let cancelled = ev.iter().any(|e| {
            matches!(
                e.body,
                FlightBody::ExchResponse {
                    status: OrderStatus::Canceled,
                    request_rejected: false,
                    ..
                }
            )
        });
        (
            cancel_sent,
            cancelled,
            out.fills.len(),
            out.engine_check.engine_num_trades,
        )
    };
    // No think time: the cancel leaves at 20 ms, arrives at 21 ms, before the sweep.
    assert_eq!(outcome(0), (Some(tick_ts(1)), true, 0, 0));
    // 2.5 ms: it leaves at 22.5 ms and arrives at 23.5 ms; the order filled at 23 ms, so the
    // exchange rejects the cancel. The fill notice only reaches the host at 24 ms.
    assert_eq!(
        outcome(2_500_000),
        (Some(tick_ts(1) + 2_500_000), false, 1, 1)
    );
    // 5 ms: the fill notice (24 ms) arrives while the host is still thinking, and the cancel it
    // decided at 20 ms leaves at 25 ms anyway, as decided. The exchange rejects it with the
    // order's status, Filled, and the engine's local account applies that fill a second time
    // (docs/EXECUTION_MODEL.md, defect 6): two engine trades for one execution. BlackBox's ledger
    // records one fill.
    assert_eq!(outcome(5 * MS), (Some(tick_ts(1) + 5 * MS), false, 1, 2));
}
