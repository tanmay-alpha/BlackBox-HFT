//! Touch maker: joins (or sits a fixed offset behind) the best bid and best ask with post-only
//! orders and shifts both quotes against its inventory.

use bb_core::Side;
use bb_core::config::TouchMakerParams;
use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::{Intent, LocalView, Strategy};

use crate::quoting::{QuoteRules, Quotes, cancel_all_one_sided, sync_quotes};
use crate::text::{Text, Ticks};

const NAME: &str = "touch_maker";

/// Slot 0 is the bid quote, slot 1 the ask quote; both are `Gtx` limit orders.
///
/// Each tick: `skew = round(skew_ticks_per_unit * position)`,
/// `bid = best_bid - offset_ticks - skew`, `ask = best_ask + offset_ticks - skew`, both kept off
/// the opposite side of the locally known book. A quote already resting at the desired price is
/// left alone so it keeps its queue position; a quote at any other price is cancelled and
/// replaced. A side stops quoting when one more fill would take the position past
/// `max_position`. When the local book loses a side every order is cancelled.
#[derive(Clone, Debug)]
pub struct TouchMaker {
    params: TouchMakerParams,
    text: Text,
}

impl TouchMaker {
    pub fn new(params: TouchMakerParams, instrument: &InstrumentMeta) -> Self {
        Self {
            params,
            text: Text::new(instrument),
        }
    }
}

impl Strategy for TouchMaker {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        let p = &self.params;
        let text = &self.text;
        let (Some(best_bid), Some(best_ask)) =
            (view.market.best_bid_tick, view.market.best_ask_tick)
        else {
            cancel_all_one_sided(NAME, text, view, out);
            return;
        };

        // Positive inventory lowers both quotes: the bid becomes less likely to add to the
        // position and the ask more likely to reduce it.
        let skew = (p.skew_ticks_per_unit * view.position).round() as i64;
        let raw_bid = best_bid.saturating_sub(p.offset_ticks).saturating_sub(skew);
        let raw_ask = best_ask.saturating_add(p.offset_ticks).saturating_sub(skew);
        let quotes = Quotes::clamped(raw_bid, raw_ask, best_bid, best_ask);

        let rules = QuoteRules {
            name: NAME,
            text,
            order_qty: p.order_qty,
            max_position: p.max_position,
        };
        let rationale = |side: Side| {
            let (touch, touch_tick, offset_sign) = match side {
                Side::Buy => ("best bid", best_bid, '-'),
                Side::Sell => ("best ask", best_ask, '+'),
            };
            format!(
                "{touch} {} {offset_sign} offset {} - inventory skew {} \
                 (round({} ticks per unit x position {}))",
                text.px(touch_tick),
                Ticks(p.offset_ticks),
                Ticks(skew),
                p.skew_ticks_per_unit,
                text.qty(view.position),
            )
        };
        sync_quotes(&rules, view, &quotes, rationale, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        INSTRUMENT, cancelled, market, pending_cancel, pending_new, placed, reasons, resting,
        run, view,
    };
    use bb_core::Tif;
    use bb_core::strategy::order_id;

    fn maker(offset_ticks: i64, skew_ticks_per_unit: f64) -> TouchMaker {
        TouchMaker::new(
            TouchMakerParams {
                order_qty: 1.0,
                max_position: 3.0,
                offset_ticks,
                skew_ticks_per_unit,
            },
            &INSTRUMENT,
        )
    }

    #[test]
    fn joins_both_sides_on_a_fresh_book() {
        let mut s = maker(0, 0.0);
        let out = run(&mut s, &view(0, market(Some(10_000), Some(10_010)), 0.0, &[]));
        assert_eq!(
            placed(&out),
            vec![
                (0, Side::Buy, 10_000, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_010, 1.0, Tif::Gtx),
            ]
        );
        assert!(cancelled(&out).is_empty());
        for intent in &out {
            assert!(matches!(
                intent,
                Intent::Place {
                    ord_type: bb_core::OrdType::Limit,
                    ..
                }
            ));
        }
        assert_eq!(
            reasons(&out)[0],
            "touch_maker: quote bid 100.00 x 1.000 post-only: best bid 100.00 - offset 0 ticks \
             - inventory skew 0 ticks (round(0 ticks per unit x position 0.000))"
        );
    }

    #[test]
    fn offset_quotes_behind_the_touch() {
        let mut s = maker(2, 0.0);
        let out = run(&mut s, &view(0, market(Some(10_000), Some(10_010)), 0.0, &[]));
        assert_eq!(
            placed(&out),
            vec![
                (0, Side::Buy, 9_998, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_012, 1.0, Tif::Gtx),
            ]
        );
    }

    #[test]
    fn keeps_an_unchanged_quote() {
        let mut s = maker(0, 0.0);
        let book = market(Some(10_000), Some(10_010));
        // One quote acknowledged, the other still waiting for its acknowledgement: neither is
        // touched and neither is duplicated.
        let open = [
            resting(order_id(0, 0), Side::Buy, 10_000),
            pending_new(order_id(0, 1), Side::Sell, 10_010),
        ];
        assert!(run(&mut s, &view(1, book, 0.0, &open)).is_empty());
        assert!(run(&mut s, &view(2, book, 0.0, &open)).is_empty());
    }

    #[test]
    fn replaces_a_quote_when_the_touch_moves() {
        let mut s = maker(0, 0.0);
        let open = [
            resting(order_id(0, 0), Side::Buy, 10_000),
            resting(order_id(0, 1), Side::Sell, 10_010),
        ];
        let out = run(&mut s, &view(1, market(Some(10_001), Some(10_010)), 0.0, &open));
        // Only the bid moved, so only the bid is replaced; the ask keeps its queue position.
        assert_eq!(cancelled(&out), vec![order_id(0, 0)]);
        assert_eq!(placed(&out), vec![(0, Side::Buy, 10_001, 1.0, Tif::Gtx)]);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Intent::Cancel { .. }));
        assert!(reasons(&out)[0].contains("cancel bid at 100.00: desired bid is now 100.01"));
    }

    #[test]
    fn skews_with_inventory() {
        let book = market(Some(10_000), Some(10_010));

        let long = run(&mut maker(0, 1.0), &view(0, book, 2.0, &[]));
        assert_eq!(
            placed(&long),
            vec![
                (0, Side::Buy, 9_998, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_008, 1.0, Tif::Gtx),
            ]
        );

        let short = run(&mut maker(0, 1.0), &view(0, book, -2.0, &[]));
        assert_eq!(
            placed(&short),
            vec![
                (0, Side::Buy, 10_002, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_012, 1.0, Tif::Gtx),
            ]
        );

        // 0.6 ticks per unit x 2 units = 1.2, rounded to 1 tick.
        let fractional = run(&mut maker(0, 0.6), &view(0, book, 2.0, &[]));
        assert_eq!(
            placed(&fractional),
            vec![
                (0, Side::Buy, 9_999, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_009, 1.0, Tif::Gtx),
            ]
        );
    }

    #[test]
    fn skewed_quotes_never_cross_the_local_book() {
        let tight = market(Some(10_000), Some(10_001));

        // Long: the raw ask (9_999) would sit through the bid; it is held one tick above it.
        let long = run(&mut maker(0, 1.0), &view(0, tight, 2.0, &[]));
        assert_eq!(
            placed(&long),
            vec![
                (0, Side::Buy, 9_998, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_001, 1.0, Tif::Gtx),
            ]
        );
        assert!(reasons(&long)[1].contains("clamped from 99.99 to 100.01"));
        assert!(!reasons(&long)[0].contains("clamped"));

        // Short: the raw bid (10_002) would sit through the ask.
        let short = run(&mut maker(0, 1.0), &view(0, tight, -2.0, &[]));
        assert_eq!(
            placed(&short),
            vec![
                (0, Side::Buy, 10_000, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_003, 1.0, Tif::Gtx),
            ]
        );
        assert!(reasons(&short)[0].contains("clamped from 100.02 to 100.00"));
    }

    #[test]
    fn disables_the_buy_side_at_max_long_and_cancels_the_resting_bid() {
        let book = market(Some(10_000), Some(10_010));
        let open = [
            resting(order_id(0, 0), Side::Buy, 10_000),
            resting(order_id(0, 1), Side::Sell, 10_010),
        ];

        // Position 2 + order 1 = max 3: still allowed, nothing changes.
        assert!(run(&mut maker(0, 0.0), &view(1, book, 2.0, &open)).is_empty());

        // Position 3 + order 1 > max 3: the bid goes even though its price is still right.
        let out = run(&mut maker(0, 0.0), &view(1, book, 3.0, &open));
        assert_eq!(cancelled(&out), vec![order_id(0, 0)]);
        assert!(placed(&out).is_empty());
        assert_eq!(
            reasons(&out)[0],
            "touch_maker: cancel bid at 100.00: buy side disabled: position 3.000 + order qty \
             1.000 would exceed max position 3.000"
        );

        // With no orders open, only the ask is quoted.
        let fresh = run(&mut maker(0, 0.0), &view(1, book, 3.0, &[]));
        assert_eq!(placed(&fresh), vec![(1, Side::Sell, 10_010, 1.0, Tif::Gtx)]);

        // Symmetric at max short.
        let short = run(&mut maker(0, 0.0), &view(1, book, -3.0, &open));
        assert_eq!(cancelled(&short), vec![order_id(0, 1)]);
        assert!(placed(&short).is_empty());
    }

    #[test]
    fn never_cancels_a_pending_order() {
        let mut s = maker(0, 0.0);
        // Both orders sit at stale prices, but one has not been acknowledged yet and the other
        // already has a cancel in flight.
        let open = [
            pending_new(order_id(0, 0), Side::Buy, 9_990),
            pending_cancel(order_id(0, 1), Side::Sell, 10_020),
        ];
        let out = run(&mut s, &view(1, market(Some(10_000), Some(10_010)), 0.0, &open));
        assert!(cancelled(&out).is_empty());
        assert_eq!(
            placed(&out),
            vec![
                (0, Side::Buy, 10_000, 1.0, Tif::Gtx),
                (1, Side::Sell, 10_010, 1.0, Tif::Gtx),
            ]
        );

        // Not even when the side is disabled or the book is one-sided.
        let maxed = run(&mut s, &view(2, market(Some(10_000), Some(10_010)), 3.0, &open));
        assert!(cancelled(&maxed).is_empty());
        assert!(run(&mut s, &view(3, market(None, Some(10_010)), 0.0, &open)).is_empty());
    }

    #[test]
    fn an_order_being_cancelled_is_not_a_live_quote() {
        let mut s = maker(0, 0.0);
        // The bid at the desired price is on its way out, so a fresh bid is placed; the pending
        // new ask at the desired price counts and is not duplicated.
        let open = [
            pending_cancel(order_id(0, 0), Side::Buy, 10_000),
            pending_new(order_id(0, 1), Side::Sell, 10_010),
        ];
        let out = run(&mut s, &view(1, market(Some(10_000), Some(10_010)), 0.0, &open));
        assert!(cancelled(&out).is_empty());
        assert_eq!(placed(&out), vec![(0, Side::Buy, 10_000, 1.0, Tif::Gtx)]);
    }

    #[test]
    fn one_sided_book_cancels_everything_cancellable() {
        let mut s = maker(0, 0.0);
        let open = [
            resting(order_id(0, 0), Side::Buy, 10_000),
            resting(order_id(0, 1), Side::Sell, 10_010),
            pending_new(order_id(1, 0), Side::Buy, 10_001),
        ];
        let out = run(&mut s, &view(2, market(Some(10_000), None), 0.0, &open));
        assert_eq!(cancelled(&out), vec![order_id(0, 0), order_id(0, 1)]);
        assert!(placed(&out).is_empty());
        assert_eq!(
            reasons(&out)[1],
            "touch_maker: cancel ask at 100.10: local book is one-sided (best bid 100.00, best \
             ask none), so there is no two-sided reference to quote against"
        );

        // Nothing to cancel and nothing to quote on an empty book.
        assert!(run(&mut s, &view(3, market(None, None), 0.0, &[])).is_empty());
    }
}
