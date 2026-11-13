//! The contract between the simulation harness and a strategy.
//!
//! A strategy is a pure state machine: on each tick of a fixed local-time grid it receives a
//! [`LocalView`] (only information that had reached the strategy host by then) and appends
//! [`Intent`]s. It never talks to the engine. The harness applies risk rules, turns surviving
//! intents into engine requests and records everything.
//!
//! # Stable logical identifiers
//!
//! An order is identified by the tick it was decided on and a strategy-chosen `slot`:
//! `order_id = (tick_no << 8) | slot`. Because ticks sit on a fixed time grid that does not depend
//! on execution assumptions, the same `order_id` names "the same decision" in two scenarios, which
//! is what lets the comparison view align them. Whether the strategy *makes* the same decision in
//! both scenarios depends on what it had observed by then.

use serde::{Deserialize, Serialize};

use crate::{OrdType, OrderStatus, RelNs, Side, Tif};

/// Maximum number of distinct order slots per tick.
pub const SLOTS_PER_TICK: u64 = 256;

/// Stable order identifier for `(tick_no, slot)`.
#[inline]
pub fn order_id(tick_no: u64, slot: u8) -> u64 {
    (tick_no << 8) | slot as u64
}

/// Inverse of [`order_id`].
#[inline]
pub fn split_order_id(order_id: u64) -> (u64, u8) {
    (order_id >> 8, (order_id & 0xff) as u8)
}

/// Top of book as the strategy host knew it, with the provenance of that knowledge.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketView {
    /// `None` while the local book has no bid.
    pub best_bid_tick: Option<i64>,
    pub best_ask_tick: Option<i64>,
    pub best_bid_qty: f64,
    pub best_ask_qty: f64,
    /// Exchange timestamp of the last feed event received locally.
    pub feed_exch_ts: Option<RelNs>,
    /// Local receipt timestamp of that event.
    pub feed_local_ts: Option<RelNs>,
}

impl MarketView {
    /// Mid price in (fractional) ticks, when both sides exist.
    pub fn mid_tick(&self) -> Option<f64> {
        match (self.best_bid_tick, self.best_ask_tick) {
            (Some(b), Some(a)) => Some((b as f64 + a as f64) * 0.5),
            _ => None,
        }
    }

    pub fn spread_ticks(&self) -> Option<i64> {
        match (self.best_bid_tick, self.best_ask_tick) {
            (Some(b), Some(a)) => Some(a - b),
            _ => None,
        }
    }

    /// `(bid_qty - ask_qty) / (bid_qty + ask_qty)` in `[-1, 1]`, when both sides have size.
    pub fn imbalance(&self) -> Option<f64> {
        let total = self.best_bid_qty + self.best_ask_qty;
        (self.best_bid_tick.is_some() && self.best_ask_tick.is_some() && total > 0.0)
            .then(|| (self.best_bid_qty - self.best_ask_qty) / total)
    }
}

/// One of the strategy's own orders as known locally.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenOrder {
    pub order_id: u64,
    pub side: Side,
    pub price_tick: i64,
    pub qty: f64,
    pub leaves_qty: f64,
    /// Last status received from the exchange; `None` until the first response arrives.
    pub status: OrderStatus,
    /// A new/cancel request for this order is still in flight.
    pub request_pending: bool,
}

impl OpenOrder {
    /// Whether a cancel request is currently permitted by the engine.
    pub fn cancellable(&self) -> bool {
        matches!(self.status, OrderStatus::New | OrderStatus::PartiallyFilled)
            && !self.request_pending
    }
}

/// A market trade as received on the local feed.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradePrint {
    pub local_ts: RelNs,
    pub price_tick: i64,
    pub qty: f64,
    /// Aggressor side, when the data states it.
    pub aggressor: Option<Side>,
}

/// Everything a strategy may look at on one tick.
#[derive(Clone, Debug)]
pub struct LocalView<'a> {
    /// Local-clock time of this tick.
    pub now: RelNs,
    /// Index of this tick on the strategy's fixed grid, starting at 0.
    pub tick_no: u64,
    pub tick_size: f64,
    pub lot_size: f64,
    pub market: MarketView,
    /// Position implied by the fills whose notifications have arrived locally.
    pub position: f64,
    /// The strategy's orders that are not yet terminal, in ascending `order_id` order.
    pub open_orders: &'a [OpenOrder],
    /// Market trades received locally since the previous tick, oldest first.
    pub recent_trades: &'a [TradePrint],
}

/// What a strategy wants done.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    Place {
        /// Unique within the tick. The resulting order id is `order_id(tick_no, slot)`.
        slot: u8,
        side: Side,
        price_tick: i64,
        qty: f64,
        tif: Tif,
        ord_type: OrdType,
        /// Deterministic, human-readable rationale. Shown verbatim in the forensics view.
        reason: String,
    },
    Cancel {
        order_id: u64,
        reason: String,
    },
}

/// A deterministic trading strategy. Implementations must not use wall-clock time, global state or
/// unseeded randomness: given the same sequence of views they must produce the same intents.
pub trait Strategy {
    /// Called once per tick. Append intents to `out` (it arrives empty).
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_id_round_trips_and_orders_by_tick() {
        assert_eq!(split_order_id(order_id(0, 0)), (0, 0));
        assert_eq!(split_order_id(order_id(123_456, 255)), (123_456, 255));
        assert!(order_id(5, 255) < order_id(6, 0));
        // Stays within JavaScript's safe integer range for any realistic run length.
        assert!(order_id(1 << 44, 255) < (1 << 53));
    }

    #[test]
    fn market_view_derived_values() {
        let v = MarketView {
            best_bid_tick: Some(100),
            best_ask_tick: Some(103),
            best_bid_qty: 3.0,
            best_ask_qty: 1.0,
            feed_exch_ts: None,
            feed_local_ts: None,
        };
        assert_eq!(v.mid_tick(), Some(101.5));
        assert_eq!(v.spread_ticks(), Some(3));
        assert_eq!(v.imbalance(), Some(0.5));
        let empty = MarketView {
            best_bid_tick: None,
            ..v
        };
        assert_eq!(empty.mid_tick(), None);
        assert_eq!(empty.imbalance(), None);
    }
}
