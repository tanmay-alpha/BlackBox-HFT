//! Imbalance taker: crosses the spread when the top of the local book is lopsided and flattens
//! after a fixed holding time.

use bb_core::config::ImbalanceTakerParams;
use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::{Intent, LocalView, Strategy};
use bb_core::{OrdType, Side, Tif};

use crate::text::{Text, verb};

const NAME: &str = "imbalance_taker";

/// Slot of an entry order placed on a tick.
const ENTRY_SLOT: u8 = 0;
/// Slot of an exit order placed on a tick.
const EXIT_SLOT: u8 = 1;

/// Slot 0 is the entry, slot 1 the exit; both are `Ioc` limit orders priced at the opposite
/// touch, so they take what is displayed there and never rest.
///
/// * While any own order is open (in flight or working) nothing is done: its outcome decides the
///   next step.
/// * Flat, cooldown elapsed, both sides present: buy `order_qty` at the best ask when
///   `imbalance > threshold`, sell at the best bid when `imbalance < -threshold`.
/// * Holding a position for `hold_ticks` ticks since the entry: send an IOC for the whole
///   position at the opposite touch, and again on later ticks while any of it remains.
///
/// "Flat" and "position" are what the strategy host knows from fill notifications that have
/// arrived, which is the only information a real strategy would have.
#[derive(Clone, Debug)]
pub struct ImbalanceTaker {
    params: ImbalanceTakerParams,
    text: Text,
    /// Tick on which the latest entry order was sent.
    entry_tick: Option<u64>,
    /// An entry was sent and the position it may have opened has not been seen flat since. While
    /// set, the holding time is measured from `entry_tick`.
    exit_due: bool,
}

impl ImbalanceTaker {
    pub fn new(params: ImbalanceTakerParams, instrument: &InstrumentMeta) -> Self {
        Self {
            params,
            text: Text::new(instrument),
            entry_tick: None,
            exit_due: false,
        }
    }

    fn try_enter(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        let p = &self.params;
        let text = &self.text;
        if let Some(entry_tick) = self.entry_tick
            && view.tick_no.saturating_sub(entry_tick) < u64::from(p.cooldown_ticks)
        {
            return;
        }
        let market = &view.market;
        let (Some(imbalance), Some(best_bid), Some(best_ask)) = (
            market.imbalance(),
            market.best_bid_tick,
            market.best_ask_tick,
        ) else {
            return;
        };
        let (side, price_tick, direction, touch, relation, threshold) = if imbalance > p.threshold {
            (
                Side::Buy,
                best_ask,
                "long",
                "best ask",
                "above",
                p.threshold,
            )
        } else if imbalance < -p.threshold {
            (
                Side::Sell,
                best_bid,
                "short",
                "best bid",
                "below",
                -p.threshold,
            )
        } else {
            return;
        };
        if side.sign() * view.position + p.order_qty > p.max_position + text.half_lot() {
            // The entry alone would exceed the position limit.
            return;
        }
        out.push(Intent::Place {
            slot: ENTRY_SLOT,
            side,
            price_tick,
            qty: p.order_qty,
            tif: Tif::Ioc,
            ord_type: OrdType::Limit,
            reason: format!(
                "{NAME}: enter {direction}: {} {} at {touch} {} (IOC) because top-of-book \
                 imbalance {imbalance:+.3} = (bid qty {} - ask qty {}) / (bid qty + ask qty) is \
                 {relation} threshold {threshold:+.3}",
                verb(side),
                text.qty(p.order_qty),
                text.px(price_tick),
                text.qty(market.best_bid_qty),
                text.qty(market.best_ask_qty),
            ),
        });
        self.entry_tick = Some(view.tick_no);
        self.exit_due = true;
    }

    fn try_exit(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        let p = &self.params;
        let text = &self.text;
        // A position this strategy did not open through a tracked entry has no holding time to
        // wait for and is flattened straight away.
        let held = match (self.exit_due, self.entry_tick) {
            (true, Some(entry_tick)) => Some((entry_tick, view.tick_no.saturating_sub(entry_tick))),
            _ => None,
        };
        if let Some((_, held_ticks)) = held
            && held_ticks < u64::from(p.hold_ticks)
        {
            return;
        }
        let (side, touch_tick, direction, touch) = if view.position > 0.0 {
            (Side::Sell, view.market.best_bid_tick, "long", "best bid")
        } else {
            (Side::Buy, view.market.best_ask_tick, "short", "best ask")
        };
        let Some(price_tick) = touch_tick else {
            // Nothing displayed to trade against; the exit is retried on the next tick.
            return;
        };
        let lots = text.lots(view.position.abs());
        if lots <= 0 {
            // A residual below one lot cannot be traded.
            return;
        }
        let qty = lots as f64 * text.lot_size();
        let timing = match held {
            Some((entry_tick, _)) => format!(
                "holding time reached: entry at tick {entry_tick}, now tick {}, hold_ticks {}",
                view.tick_no, p.hold_ticks,
            ),
            None => "no tracked entry, so there is no holding time to wait for".to_string(),
        };
        out.push(Intent::Place {
            slot: EXIT_SLOT,
            side,
            price_tick,
            qty,
            tif: Tif::Ioc,
            ord_type: OrdType::Limit,
            reason: format!(
                "{NAME}: exit {direction} position {}: {} {} at {touch} {} (IOC), {timing}",
                text.qty(view.position),
                verb(side),
                text.qty(qty),
                text.px(price_tick),
            ),
        });
    }
}

impl Strategy for ImbalanceTaker {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        if !view.open_orders.is_empty() {
            return;
        }
        if view.position.abs() < self.text.half_lot() {
            self.exit_due = false;
            self.try_enter(view, out);
        } else {
            self.try_exit(view, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        INSTRUMENT, market, market_with_qty, pending_new, placed, reasons, run, view,
    };
    use bb_core::strategy::{MarketView, order_id};

    fn taker(hold_ticks: u32, cooldown_ticks: u32) -> ImbalanceTaker {
        ImbalanceTaker::new(
            ImbalanceTakerParams {
                order_qty: 1.0,
                max_position: 5.0,
                threshold: 0.5,
                hold_ticks,
                cooldown_ticks,
            },
            &INSTRUMENT,
        )
    }

    /// Imbalance +0.8.
    fn bid_heavy() -> MarketView {
        market_with_qty(Some(10_000), Some(10_010), 9.0, 1.0)
    }

    /// Imbalance -0.8.
    fn ask_heavy() -> MarketView {
        market_with_qty(Some(10_000), Some(10_010), 1.0, 9.0)
    }

    fn balanced() -> MarketView {
        market(Some(10_000), Some(10_010))
    }

    #[test]
    fn enters_on_imbalance() {
        let mut s = taker(3, 0);
        assert!(run(&mut s, &view(0, balanced(), 0.0, &[])).is_empty());

        let out = run(&mut s, &view(1, bid_heavy(), 0.0, &[]));
        assert_eq!(placed(&out), vec![(0, Side::Buy, 10_010, 1.0, Tif::Ioc)]);
        assert!(matches!(
            out[0],
            Intent::Place {
                ord_type: OrdType::Limit,
                ..
            }
        ));
        assert_eq!(
            reasons(&out)[0],
            "imbalance_taker: enter long: buy 1.000 at best ask 100.10 (IOC) because \
             top-of-book imbalance +0.800 = (bid qty 9.000 - ask qty 1.000) / (bid qty + ask \
             qty) is above threshold +0.500"
        );

        let out = run(&mut taker(3, 0), &view(0, ask_heavy(), 0.0, &[]));
        assert_eq!(placed(&out), vec![(0, Side::Sell, 10_000, 1.0, Tif::Ioc)]);
        assert!(reasons(&out)[0].contains("imbalance -0.800"));
        assert!(reasons(&out)[0].contains("below threshold -0.500"));
    }

    #[test]
    fn does_not_enter_at_or_below_the_threshold_or_on_a_one_sided_book() {
        let mut s = taker(3, 0);
        // (3 - 1) / 4 = 0.5 is not above the threshold.
        let at_threshold = market_with_qty(Some(10_000), Some(10_010), 3.0, 1.0);
        assert!(run(&mut s, &view(0, at_threshold, 0.0, &[])).is_empty());
        let no_ask = market_with_qty(Some(10_000), None, 9.0, 0.0);
        assert!(run(&mut s, &view(1, no_ask, 0.0, &[])).is_empty());
    }

    #[test]
    fn exits_after_hold_ticks_and_repeats_until_flat() {
        let mut s = taker(3, 0);
        let entry = run(&mut s, &view(0, bid_heavy(), 0.0, &[]));
        assert_eq!(placed(&entry).len(), 1);

        // Entry still in flight: nothing happens even though the imbalance persists.
        let in_flight = [pending_new(order_id(0, 0), Side::Buy, 10_010)];
        assert!(run(&mut s, &view(1, bid_heavy(), 0.0, &in_flight)).is_empty());

        // Filled; held 2 of 3 ticks.
        assert!(run(&mut s, &view(2, bid_heavy(), 1.0, &[])).is_empty());

        // Held 3 ticks: sell the whole position at the best bid.
        let exit = run(&mut s, &view(3, bid_heavy(), 1.0, &[]));
        assert_eq!(placed(&exit), vec![(1, Side::Sell, 10_000, 1.0, Tif::Ioc)]);
        assert_eq!(
            reasons(&exit)[0],
            "imbalance_taker: exit long position 1.000: sell 1.000 at best bid 100.00 (IOC), \
             holding time reached: entry at tick 0, now tick 3, hold_ticks 3"
        );

        // Exit in flight.
        let exit_in_flight = [pending_new(order_id(3, 1), Side::Sell, 10_000)];
        assert!(run(&mut s, &view(4, bid_heavy(), 1.0, &exit_in_flight)).is_empty());

        // The exit only partially filled: the remainder is sent again.
        let retry = run(&mut s, &view(5, bid_heavy(), 0.4, &[]));
        let orders = placed(&retry);
        assert_eq!(orders.len(), 1);
        assert_eq!(
            (orders[0].0, orders[0].1, orders[0].2),
            (1, Side::Sell, 10_000)
        );
        assert!((orders[0].3 - 0.4).abs() < 1e-12);

        // No bid to sell into: wait.
        let no_bid = market_with_qty(None, Some(10_010), 0.0, 1.0);
        assert!(run(&mut s, &view(6, no_bid, 0.4, &[])).is_empty());

        // Flat again and balanced: nothing to do.
        assert!(run(&mut s, &view(7, balanced(), 0.0, &[])).is_empty());
    }

    #[test]
    fn short_position_is_bought_back_at_the_best_ask() {
        let mut s = taker(2, 0);
        let entry = run(&mut s, &view(0, ask_heavy(), 0.0, &[]));
        assert_eq!(placed(&entry), vec![(0, Side::Sell, 10_000, 1.0, Tif::Ioc)]);
        assert!(run(&mut s, &view(1, balanced(), -1.0, &[])).is_empty());
        let exit = run(&mut s, &view(2, balanced(), -1.0, &[]));
        assert_eq!(placed(&exit), vec![(1, Side::Buy, 10_010, 1.0, Tif::Ioc)]);
    }

    #[test]
    fn honours_cooldown() {
        let mut s = taker(3, 5);
        assert_eq!(
            placed(&run(&mut s, &view(0, bid_heavy(), 0.0, &[]))).len(),
            1
        );
        // The entry expired unfilled; the signal persists but the cooldown blocks re-entry.
        for tick_no in 1..5 {
            assert!(run(&mut s, &view(tick_no, bid_heavy(), 0.0, &[])).is_empty());
        }
        let out = run(&mut s, &view(5, bid_heavy(), 0.0, &[]));
        assert_eq!(placed(&out), vec![(0, Side::Buy, 10_010, 1.0, Tif::Ioc)]);
    }

    #[test]
    fn respects_max_position() {
        let mut s = ImbalanceTaker::new(
            ImbalanceTakerParams {
                order_qty: 2.0,
                max_position: 1.0,
                threshold: 0.5,
                hold_ticks: 3,
                cooldown_ticks: 0,
            },
            &INSTRUMENT,
        );
        assert!(run(&mut s, &view(0, bid_heavy(), 0.0, &[])).is_empty());
        assert!(run(&mut s, &view(1, ask_heavy(), 0.0, &[])).is_empty());
    }
}
