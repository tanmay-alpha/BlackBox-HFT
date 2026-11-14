//! BlackBox HFT strategies.
//!
//! Every strategy here is a pure, deterministic state machine implementing
//! [`bb_core::strategy::Strategy`]: on each tick of the fixed local-time grid it reads a
//! [`bb_core::strategy::LocalView`] (only what had reached the strategy host by then) and appends
//! [`bb_core::strategy::Intent`]s. Strategies never touch the simulation engine, perform no I/O
//! and use no randomness, so the same sequence of views always produces the same intents.
//!
//! Conventions shared by all strategies:
//!
//! * At most one `Place` per slot per tick; the harness derives the order id from
//!   `(tick_no, slot)`.
//! * A `Cancel` is only emitted for an order that is [`cancellable`]; an order with a request in
//!   flight is looked at again on a later tick.
//! * Quantities are compared with a half-lot tolerance.
//! * Every `reason` is built from the numbers the decision used, with prices in price units,
//!   because the forensics view shows it verbatim.
//!
//! [`cancellable`]: bb_core::strategy::OpenOrder::cancellable

mod avellaneda_stoikov;
mod glft;
mod imbalance_taker;
mod quoting;
mod scripted;
mod text;
mod touch_maker;

#[cfg(test)]
mod testutil;

use bb_core::config::{StrategyConfig, StrategyKind};
use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::Strategy;

pub use avellaneda_stoikov::AvellanedaStoikov;
pub use glft::Glft;
pub use imbalance_taker::ImbalanceTaker;
pub use scripted::Scripted;
pub use touch_maker::TouchMaker;

/// Builds the strategy selected by `cfg` for `instrument`, in its initial state.
pub fn build(cfg: &StrategyConfig, instrument: &InstrumentMeta) -> Box<dyn Strategy> {
    match &cfg.kind {
        StrategyKind::TouchMaker(params) => Box::new(TouchMaker::new(*params, instrument)),
        StrategyKind::AvellanedaStoikov(params) => {
            Box::new(AvellanedaStoikov::new(*params, cfg.interval_ns, instrument))
        }
        StrategyKind::Glft(params) => Box::new(Glft::new(*params, cfg.interval_ns, instrument)),
        StrategyKind::ImbalanceTaker(params) => Box::new(ImbalanceTaker::new(*params, instrument)),
        StrategyKind::Scripted(params) => Box::new(Scripted::new(params, instrument)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        INSTRUMENT, INTERVAL_NS, market, market_with_qty, pending_cancel, pending_new, placed,
        reasons, resting, run, view,
    };
    use bb_core::config::{
        AvellanedaStoikovParams, ImbalanceTakerParams, ScriptedOrder, ScriptedParams,
        ScriptedPrice, TouchMakerParams,
    };
    use bb_core::strategy::{Intent, MarketView, OpenOrder, order_id};
    use bb_core::{OrdType, Side, Tif};

    fn config(kind: StrategyKind) -> StrategyConfig {
        StrategyConfig {
            interval_ns: INTERVAL_NS,
            kind,
        }
    }

    fn touch_maker() -> StrategyConfig {
        config(StrategyKind::TouchMaker(TouchMakerParams {
            order_qty: 1.0,
            max_position: 2.0,
            offset_ticks: 0,
            skew_ticks_per_unit: 1.0,
        }))
    }

    fn avellaneda_stoikov(vol_halflife_ticks: u32) -> StrategyConfig {
        config(StrategyKind::AvellanedaStoikov(AvellanedaStoikovParams {
            order_qty: 1.0,
            max_position: 2.0,
            gamma: 0.5,
            kappa: 50.0,
            horizon_s: 5.0,
            vol_halflife_ticks,
        }))
    }

    fn imbalance_taker() -> StrategyConfig {
        config(StrategyKind::ImbalanceTaker(ImbalanceTakerParams {
            order_qty: 1.0,
            max_position: 2.0,
            threshold: 0.5,
            hold_ticks: 2,
            cooldown_ticks: 3,
        }))
    }

    fn scripted() -> StrategyConfig {
        let order = |at_tick, slot, side, price, cancel_after_ticks, label: &str| ScriptedOrder {
            at_tick,
            slot,
            side,
            price,
            qty: 1.0,
            tif: Tif::Gtc,
            ord_type: OrdType::Limit,
            cancel_after_ticks,
            label: label.to_string(),
        };
        config(StrategyKind::Scripted(ScriptedParams {
            orders: vec![
                order(
                    0,
                    3,
                    Side::Buy,
                    ScriptedPrice::Tick { tick: 9_990 },
                    None,
                    "resting bid far from the touch",
                ),
                order(
                    5,
                    0,
                    Side::Buy,
                    ScriptedPrice::FromTouch { offset: 0 },
                    Some(1),
                    "join the bid, cancel one tick later",
                ),
                order(
                    16,
                    1,
                    Side::Sell,
                    ScriptedPrice::FromTouch { offset: 0 },
                    None,
                    "join the ask (the ask is missing on this tick)",
                ),
            ],
        }))
    }

    #[test]
    fn build_returns_the_strategy_for_every_kind() {
        let book = market_with_qty(Some(10_000), Some(10_010), 9.0, 1.0);

        // TouchMaker: two post-only quotes at the touch.
        let out = run(
            &mut *build(&touch_maker(), &INSTRUMENT),
            &view(0, book, 0.0, &[]),
        );
        assert_eq!(
            placed(&out),
            vec![
                (0, Side::Buy, 10_000, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_010, 1.0, Tif::Gtx),
            ]
        );
        assert!(reasons(&out).iter().all(|r| r.starts_with("touch_maker: ")));

        // AvellanedaStoikov with a one-tick warm-up: two post-only quotes around the mid, one
        // order-arrival half spread (ln(1 + 0.5 / 50) / 0.5 = 0.0199 -> 2 ticks) away.
        let out = run(
            &mut *build(&avellaneda_stoikov(1), &INSTRUMENT),
            &view(0, book, 0.0, &[]),
        );
        assert_eq!(
            placed(&out),
            vec![
                (0, Side::Buy, 10_003, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_007, 1.0, Tif::Gtx),
            ]
        );
        assert!(
            reasons(&out)
                .iter()
                .all(|r| r.starts_with("avellaneda_stoikov: "))
        );

        // ImbalanceTaker: one IOC at the opposite touch.
        let out = run(
            &mut *build(&imbalance_taker(), &INSTRUMENT),
            &view(0, book, 0.0, &[]),
        );
        assert_eq!(placed(&out), vec![(0, Side::Buy, 10_010, 1.0, Tif::Ioc)]);
        assert!(reasons(&out)[0].starts_with("imbalance_taker: "));

        // Scripted: exactly the scripted order, labelled with the script's text.
        let out = run(
            &mut *build(&scripted(), &INSTRUMENT),
            &view(0, book, 0.0, &[]),
        );
        assert_eq!(placed(&out), vec![(3, Side::Buy, 9_990, 1.0, Tif::Gtc)]);
        assert_eq!(reasons(&out), vec!["resting bid far from the touch"]);
    }

    #[test]
    fn avellaneda_stoikov_takes_its_interval_from_the_config() {
        // Same mid path, same parameters; only the grid step differs. A longer step means less
        // variance per second, hence a narrower spread.
        let spread = |interval_ns: i64| {
            let cfg = StrategyConfig {
                interval_ns,
                kind: StrategyKind::AvellanedaStoikov(AvellanedaStoikovParams {
                    order_qty: 1.0,
                    max_position: 2.0,
                    gamma: 1.0,
                    kappa: 1.5,
                    horizon_s: 10.0,
                    vol_halflife_ticks: 2,
                }),
            };
            let mut strategy = build(&cfg, &INSTRUMENT);
            let mut out = Vec::new();
            for tick_no in 0..4u64 {
                let shift = if tick_no % 2 == 0 { 0 } else { 10 };
                let book = market(Some(10_000 + shift), Some(10_200 + shift));
                out = run(&mut *strategy, &view(tick_no, book, 0.0, &[]));
            }
            let quotes = placed(&out);
            assert_eq!(quotes.len(), 2);
            quotes[1].2 - quotes[0].2
        };
        assert!(spread(INTERVAL_NS) > spread(10 * INTERVAL_NS));
    }

    struct Frame {
        market: MarketView,
        position: f64,
        open: Vec<OpenOrder>,
    }

    /// A fixed, irregular sequence of views: the touch wanders, sizes change, the ask disappears
    /// once, inventory comes and goes, and own orders show up in every lifecycle state.
    fn frames() -> Vec<Frame> {
        (0..60u64)
            .map(|i| {
                let drift = [0, 1, 3, 2, -1, -2, 0, 4][(i % 8) as usize];
                let best_bid = 10_000 + drift;
                let best_ask = best_bid + 2 + (i % 3) as i64;
                let market = if i % 17 == 16 {
                    market_with_qty(Some(best_bid), None, 2.0, 0.0)
                } else {
                    market_with_qty(
                        Some(best_bid),
                        Some(best_ask),
                        1.0 + (i % 7) as f64,
                        1.0 + ((i * 5) % 11) as f64,
                    )
                };
                let position = [0.0, 0.0, 1.0, 1.0, 0.0, -1.0, -2.0, 0.0][((i / 4) % 8) as usize];
                let open = match i % 5 {
                    1 => vec![pending_new(order_id(i - 1, 0), Side::Buy, best_bid - 1)],
                    2 => vec![
                        resting(order_id(i - 2, 0), Side::Buy, best_bid),
                        resting(order_id(i - 2, 1), Side::Sell, best_ask + 1),
                    ],
                    3 => vec![pending_cancel(order_id(i - 3, 1), Side::Sell, best_ask)],
                    _ => Vec::new(),
                };
                Frame {
                    market,
                    position,
                    open,
                }
            })
            .collect()
    }

    fn replay(cfg: &StrategyConfig, frames: &[Frame]) -> Vec<Vec<Intent>> {
        let mut strategy = build(cfg, &INSTRUMENT);
        frames
            .iter()
            .enumerate()
            .map(|(i, frame)| {
                run(
                    &mut *strategy,
                    &view(i as u64, frame.market, frame.position, &frame.open),
                )
            })
            .collect()
    }

    #[test]
    fn same_views_yield_identical_intents() {
        let frames = frames();
        for cfg in [
            touch_maker(),
            avellaneda_stoikov(3),
            imbalance_taker(),
            scripted(),
        ] {
            let first = replay(&cfg, &frames);
            let second = replay(&cfg, &frames);
            assert_eq!(first, second, "{} is not deterministic", cfg.kind.label());

            let all: Vec<&Intent> = first.iter().flatten().collect();
            let places = all
                .iter()
                .filter(|i| matches!(i, Intent::Place { .. }))
                .count();
            let cancels = all.len() - places;
            // The comparison must not pass vacuously on a strategy that never acted.
            assert!(places > 0, "{} placed nothing", cfg.kind.label());
            if !matches!(cfg.kind, StrategyKind::ImbalanceTaker(_)) {
                assert!(cancels > 0, "{} cancelled nothing", cfg.kind.label());
            }
        }
    }

    #[test]
    fn never_cancels_an_order_that_is_not_cancellable() {
        let frames = frames();
        for cfg in [
            touch_maker(),
            avellaneda_stoikov(3),
            imbalance_taker(),
            scripted(),
        ] {
            for (frame, intents) in frames.iter().zip(replay(&cfg, &frames)) {
                for intent in intents {
                    if let Intent::Cancel { order_id, .. } = intent {
                        let order = frame
                            .open
                            .iter()
                            .find(|o| o.order_id == order_id)
                            .unwrap_or_else(|| panic!("cancel for unknown order {order_id}"));
                        assert!(order.cancellable(), "{}: {order:?}", cfg.kind.label());
                    }
                }
            }
        }
    }

    #[test]
    fn at_most_one_place_per_slot_per_tick() {
        let frames = frames();
        for cfg in [
            touch_maker(),
            avellaneda_stoikov(3),
            imbalance_taker(),
            scripted(),
        ] {
            for intents in replay(&cfg, &frames) {
                let mut slots: Vec<u8> = placed(&intents).iter().map(|p| p.0).collect();
                let total = slots.len();
                slots.sort_unstable();
                slots.dedup();
                assert_eq!(slots.len(), total, "{}: {intents:?}", cfg.kind.label());
            }
        }
    }
}
