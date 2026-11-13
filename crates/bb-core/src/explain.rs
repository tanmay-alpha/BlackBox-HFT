//! Rule-based explanations of a single order's outcome.
//!
//! An explanation is assembled deterministically from tape events. Each step cites the events it
//! rests on and states its [`Basis`], so the UI can show what was observed in market data, what
//! the simulation model estimated, and what cannot be known. No language model is involved.

use serde::{Deserialize, Serialize};

use crate::{Basis, RelNs};

/// The question being answered.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    WhyFilled,
    WhyNotFilled,
    /// Asked of an order that filled only in part.
    WhyPartiallyFilled,
}

/// Machine-readable conclusion. Exactly one applies to an order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    // ----- filled -----
    /// Resting order; trades at its price exhausted the modeled queue ahead of it.
    FilledQueueExhausted,
    /// Resting order; the market traded through its price or the opposite best crossed it, which
    /// the exchange model treats as a certain fill regardless of queue position.
    FilledPriceCrossed,
    /// Marketable on arrival; executed against displayed depth as a taker.
    FilledTakerOnArrival,
    /// Filled by more than one of the above mechanisms.
    FilledMixed,
    // ----- not (fully) filled -----
    /// Rested, trades printed at its price, but not enough to exhaust the modeled queue ahead.
    NotFilledQueueNotReached,
    /// Rested, and no trade printed at its price while it was live.
    NotFilledNoTradeAtPrice,
    /// Cancelled while still behind modeled queue.
    NotFilledCanceled,
    /// Post-only order would have crossed the book on arrival, so the exchange model expired it.
    NotFilledPostOnlyWouldCross,
    /// IOC/FOK found insufficient displayed liquidity on arrival.
    NotFilledNoLiquidityOnArrival,
    /// Stopped by a pre-trade risk rule.
    NotFilledRiskBlocked,
    /// The engine refused the request synchronously.
    NotFilledRefused,
    /// The dataset ended before the request reached the exchange.
    NotFilledDataEndedInFlight,
    /// Still resting when the dataset ended.
    NotFilledWorkingAtEnd,
}

/// One link in the chain of reasoning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplainStep {
    pub basis: Basis,
    /// One or two plain sentences with concrete numbers.
    pub statement: String,
    /// Tape `seq` values this statement rests on (possibly empty for `Unobservable` caveats).
    pub event_seqs: Vec<u64>,
}

/// Named time intervals of the order's life. Each is `None` when one of its endpoints never
/// occurred.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatencyBreakdown {
    /// Age of the market data the decision was based on: decision time minus the local receipt
    /// time of the last feed event (local clock).
    pub feed_staleness_ns: Option<i64>,
    /// Feed latency of that event: its local receipt minus its exchange time, including the
    /// run's configured `feed_latency_offset_ns`. The dataset part is measured, synthetic or
    /// assumed according to the dataset's provenance; the offset is always an assumption.
    pub feed_latency_ns: Option<i64>,
    /// Modeled order-entry latency.
    pub entry_latency_ns: Option<i64>,
    /// Modeled response latency of the first response.
    pub response_latency_ns: Option<i64>,
    /// Exchange clock: arrival to first fill.
    pub time_to_first_fill_ns: Option<i64>,
    /// Exchange clock: arrival to terminal state.
    pub resting_time_ns: Option<i64>,
    /// Modeled entry latency of the cancel request.
    pub cancel_entry_latency_ns: Option<i64>,
}

/// One point of the order's modeled queue trajectory, for charting.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuePoint {
    /// Exchange clock.
    pub ts: RelNs,
    pub seq: u64,
    pub ahead: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Explanation {
    pub order_id: u64,
    pub question: Question,
    pub verdict: Verdict,
    /// One-sentence answer.
    pub headline: String,
    /// Ordered reasoning, earliest first.
    pub steps: Vec<ExplainStep>,
    /// Limits of what this explanation can claim. Always includes that queue position on
    /// Market-By-Price data is modeled and that replayed orders do not affect historical prices.
    pub caveats: Vec<String>,
    pub latency: LatencyBreakdown,
    /// Modeled quantity ahead over time (empty for orders that never rested).
    pub queue_path: Vec<QueuePoint>,
}
