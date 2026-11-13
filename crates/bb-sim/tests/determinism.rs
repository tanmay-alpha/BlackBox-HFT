//! Repeated runs of one configuration must produce the same tape, fills and equity, including
//! when a single market event fills several orders (where the engine's internal iteration order
//! is not reproducible) and when latency is jittered.

mod common;

use bb_core::{
    Side, Tif,
    config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig},
    hash::sha256_json,
    strategy::order_id,
};
use common::*;

fn crowded_market() -> Mkt {
    let mut m = Mkt::standard();
    // Far below every resting buy: the exchange checks all of them in one step. The gap between
    // the trade price and the best ask is much larger than the number of orders, which is the
    // condition under which 0.9.4 iterates its order HashMap directly.
    m.trade(40 * MS, Side::Sell, 90.0, 1.0)
        // And far above every resting sell.
        .trade(60 * MS, Side::Buy, 112.0, 1.0)
        .keepalive(150 * MS);
    m
}

fn crowd() -> Vec<(u64, Act)> {
    let mut acts = Vec::new();
    // Eight buys over three prices (several share a price level) and eight sells likewise.
    for slot in 0..8u8 {
        let px = [100.0, 99.5, 99.0][slot as usize % 3];
        acts.push((0, buy(slot, px, 1.0 + slot as f64, Tif::Gtc)));
        let px = [101.0, 101.5, 102.0][slot as usize % 3];
        acts.push((1, sell(slot, px, 1.0 + slot as f64, Tif::Gtc)));
    }
    acts
}

fn fingerprint(out: &bb_sim::SimOutput) -> String {
    sha256_json(&(&out.tape, &out.fills, &out.equity))
}

#[test]
fn simultaneous_fills_are_recorded_in_a_reproducible_order() {
    let m = crowded_market();
    for exchange_model in [ExchangeModel::NoPartialFill, ExchangeModel::PartialFill] {
        for order_latency in [
            OrderLatencyConfig::Constant {
                entry_ns: ENTRY,
                response_ns: RESPONSE,
            },
            OrderLatencyConfig::Jitter {
                entry_ns: ENTRY,
                response_ns: RESPONSE,
                jitter_ns: 3 * MS,
            },
        ] {
            let mut cfg = config();
            cfg.execution.exchange_model = exchange_model;
            cfg.execution.order_latency = order_latency;
            cfg.execution.queue_model = QueueModelConfig::PowerProb3 { n: 3.0 };

            let first = run(&m, &cfg, crowd());
            // All sixteen orders fill: eight in the 40 ms step, eight in the 60 ms step.
            assert_eq!(first.fills.len(), 16);
            let at_40: Vec<u64> = first
                .fills
                .iter()
                .filter(|f| f.ts == 40 * MS)
                .map(|f| f.order_id)
                .collect();
            // Within one engine step, fills are committed in ascending order id.
            assert_eq!(at_40, (0..8u8).map(|s| order_id(0, s)).collect::<Vec<_>>());
            // Running position is therefore reproducible too: 1, 1+2, 1+2+3, ...
            let positions: Vec<f64> = first.fills[..8].iter().map(|f| f.position_after).collect();
            assert_eq!(positions, [1.0, 3.0, 6.0, 10.0, 15.0, 21.0, 28.0, 36.0]);

            let want = fingerprint(&first);
            // Every run builds fresh HashMaps with fresh random hash seeds, so repeating in one
            // process exercises different internal iteration orders.
            for i in 0..40 {
                let again = run(&m, &cfg, crowd());
                assert_eq!(
                    fingerprint(&again),
                    want,
                    "run {i} differs ({exchange_model:?}, {order_latency:?})"
                );
            }
        }
    }
}

#[test]
fn seed_changes_jittered_latency_and_nothing_else_is_hidden() {
    let m = crowded_market();
    let mut cfg = config();
    cfg.execution.order_latency = OrderLatencyConfig::Jitter {
        entry_ns: ENTRY,
        response_ns: RESPONSE,
        jitter_ns: 3 * MS,
    };
    let a = fingerprint(&run(&m, &cfg, crowd()));
    cfg.seed += 1;
    let b = fingerprint(&run(&m, &cfg, crowd()));
    assert_ne!(a, b, "a different seed must change jittered latencies");

    // With constant latency the seed has nothing to act on.
    let mut cfg = config();
    let a = fingerprint(&run(&m, &cfg, crowd()));
    cfg.seed += 1;
    let b = fingerprint(&run(&m, &cfg, crowd()));
    assert_eq!(a, b);
}

/// The strategy tick grid is anchored to the dataset, not to execution assumptions, which is what
/// makes an order id name the same decision point in two scenarios.
#[test]
fn tick_grid_does_not_depend_on_execution_assumptions() {
    let m = crowded_market();
    let ticks = |entry_ns, offset| {
        let mut cfg = config();
        cfg.execution.order_latency = OrderLatencyConfig::Constant {
            entry_ns,
            response_ns: RESPONSE,
        };
        cfg.execution.feed_latency_offset_ns = offset;
        let script = Script::new(crowd());
        let seen = script.seen.clone();
        run_with(&m, &cfg, script);
        let v: Vec<(u64, i64)> = seen.borrow().iter().map(|s| (s.0, s.1)).collect();
        v
    };
    let base = ticks(ENTRY, 0);
    assert_eq!(base[0], (0, tick_ts(0)));
    let slow = ticks(25 * MS, 0);
    assert_eq!(base, slow);
    // A feed offset lengthens the run (later data arrives later) but never moves a tick.
    let delayed = ticks(ENTRY, 7 * MS);
    assert_eq!(base[..], delayed[..base.len()]);
}
