//! The execution flight recorder's event model.
//!
//! A run produces one totally ordered tape of [`FlightEvent`]s. `seq` is the order in which the
//! engine and harness produced them, which is deterministic. Each event names its clock and its
//! source, so engine-generated facts are always distinguishable from BlackBox annotations.
//!
//! # Where each engine event comes from (hftbacktest 0.9.4)
//!
//! | Event | Captured in | Engine call site |
//! |---|---|---|
//! | [`FlightBody::RequestSent`] | `LatencyModel::entry` | `LocalToExch::request`, when the local side sends a request |
//! | [`FlightBody::ExchQueued`] | `QueueModel::new_order` | exchange `ack_new`, when a limit order is accepted to rest |
//! | [`FlightBody::QueueUpdate`] | `QueueModel::trade` / `QueueModel::depth` | exchange processing a trade or a depth change at the order's price |
//! | [`FlightBody::ExchFill`] | `FeeModel::amount` on the exchange-side state | exchange `fill`, once per execution |
//! | [`FlightBody::ExchResponse`] | `LatencyModel::response` | `ExchToLocal::respond`, when the exchange emits a response |
//! | [`FlightBody::LocalResponse`] | `Local::process_recv_order_` handler | local side receiving that response |
//!
//! The engine does not expose a hook for "the exchange looked at this order and did nothing", so
//! no such event exists. Absence of a `QueueUpdate` means no trade or depth change occurred at the
//! order's price, not that the recorder skipped something (truncation is always marked explicitly).

use serde::{Deserialize, Serialize};

use crate::{Clock, OrdType, OrderStatus, RelNs, ReqKind, Side, Source, Tif, strategy::MarketView};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlightEvent {
    /// Position on the tape, starting at 0, contiguous.
    pub seq: u64,
    /// When the event happened, in `clock`'s domain.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub ts: RelNs,
    pub clock: Clock,
    pub source: Source,
    /// The order this event concerns. `None` for run-level events.
    pub order_id: Option<u64>,
    pub body: FlightBody,
}

/// The market event that moved a queue estimate.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueueCause {
    /// A market trade printed at the order's price. `qty` is the traded quantity.
    Trade { qty: f64 },
    /// The displayed quantity at the order's price changed from `prev_qty` to `new_qty`.
    Depth { prev_qty: f64, new_qty: f64 },
    /// After a partial maker fill the engine's queue model puts the order's remainder at the
    /// front of the queue (estimate reset to zero). Recorded directly after the `ExchFill` it
    /// belongs to, and only when the order survives the fill.
    FillReset,
}

/// Pre-trade risk rule identifiers.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RiskRule {
    MaxPosition,
    MaxOrderQty,
    MaxOpenOrders,
    MaxDrawdown,
    /// The order is not a positive multiple of the lot size, or its price is not positive.
    InvalidOrder,
    /// The strategy is halted by an earlier `MaxDrawdown` breach.
    Halted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlightBody {
    // ----- Source::Strategy, Clock::Local -----
    /// The strategy decided to place an order. Recorded before risk checks.
    OrderIntent {
        tick_no: u64,
        slot: u8,
        side: Side,
        price_tick: i64,
        qty: f64,
        tif: Tif,
        ord_type: OrdType,
        reason: String,
        /// Market as known locally at the decision.
        market: MarketView,
        /// Position as known locally at the decision.
        known_position: f64,
    },
    /// The strategy decided to cancel an order.
    CancelIntent {
        tick_no: u64,
        reason: String,
        market: MarketView,
        known_position: f64,
    },
    /// A risk rule stopped an intent before it reached the engine. `order_id` is the id the order
    /// would have had.
    RiskBlocked {
        tick_no: u64,
        rule: RiskRule,
        detail: String,
    },
    /// A run-level risk rule fired; the harness cancels working orders and stops placing new ones.
    RiskHalt { rule: RiskRule, detail: String },

    // ----- Source::Engine -----
    /// Clock::Local. The local side sent a request. `entry_latency_ns` is the modeled latency the
    /// engine drew for it. A negative value is the engine's convention for "rejected before
    /// reaching the matching engine"; then `exch_arrival_ts` is `None`.
    RequestSent {
        req: ReqKind,
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        entry_latency_ns: i64,
        /// Scheduled arrival at the exchange (`ts + entry_latency_ns`). The engine's order bus
        /// never lets a request overtake an earlier one, so actual processing can be later; the
        /// exchange-clock events show when it really happened.
        #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
        exch_arrival_ts: Option<RelNs>,
    },
    /// Clock::Local. The engine refused a request synchronously (for example a cancel while
    /// another request for the same order is in flight). Nothing was sent.
    RequestRefused { req: ReqKind, error: String },
    /// Clock::Exchange. The exchange model accepted a limit order to rest and the queue model
    /// initialised its position.
    ExchQueued {
        /// Modeled quantity ahead of the order. `None` only if the queue model's state could not
        /// be read, which the recorder reports rather than guesses.
        queue_ahead: Option<f64>,
        /// Displayed quantity at the order's price at that moment (observed market data).
        level_qty: f64,
        best_bid_tick: Option<i64>,
        best_ask_tick: Option<i64>,
    },
    /// Clock::Exchange. A trade or depth change at the order's price moved the queue estimate.
    QueueUpdate {
        cause: QueueCause,
        ahead_before: Option<f64>,
        /// Negative means the modeled queue ahead is exhausted by that much; the exchange model
        /// turns this into a fill.
        ahead_after: Option<f64>,
    },
    /// Clock::Exchange. One simulated execution.
    ExchFill {
        price_tick: i64,
        qty: f64,
        /// `true`: filled while resting. `false`: took liquidity on arrival.
        maker: bool,
        /// Fee charged for this execution in quote currency (negative is a rebate).
        fee: f64,
        /// Traded value in quote currency.
        value: f64,
        leaves_qty: f64,
    },
    /// Clock::Exchange. The exchange emitted a response toward the local side.
    ExchResponse {
        status: OrderStatus,
        /// The exchange rejected the request this responds to (a cancel for an order that no
        /// longer exists there).
        request_rejected: bool,
        leaves_qty: f64,
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        response_latency_ns: i64,
        /// Scheduled local arrival. Same ordering caveat as `RequestSent::exch_arrival_ts`.
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        local_arrival_ts: RelNs,
    },
    /// Clock::Local. The local side received a response and updated its view of the order.
    LocalResponse {
        status: OrderStatus,
        request_rejected: bool,
        /// Quantity of the execution this response reports (0 when it reports none).
        exec_qty: f64,
        exec_price_tick: i64,
        leaves_qty: f64,
        /// When the exchange produced this response.
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        exch_ts: RelNs,
    },

    // ----- Source::Recorder -----
    /// The per-order cap on queue updates was reached; `dropped` updates that changed the estimate
    /// by less than one lot were not kept after this point.
    QueueTruncated { dropped: u64 },
}

impl FlightBody {
    /// The source every event of this kind must carry.
    pub fn source(&self) -> Source {
        match self {
            FlightBody::OrderIntent { .. }
            | FlightBody::CancelIntent { .. }
            | FlightBody::RiskBlocked { .. }
            | FlightBody::RiskHalt { .. } => Source::Strategy,
            FlightBody::RequestSent { .. }
            | FlightBody::RequestRefused { .. }
            | FlightBody::ExchQueued { .. }
            | FlightBody::QueueUpdate { .. }
            | FlightBody::ExchFill { .. }
            | FlightBody::ExchResponse { .. }
            | FlightBody::LocalResponse { .. } => Source::Engine,
            FlightBody::QueueTruncated { .. } => Source::Recorder,
        }
    }

    /// The clock every event of this kind must carry.
    pub fn clock(&self) -> Clock {
        match self {
            FlightBody::ExchQueued { .. }
            | FlightBody::QueueUpdate { .. }
            | FlightBody::ExchFill { .. }
            | FlightBody::ExchResponse { .. }
            | FlightBody::QueueTruncated { .. } => Clock::Exchange,
            FlightBody::OrderIntent { .. }
            | FlightBody::CancelIntent { .. }
            | FlightBody::RiskBlocked { .. }
            | FlightBody::RiskHalt { .. }
            | FlightBody::RequestSent { .. }
            | FlightBody::RequestRefused { .. }
            | FlightBody::LocalResponse { .. } => Clock::Local,
        }
    }

    /// Stable snake_case name, identical to the serialized `type` tag.
    pub fn type_name(&self) -> &'static str {
        match self {
            FlightBody::OrderIntent { .. } => "order_intent",
            FlightBody::CancelIntent { .. } => "cancel_intent",
            FlightBody::RiskBlocked { .. } => "risk_blocked",
            FlightBody::RiskHalt { .. } => "risk_halt",
            FlightBody::RequestSent { .. } => "request_sent",
            FlightBody::RequestRefused { .. } => "request_refused",
            FlightBody::ExchQueued { .. } => "exch_queued",
            FlightBody::QueueUpdate { .. } => "queue_update",
            FlightBody::ExchFill { .. } => "exch_fill",
            FlightBody::ExchResponse { .. } => "exch_response",
            FlightBody::LocalResponse { .. } => "local_response",
            FlightBody::QueueTruncated { .. } => "queue_truncated",
        }
    }
}

impl FlightEvent {
    /// Builds an event, deriving `clock` and `source` from the body so they cannot disagree.
    pub fn new(seq: u64, ts: RelNs, order_id: Option<u64>, body: FlightBody) -> Self {
        Self {
            seq,
            ts,
            clock: body.clock(),
            source: body.source(),
            order_id,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_tag_matches_type_name() {
        let bodies = [
            FlightBody::RiskHalt {
                rule: RiskRule::MaxDrawdown,
                detail: String::new(),
            },
            FlightBody::ExchFill {
                price_tick: 1,
                qty: 1.0,
                maker: true,
                fee: 0.0,
                value: 1.0,
                leaves_qty: 0.0,
            },
            FlightBody::QueueUpdate {
                cause: QueueCause::Trade { qty: 1.0 },
                ahead_before: Some(2.0),
                ahead_after: Some(1.0),
            },
            FlightBody::QueueTruncated { dropped: 3 },
        ];
        for b in bodies {
            let v = serde_json::to_value(&b).unwrap();
            assert_eq!(v["type"], b.type_name());
            let back: FlightBody = serde_json::from_value(v).unwrap();
            assert_eq!(back, b);
        }
    }

    #[test]
    fn constructor_keeps_clock_and_source_consistent() {
        let e = FlightEvent::new(
            0,
            5,
            Some(1),
            FlightBody::ExchQueued {
                queue_ahead: Some(1.5),
                level_qty: 1.5,
                best_bid_tick: Some(10),
                best_ask_tick: Some(11),
            },
        );
        assert_eq!(e.clock, Clock::Exchange);
        assert_eq!(e.source, Source::Engine);
    }
}
