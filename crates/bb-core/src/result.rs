//! What a completed run produces.

use serde::{Deserialize, Serialize};

use crate::{
    OrdType, OrderStatus, RelNs, Side, Tif, config::RunConfig, dataset::Quantiles,
    flight::FlightEvent,
};

/// Horizons at which post-fill mid-price markouts are measured.
pub const MARKOUT_HORIZONS_NS: [i64; 4] =
    [100_000_000, 1_000_000_000, 5_000_000_000, 30_000_000_000];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetRef {
    pub id: String,
    /// SHA-256 of the dataset's event file.
    pub sha256: String,
    pub rows: u64,
    pub origin_ns: String,
}

/// Everything needed to reproduce a run and to detect that two runs are the same experiment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunManifest {
    pub contract_version: u32,
    /// First 16 hex characters of `config_sha256`. Identical inputs give an identical id.
    pub run_id: String,
    pub blackbox_version: String,
    /// [`crate::SIMULATION_REVISION`] of the code that produced the run. Part of the run
    /// identity. Runs stored before the field existed read back as 0.
    #[serde(default)]
    pub simulation_revision: u32,
    pub engine: EngineInfo,
    /// Platform class of the binary that ran the simulation ([`crate::platform::platform_class`],
    /// e.g. `"linux-x86_64"`, `"windows-x86_64-gnu"`). Runs stored before platform classes
    /// existed hold `"<os>-<arch>"`. A stored result is reused only on the same class.
    pub platform: String,
    /// `"debug"` or `"release"`.
    pub profile: String,
    pub dataset: DatasetRef,
    /// Includes the seed.
    pub config: RunConfig,
    /// SHA-256 over (contract version, engine name+version, dataset sha256, config). Does not
    /// cover platform, profile or timing.
    pub config_sha256: String,
    /// SHA-256 over the flight tape, fills and equity samples. Two runs with equal
    /// `config_sha256` on the same platform class must have equal `result_sha256`.
    pub result_sha256: String,
    /// Wall-clock facts. Informational only; excluded from both hashes.
    pub created_unix_ms: u64,
    pub wall_time_ms: u64,
}

/// One simulated execution with its accounting effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FillRecord {
    /// `seq` of the corresponding `ExchFill` on the tape.
    pub seq: u64,
    pub order_id: u64,
    /// Exchange clock.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub ts: RelNs,
    pub side: Side,
    pub price_tick: i64,
    pub qty: f64,
    pub maker: bool,
    pub fee: f64,
    pub value: f64,
    /// Position after this fill (all simulated fills up to here, exchange clock).
    pub position_after: f64,
    /// Realized P&L booked by this fill under average-cost accounting, before fees.
    pub realized_pnl: f64,
    /// Derived: mid price (fractional ticks) of the dataset's book at the fill's exchange time.
    pub mid_tick: Option<f64>,
    /// Derived: `side.sign() * (mid(t + h) - price_tick)` in ticks for each horizon in
    /// [`MARKOUT_HORIZONS_NS`]. Positive means the market moved in the fill's favour. `None` when
    /// the dataset ends before the horizon or the book was not valid then.
    pub markout_ticks: Vec<Option<f64>>,
}

/// Periodic account snapshot on the local clock.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EquitySample {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub ts: RelNs,
    /// Strategy tick the sample was taken on. A run's last sample is its closing sample: it is
    /// taken one interval after the final tick, once everything that followed that tick has been
    /// processed, and carries `tick_no` equal to the number of ticks evaluated.
    pub tick_no: u64,
    /// Local mid used for marking (fractional ticks). `None` until the local book has two sides.
    pub mid_tick: Option<f64>,
    /// Position from all simulated fills that have happened at the exchange by `ts`.
    pub position: f64,
    /// Position the strategy knew about at `ts` (fill notifications received).
    pub known_position: f64,
    /// Cumulative signed cash flow from fills, before fees.
    pub cash: f64,
    pub fees: f64,
    pub realized_pnl: f64,
    /// `position * (mid - average entry price)`; 0 while flat or unmarked.
    pub unrealized_pnl: f64,
    /// `realized_pnl + unrealized_pnl - fees`.
    pub equity: f64,
    pub open_orders: u32,
}

/// Lifecycle summary of one order, assembled from the tape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrderRecord {
    pub order_id: u64,
    pub tick_no: u64,
    pub slot: u8,
    pub side: Side,
    pub price_tick: i64,
    pub qty: f64,
    pub tif: Tif,
    pub ord_type: OrdType,
    pub reason: String,
    pub outcome: OrderOutcome,
    /// Local clock: when the strategy decided.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub decision_ts: RelNs,
    /// Exchange clock: first exchange-side event for the order (arrival). `None` if it never
    /// reached the exchange (risk-blocked, refused, or data ended first).
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub exch_arrival_ts: Option<RelNs>,
    /// Local clock: first response received.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub first_response_ts: Option<RelNs>,
    /// Modeled quantity ahead when the order started resting.
    pub queue_ahead_at_entry: Option<f64>,
    /// Smallest modeled quantity ahead ever recorded.
    pub min_queue_ahead: Option<f64>,
    /// Market trade quantity that printed at the order's price while it rested.
    pub traded_at_price_qty: f64,
    pub filled_qty: f64,
    pub maker_qty: f64,
    pub taker_qty: f64,
    /// Quantity-weighted average fill price in ticks.
    pub avg_fill_tick: Option<f64>,
    pub fees: f64,
    pub fill_count: u32,
    /// Exchange clock.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub first_fill_ts: Option<RelNs>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub last_fill_ts: Option<RelNs>,
    /// Local clock: when a cancel request was sent.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub cancel_sent_ts: Option<RelNs>,
    pub cancel_reason: Option<String>,
    /// The exchange rejected the cancel because the order was already gone.
    pub cancel_rejected: bool,
    /// Exchange clock: when the order became terminal at the exchange.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub terminal_ts: Option<RelNs>,
    /// Last exchange status observed.
    pub final_status: OrderStatus,
    /// First and last tape `seq` belonging to this order.
    pub first_seq: u64,
    pub last_seq: u64,
}

/// Coarse classification used for filtering and for scenario alignment.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OrderOutcome {
    /// Stopped by a risk rule; never sent.
    RiskBlocked,
    /// The engine refused the request synchronously; never sent.
    Refused,
    /// Sent, but the data ended before the exchange processed it.
    InFlightAtEnd,
    /// Filled completely.
    Filled,
    /// Partly filled, then cancelled, expired or still working at the end.
    PartiallyFilled,
    /// Cancelled with no fill.
    Canceled,
    /// Expired by the exchange model with no fill (post-only would cross, IOC/FOK found nothing).
    Expired,
    /// Still resting, unfilled, when the data ended.
    WorkingAtEnd,
}

/// One markout horizon aggregated over fills.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarkoutStat {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub horizon_ns: i64,
    /// Fills for which the markout could be measured.
    pub fills: u64,
    /// Quantity-weighted mean markout in ticks. Negative means adverse selection.
    pub mean_ticks: Option<f64>,
}

/// Run-level measured outcomes. Every number is computed from the tape, fills and equity samples.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Metrics {
    // Orders
    pub orders_intended: u64,
    pub orders_risk_blocked: u64,
    pub orders_refused: u64,
    pub orders_submitted: u64,
    pub orders_filled: u64,
    pub orders_partially_filled: u64,
    pub orders_canceled: u64,
    pub orders_expired: u64,
    pub orders_working_at_end: u64,
    pub cancels_sent: u64,
    pub cancels_rejected: u64,
    // Fills
    pub fill_count: u64,
    pub maker_fill_count: u64,
    pub taker_fill_count: u64,
    /// Orders with at least one fill divided by orders submitted.
    pub fill_ratio_orders: Option<f64>,
    /// Filled quantity divided by submitted quantity.
    pub fill_ratio_qty: Option<f64>,
    // Timing (nanoseconds)
    /// Exchange clock: arrival to terminal state, for orders that rested.
    pub order_age_ns: Option<Quantiles>,
    /// Exchange clock: arrival to first fill.
    pub time_to_first_fill_ns: Option<Quantiles>,
    pub entry_latency_ns: Option<Quantiles>,
    pub response_latency_ns: Option<Quantiles>,
    /// Local clock: request sent to first response received, new orders only.
    pub round_trip_ns: Option<Quantiles>,
    // Activity
    /// Total filled quantity.
    pub volume: f64,
    /// Total traded value in quote currency.
    pub turnover: f64,
    pub fees: f64,
    // Inventory
    pub final_position: f64,
    pub max_long: f64,
    pub max_short: f64,
    /// Mean of `|position|` over equity samples.
    pub mean_abs_position: f64,
    // P&L (quote currency)
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    /// `realized_pnl + unrealized_pnl - fees` at the last sample.
    pub net_pnl: f64,
    /// Largest peak-to-trough fall of sampled equity.
    pub max_drawdown: f64,
    // Execution quality
    /// Sum over maker fills of `side.sign() * (mid_at_fill - fill_price) * qty` in quote
    /// currency: the half-spread earned at the moment of each passive fill.
    pub spread_capture: f64,
    /// The same, quantity-weighted, in ticks.
    pub spread_capture_ticks: Option<f64>,
    /// One entry per horizon in [`MARKOUT_HORIZONS_NS`].
    pub markouts: Vec<MarkoutStat>,
    // Risk
    pub risk_blocks: u64,
    pub risk_halted: bool,
}

/// Cross-check of BlackBox's own ledger against the engine's local-side account state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EngineStateCheck {
    /// Position, cash, fee and trade-count agreement at the end of the run.
    pub agrees: bool,
    pub ledger_position: f64,
    pub engine_position: f64,
    pub ledger_cash: f64,
    pub engine_balance: f64,
    pub ledger_fees: f64,
    pub engine_fee: f64,
    pub ledger_fills: u64,
    pub engine_num_trades: i64,
    /// Explains an expected disagreement (see docs/EXECUTION_MODEL.md), or is empty.
    pub note: String,
}

/// A complete run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunResult {
    pub manifest: RunManifest,
    pub metrics: Metrics,
    pub engine_check: EngineStateCheck,
    /// Number of strategy ticks evaluated.
    pub ticks: u64,
    pub orders: Vec<OrderRecord>,
    pub fills: Vec<FillRecord>,
    pub equity: Vec<EquitySample>,
    pub tape: Vec<FlightEvent>,
}

/// Listing entry for a stored run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunSummary {
    pub manifest: RunManifest,
    pub metrics: Metrics,
    pub ticks: u64,
    pub order_count: u64,
    pub fill_count: u64,
    pub tape_len: u64,
    /// User-facing label, e.g. a scenario name. Not part of any hash.
    pub label: String,
}
