//! Counterfactual comparison of two runs and bounded sensitivity grids.
//!
//! A comparison never attributes a P&L difference to one parameter. It first checks whether the
//! two runs can be compared order by order at all ([`Eligibility`]), then reports which parameters
//! differ, which decisions, outcomes and timings diverged, and which statements the two
//! simulations support.

use serde::{Deserialize, Serialize};

use crate::{
    Basis, OrdType, OrderStatus, RelNs, Side, Tif,
    config::{QueueModelConfig, RunConfig},
    result::{Metrics, OrderOutcome},
};

/// Hard upper bound on the number of simulations in one sensitivity grid.
pub const MAX_GRID_CELLS: usize = 100;

/// One configuration field that differs between two runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfigDiff {
    /// Dotted path into the serialized [`RunConfig`], e.g. `execution.order_latency.entry_ns`.
    pub path: String,
    pub a: serde_json::Value,
    pub b: serde_json::Value,
}

/// One measured outcome side by side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MetricRow {
    /// Stable key, e.g. `net_pnl`.
    pub key: String,
    pub label: String,
    /// `"quote"`, `"qty"`, `"count"`, `"ratio"`, `"ns"` or `"ticks"`.
    pub unit: String,
    pub a: Option<f64>,
    pub b: Option<f64>,
    /// `b - a` when both exist.
    pub delta: Option<f64>,
}

/// What an eligibility difference does to a comparison.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EligibilityEffect {
    /// The same order id does not name the same decision in both runs (different market,
    /// instrument, strategy, decision grid or simulator revision). No per-order alignment is
    /// produced; metric deltas are still returned, labelled not like-for-like.
    NoAlignment,
    /// Order ids still name the same decisions, but metric deltas and pair differences also
    /// reflect this field, not only the configured assumptions.
    NotLikeForLike,
}

/// One identity field that differs between the runs (or could not be verified).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EligibilityReason {
    /// Dotted name, e.g. `strategy.interval_ns`, `dataset.sha256`, `instrument.tick_size`.
    pub field: String,
    /// The value in run A; `null` when unknown.
    pub a: serde_json::Value,
    pub b: serde_json::Value,
    pub effect: EligibilityEffect,
    pub explanation: String,
}

/// Whether two runs can be compared order by order and like for like. Checked before any
/// per-order alignment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Eligibility {
    /// Per-order alignment was produced: no reason has [`EligibilityEffect::NoAlignment`].
    pub aligned: bool,
    /// No reason at all: metric deltas compare the same market, instrument, strategy kind,
    /// decision grid, sampling and simulator.
    pub like_for_like: bool,
    pub reasons: Vec<EligibilityReason>,
}

/// What one scenario did with a decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DecisionOutcome {
    pub side: Side,
    pub price_tick: i64,
    pub qty: f64,
    pub tif: Tif,
    pub ord_type: OrdType,
    /// Local clock.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub decision_ts: RelNs,
    pub outcome: OrderOutcome,
    pub final_status: OrderStatus,
    pub filled_qty: f64,
    pub avg_fill_tick: Option<f64>,
    pub maker_qty: f64,
    pub taker_qty: f64,
    pub fees: f64,
    pub fill_count: u32,
    pub queue_ahead_at_entry: Option<f64>,
    /// Exchange clock.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub exch_arrival_ts: Option<RelNs>,
    /// Local clock.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub first_response_ts: Option<RelNs>,
    /// Exchange clock.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub first_fill_ts: Option<RelNs>,
    /// Exchange clock.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub last_fill_ts: Option<RelNs>,
    /// Local clock.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub cancel_sent_ts: Option<RelNs>,
    /// Exchange clock: when the order became terminal (its completion).
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub terminal_ts: Option<RelNs>,
}

/// How a pair of same-id orders relate. For two present orders the classes are checked in the
/// order listed; the first group that differs decides the class. `identical` therefore means
/// that nothing below differs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PairClass {
    /// Same decision (side, price, quantity, time in force, order type) and decision time, same
    /// exchange arrival, first response, cancel and terminal times, same final status and
    /// outcome, every fill equal in time, price, quantity, maker/taker and fee, and the same
    /// recorded lifecycle events.
    Identical,
    /// Decision, timing and every fill are identical, but the recorded lifecycle differs (for
    /// example the modeled queue ahead, queue updates, a rejected cancel or the market the
    /// strategy saw): identical fills under distinct causal histories.
    SameFillsDifferentHistory,
    /// Decision, timing and fills (time, price, quantity, maker/taker) are identical; only the
    /// fees charged differ.
    SameFillsDifferentFees,
    /// Same decision, outcome and fills (price, quantity, maker/taker), but at least one
    /// timestamp differs: decision, exchange arrival, first response, a fill, cancel or terminal
    /// time (or exists in only one run).
    SameOutcomeDifferentTiming,
    /// The strategy made the same decision in both scenarios but the simulated execution
    /// differed: outcome, final status, filled quantity, fill count, or a fill's price, quantity
    /// or maker/taker side.
    SameDecisionDifferentOutcome,
    /// The strategy decided differently at this tick/slot: its inputs (position, working orders)
    /// had already diverged.
    DifferentDecision,
    /// The decision exists only in scenario A.
    OnlyA,
    /// The decision exists only in scenario B.
    OnlyB,
}

impl PairClass {
    /// The decision or its fills differ (or exist in one run only), as opposed to a pair whose
    /// decision and fills agree and that differs only in timing, fees or recorded history.
    pub fn is_outcome_divergence(self) -> bool {
        matches!(
            self,
            PairClass::SameDecisionDifferentOutcome
                | PairClass::DifferentDecision
                | PairClass::OnlyA
                | PairClass::OnlyB
        )
    }
}

/// Per-pair timing deltas, B minus A in nanoseconds. A field is `null` unless both orders have
/// the timestamp.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairTiming {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub decision_ns: Option<i64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub exch_arrival_ns: Option<i64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub first_response_ns: Option<i64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub first_fill_ns: Option<i64>,
    /// Terminal (completion) time at the exchange.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub completion_ns: Option<i64>,
}

/// Distribution of one timing delta (B minus A, nanoseconds) over the pairs that have it.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeltaStats {
    /// Pairs where both orders have the timestamp.
    pub count: u64,
    /// Of those, pairs where the delta is not zero.
    pub nonzero: u64,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub min: Option<i64>,
    /// Mean of the two middle values when `count` is even.
    pub median: Option<f64>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub max: Option<i64>,
}

/// Timing deltas summarised over every pair present in both runs with the same decision (all
/// classes except `different_decision`, `only_a` and `only_b`), identical pairs included.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TimingSummary {
    pub decision: DeltaStats,
    pub exch_arrival: DeltaStats,
    pub first_response: DeltaStats,
    pub first_fill: DeltaStats,
    pub completion: DeltaStats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrderPair {
    pub order_id: u64,
    pub tick_no: u64,
    /// Local-clock decision time in A (in B for `only_b`). Each side's own time is in `a`/`b`.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub decision_ts: RelNs,
    pub class: PairClass,
    /// Every compared field that differs, most significant group first (e.g. `price_tick`,
    /// `outcome`, `fills`, `exch_arrival_ts`, `fill_ts`, `fees`, `queue_ahead_at_entry`,
    /// `lifecycle_events`). Empty for `identical`, `only_a` and `only_b`.
    pub differences: Vec<String>,
    /// `null` unless the order exists in both runs.
    pub timing: Option<PairTiming>,
    pub a: Option<DecisionOutcome>,
    pub b: Option<DecisionOutcome>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Alignment {
    pub identical: u64,
    pub same_fills_different_history: u64,
    pub same_fills_different_fees: u64,
    pub same_outcome_different_timing: u64,
    pub same_decision_different_outcome: u64,
    pub different_decision: u64,
    pub only_a: u64,
    pub only_b: u64,
    /// The earliest non-identical pair in decision order (tick, slot). Every pair before it is
    /// `identical`; everything after it may differ as a knock-on effect.
    pub first_divergence: Option<OrderPair>,
    /// The earliest pair whose decision or fills differ (see [`PairClass::is_outcome_divergence`]).
    /// Pairs before it may differ in timing, fees or recorded history, never in decision or fills.
    pub first_outcome_divergence: Option<OrderPair>,
    pub timing: TimingSummary,
    /// Non-identical pairs in decision order, capped at `pairs_cap`.
    pub pairs: Vec<OrderPair>,
    pub pairs_cap: u64,
    pub pairs_truncated: bool,
}

/// A statement about the comparison and how well it is supported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InterpretationNote {
    pub basis: Basis,
    pub statement: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Comparison {
    pub run_a: String,
    pub run_b: String,
    pub label_a: String,
    pub label_b: String,
    pub eligibility: Eligibility,
    pub config_diffs: Vec<ConfigDiff>,
    /// Returned even when `eligibility.like_for_like` is false; they are then not like-for-like.
    pub metrics: Vec<MetricRow>,
    /// `None` when `eligibility.aligned` is false: order ids would not name the same decisions.
    pub alignment: Option<Alignment>,
    /// Supported and unsupported readings of the difference.
    pub interpretation: Vec<InterpretationNote>,
}

/// A parameter swept along one axis of a sensitivity grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GridAxis {
    /// Sets both entry and response order latency to each value: the location parameter of the
    /// model (`OrderLatencyConfig::with_base_ns`), keeping jitter and shape parameters.
    OrderLatencyNs {
        #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
        values: Vec<i64>,
    },
    /// Sets only the entry latency (new orders and cancels), the same way.
    EntryLatencyNs {
        #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
        values: Vec<i64>,
    },
    FeedLatencyOffsetNs {
        #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
        values: Vec<i64>,
    },
    QueueModel {
        values: Vec<QueueModelConfig>,
    },
    MakerFee {
        values: Vec<f64>,
    },
    TakerFee {
        values: Vec<f64>,
    },
}

impl GridAxis {
    pub fn len(&self) -> usize {
        match self {
            GridAxis::OrderLatencyNs { values }
            | GridAxis::EntryLatencyNs { values }
            | GridAxis::FeedLatencyOffsetNs { values } => values.len(),
            GridAxis::QueueModel { values } => values.len(),
            GridAxis::MakerFee { values } | GridAxis::TakerFee { values } => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn name(&self) -> &'static str {
        match self {
            GridAxis::OrderLatencyNs { .. } => "order_latency_ns",
            GridAxis::EntryLatencyNs { .. } => "entry_latency_ns",
            GridAxis::FeedLatencyOffsetNs { .. } => "feed_latency_offset_ns",
            GridAxis::QueueModel { .. } => "queue_model",
            GridAxis::MakerFee { .. } => "maker_fee",
            GridAxis::TakerFee { .. } => "taker_fee",
        }
    }

    /// Display label of the `i`-th value.
    pub fn value_label(&self, i: usize) -> String {
        match self {
            GridAxis::OrderLatencyNs { values }
            | GridAxis::EntryLatencyNs { values }
            | GridAxis::FeedLatencyOffsetNs { values } => Self::duration_label(values[i]),
            GridAxis::QueueModel { values } => values[i].label(),
            GridAxis::MakerFee { values } | GridAxis::TakerFee { values } => {
                // Fee fractions in basis points, rounded to hide binary noise (0.3 bp, not
                // 0.000029999999999999997).
                format!("{} bp", (values[i] * 1e4 * 1e6).round() / 1e6)
            }
        }
    }

    /// `1500000` -> `"1.5 ms"`: the largest unit that keeps the value exact to three decimals.
    fn duration_label(ns: i64) -> String {
        let (scale, unit) = match ns.unsigned_abs() {
            n if n >= 1_000_000_000 && n % 1_000_000 == 0 => (1e9, "s"),
            n if n >= 1_000_000 && n % 1_000 == 0 => (1e6, "ms"),
            n if n >= 1_000 => (1e3, "us"),
            _ => (1.0, "ns"),
        };
        let v = ns as f64 / scale;
        let s = format!("{v:.3}");
        format!("{} {unit}", s.trim_end_matches('0').trim_end_matches('.'))
    }

    /// Returns `config` with this axis' `i`-th value applied.
    pub fn apply(&self, config: &RunConfig, i: usize) -> RunConfig {
        let mut c = config.clone();
        match self {
            GridAxis::OrderLatencyNs { values } => {
                let v = values[i];
                c.execution.order_latency =
                    c.execution.order_latency.with_base_ns(Some(v), Some(v));
            }
            GridAxis::EntryLatencyNs { values } => {
                let v = values[i];
                c.execution.order_latency = c.execution.order_latency.with_base_ns(Some(v), None);
            }
            GridAxis::FeedLatencyOffsetNs { values } => {
                c.execution.feed_latency_offset_ns = values[i];
            }
            GridAxis::QueueModel { values } => c.execution.queue_model = values[i],
            GridAxis::MakerFee { values } => c.execution.fees.maker = values[i],
            GridAxis::TakerFee { values } => c.execution.fees.taker = values[i],
        }
        c
    }
}

/// A bounded two-dimensional sweep around a base configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GridSpec {
    pub base: RunConfig,
    pub x: GridAxis,
    pub y: GridAxis,
}

impl GridSpec {
    pub fn cells(&self) -> usize {
        self.x.len().saturating_mul(self.y.len())
    }

    /// Human-readable problems; empty means the grid may run.
    ///
    /// The structural checks run first and return early when they fail, so an oversized grid is
    /// rejected without visiting a single cell.
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        if self.x.is_empty() || self.y.is_empty() {
            e.push("both grid axes need at least one value".into());
        }
        if self.cells() > MAX_GRID_CELLS {
            e.push(format!(
                "grid has {} cells; the limit is {MAX_GRID_CELLS}",
                self.cells()
            ));
        }
        if !e.is_empty() {
            return e;
        }
        e.extend(self.base.validate());
        if self.x.name() == self.y.name() {
            e.push("grid axes must sweep different parameters".into());
        }
        let overlapping = |a: &GridAxis, b: &GridAxis| {
            matches!(
                (a, b),
                (
                    GridAxis::OrderLatencyNs { .. },
                    GridAxis::EntryLatencyNs { .. }
                ) | (
                    GridAxis::EntryLatencyNs { .. },
                    GridAxis::OrderLatencyNs { .. }
                )
            )
        };
        if overlapping(&self.x, &self.y) {
            e.push("order_latency_ns and entry_latency_ns both set the entry latency".into());
        }
        for xi in 0..self.x.len() {
            for yi in 0..self.y.len() {
                let c = self.y.apply(&self.x.apply(&self.base, xi), yi);
                for problem in c.validate() {
                    let msg = format!("cell ({xi},{yi}): {problem}");
                    if !e.contains(&msg) {
                        e.push(msg);
                    }
                }
            }
        }
        e
    }
}

/// One cell of a grid. A cell with `metrics: None` did not complete and must be rendered as
/// missing, never interpolated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GridCell {
    pub xi: u32,
    pub yi: u32,
    /// Run id of the completed simulation backing this cell.
    pub run_id: Option<String>,
    pub result_sha256: Option<String>,
    pub metrics: Option<Metrics>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GridResult {
    pub spec: GridSpec,
    pub x_labels: Vec<String>,
    pub y_labels: Vec<String>,
    /// Row-major: `cells[yi * x_len + xi]`.
    pub cells: Vec<GridCell>,
    pub wall_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_labels_are_human_readable() {
        let lat = GridAxis::OrderLatencyNs {
            values: vec![0, 1_500, 1_000_000, 20_000_000, 1_500_000_000],
        };
        let labels: Vec<String> = (0..5).map(|i| lat.value_label(i)).collect();
        assert_eq!(labels, ["0 ns", "1.5 us", "1 ms", "20 ms", "1.5 s"]);
        let fee = GridAxis::MakerFee {
            values: vec![0.00003, -0.0001, 0.0002],
        };
        let labels: Vec<String> = (0..3).map(|i| fee.value_label(i)).collect();
        assert_eq!(labels, ["0.3 bp", "-1 bp", "2 bp"]);
    }
}
