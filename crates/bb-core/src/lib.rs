//! BlackBox HFT shared type contract.
//!
//! Every crate and the web UI exchange data through the types defined here. The crate has no
//! dependency on the simulation engine, so data, strategy and forensics code stay engine-agnostic.
//!
//! # Conventions (binding for every producer and consumer)
//!
//! * **Time.** Internally the engine runs on absolute nanoseconds since the Unix epoch. Every
//!   timestamp in a serialized type is a [`RelNs`]: nanoseconds relative to the dataset's
//!   `origin_ns`. Absolute epoch nanoseconds (~1.6e18) exceed JavaScript's 2^53 safe-integer
//!   range, relative ones do not for any span under 104 days. The origin itself is serialized as a
//!   decimal string.
//! * **Clocks.** There are two clock domains. [`Clock::Exchange`] is the time an event happened at
//!   the (simulated) matching engine. [`Clock::Local`] is the time the strategy host saw or did
//!   something. They are never mixed silently; every event names its clock.
//! * **Prices** are integer ticks (`price = tick * tick_size`). **Quantities** are `f64` in base
//!   units and are always multiples of the instrument lot size.
//! * **Provenance.** Every recorded fact names its [`Source`]. Engine callbacks, strategy
//!   decisions, recorder bookkeeping and post-hoc derivations are never conflated.
//! * **Simulated, not real.** Nothing in these types represents a real exchange execution.

pub mod compare;
pub mod config;
pub mod dataset;
pub mod explain;
pub mod flight;
pub mod hash;
pub mod limits;
pub mod market;
pub mod platform;
pub mod result;
pub mod rng;
pub mod strategy;
pub mod sweep;
pub mod time;

pub use time::{Ns, RelNs};

/// Version of the BlackBox result/contract format. Bump on any breaking change to these types.
pub const CONTRACT_VERSION: u32 = 1;

/// Revision of BlackBox's simulation semantics: everything in the adapter, harness, ledger and
/// strategies that can change a run's result for the same configuration. It is part of every
/// run's identity, so a stored run is never returned for code that would now produce a
/// different result. `crates/bb-app/tests/golden.rs` pins a result hash; when it fails because of
/// an intended change, bump this and update the pinned hash in the same commit.
///
/// History: 1 = first release. 2 = review fixes (late cancel rejections recorded, fill_reset
/// queue events, hard risk limits, market orders rejected, drawdown halt on locally known fills).
pub const SIMULATION_REVISION: u32 = 2;

/// Name and exact version of the pinned simulation engine.
pub const ENGINE_NAME: &str = "hftbacktest";
pub const ENGINE_VERSION: &str = "0.9.4";

/// Order side. Also used for the aggressor side of a market trade.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// `+1.0` for buy, `-1.0` for sell.
    #[inline]
    pub fn sign(self) -> f64 {
        match self {
            Side::Buy => 1.0,
            Side::Sell => -1.0,
        }
    }

    #[inline]
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

/// Time in force. Semantics are the pinned engine's:
/// * `Gtc`: rests until filled or cancelled; if it crosses on arrival it takes liquidity.
/// * `Gtx`: post-only; if it would cross on arrival the exchange model expires it.
/// * `Ioc`: executes what it can on arrival, remainder expires.
/// * `Fok`: executes fully on arrival or expires.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Tif {
    Gtc,
    Gtx,
    Ioc,
    Fok,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OrdType {
    Limit,
    Market,
}

/// Order status as reported by the engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// No exchange response has been received yet.
    None,
    /// Accepted and resting.
    New,
    /// Removed by the exchange model without (full) execution: post-only cross, IOC/FOK remainder,
    /// or a request rejected before reaching the matching engine.
    Expired,
    Filled,
    Canceled,
    PartiallyFilled,
    /// Any engine status BlackBox does not model. Never produced by the supported configurations.
    Unsupported,
}

impl OrderStatus {
    /// True once no further exchange event can change the order.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OrderStatus::Expired | OrderStatus::Filled | OrderStatus::Canceled
        )
    }
}

/// Which kind of request a latency or response event refers to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReqKind {
    New,
    Cancel,
    Modify,
    /// An exchange-initiated message with no outstanding request (a passive fill notification).
    None,
}

/// Clock domain of a timestamp.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    /// Time at the simulated matching engine.
    Exchange,
    /// Time at the strategy host.
    Local,
}

/// Who produced a recorded fact. The UI must render these distinguishably.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Captured inside a callback the pinned engine invoked (queue, latency, fee or order-response
    /// hook). The values are the engine's own.
    Engine,
    /// Emitted by the BlackBox strategy harness: decisions, intents and risk rulings.
    Strategy,
    /// Recorder bookkeeping that is not an engine event (for example a truncation marker).
    Recorder,
    /// Computed after the run from recorded events and market data.
    Derived,
}

/// How strongly a statement in an explanation is supported.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Present in the historical market data.
    Observed,
    /// Output of the configured simulation model (queue position, latency, fills). An estimate.
    Modeled,
    /// Arithmetic over observed and modeled values.
    Derived,
    /// Not knowable from the available data. Stated so the UI never implies otherwise.
    Unobservable,
}
