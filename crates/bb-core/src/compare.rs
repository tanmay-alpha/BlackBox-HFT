//! Counterfactual comparison of two runs and bounded sensitivity grids.
//!
//! A comparison never attributes a P&L difference to one parameter. It reports which parameters
//! differ, which decisions and outcomes diverged, and which statements the two simulations support.

use serde::{Deserialize, Serialize};

use crate::{
    config::{QueueModelConfig, RunConfig},
    result::{Metrics, OrderOutcome},
    Basis, RelNs, Side,
};

/// Hard upper bound on the number of simulations in one sensitivity grid.
pub const MAX_GRID_CELLS: usize = 100;

/// One configuration field that differs between two runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigDiff {
    /// Dotted path into the serialized [`RunConfig`], e.g. `execution.order_latency.entry_ns`.
    pub path: String,
    pub a: serde_json::Value,
    pub b: serde_json::Value,
}

/// One measured outcome side by side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

/// What one scenario did with a decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionOutcome {
    pub side: Side,
    pub price_tick: i64,
    pub qty: f64,
    pub outcome: OrderOutcome,
    pub filled_qty: f64,
    pub avg_fill_tick: Option<f64>,
    pub queue_ahead_at_entry: Option<f64>,
    pub exch_arrival_ts: Option<RelNs>,
    pub first_fill_ts: Option<RelNs>,
}

/// How a pair of same-id orders relate.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairClass {
    /// Same decision, same outcome and filled quantity.
    Identical,
    /// The strategy made the same decision (side, price, quantity) in both scenarios but the
    /// simulated execution differed. This is a direct effect of the execution assumptions on
    /// this order, given everything that had already diverged before it.
    SameDecisionDifferentOutcome,
    /// The strategy decided differently at this tick/slot: its inputs (position, working orders)
    /// had already diverged.
    DifferentDecision,
    /// The decision exists only in scenario A.
    OnlyA,
    /// The decision exists only in scenario B.
    OnlyB,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderPair {
    pub order_id: u64,
    pub tick_no: u64,
    /// Local-clock decision time (identical in both scenarios by construction of the tick grid).
    pub decision_ts: RelNs,
    pub class: PairClass,
    pub a: Option<DecisionOutcome>,
    pub b: Option<DecisionOutcome>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub identical: u64,
    pub same_decision_different_outcome: u64,
    pub different_decision: u64,
    pub only_a: u64,
    pub only_b: u64,
    /// The earliest non-identical pair. Everything before it is the same in both scenarios;
    /// everything after it may differ as a knock-on effect.
    pub first_divergence: Option<OrderPair>,
    /// Non-identical pairs in decision order, capped at `pairs_cap`.
    pub pairs: Vec<OrderPair>,
    pub pairs_cap: u64,
    pub pairs_truncated: bool,
}

/// A statement about the comparison and how well it is supported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterpretationNote {
    pub basis: Basis,
    pub statement: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub run_a: String,
    pub run_b: String,
    pub label_a: String,
    pub label_b: String,
    /// Both runs used the same dataset hash.
    pub same_dataset: bool,
    /// Both runs used the same strategy configuration.
    pub same_strategy: bool,
    pub config_diffs: Vec<ConfigDiff>,
    pub metrics: Vec<MetricRow>,
    pub alignment: Alignment,
    /// Supported and unsupported readings of the difference.
    pub interpretation: Vec<InterpretationNote>,
}

/// A parameter swept along one axis of a sensitivity grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GridAxis {
    /// Sets both entry and response order latency to each value.
    OrderLatencyNs { values: Vec<i64> },
    /// Sets only the entry latency.
    EntryLatencyNs { values: Vec<i64> },
    FeedLatencyOffsetNs { values: Vec<i64> },
    QueueModel { values: Vec<QueueModelConfig> },
    MakerFee { values: Vec<f64> },
    TakerFee { values: Vec<f64> },
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
            | GridAxis::FeedLatencyOffsetNs { values } => values[i].to_string(),
            GridAxis::QueueModel { values } => values[i].label(),
            GridAxis::MakerFee { values } | GridAxis::TakerFee { values } => values[i].to_string(),
        }
    }

    /// Returns `config` with this axis' `i`-th value applied.
    pub fn apply(&self, config: &RunConfig, i: usize) -> RunConfig {
        use crate::config::OrderLatencyConfig as L;
        let mut c = config.clone();
        match self {
            GridAxis::OrderLatencyNs { values } => {
                let v = values[i];
                c.execution.order_latency = match c.execution.order_latency {
                    L::Constant { .. } => L::Constant {
                        entry_ns: v,
                        response_ns: v,
                    },
                    L::Jitter { jitter_ns, .. } => L::Jitter {
                        entry_ns: v,
                        response_ns: v,
                        jitter_ns,
                    },
                };
            }
            GridAxis::EntryLatencyNs { values } => {
                let v = values[i];
                c.execution.order_latency = match c.execution.order_latency {
                    L::Constant { response_ns, .. } => L::Constant {
                        entry_ns: v,
                        response_ns,
                    },
                    L::Jitter {
                        response_ns,
                        jitter_ns,
                        ..
                    } => L::Jitter {
                        entry_ns: v,
                        response_ns,
                        jitter_ns,
                    },
                };
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
pub struct GridSpec {
    pub base: RunConfig,
    pub x: GridAxis,
    pub y: GridAxis,
}

impl GridSpec {
    pub fn cells(&self) -> usize {
        self.x.len() * self.y.len()
    }

    /// Human-readable problems; empty means the grid may run.
    pub fn validate(&self) -> Vec<String> {
        let mut e = self.base.validate();
        if self.x.is_empty() || self.y.is_empty() {
            e.push("both grid axes need at least one value".into());
        }
        if self.x.name() == self.y.name() {
            e.push("grid axes must sweep different parameters".into());
        }
        let overlapping = |a: &GridAxis, b: &GridAxis| {
            matches!(
                (a, b),
                (GridAxis::OrderLatencyNs { .. }, GridAxis::EntryLatencyNs { .. })
                    | (GridAxis::EntryLatencyNs { .. }, GridAxis::OrderLatencyNs { .. })
            )
        };
        if overlapping(&self.x, &self.y) {
            e.push("order_latency_ns and entry_latency_ns both set the entry latency".into());
        }
        if self.cells() > MAX_GRID_CELLS {
            e.push(format!(
                "grid has {} cells; the limit is {MAX_GRID_CELLS}",
                self.cells()
            ));
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
pub struct GridResult {
    pub spec: GridSpec,
    pub x_labels: Vec<String>,
    pub y_labels: Vec<String>,
    /// Row-major: `cells[yi * x_len + xi]`.
    pub cells: Vec<GridCell>,
    pub wall_time_ms: u64,
}
