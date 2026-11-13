//! Scripted strategy: places exactly the listed orders, for curated forensics demos and tests.

use bb_core::Side;
use bb_core::config::{ScriptedOrder, ScriptedParams, ScriptedPrice};
use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::{Intent, LocalView, Strategy, order_id};

use crate::text::{Text, verb};

/// A placed order whose script asks for a cancel and which has not been seen gone yet.
#[derive(Copy, Clone, Debug)]
struct PendingCancel {
    /// Index into [`Scripted::orders`].
    index: usize,
    order_id: u64,
    /// The script's `cancel_after_ticks`.
    after: u64,
}

/// Places each [`ScriptedOrder`] on its `at_tick`, with the script's label as the reason.
///
/// * [`ScriptedPrice::Tick`] is an absolute price. [`ScriptedPrice::FromTouch`] is resolved
///   against the local book at `at_tick`: a buy at `best_bid + offset`, a sell at
///   `best_ask - offset`. **If that best price is missing on `at_tick` the order is skipped
///   permanently and never retried**: retrying would move it to a different tick and therefore a
///   different order id, and the script would no longer mean the same thing in two scenarios.
///   The forensics view then simply has no such order.
/// * With `cancel_after_ticks = Some(n)`, from tick `at_tick + n` on a cancel is emitted as soon
///   as the order is listed in `open_orders` and is cancellable, and again on a later tick
///   should it still be open and cancellable. Tracking stops once the order is no longer open
///   (filled, expired, cancelled, or never accepted by the harness).
#[derive(Clone, Debug)]
pub struct Scripted {
    /// Sorted by `(at_tick, slot)`.
    orders: Vec<ScriptedOrder>,
    /// First order whose `at_tick` has not been reached.
    next: usize,
    cancels: Vec<PendingCancel>,
    text: Text,
}

impl Scripted {
    pub fn new(params: &ScriptedParams, instrument: &InstrumentMeta) -> Self {
        let mut orders = params.orders.clone();
        orders.sort_by_key(|o| (o.at_tick, o.slot));
        Self {
            orders,
            next: 0,
            cancels: Vec::new(),
            text: Text::new(instrument),
        }
    }
}

impl Strategy for Scripted {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        let orders = &self.orders;
        let text = &self.text;

        // Every tracked order was placed on an earlier tick, so by now it is either listed in
        // `open_orders` or it is gone for good.
        self.cancels.retain(|pending| {
            let Some(open) = view
                .open_orders
                .iter()
                .find(|o| o.order_id == pending.order_id)
            else {
                return false;
            };
            let script = &orders[pending.index];
            let after = pending.after;
            let due_tick = script.at_tick.saturating_add(after);
            if view.tick_no >= due_tick && open.cancellable() {
                out.push(Intent::Cancel {
                    order_id: pending.order_id,
                    reason: format!(
                        "scripted: cancel '{}' ({} {} at {}): due from tick {due_tick} (placed \
                         at tick {}, cancel_after_ticks {after}), order is cancellable at tick {}",
                        script.label,
                        verb(open.side),
                        text.qty(open.leaves_qty),
                        text.px(open.price_tick),
                        script.at_tick,
                        view.tick_no,
                    ),
                });
            }
            true
        });

        while let Some(script) = orders.get(self.next) {
            if script.at_tick > view.tick_no {
                break;
            }
            let index = self.next;
            self.next += 1;
            if script.at_tick < view.tick_no {
                // Its tick never ran (the grid started later); placing it now would give it a
                // different order id.
                continue;
            }
            let price_tick = match script.price {
                ScriptedPrice::Tick { tick } => Some(tick),
                ScriptedPrice::FromTouch { offset } => match script.side {
                    Side::Buy => view
                        .market
                        .best_bid_tick
                        .map(|bid| bid.saturating_add(offset)),
                    Side::Sell => view
                        .market
                        .best_ask_tick
                        .map(|ask| ask.saturating_sub(offset)),
                },
            };
            let Some(price_tick) = price_tick else {
                // No touch to price against: skipped permanently, see the type documentation.
                continue;
            };
            out.push(Intent::Place {
                slot: script.slot,
                side: script.side,
                price_tick,
                qty: script.qty,
                tif: script.tif,
                ord_type: script.ord_type,
                reason: script.label.clone(),
            });
            if let Some(after) = script.cancel_after_ticks {
                self.cancels.push(PendingCancel {
                    index,
                    order_id: order_id(script.at_tick, script.slot),
                    after,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        INSTRUMENT, cancelled, market, pending_cancel, pending_new, placed, reasons, resting, run,
        view,
    };
    use bb_core::{OrdType, Tif};

    fn order(
        at_tick: u64,
        slot: u8,
        side: Side,
        price: ScriptedPrice,
        cancel_after_ticks: Option<u64>,
    ) -> ScriptedOrder {
        ScriptedOrder {
            at_tick,
            slot,
            side,
            price,
            qty: 1.0,
            tif: Tif::Gtc,
            ord_type: OrdType::Limit,
            cancel_after_ticks,
            label: format!("demo {side:?} at tick {at_tick}"),
        }
    }

    fn scripted(orders: Vec<ScriptedOrder>) -> Scripted {
        Scripted::new(&ScriptedParams { orders }, &INSTRUMENT)
    }

    #[test]
    fn places_at_the_right_tick_with_both_price_modes() {
        // Listed out of order on purpose: the script is sorted by (at_tick, slot).
        let mut s = scripted(vec![
            order(
                3,
                1,
                Side::Sell,
                ScriptedPrice::FromTouch { offset: -2 },
                None,
            ),
            order(
                3,
                0,
                Side::Buy,
                ScriptedPrice::FromTouch { offset: 1 },
                None,
            ),
            order(2, 4, Side::Buy, ScriptedPrice::Tick { tick: 9_950 }, None),
        ]);
        let book = market(Some(10_000), Some(10_010));

        assert!(run(&mut s, &view(0, book, 0.0, &[])).is_empty());
        assert!(run(&mut s, &view(1, book, 0.0, &[])).is_empty());

        let out = run(&mut s, &view(2, book, 0.0, &[]));
        assert_eq!(placed(&out), vec![(4, Side::Buy, 9_950, 1.0, Tif::Gtc)]);
        assert_eq!(reasons(&out), vec!["demo Buy at tick 2"]);

        // Buy one tick through the best bid; sell two ticks behind the best ask.
        let out = run(&mut s, &view(3, book, 0.0, &[]));
        assert_eq!(
            placed(&out),
            vec![
                (0, Side::Buy, 10_001, 1.0, Tif::Gtc),
                (1, Side::Sell, 10_012, 1.0, Tif::Gtc),
            ]
        );
        assert_eq!(
            reasons(&out),
            vec!["demo Buy at tick 3", "demo Sell at tick 3"]
        );

        for tick_no in 4..10 {
            assert!(run(&mut s, &view(tick_no, book, 0.0, &[])).is_empty());
        }
    }

    #[test]
    fn passes_time_in_force_and_order_type_through() {
        let mut s = scripted(vec![ScriptedOrder {
            tif: Tif::Ioc,
            ord_type: OrdType::Market,
            qty: 0.25,
            ..order(0, 7, Side::Sell, ScriptedPrice::Tick { tick: 10_000 }, None)
        }]);
        let out = run(&mut s, &view(0, market(None, None), 0.0, &[]));
        assert_eq!(
            out,
            vec![Intent::Place {
                slot: 7,
                side: Side::Sell,
                price_tick: 10_000,
                qty: 0.25,
                tif: Tif::Ioc,
                ord_type: OrdType::Market,
                reason: "demo Sell at tick 0".to_string(),
            }]
        );
    }

    #[test]
    fn cancels_after_n_ticks_only_once_cancellable() {
        let mut s = scripted(vec![order(
            1,
            0,
            Side::Buy,
            ScriptedPrice::Tick { tick: 10_000 },
            Some(2),
        )]);
        let book = market(Some(10_000), Some(10_010));
        let id = order_id(1, 0);

        assert!(run(&mut s, &view(0, book, 0.0, &[])).is_empty());
        assert_eq!(placed(&run(&mut s, &view(1, book, 0.0, &[]))).len(), 1);

        // Tick 2: resting and cancellable, but the cancel is not due before tick 3.
        let acked = [resting(id, Side::Buy, 10_000)];
        assert!(run(&mut s, &view(2, book, 0.0, &acked)).is_empty());

        // Tick 3: due, but the order has a request in flight, so the engine would refuse.
        let unacked = [pending_new(id, Side::Buy, 10_000)];
        assert!(run(&mut s, &view(3, book, 0.0, &unacked)).is_empty());

        // Tick 4: due and cancellable.
        let out = run(&mut s, &view(4, book, 0.0, &acked));
        assert_eq!(cancelled(&out), vec![id]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            reasons(&out)[0],
            "scripted: cancel 'demo Buy at tick 1' (buy 1.000 at 100.00): due from tick 3 \
             (placed at tick 1, cancel_after_ticks 2), order is cancellable at tick 4"
        );

        // Tick 5: the cancel is in flight; no second request.
        let cancelling = [pending_cancel(id, Side::Buy, 10_000)];
        assert!(run(&mut s, &view(5, book, 0.0, &cancelling)).is_empty());

        // Tick 6: gone. Tracking stops; even if the id showed up again nothing is sent.
        assert!(run(&mut s, &view(6, book, 0.0, &[])).is_empty());
        assert!(run(&mut s, &view(7, book, 0.0, &acked)).is_empty());
    }

    #[test]
    fn stops_tracking_an_order_that_ended_before_its_cancel_was_due() {
        let mut s = scripted(vec![order(
            0,
            0,
            Side::Buy,
            ScriptedPrice::Tick { tick: 10_000 },
            Some(5),
        )]);
        let book = market(Some(10_000), Some(10_010));
        assert_eq!(placed(&run(&mut s, &view(0, book, 0.0, &[]))).len(), 1);
        // Filled (or blocked by a risk rule) before tick 1: it never appears in `open_orders`.
        for tick_no in 1..8 {
            assert!(run(&mut s, &view(tick_no, book, 1.0, &[])).is_empty());
        }
    }

    #[test]
    fn immediate_cancel_waits_for_the_order_to_appear() {
        let mut s = scripted(vec![order(
            0,
            0,
            Side::Sell,
            ScriptedPrice::Tick { tick: 10_010 },
            Some(0),
        )]);
        let book = market(Some(10_000), Some(10_010));
        let id = order_id(0, 0);
        // On the placement tick itself only the placement is emitted.
        let out = run(&mut s, &view(0, book, 0.0, &[]));
        assert_eq!(placed(&out).len(), 1);
        assert!(cancelled(&out).is_empty());

        let unacked = [pending_new(id, Side::Sell, 10_010)];
        assert!(run(&mut s, &view(1, book, 0.0, &unacked)).is_empty());
        let acked = [resting(id, Side::Sell, 10_010)];
        let out = run(&mut s, &view(2, book, 0.0, &acked));
        assert_eq!(cancelled(&out), vec![id]);
    }

    #[test]
    fn skips_permanently_when_the_touch_is_missing() {
        let mut s = scripted(vec![
            order(
                1,
                0,
                Side::Buy,
                ScriptedPrice::FromTouch { offset: 0 },
                Some(1),
            ),
            order(
                1,
                1,
                Side::Sell,
                ScriptedPrice::FromTouch { offset: 0 },
                None,
            ),
        ]);
        // Only the ask exists on tick 1: the sell is placed, the buy is skipped.
        let out = run(&mut s, &view(1, market(None, Some(10_010)), 0.0, &[]));
        assert_eq!(placed(&out), vec![(1, Side::Sell, 10_010, 1.0, Tif::Gtc)]);

        // The bid is back, but the buy is not retried and no cancel is ever sent for it.
        let book = market(Some(10_000), Some(10_010));
        for tick_no in 2..6 {
            assert!(run(&mut s, &view(tick_no, book, 0.0, &[])).is_empty());
        }
    }
}
