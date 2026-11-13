//! Two-sided post-only quote maintenance shared by the market-making strategies.
//!
//! A strategy computes the price it wants on each side; this module turns "desired quotes" plus
//! "orders currently open" into the minimal set of cancels and placements, and applies the
//! per-side inventory gate.

use bb_core::strategy::{Intent, LocalView, OpenOrder};
use bb_core::{OrdType, OrderStatus, Side, Tif};

use crate::text::{Text, Ticks, quote_name, verb};

/// Slot of the bid quote placed on a tick.
pub(crate) const BID_SLOT: u8 = 0;
/// Slot of the ask quote placed on a tick.
pub(crate) const ASK_SLOT: u8 = 1;

/// The parts of a market maker's configuration the shared logic needs.
pub(crate) struct QuoteRules<'a> {
    /// Strategy label that prefixes every reason.
    pub name: &'static str,
    pub text: &'a Text,
    pub order_qty: f64,
    pub max_position: f64,
}

/// Desired quote prices before and after being kept off the opposite side of the local book.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Quotes {
    pub bid_tick: i64,
    pub ask_tick: i64,
    raw_bid_tick: i64,
    raw_ask_tick: i64,
    best_bid_tick: i64,
    best_ask_tick: i64,
}

impl Quotes {
    /// Clamps the strategy's raw prices so that `bid <= best_ask - 1` and `ask >= best_bid + 1`.
    /// A post-only order that crosses is expired by the exchange model, so quoting through the
    /// locally known book would only burn a request.
    pub(crate) fn clamped(
        raw_bid_tick: i64,
        raw_ask_tick: i64,
        best_bid_tick: i64,
        best_ask_tick: i64,
    ) -> Self {
        Self {
            bid_tick: raw_bid_tick.min(best_ask_tick.saturating_sub(1)),
            ask_tick: raw_ask_tick.max(best_bid_tick.saturating_add(1)),
            raw_bid_tick,
            raw_ask_tick,
            best_bid_tick,
            best_ask_tick,
        }
    }

    /// The strategy's own derivation of the price, extended with the clamp when it applied.
    fn derivation<R>(&self, side: Side, text: &Text, rationale: &R) -> String
    where
        R: Fn(Side) -> String,
    {
        let base = rationale(side);
        match side {
            Side::Buy if self.bid_tick != self.raw_bid_tick => format!(
                "{base}; clamped from {} to {}, {} below best ask {}, so the post-only quote \
                 cannot cross the locally known book",
                text.px(self.raw_bid_tick),
                text.px(self.bid_tick),
                Ticks(1),
                text.px(self.best_ask_tick),
            ),
            Side::Sell if self.ask_tick != self.raw_ask_tick => format!(
                "{base}; clamped from {} to {}, {} above best bid {}, so the post-only quote \
                 cannot cross the locally known book",
                text.px(self.raw_ask_tick),
                text.px(self.ask_tick),
                Ticks(1),
                text.px(self.best_bid_tick),
            ),
            _ => base,
        }
    }
}

/// Why a side must not quote this tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Block {
    /// One more fill of `order_qty` would take the position beyond `max_position`.
    PositionLimit,
    /// The desired price is zero or negative, which no exchange accepts.
    NonPositivePrice,
}

fn block(rules: &QuoteRules<'_>, side: Side, price_tick: i64, position: f64) -> Option<Block> {
    // `sign * position` is the inventory in the direction this side would grow.
    if side.sign() * position + rules.order_qty > rules.max_position + rules.text.half_lot() {
        Some(Block::PositionLimit)
    } else if price_tick <= 0 {
        Some(Block::NonPositivePrice)
    } else {
        None
    }
}

/// A resting order with a request in flight has a cancel on its way (a pending *new* request
/// shows as status `None`). Its queue slot is about to disappear, so it is not a live quote.
fn being_cancelled(order: &OpenOrder) -> bool {
    order.request_pending && order.status != OrderStatus::None
}

fn position_limit_text(rules: &QuoteRules<'_>, side: Side, position: f64) -> String {
    let text = rules.text;
    match side {
        Side::Buy => format!(
            "buy side disabled: position {} + order qty {} would exceed max position {}",
            text.qty(position),
            text.qty(rules.order_qty),
            text.qty(rules.max_position),
        ),
        Side::Sell => format!(
            "sell side disabled: position {} - order qty {} would exceed max short position {}",
            text.qty(position),
            text.qty(rules.order_qty),
            text.qty(rules.max_position),
        ),
    }
}

fn sync_side<R>(
    rules: &QuoteRules<'_>,
    view: &LocalView<'_>,
    quotes: &Quotes,
    side: Side,
    rationale: &R,
    out: &mut Vec<Intent>,
) where
    R: Fn(Side) -> String,
{
    let text = rules.text;
    let name = rules.name;
    let (slot, price_tick) = match side {
        Side::Buy => (BID_SLOT, quotes.bid_tick),
        Side::Sell => (ASK_SLOT, quotes.ask_tick),
    };
    let blocked = block(rules, side, price_tick, view.position);

    let mut live_at_price = false;
    for order in view.open_orders.iter().filter(|o| o.side == side) {
        if blocked.is_none() && order.price_tick == price_tick {
            // Deliberately left untouched: an order kept at an unchanged price keeps its place
            // in the queue, whereas cancel-and-replace would rejoin at the back. An order whose
            // new request is still in flight counts as live too, otherwise every tick spent
            // waiting for the acknowledgement would stack a duplicate.
            live_at_price |= !being_cancelled(order);
            continue;
        }
        if !order.cancellable() {
            // The engine refuses a cancel while another request is in flight; the order is
            // looked at again on the next tick.
            continue;
        }
        let resting = text.px(order.price_tick);
        let quote = quote_name(side);
        let reason = match blocked {
            Some(Block::PositionLimit) => format!(
                "{name}: cancel {quote} at {resting}: {}",
                position_limit_text(rules, side, view.position),
            ),
            Some(Block::NonPositivePrice) => format!(
                "{name}: cancel {quote} at {resting}: {} side disabled: desired {quote} {} is \
                 not a positive price ({})",
                verb(side),
                text.px(price_tick),
                quotes.derivation(side, text, rationale),
            ),
            None => format!(
                "{name}: cancel {quote} at {resting}: desired {quote} is now {} ({})",
                text.px(price_tick),
                quotes.derivation(side, text, rationale),
            ),
        };
        out.push(Intent::Cancel {
            order_id: order.order_id,
            reason,
        });
    }

    if blocked.is_none() && !live_at_price {
        out.push(Intent::Place {
            slot,
            side,
            price_tick,
            qty: rules.order_qty,
            tif: Tif::Gtx,
            ord_type: OrdType::Limit,
            reason: format!(
                "{name}: quote {} {} x {} post-only: {}",
                quote_name(side),
                text.px(price_tick),
                text.qty(rules.order_qty),
                quotes.derivation(side, text, rationale),
            ),
        });
    }
}

/// Brings the strategy's open orders in line with `quotes`: per side, cancel what is at the wrong
/// price (or all of it when the side is disabled) and place a quote when none is live at the
/// desired price. `rationale(side)` explains how the unclamped price was derived; it is only
/// called when an intent is actually emitted.
pub(crate) fn sync_quotes<R>(
    rules: &QuoteRules<'_>,
    view: &LocalView<'_>,
    quotes: &Quotes,
    rationale: R,
    out: &mut Vec<Intent>,
) where
    R: Fn(Side) -> String,
{
    sync_side(rules, view, quotes, Side::Buy, &rationale, out);
    sync_side(rules, view, quotes, Side::Sell, &rationale, out);
}

/// Cancels every cancellable order because the local book has lost a side and there is no
/// two-sided reference to quote against.
pub(crate) fn cancel_all_one_sided(
    name: &'static str,
    text: &Text,
    view: &LocalView<'_>,
    out: &mut Vec<Intent>,
) {
    for order in view.open_orders.iter().filter(|o| o.cancellable()) {
        out.push(Intent::Cancel {
            order_id: order.order_id,
            reason: format!(
                "{name}: cancel {} at {}: local book is one-sided (best bid {}, best ask {}), \
                 so there is no two-sided reference to quote against",
                quote_name(order.side),
                text.px(order.price_tick),
                text.opt_px(view.market.best_bid_tick),
                text.opt_px(view.market.best_ask_tick),
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_quotes_off_the_opposite_touch() {
        let inside = Quotes::clamped(99, 104, 100, 103);
        assert_eq!((inside.bid_tick, inside.ask_tick), (99, 104));

        // A bid at or through the ask is pulled back to one tick below it.
        let bid_through = Quotes::clamped(105, 110, 100, 103);
        assert_eq!((bid_through.bid_tick, bid_through.ask_tick), (102, 110));

        // An ask at or through the bid is pushed up to one tick above it.
        let ask_through = Quotes::clamped(95, 100, 100, 103);
        assert_eq!((ask_through.bid_tick, ask_through.ask_tick), (95, 101));
    }
}
