//! Everything a user can choose for a simulation run. A [`RunConfig`] plus a dataset hash and the
//! engine version fully determines a run's result.

use serde::{Deserialize, Serialize};

use crate::{OrdType, Side, Tif};

/// Complete, self-contained description of one simulation run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
// Every client-supplied type rejects unknown fields: a misspelled optional field such as
// `max_drawdwon` must be an error, not a silently disabled risk limit.
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    /// Dataset identifier (directory name under the dataset root).
    pub dataset_id: String,
    pub strategy: StrategyConfig,
    pub execution: ExecutionConfig,
    pub risk: RiskConfig,
    pub record: RecordConfig,
    /// Seed for every stochastic component: the `jitter`, `log_normal` and `heavy_tail` order
    /// latency models ([`OrderLatencyConfig`]). Each component draws from its own named sub-stream.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub seed: u64,
}

/// The modeled execution assumptions. These are what counterfactual scenarios vary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    pub exchange_model: ExchangeModel,
    pub queue_model: QueueModelConfig,
    pub order_latency: OrderLatencyConfig,
    /// Strategy compute ("think") time: requests decided on a tick are handed to the order path
    /// this long after the decision (ABIDES "computation delay"). The decision itself uses only
    /// what had arrived by the tick. Zero (the default, omitted from JSON) sends at the tick.
    /// Must be below `strategy.interval_ns` so the decision grid does not move.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub think_time_ns: i64,
    /// Added to every feed event's local receipt time (engine `latency_offset`). Zero keeps the
    /// dataset's own exchange-to-local latency. Must keep `local_ts > exch_ts` for every event.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub feed_latency_offset_ns: i64,
    pub fees: FeeConfig,
}

/// Which pinned-engine exchange model matches orders.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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

/// Order-entry and order-response latency. `constant` and `jitter` apply one entry latency to new
/// orders and cancels alike; `log_normal` and `heavy_tail` may give cancels their own entry
/// distribution (the engine's `LatencyModel::entry` sees the request kind).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OrderLatencyConfig {
    /// Engine `ConstantLatency`.
    Constant {
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        entry_ns: i64,
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        response_ns: i64,
    },
    /// BlackBox model implementing the engine's `LatencyModel` trait: the base latency plus a
    /// seeded uniform draw from `[0, jitter_ns]`. The draw is a pure function of the seed and of
    /// what is delayed (entry: order id and request time; response: exchange timestamp), never a
    /// sequential stream, so results do not depend on the engine's internal iteration order. The
    /// engine's order bus keeps messages in send order, so jitter never reorders them.
    Jitter {
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        entry_ns: i64,
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        response_ns: i64,
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        jitter_ns: i64,
    },
    /// BlackBox model: every leg is an independent seeded log-normal draw
    /// `median_ns * exp(sigma * Z)`, `Z ~ N(0, 1)`, keyed like `jitter`. `cancel` is the entry
    /// distribution of cancel requests; `None` draws cancels from `entry`'s parameters (on the
    /// cancel sub-stream).
    LogNormal {
        entry: LogNormalLeg,
        response: LogNormalLeg,
        #[serde(default)]
        cancel: Option<LogNormalLeg>,
    },
    /// BlackBox model: every leg is ABIDES' "cubic" latency, `min + (jitter / x^3) * (min / unit)`
    /// with `x` uniform on `(clip, 1]` (abides-core `latency_model.py`). A clipped power-law
    /// tail: rare messages are very slow. Keyed and sub-streamed like `log_normal`.
    HeavyTail {
        entry: CubicLeg,
        response: CubicLeg,
        #[serde(default)]
        cancel: Option<CubicLeg>,
    },
}

impl OrderLatencyConfig {
    /// Whether the run's seed changes any latency (and so possibly the result): `false` for
    /// `constant` and for `jitter` with `jitter_ns == 0`. The seed drives nothing else, so two
    /// configurations that differ only in the seed give the same result when this is `false`.
    pub fn uses_seed(&self) -> bool {
        match self {
            OrderLatencyConfig::Constant { .. } => false,
            OrderLatencyConfig::Jitter { jitter_ns, .. } => *jitter_ns > 0,
            OrderLatencyConfig::LogNormal { .. } | OrderLatencyConfig::HeavyTail { .. } => true,
        }
    }
}

/// One leg of the `log_normal` latency model.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LogNormalLeg {
    /// Median latency, `exp(mu)`, in ns.
    pub median_ns: i64,
    /// Standard deviation of `ln(latency)`. Zero makes the leg constant at the median.
    pub sigma: f64,
}

/// One leg of the `heavy_tail` (ABIDES cubic) latency model.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CubicLeg {
    /// Minimum latency in ns; also the scale of the jitter.
    pub min_ns: i64,
    /// ABIDES `jitter` (`a`): shape of the cubic curve, in `[0, 1]`.
    pub jitter: f64,
    /// ABIDES `jitter_clip`: `x` is drawn from `(clip, 1]`. Must be in `(0, 1)`; it bounds the
    /// largest latency at `min + jitter * min / (unit * clip^3)`.
    pub clip: f64,
    /// ABIDES `jitter_unit`: the jitter is measured in units of `min / unit`.
    pub unit: f64,
}

impl CubicLeg {
    /// Largest latency the leg can produce (`x = clip`), in ns.
    pub fn max_ns(&self) -> f64 {
        let min = self.min_ns as f64;
        min + self.jitter / (self.clip * self.clip * self.clip) * (min / self.unit)
    }
}

/// Largest accepted log-normal `sigma`. At 3 the 99.9th percentile is `exp(9.27)` (about 10,600)
/// times the median; wider spreads stop describing a network and start describing outages.
pub const MAX_LOG_NORMAL_SIGMA: f64 = 3.0;

impl OrderLatencyConfig {
    /// The model's location parameter on the entry path: `entry_ns` for `constant` and
    /// `jitter`, the entry median for `log_normal`, the entry minimum for `heavy_tail`.
    pub fn base_entry_ns(&self) -> i64 {
        match self {
            OrderLatencyConfig::Constant { entry_ns, .. }
            | OrderLatencyConfig::Jitter { entry_ns, .. } => *entry_ns,
            OrderLatencyConfig::LogNormal { entry, .. } => entry.median_ns,
            OrderLatencyConfig::HeavyTail { entry, .. } => entry.min_ns,
        }
    }

    /// This model with its location parameter replaced on the entry path (new orders and
    /// cancels) and/or the response path, keeping every shape parameter: `entry_ns` /
    /// `response_ns`, the medians of `log_normal`, the minima of `heavy_tail`. Used by sweeps.
    pub fn with_base_ns(&self, entry_ns: Option<i64>, response_ns: Option<i64>) -> Self {
        let mut m = *self;
        match &mut m {
            OrderLatencyConfig::Constant {
                entry_ns: e,
                response_ns: r,
            }
            | OrderLatencyConfig::Jitter {
                entry_ns: e,
                response_ns: r,
                ..
            } => {
                *e = entry_ns.unwrap_or(*e);
                *r = response_ns.unwrap_or(*r);
            }
            OrderLatencyConfig::LogNormal {
                entry,
                response,
                cancel,
            } => {
                if let Some(v) = entry_ns {
                    entry.median_ns = v;
                    if let Some(c) = cancel {
                        c.median_ns = v;
                    }
                }
                if let Some(v) = response_ns {
                    response.median_ns = v;
                }
            }
            OrderLatencyConfig::HeavyTail {
                entry,
                response,
                cancel,
            } => {
                if let Some(v) = entry_ns {
                    entry.min_ns = v;
                    if let Some(c) = cancel {
                        c.min_ns = v;
                    }
                }
                if let Some(v) = response_ns {
                    response.min_ns = v;
                }
            }
        }
        m
    }

    /// One plain-language sentence describing the model, for reports and explanations.
    pub fn describe(&self) -> String {
        let ln = |l: &LogNormalLeg| format!("median {} ns, sigma {}", l.median_ns, l.sigma);
        let cubic = |l: &CubicLeg| {
            format!(
                "min {} ns, jitter {}, clip {}, unit {} (at most {:.0} ns)",
                l.min_ns,
                l.jitter,
                l.clip,
                l.unit,
                l.max_ns()
            )
        };
        match self {
            OrderLatencyConfig::Constant {
                entry_ns,
                response_ns,
            } => format!(
                "Constant modeled order latency: entry {entry_ns} ns; response {response_ns} ns."
            ),
            OrderLatencyConfig::Jitter {
                entry_ns,
                response_ns,
                jitter_ns,
            } => format!(
                "BlackBox seeded jitter model: base entry {entry_ns} ns and response \
                 {response_ns} ns, each plus a draw from 0 through {jitter_ns} ns. Messages never \
                 overtake."
            ),
            OrderLatencyConfig::LogNormal {
                entry,
                response,
                cancel,
            } => format!(
                "BlackBox seeded log-normal model: entry {}; cancel entry {}; response {}. \
                 Messages never overtake.",
                ln(entry),
                cancel.as_ref().map_or_else(|| "as entry".to_string(), ln),
                ln(response)
            ),
            OrderLatencyConfig::HeavyTail {
                entry,
                response,
                cancel,
            } => format!(
                "BlackBox seeded heavy-tailed model (ABIDES cubic): entry {}; cancel entry {}; \
                 response {}. Messages never overtake.",
                cubic(entry),
                cancel
                    .as_ref()
                    .map_or_else(|| "as entry".to_string(), cubic),
                cubic(response)
            ),
        }
    }

    /// Checks every leg's parameters against the bounds; pushes human-readable problems.
    fn validate_into(&self, e: &mut Vec<String>) {
        let latency_ok = |v: i64| (0..=MAX_LATENCY_NS).contains(&v);
        match *self {
            OrderLatencyConfig::Constant {
                entry_ns,
                response_ns,
            } => {
                if !latency_ok(entry_ns) || !latency_ok(response_ns) {
                    e.push(format!(
                        "order latency must be between 0 and {MAX_LATENCY_NS} ns (60 s)"
                    ));
                }
            }
            OrderLatencyConfig::Jitter {
                entry_ns,
                response_ns,
                jitter_ns,
            } => {
                if !latency_ok(entry_ns) || !latency_ok(response_ns) || !latency_ok(jitter_ns) {
                    e.push(format!(
                        "order latency and jitter must each be between 0 and {MAX_LATENCY_NS} \
                         ns (60 s)"
                    ));
                }
            }
            OrderLatencyConfig::LogNormal {
                entry,
                response,
                cancel,
            } => {
                for (name, leg) in [("entry", Some(entry)), ("response", Some(response))]
                    .into_iter()
                    .chain([("cancel", cancel)])
                {
                    let Some(leg) = leg else { continue };
                    if !(1..=MAX_LATENCY_NS).contains(&leg.median_ns) {
                        e.push(format!(
                            "log_normal {name}.median_ns must be between 1 and {MAX_LATENCY_NS} \
                             ns (60 s)"
                        ));
                    }
                    if !(leg.sigma.is_finite() && (0.0..=MAX_LOG_NORMAL_SIGMA).contains(&leg.sigma))
                    {
                        e.push(format!(
                            "log_normal {name}.sigma must be between 0 and {MAX_LOG_NORMAL_SIGMA}"
                        ));
                    }
                }
            }
            OrderLatencyConfig::HeavyTail {
                entry,
                response,
                cancel,
            } => {
                for (name, leg) in [("entry", Some(entry)), ("response", Some(response))]
                    .into_iter()
                    .chain([("cancel", cancel)])
                {
                    let Some(leg) = leg else { continue };
                    let shape_ok = leg.jitter.is_finite()
                        && (0.0..=1.0).contains(&leg.jitter)
                        && leg.clip.is_finite()
                        && leg.clip > 0.0
                        && leg.clip < 1.0
                        && leg.unit.is_finite()
                        && leg.unit > 0.0;
                    if !shape_ok {
                        e.push(format!(
                            "heavy_tail {name}: jitter must be in [0, 1], clip in (0, 1) and unit \
                             finite and positive"
                        ));
                    } else if !latency_ok(leg.min_ns) || leg.max_ns() > MAX_LATENCY_NS as f64 {
                        e.push(format!(
                            "heavy_tail {name}: min_ns and the largest possible latency \
                             min + jitter * min / (unit * clip^3) = {:.0} ns must be between 0 \
                             and {MAX_LATENCY_NS} ns (60 s)",
                            leg.max_ns()
                        ));
                    }
                }
            }
        }
    }
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// Fees as a fraction of traded value (engine `TradingValueFeeModel<CommonFees>`). Negative means
/// a rebate. Example: `0.0002` is 2 basis points.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FeeConfig {
    pub maker: f64,
    pub taker: f64,
}

/// Pre-trade risk rules enforced by the strategy harness before a request reaches the engine.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct StrategyConfig {
    /// Local-clock time between strategy evaluations.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub interval_ns: i64,
    pub kind: StrategyKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StrategyKind {
    /// Joins the best bid and best ask with post-only orders and skews away from inventory.
    TouchMaker(TouchMakerParams),
    /// Avellaneda-Stoikov (2008) reservation price and spread.
    AvellanedaStoikov(AvellanedaStoikovParams),
    /// Guéant-Lehalle-Fernandez-Tapia (2013) asymptotic quotes with Guéant (2017) trade size.
    Glft(GlftParams),
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
            StrategyKind::Glft(_) => "glft",
            StrategyKind::ImbalanceTaker(_) => "imbalance_taker",
            StrategyKind::Scripted(_) => "scripted",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
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

/// Guéant-Lehalle-Fernandez-Tapia market maker (`crates/bb-strategies/src/glft.rs`). Units:
/// prices in price units, quantities in base units; `order_qty` is the trade size `Delta` of
/// Guéant (2017) and the inventory `q` is the locally known position, both in base units.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GlftParams {
    /// Quote size `Delta` (base units); also the trade size in the formulas.
    pub order_qty: f64,
    /// Inventory bound `Q` (base units): a side is not quoted when one more fill would pass it.
    pub max_position: f64,
    /// Absolute risk aversion `gamma` of the CARA objective, per unit of wealth
    /// (1 / (price unit x base unit)). `xi = gamma` (Guéant 2017, Model A).
    pub gamma: f64,
    /// Intensity decay `k` (per price unit) in `Lambda(delta) = A * exp(-k * delta)`.
    pub kappa: f64,
    /// Intensity scale `A`: fills per second of a quote at the reference price. An assumption,
    /// not estimated from the data.
    pub intensity_a: f64,
    /// Which observed price series `sigma` is estimated from.
    pub sigma_source: SigmaSource,
    /// Half-life, in strategy ticks, of the exponentially weighted variance estimate.
    pub vol_halflife_ticks: u32,
}

/// Price series whose clock-sampled changes estimate `sigma`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SigmaSource {
    /// Mid `(bid + ask) / 2`. Jumps by half the spread whenever one side of a wide book moves.
    Mid,
    /// Size-weighted mid `(bid * ask_qty + ask * bid_qty) / (bid_qty + ask_qty)` (Stoikov 2018,
    /// eqs. 2-3; not his calibrated micro-price).
    WeightedMid,
    /// Last trade price received locally, carried forward between trades.
    Trade,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ScriptedParams {
    pub orders: Vec<ScriptedOrder>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScriptedPrice {
    /// Absolute price in ticks.
    Tick { tick: i64 },
    /// Relative to the same-side best price seen locally at placement: for a buy,
    /// `best_bid + offset`; for a sell, `best_ask - offset`. Positive offsets are more aggressive.
    FromTouch { offset: i64 },
}

/// Shortest allowed strategy evaluation interval (1 ms).
pub const MIN_INTERVAL_NS: i64 = 1_000_000;
/// Longest allowed strategy evaluation interval (1 h).
pub const MAX_INTERVAL_NS: i64 = 3_600_000_000_000;
/// Upper bound on any single modeled latency, jitter or feed offset (60 s).
pub const MAX_LATENCY_NS: i64 = 60_000_000_000;
/// Upper bound on strategy evaluations in one run, checked against the dataset's span by the
/// application before simulating. At the minimum interval it allows a ~33-minute dataset.
pub const MAX_TICKS_PER_RUN: i64 = 2_000_000;
/// Largest accepted absolute fee rate, as a fraction of traded value (1% = 100 bp).
pub const MAX_ABS_FEE: f64 = 0.01;

impl RunConfig {
    /// Checks every numeric parameter. Returns human-readable problems; empty means valid.
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        let finite_pos = |v: f64| v.is_finite() && v > 0.0;
        if self.dataset_id.is_empty() {
            e.push("dataset_id is empty".into());
        }
        // Bounds keep every run's work finite and every timestamp sum far from i64 overflow:
        // a 1 ns interval would ask for a billion strategy evaluations per simulated second, and
        // an i64::MAX interval wraps the engine clock.
        if !(MIN_INTERVAL_NS..=MAX_INTERVAL_NS).contains(&self.strategy.interval_ns) {
            e.push(format!(
                "strategy.interval_ns must be between {MIN_INTERVAL_NS} (1 ms) and \
                 {MAX_INTERVAL_NS} (1 h)"
            ));
        }
        self.execution.order_latency.validate_into(&mut e);
        let think = self.execution.think_time_ns;
        if !(0..=MAX_LATENCY_NS).contains(&think)
            || (think > 0 && think >= self.strategy.interval_ns)
        {
            e.push(format!(
                "execution.think_time_ns must be at least 0, at most {MAX_LATENCY_NS} ns (60 s) \
                 and below strategy.interval_ns ({}), so that every decision is sent before the \
                 next one is made; got {think}",
                self.strategy.interval_ns
            ));
        }
        if self.execution.feed_latency_offset_ns < 0 {
            e.push(
                "feed_latency_offset_ns must be non-negative: a negative offset would let the \
                 strategy see events before the dataset says they were received"
                    .into(),
            );
        } else if self.execution.feed_latency_offset_ns > MAX_LATENCY_NS {
            e.push(format!(
                "feed_latency_offset_ns must be at most {MAX_LATENCY_NS} ns (60 s)"
            ));
        }
        match self.execution.queue_model {
            QueueModelConfig::PowerProb { n }
            | QueueModelConfig::PowerProb2 { n }
            | QueueModelConfig::PowerProb3 { n }
                if !finite_pos(n) =>
            {
                e.push("queue model exponent n must be finite and positive".into());
            }
            _ => {}
        }
        // |fee| <= 1% of traded value: real venue fees are basis points, so anything larger is
        // almost certainly a unit mistake (5 instead of 0.0005).
        for (name, fee) in [
            ("maker", self.execution.fees.maker),
            ("taker", self.execution.fees.taker),
        ] {
            if !fee.is_finite() || fee.abs() > MAX_ABS_FEE {
                e.push(format!(
                    "fees.{name} must be a fraction of traded value between -{MAX_ABS_FEE} and \
                     {MAX_ABS_FEE} (100 bp); got {fee}"
                ));
            }
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
        // JSON clients that read numbers as IEEE-754 doubles (every browser) silently round
        // larger integers, so a displayed seed would not reproduce the run.
        if self.seed > crate::time::MAX_SAFE_INTEGER as u64 {
            e.push(format!(
                "seed must be at most {} (2^53 - 1) so that JSON clients reading numbers as \
                 doubles keep it exact; got {}",
                crate::time::MAX_SAFE_INTEGER,
                self.seed
            ));
        }
        // A strategy whose order size can never pass its own position limit or the order-size
        // rule would produce a run with no orders at all; that is a configuration error.
        let sizes = match &self.strategy.kind {
            StrategyKind::TouchMaker(p) => Some((p.order_qty, p.max_position)),
            StrategyKind::AvellanedaStoikov(p) => Some((p.order_qty, p.max_position)),
            StrategyKind::Glft(p) => Some((p.order_qty, p.max_position)),
            StrategyKind::ImbalanceTaker(p) => Some((p.order_qty, p.max_position)),
            StrategyKind::Scripted(_) => None,
        };
        if let Some((order_qty, max_position)) = sizes
            && finite_pos(order_qty)
        {
            if order_qty > max_position {
                e.push(format!(
                    "strategy order_qty {order_qty} exceeds the strategy's max_position \
                     {max_position}: it could never place an order"
                ));
            }
            if order_qty > self.risk.max_order_qty {
                e.push(format!(
                    "strategy order_qty {order_qty} exceeds risk.max_order_qty {}: every order \
                     would be blocked",
                    self.risk.max_order_qty
                ));
            }
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
            StrategyKind::Glft(p) => {
                if !finite_pos(p.order_qty) || !finite_pos(p.max_position) {
                    e.push("glft: order_qty and max_position must be positive".into());
                }
                if !finite_pos(p.gamma) || !finite_pos(p.kappa) || !finite_pos(p.intensity_a) {
                    e.push("glft: gamma, kappa and intensity_a must be finite and positive".into());
                }
                if p.vol_halflife_ticks == 0 {
                    e.push("glft: vol_halflife_ticks must be at least 1".into());
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
                        e.push(format!(
                            "scripted: order '{}' qty must be positive",
                            o.label
                        ));
                    }
                    if o.ord_type == OrdType::Market {
                        e.push(format!(
                            "scripted: order '{}' is a market order; market orders are not \
                             supported on hftbacktest 0.9.4 (see docs/EXECUTION_MODEL.md, \
                             defect 5); use an immediate-or-cancel limit order",
                            o.label
                        ));
                    }
                }
            }
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RunConfig {
        RunConfig {
            dataset_id: "ds".into(),
            strategy: StrategyConfig {
                interval_ns: 100_000_000,
                kind: StrategyKind::TouchMaker(TouchMakerParams {
                    order_qty: 1.0,
                    max_position: 10.0,
                    offset_ticks: 0,
                    skew_ticks_per_unit: 0.0,
                }),
            },
            execution: ExecutionConfig {
                exchange_model: ExchangeModel::NoPartialFill,
                queue_model: QueueModelConfig::RiskAdverse,
                order_latency: OrderLatencyConfig::Constant {
                    entry_ns: 1_000_000,
                    response_ns: 1_000_000,
                },
                think_time_ns: 0,
                feed_latency_offset_ns: 0,
                fees: FeeConfig {
                    maker: 0.0,
                    taker: 0.0,
                },
            },
            risk: RiskConfig {
                max_position: 10.0,
                max_order_qty: 5.0,
                max_open_orders: 4,
                max_drawdown: None,
            },
            record: RecordConfig::default(),
            seed: 1,
        }
    }

    /// A zero think time is omitted, so every configuration written before the field existed
    /// serializes (and therefore hashes into its run identity) exactly as before.
    #[test]
    fn zero_think_time_keeps_the_serialized_configuration_unchanged() {
        let json = serde_json::to_value(base()).unwrap();
        assert!(json["execution"].get("think_time_ns").is_none(), "{json}");
        let back: RunConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back, base());
        let mut thinking = base();
        thinking.execution.think_time_ns = 2_000_000;
        let json = serde_json::to_value(&thinking).unwrap();
        assert_eq!(json["execution"]["think_time_ns"], 2_000_000);
    }

    #[test]
    fn new_latency_and_strategy_shapes_round_trip() {
        let lognormal = r#"{"type":"log_normal","entry":{"median_ns":1000000,"sigma":0.5},
            "response":{"median_ns":2000000,"sigma":0.25}}"#;
        let parsed: OrderLatencyConfig = serde_json::from_str(lognormal).unwrap();
        assert_eq!(
            parsed,
            OrderLatencyConfig::LogNormal {
                entry: LogNormalLeg {
                    median_ns: 1_000_000,
                    sigma: 0.5
                },
                response: LogNormalLeg {
                    median_ns: 2_000_000,
                    sigma: 0.25
                },
                cancel: None,
            }
        );
        let heavy = r#"{"type":"heavy_tail",
            "entry":{"min_ns":1000000,"jitter":0.5,"clip":0.1,"unit":10.0},
            "response":{"min_ns":1000000,"jitter":0.5,"clip":0.1,"unit":10.0},
            "cancel":{"min_ns":500000,"jitter":0.2,"clip":0.2,"unit":10.0}}"#;
        let parsed: OrderLatencyConfig = serde_json::from_str(heavy).unwrap();
        let OrderLatencyConfig::HeavyTail { entry, cancel, .. } = parsed else {
            panic!("{parsed:?}");
        };
        // ABIDES bound: 1 ms + 0.5 / 0.1^3 * (1 ms / 10) = 51 ms.
        assert!((entry.max_ns() - 51_000_000.0).abs() < 1e-3);
        assert_eq!(cancel.unwrap().min_ns, 500_000);
        assert!(
            serde_json::from_str::<OrderLatencyConfig>(
                r#"{"type":"log_normal","entry":{"median_ns":1,"sigma":0.5,"mu":0},
                "response":{"median_ns":1,"sigma":0.5}}"#
            )
            .is_err(),
            "unknown fields are refused"
        );
        let glft = r#"{"type":"glft","order_qty":0.01,"max_position":0.1,"gamma":100.0,
            "kappa":5.0,"intensity_a":1.0,"sigma_source":"weighted_mid","vol_halflife_ticks":50}"#;
        let kind: StrategyKind = serde_json::from_str(glft).unwrap();
        assert_eq!(kind.label(), "glft");
    }

    #[test]
    fn bounds_are_enforced() {
        let problems = |f: &dyn Fn(&mut RunConfig)| {
            let mut c = base();
            f(&mut c);
            c.validate()
        };
        let leg = LogNormalLeg {
            median_ns: 1_000_000,
            sigma: 0.5,
        };
        let cubic = CubicLeg {
            min_ns: 1_000_000,
            jitter: 0.5,
            clip: 0.1,
            unit: 10.0,
        };
        let ok = |l| move |c: &mut RunConfig| c.execution.order_latency = l;
        assert!(
            problems(&ok(OrderLatencyConfig::LogNormal {
                entry: leg,
                response: leg,
                cancel: Some(leg)
            }))
            .is_empty()
        );
        assert!(
            problems(&ok(OrderLatencyConfig::HeavyTail {
                entry: cubic,
                response: cubic,
                cancel: None
            }))
            .is_empty()
        );
        for bad in [
            LogNormalLeg {
                median_ns: 0,
                ..leg
            },
            LogNormalLeg { sigma: 3.5, ..leg },
            LogNormalLeg {
                sigma: f64::NAN,
                ..leg
            },
        ] {
            let p = problems(&ok(OrderLatencyConfig::LogNormal {
                entry: leg,
                response: leg,
                cancel: Some(bad),
            }));
            assert_eq!(p.len(), 1, "{bad:?}: {p:?}");
            assert!(p[0].contains("log_normal cancel"), "{p:?}");
        }
        for bad in [
            CubicLeg { clip: 0.0, ..cubic },
            CubicLeg { clip: 1.0, ..cubic },
            CubicLeg {
                jitter: 1.5,
                ..cubic
            },
            CubicLeg { unit: 0.0, ..cubic },
            // 1 s + 1 / 0.01^3 * (1 s / 1) is far beyond 60 s.
            CubicLeg {
                min_ns: 1_000_000_000,
                jitter: 1.0,
                clip: 0.01,
                unit: 1.0,
            },
        ] {
            let p = problems(&ok(OrderLatencyConfig::HeavyTail {
                entry: bad,
                response: cubic,
                cancel: None,
            }));
            assert_eq!(p.len(), 1, "{bad:?}: {p:?}");
            assert!(p[0].contains("heavy_tail entry"), "{p:?}");
        }
        // Think time: non-negative and strictly below the interval.
        assert!(problems(&|c| c.execution.think_time_ns = 99_999_999).is_empty());
        for think in [-1, 100_000_000, 200_000_000] {
            let p = problems(&|c| c.execution.think_time_ns = think);
            assert_eq!(p.len(), 1, "{think}: {p:?}");
        }
        // GLFT parameters.
        let glft = GlftParams {
            order_qty: 1.0,
            max_position: 10.0,
            gamma: 0.1,
            kappa: 1.5,
            intensity_a: 1.0,
            sigma_source: SigmaSource::Mid,
            vol_halflife_ticks: 10,
        };
        assert!(problems(&|c| c.strategy.kind = StrategyKind::Glft(glft)).is_empty());
        for bad in [
            GlftParams {
                intensity_a: 0.0,
                ..glft
            },
            GlftParams {
                gamma: f64::INFINITY,
                ..glft
            },
            GlftParams {
                vol_halflife_ticks: 0,
                ..glft
            },
            GlftParams {
                order_qty: 20.0,
                ..glft
            },
        ] {
            assert!(
                !problems(&|c| c.strategy.kind = StrategyKind::Glft(bad)).is_empty(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn sweeps_move_the_location_and_keep_the_shape() {
        let leg = LogNormalLeg {
            median_ns: 1_000_000,
            sigma: 0.5,
        };
        let m = OrderLatencyConfig::LogNormal {
            entry: leg,
            response: leg,
            cancel: Some(leg),
        };
        let OrderLatencyConfig::LogNormal {
            entry,
            response,
            cancel,
        } = m.with_base_ns(Some(5), None)
        else {
            unreachable!()
        };
        assert_eq!((entry.median_ns, entry.sigma), (5, 0.5));
        assert_eq!(cancel.unwrap().median_ns, 5);
        assert_eq!(response, leg);
        let c = OrderLatencyConfig::Jitter {
            entry_ns: 1,
            response_ns: 2,
            jitter_ns: 3,
        };
        assert_eq!(
            c.with_base_ns(Some(7), Some(8)),
            OrderLatencyConfig::Jitter {
                entry_ns: 7,
                response_ns: 8,
                jitter_ns: 3
            }
        );
        assert_eq!(c.base_entry_ns(), 1);
    }
}
