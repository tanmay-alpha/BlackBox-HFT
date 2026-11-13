//! Everything a user can choose for a simulation run. A [`RunConfig`] plus a dataset hash and the
//! engine version fully determines a run's result.

use serde::{Deserialize, Serialize};

use crate::{OrdType, Side, Tif};

/// Complete, self-contained description of one simulation run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunConfig {
    /// Dataset identifier (directory name under the dataset root).
    pub dataset_id: String,
    pub strategy: StrategyConfig,
    pub execution: ExecutionConfig,
    pub risk: RiskConfig,
    pub record: RecordConfig,
    /// Seed for every stochastic component (currently only [`OrderLatencyConfig::Jitter`]).
    pub seed: u64,
}

/// The modeled execution assumptions. These are what counterfactual scenarios vary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionConfig {
    pub exchange_model: ExchangeModel,
    pub queue_model: QueueModelConfig,
    pub order_latency: OrderLatencyConfig,
    /// Added to every feed event's local receipt time (engine `latency_offset`). Zero keeps the
    /// dataset's own exchange-to-local latency. Must keep `local_ts > exch_ts` for every event.
    pub feed_latency_offset_ns: i64,
    pub fees: FeeConfig,
}

/// Which pinned-engine exchange model matches orders.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeModel {
    /// `NoPartialFillExchange`: a resting order fills completely as soon as its fill condition is
    /// met. Optimistic about size, simplest to reason about.
    NoPartialFill,
    /// `PartialFillExchange`: a resting order at the front of the queue fills by the remaining
    /// traded quantity; a taking order consumes displayed depth level by level.
    PartialFill,
}

/// Queue-position model for resting orders on Market-By-Price data. All are estimates: L2 data
/// does not show individual orders, so position in the queue is never observed.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueueModelConfig {
    /// Engine `RiskAdverseQueueModel`: the queue ahead shrinks only when trades print at the
    /// order's price. Cancellations ahead are never credited. Most conservative.
    RiskAdverse,
    /// Engine `ProbQueueModel<LogProbQueueFunc>`: a decrease in level quantity is split between
    /// ahead and behind with `p(behind) = f(back) / (f(back) + f(front))`, `f(x) = ln(1 + x)`.
    LogProb,
    /// Engine `ProbQueueModel<LogProbQueueFunc2>`: `p(behind) = f(back) / f(back + front)`,
    /// `f(x) = ln(1 + x)`.
    LogProb2,
    /// Engine `ProbQueueModel<PowerProbQueueFunc>`: `f(x) = x^n`,
    /// `p(behind) = f(back) / (f(back) + f(front))`.
    PowerProb { n: f64 },
    /// Engine `ProbQueueModel<PowerProbQueueFunc2>`: `f(x) = x^n`,
    /// `p(behind) = f(back) / f(back + front)`.
    PowerProb2 { n: f64 },
    /// Engine `ProbQueueModel<PowerProbQueueFunc3>`: `p(behind) = 1 - (front / (front + back))^n`.
    PowerProb3 { n: f64 },
}

impl QueueModelConfig {
    /// Short stable label for tables and grid axes.
    pub fn label(&self) -> String {
        match self {
            QueueModelConfig::RiskAdverse => "risk_adverse".into(),
            QueueModelConfig::LogProb => "log_prob".into(),
            QueueModelConfig::LogProb2 => "log_prob2".into(),
            QueueModelConfig::PowerProb { n } => format!("power_prob(n={n})"),
            QueueModelConfig::PowerProb2 { n } => format!("power_prob2(n={n})"),
            QueueModelConfig::PowerProb3 { n } => format!("power_prob3(n={n})"),
        }
    }
}

/// Order-entry and order-response latency. Entry latency applies to new orders and cancels alike.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrderLatencyConfig {
    /// Engine `ConstantLatency`.
    Constant { entry_ns: i64, response_ns: i64 },
    /// BlackBox model implementing the engine's `LatencyModel` trait: the base latency plus a
    /// seeded uniform draw from `[0, jitter_ns]`. The draw is a pure function of the seed and of
    /// what is delayed (entry: order id and request time; response: exchange timestamp), never a
    /// sequential stream, so results do not depend on the engine's internal iteration order. The
    /// engine's order bus keeps messages in send order, so jitter never reorders them.
    Jitter {
        entry_ns: i64,
        response_ns: i64,
        jitter_ns: i64,
    },
}

/// Fees as a fraction of traded value (engine `TradingValueFeeModel<CommonFees>`). Negative means
/// a rebate. Example: `0.0002` is 2 basis points.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeeConfig {
    pub maker: f64,
    pub taker: f64,
}

/// Pre-trade risk rules enforced by the strategy harness before a request reaches the engine.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskConfig {
    /// Reject an order whose fill would take `|position|` above this. Uses the locally known
    /// position plus all working orders on the same side (worst case).
    pub max_position: f64,
    /// Reject any single order larger than this.
    pub max_order_qty: f64,
    /// Reject new orders while this many are already working.
    pub max_open_orders: u32,
    /// Halt (cancel everything, stop quoting) once equity falls this far below its running peak.
    /// `None` disables the rule.
    pub max_drawdown: Option<f64>,
}

/// What the flight recorder keeps.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordConfig {
    /// Equity, position and market are sampled every this many strategy ticks.
    pub sample_every_ticks: u32,
    /// Soft cap on queue-update events kept per order. Updates caused by trades are always kept.
    /// Once an order has this many updates, a depth-driven update is kept only if it moves the
    /// estimate by at least one lot; the rest are counted and reported by one explicit
    /// `QueueTruncated` marker. Nothing is dropped silently.
    pub max_queue_updates_per_order: u32,
}

impl Default for RecordConfig {
    fn default() -> Self {
        Self {
            sample_every_ticks: 1,
            max_queue_updates_per_order: 2_000,
        }
    }
}

/// Strategy selection. Every strategy is evaluated on a fixed local-time grid of `interval_ns`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// Local-clock time between strategy evaluations.
    pub interval_ns: i64,
    pub kind: StrategyKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StrategyKind {
    /// Joins the best bid and best ask with post-only orders and skews away from inventory.
    TouchMaker(TouchMakerParams),
    /// Avellaneda-Stoikov (2008) reservation price and spread.
    AvellanedaStoikov(AvellanedaStoikovParams),
    /// Takes liquidity when top-of-book imbalance exceeds a threshold, exits after a holding time.
    ImbalanceTaker(ImbalanceTakerParams),
    /// Places exactly the listed orders. Used for curated forensics demos and tests.
    Scripted(ScriptedParams),
}

impl StrategyKind {
    pub fn label(&self) -> &'static str {
        match self {
            StrategyKind::TouchMaker(_) => "touch_maker",
            StrategyKind::AvellanedaStoikov(_) => "avellaneda_stoikov",
            StrategyKind::ImbalanceTaker(_) => "imbalance_taker",
            StrategyKind::Scripted(_) => "scripted",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TouchMakerParams {
    pub order_qty: f64,
    /// Stop quoting the side that would grow `|position|` beyond this.
    pub max_position: f64,
    /// Quote this many ticks behind the touch (0 joins the best price).
    pub offset_ticks: i64,
    /// Shift both quotes by `-skew_ticks_per_unit * position` ticks (rounded), so inventory
    /// pushes quotes toward flattening.
    pub skew_ticks_per_unit: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvellanedaStoikovParams {
    pub order_qty: f64,
    pub max_position: f64,
    /// Risk aversion `gamma` (per price unit).
    pub gamma: f64,
    /// Order-arrival decay `kappa` (per price unit) in `lambda(delta) = A * exp(-kappa * delta)`.
    pub kappa: f64,
    /// Remaining horizon `T - t` in seconds, held constant (the common stationary variant).
    pub horizon_s: f64,
    /// Volatility is an exponentially weighted estimate of mid-price changes with this half-life.
    pub vol_halflife_ticks: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImbalanceTakerParams {
    pub order_qty: f64,
    pub max_position: f64,
    /// Enter when `(bid_qty - ask_qty) / (bid_qty + ask_qty)` exceeds this in absolute value.
    pub threshold: f64,
    /// Close the position this many strategy ticks after entry.
    pub hold_ticks: u32,
    /// Minimum ticks between entries.
    pub cooldown_ticks: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptedParams {
    pub orders: Vec<ScriptedOrder>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptedOrder {
    /// Strategy tick at which to place the order.
    pub at_tick: u64,
    /// Slot within the tick; `(at_tick, slot)` must be unique across the script.
    pub slot: u8,
    pub side: Side,
    pub price: ScriptedPrice,
    pub qty: f64,
    pub tif: Tif,
    pub ord_type: OrdType,
    /// Request cancellation this many ticks after placement if still working.
    pub cancel_after_ticks: Option<u64>,
    /// Free-text label shown as the order's reason.
    pub label: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScriptedPrice {
    /// Absolute price in ticks.
    Tick { tick: i64 },
    /// Relative to the same-side best price seen locally at placement: for a buy,
    /// `best_bid + offset`; for a sell, `best_ask - offset`. Positive offsets are more aggressive.
    FromTouch { offset: i64 },
}

impl RunConfig {
    /// Checks every numeric parameter. Returns human-readable problems; empty means valid.
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        let finite_pos = |v: f64| v.is_finite() && v > 0.0;
        if self.dataset_id.is_empty() {
            e.push("dataset_id is empty".into());
        }
        if self.strategy.interval_ns <= 0 {
            e.push("strategy.interval_ns must be positive".into());
        }
        match self.execution.order_latency {
            OrderLatencyConfig::Constant {
                entry_ns,
                response_ns,
            } => {
                if entry_ns < 0 || response_ns < 0 {
                    e.push("order latency must be non-negative".into());
                }
            }
            OrderLatencyConfig::Jitter {
                entry_ns,
                response_ns,
                jitter_ns,
            } => {
                if entry_ns < 0 || response_ns < 0 || jitter_ns < 0 {
                    e.push("order latency and jitter must be non-negative".into());
                }
            }
        }
        if self.execution.feed_latency_offset_ns < 0 {
            e.push(
                "feed_latency_offset_ns must be non-negative: a negative offset would let the \
                 strategy see events before the dataset says they were received"
                    .into(),
            );
        }
        match self.execution.queue_model {
            QueueModelConfig::PowerProb { n }
            | QueueModelConfig::PowerProb2 { n }
            | QueueModelConfig::PowerProb3 { n } => {
                if !finite_pos(n) {
                    e.push("queue model exponent n must be finite and positive".into());
                }
            }
            _ => {}
        }
        if !self.execution.fees.maker.is_finite() || !self.execution.fees.taker.is_finite() {
            e.push("fees must be finite".into());
        }
        if !finite_pos(self.risk.max_position) {
            e.push("risk.max_position must be finite and positive".into());
        }
        if !finite_pos(self.risk.max_order_qty) {
            e.push("risk.max_order_qty must be finite and positive".into());
        }
        if self.risk.max_open_orders == 0 {
            e.push("risk.max_open_orders must be at least 1".into());
        }
        if let Some(dd) = self.risk.max_drawdown
            && !finite_pos(dd)
        {
            e.push("risk.max_drawdown must be finite and positive when set".into());
        }
        if self.record.sample_every_ticks == 0 {
            e.push("record.sample_every_ticks must be at least 1".into());
        }
        match &self.strategy.kind {
            StrategyKind::TouchMaker(p) => {
                if !finite_pos(p.order_qty) || !finite_pos(p.max_position) {
                    e.push("touch_maker: order_qty and max_position must be positive".into());
                }
                if p.offset_ticks < 0 {
                    e.push("touch_maker: offset_ticks must be non-negative".into());
                }
                if !p.skew_ticks_per_unit.is_finite() || p.skew_ticks_per_unit < 0.0 {
                    e.push("touch_maker: skew_ticks_per_unit must be finite and >= 0".into());
                }
            }
            StrategyKind::AvellanedaStoikov(p) => {
                if !finite_pos(p.order_qty) || !finite_pos(p.max_position) {
                    e.push(
                        "avellaneda_stoikov: order_qty and max_position must be positive".into(),
                    );
                }
                if !finite_pos(p.gamma) || !finite_pos(p.kappa) || !finite_pos(p.horizon_s) {
                    e.push("avellaneda_stoikov: gamma, kappa, horizon_s must be positive".into());
                }
                if p.vol_halflife_ticks == 0 {
                    e.push("avellaneda_stoikov: vol_halflife_ticks must be at least 1".into());
                }
            }
            StrategyKind::ImbalanceTaker(p) => {
                if !finite_pos(p.order_qty) || !finite_pos(p.max_position) {
                    e.push("imbalance_taker: order_qty and max_position must be positive".into());
                }
                if !(p.threshold > 0.0 && p.threshold < 1.0) {
                    e.push("imbalance_taker: threshold must be in (0, 1)".into());
                }
                if p.hold_ticks == 0 {
                    e.push("imbalance_taker: hold_ticks must be at least 1".into());
                }
            }
            StrategyKind::Scripted(p) => {
                let mut seen = std::collections::BTreeSet::new();
                for o in &p.orders {
                    if !seen.insert((o.at_tick, o.slot)) {
                        e.push(format!(
                            "scripted: duplicate (at_tick={}, slot={})",
                            o.at_tick, o.slot
                        ));
                    }
                    if !finite_pos(o.qty) {
                        e.push(format!("scripted: order '{}' qty must be positive", o.label));
                    }
                }
            }
        }
        e
    }
}
