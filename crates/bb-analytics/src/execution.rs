//! E1–E10 exchange-clock execution analytics.
//!
//! Fills come from `ExchFill`, never the enriched at-fill `FillRecord.mid_tick`. References
//! use the last completed BBO batch strictly before a fill. Costs are positive when bad;
//! markouts are positive when favourable. On a matched horizon cohort, **ES = RS - AS**.
//! Quote-currency amounts include `contract_size`; price differences do not.
//!
//! Bootstrap bands are deterministic, percentile, Poissonized block-bootstrap bands: all
//! fills in one fixed exchange-time block receive the same Poisson(1) multiplicity in a
//! replicate. Only one order's accumulators are live at a time; compact tape links preserve
//! each order's event order without copying events. Global fills are accumulated in tape order
//! for byte-identical sums. Exact KM and quantiles retain one observation per order.
use std::collections::BTreeMap;

use crate::{Sum, Value};
use bb_core::{
    OrderStatus, RelNs, ReqKind,
    dataset::InstrumentMeta,
    flight::{FlightBody, FlightEvent},
    market::BboSeries,
    result::{OrderRecord, RunResult},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DirectionalOrder {
    pub order_id: u64,
    /// Explicit decision price in quote/base; None uses the local OrderIntent market mid.
    /// The limit price is NEVER substituted for a missing decision price.
    pub decision_price: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExecutionConfig {
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>))]
    pub horizons_ns: Vec<i64>,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub queue_horizon_ns: i64,
    /// Finite, increasing bucket edges; buckets are [lower, upper), with outer tails.
    pub relative_queue_edges: Vec<f64>,
    pub absolute_queue_lot_edges: Vec<f64>,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub bootstrap_block_ns: i64,
    pub bootstrap_replicates: usize,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub bootstrap_seed: u64,
    /// Only these explicitly directional intents are eligible for E4/E10.
    pub directional_orders: Vec<DirectionalOrder>,
    /// Exchange-clock observation end. None uses the last BBO/recorded exchange timestamp.
    /// Set this when the run stopped before the dataset ended (e.g. a risk halt).
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub observation_end_ns: Option<RelNs>,
}
impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            horizons_ns: vec![100_000_000, 1_000_000_000, 5_000_000_000, 30_000_000_000],
            queue_horizon_ns: 1_000_000_000,
            relative_queue_edges: vec![0.25, 0.5, 1.0, 2.0],
            absolute_queue_lot_edges: vec![1.0, 10.0, 100.0, 1000.0],
            bootstrap_block_ns: 1_000_000_000,
            bootstrap_replicates: 512,
            bootstrap_seed: 0,
            directional_orders: Vec::new(),
            observation_end_ns: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("invalid execution configuration: {0}")]
    Config(&'static str),
    #[error("invalid instrument metadata")]
    Instrument,
    #[error("inconsistent BBO column lengths")]
    BookShape,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PriceMetric {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub price: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub ticks: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub bps: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfidenceBand {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub lower: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub upper: Value,
    pub method: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HorizonMetrics {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub horizon_ns: i64,
    /// Same valid cohort as RS and AS, allowing the identity to be checked after exclusions.
    pub matched_effective_half: PriceMetric,
    pub realised_half: PriceMetric,
    pub markout: PriceMetric,
    pub adverse_selection: PriceMetric,
    pub adverse_selection_95: ConfidenceBand,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SpreadMetrics {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub filled_quantity: Value,
    pub effective_half: PriceMetric,
    pub effective_full: PriceMetric,
    pub arrival_slippage: PriceMetric,
    pub decision_slippage: PriceMetric,
    pub latency_drift: PriceMetric,
    pub horizons: Vec<HorizonMetrics>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LiquiditySplit<T> {
    pub all: T,
    pub maker: T,
    pub taker: T,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Shortfall {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub execution: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub delay: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub trading: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub opportunity: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub fees: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub total: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub bps: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub decision_price: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub arrival_mid: Value,
    pub arrival_slippage: PriceMetric,
    pub decision_slippage: PriceMetric,
    pub latency_drift: PriceMetric,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpportunityCost {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub horizon_ns: i64,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub cost: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CancelMetrics {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub sent_ns: RelNs,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub to_exchange: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub round_trip: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub in_flight_quantity: Value,
    pub late_cancel: bool,
    pub exchange_censored: bool,
    pub acknowledgement_censored: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrderExecution {
    pub order_id: u64,
    pub spreads: LiquiditySplit<SpreadMetrics>,
    pub shortfall: Shortfall,
    pub opportunities: Vec<OpportunityCost>,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub time_to_first_fill: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub time_to_complete_fill: Value,
    pub first_fill_censored: bool,
    pub complete_fill_censored: bool,
    pub cancels: Vec<CancelMetrics>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Distribution {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub total: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub median: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub p05: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub p95: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ShortfallComponents {
    pub execution: Distribution,
    pub delay: Distribution,
    pub trading: Distribution,
    pub opportunity: Distribution,
    pub fees: Distribution,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CancelSummary {
    pub attempts: u64,
    pub exchange_censored: u64,
    pub acknowledgement_censored: u64,
    pub late_cancels: u64,
    pub to_exchange_ns: Distribution,
    pub round_trip_ns: Distribution,
    pub in_flight_quantity: Distribution,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SurvivalPoint {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub elapsed_ns: i64,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub survival: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub fill_probability: Value,
    pub probability_95: ConfidenceBand,
    pub at_risk: u64,
    pub events: u64,
    pub censored: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct KaplanMeier {
    pub n: u64,
    pub excluded: u64,
    pub censored: u64,
    pub curve: Vec<SurvivalPoint>,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub fill_probability: Value,
    pub probability_95: ConfidenceBand,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub median_ns: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QueueBucket {
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub covariate_unit: String,
    pub estimate: KaplanMeier,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QueueMetrics {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub horizon_ns: i64,
    pub modeled: bool,
    pub relative: Vec<QueueBucket>,
    pub absolute_lots: Vec<QueueBucket>,
    pub excluded_relative: u64,
    pub excluded_absolute: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FillTimes {
    pub observed_ns: Distribution,
    pub km: KaplanMeier,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PnlDecomposition {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub edge: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub drift: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub fees: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub net: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExecutionReport {
    pub analytics_version: String,
    pub order_count: u64,
    /// True when details were delivered to a sink rather than retained in `orders`.
    pub order_details_streamed: bool,
    pub spreads: LiquiditySplit<SpreadMetrics>,
    pub orders: Vec<OrderExecution>,
    pub shortfall: Distribution,
    pub shortfall_components: ShortfallComponents,
    pub cancels: CancelSummary,
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<(String, Distribution)>))]
    pub opportunities: Vec<(i64, Distribution)>,
    pub queue: QueueMetrics,
    pub first_fill: FillTimes,
    pub complete_fill: FillTimes,
    pub pnl: LiquiditySplit<PnlDecomposition>,
    #[cfg_attr(feature = "openapi", schema(value_type = crate::AnalyticsValue))]
    pub exchange_end_mid: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub observation_end_ns: Option<RelNs>,
    pub unassociated_fills: u64,
    pub caveats: Vec<String>,
}

#[derive(Clone, Copy, Default)]
struct Mean {
    sum: Sum,
    weight: Sum,
    n: u64,
    excluded: u64,
}
impl Mean {
    fn add(&mut self, x: Option<f64>, w: f64) {
        if let Some(x) = x.filter(|v| v.is_finite() && w.is_finite() && w > 0.0) {
            self.sum.add(x * w);
            self.weight.add(w);
            self.n += 1;
        } else {
            self.excluded += 1;
        }
    }
    fn value(&self, unit: &str) -> Value {
        if self.weight.get() > 0.0 {
            Value::measured(
                self.sum.get() / self.weight.get(),
                unit,
                self.n,
                self.excluded,
            )
        } else {
            Value::unavailable("no valid observations/reference mid", unit, self.excluded)
        }
    }
}
#[derive(Clone, Copy, Default)]
struct PriceAcc {
    price: Mean,
    bps: Mean,
}
impl PriceAcc {
    fn add(&mut self, x: Option<f64>, reference: Option<f64>, qty: f64) {
        self.price.add(x, qty);
        self.bps.add(
            x.zip(reference)
                .and_then(|(x, m)| (m > 0.0).then_some(x / m * 10_000.0)),
            qty,
        );
    }
    fn report(&self, tick: f64, scale: f64) -> PriceMetric {
        let scaled = |v: Value, factor: f64, unit: &str| match v {
            Value::Available {
                value, n, excluded, ..
            } => Value::measured(value * factor, unit, n, excluded),
            Value::Unavailable {
                reason, excluded, ..
            } => Value::unavailable(&reason, unit, excluded),
        };
        PriceMetric {
            price: scaled(self.price.value("quote/base"), scale, "quote/base"),
            ticks: scaled(self.price.value("quote/base"), scale / tick, "ticks"),
            bps: scaled(self.bps.value("bps"), scale, "bps"),
        }
    }
}
#[derive(Default)]
struct HorizonAcc {
    es: PriceAcc,
    rs: PriceAcc,
    adverse: PriceAcc,
}
struct Spreads {
    qty: Sum,
    n: u64,
    es: PriceAcc,
    arrival: PriceAcc,
    decision: PriceAcc,
    delay: PriceAcc,
    curves: Vec<HorizonAcc>,
}
impl Spreads {
    fn new(h: usize) -> Self {
        Self {
            qty: Sum::default(),
            n: 0,
            es: PriceAcc::default(),
            arrival: PriceAcc::default(),
            decision: PriceAcc::default(),
            delay: PriceAcc::default(),
            curves: (0..h).map(|_| HorizonAcc::default()).collect(),
        }
    }
    // Price/quantity/sign belong to one fill and are shared by spread and P&L accumulators.
    fn fill(
        &mut self,
        fill: FillTerms,
        mid: Option<f64>,
        future: &[Option<f64>],
        arrival: Option<f64>,
        decision: Option<f64>,
    ) {
        let FillTerms { qty, sign, price } = fill;
        self.qty.add(qty);
        self.n += 1;
        self.es.add(mid.map(|m| sign * (price - m)), mid, qty);
        self.arrival
            .add(arrival.map(|m| sign * (price - m)), arrival, qty);
        self.decision
            .add(decision.map(|d| sign * (price - d)), decision, qty);
        self.delay.add(
            arrival.zip(decision).map(|(m, d)| sign * (m - d)),
            decision,
            qty,
        );
        for (a, mh) in self.curves.iter_mut().zip(future) {
            let pair = mid.zip(*mh);
            a.es.add(pair.map(|(m, _)| sign * (price - m)), mid, qty);
            a.rs.add(pair.map(|(_, m)| sign * (price - m)), mid, qty);
            a.adverse.add(pair.map(|(m, mh)| sign * (m - mh)), mid, qty);
        }
    }
    fn report(&self, cfg: &ExecutionConfig, tick: f64, boot: Option<&Bootstrap>) -> SpreadMetrics {
        SpreadMetrics {
            filled_quantity: Value::measured(self.qty.get(), "base", self.n, 0),
            effective_half: self.es.report(tick, 1.0),
            effective_full: self.es.report(tick, 2.0),
            arrival_slippage: self.arrival.report(tick, 1.0),
            decision_slippage: self.decision.report(tick, 1.0),
            latency_drift: self.delay.report(tick, 1.0),
            horizons: self
                .curves
                .iter()
                .enumerate()
                .map(|(i, c)| HorizonMetrics {
                    horizon_ns: cfg.horizons_ns[i],
                    matched_effective_half: c.es.report(tick, 1.0),
                    realised_half: c.rs.report(tick, 1.0),
                    markout: c.rs.report(tick, -1.0),
                    adverse_selection: c.adverse.report(tick, 1.0),
                    adverse_selection_95: boot.map(|b| b.band(i)).unwrap_or_else(|| {
                        missing_band(
                            "per-order confidence bands are not estimated",
                            "quote/base",
                            c.adverse.price.excluded,
                        )
                    }),
                })
                .collect(),
        }
    }
}
fn split_spreads(h: usize) -> LiquiditySplit<Spreads> {
    LiquiditySplit {
        all: Spreads::new(h),
        maker: Spreads::new(h),
        taker: Spreads::new(h),
    }
}
impl LiquiditySplit<Spreads> {
    fn report(
        &self,
        cfg: &ExecutionConfig,
        tick: f64,
        boot: Option<&LiquiditySplit<Bootstrap>>,
    ) -> LiquiditySplit<SpreadMetrics> {
        LiquiditySplit {
            all: self.all.report(cfg, tick, boot.map(|b| &b.all)),
            maker: self.maker.report(cfg, tick, boot.map(|b| &b.maker)),
            taker: self.taker.report(cfg, tick, boot.map(|b| &b.taker)),
        }
    }
}
fn missing_band(reason: &str, unit: &str, excluded: u64) -> ConfidenceBand {
    ConfidenceBand {
        lower: Value::unavailable(reason, unit, excluded),
        upper: Value::unavailable(reason, unit, excluded),
        method: reason.into(),
    }
}
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn poisson(seed: u64, block: i64, replicate: usize) -> f64 {
    // Domain-separate block and replicate so swapped indices do not share a draw.
    // [0,1), not midpoint rounding (which can round the largest draw up to 1).
    let hash = mix(mix(seed ^ block as u64)
        .wrapping_add(0x9e3779b97f4a7c15_u64.wrapping_mul((replicate as u64).wrapping_add(1))));
    let u = (hash >> 11) as f64 / 9_007_199_254_740_992.0;
    let mut p = 0.36787944117144233;
    let mut cdf = p;
    let mut k = 0;
    while u > cdf {
        k += 1;
        p /= k as f64;
        cdf += p;
    }
    k as f64
}
struct BootHorizon {
    means: Vec<Mean>,
    last_block: Option<i64>,
    blocks: u64,
    n: u64,
    excluded: u64,
}
struct Bootstrap {
    horizons: Vec<BootHorizon>,
}
impl Bootstrap {
    fn new(cfg: &ExecutionConfig) -> Self {
        Self {
            horizons: cfg
                .horizons_ns
                .iter()
                .map(|_| BootHorizon {
                    means: vec![Mean::default(); cfg.bootstrap_replicates],
                    last_block: None,
                    blocks: 0,
                    n: 0,
                    excluded: 0,
                })
                .collect(),
        }
    }
    fn fill(
        &mut self,
        block: i64,
        weights: &[f64],
        qty: f64,
        sign: f64,
        mid: Option<f64>,
        future: &[Option<f64>],
    ) {
        for (h, mh) in self.horizons.iter_mut().zip(future) {
            if let Some((m, mh)) = mid.zip(*mh) {
                h.n += 1;
                if h.last_block != Some(block) {
                    h.blocks += 1;
                    h.last_block = Some(block);
                }
                for (a, w) in h.means.iter_mut().zip(weights) {
                    if *w > 0.0 {
                        a.add(Some(sign * (m - mh)), qty * w);
                    }
                }
            } else {
                h.excluded += 1;
            }
        }
    }
    fn band(&self, i: usize) -> ConfidenceBand {
        let h = &self.horizons[i];
        if h.blocks < 2 || h.means.len() < 2 {
            return missing_band(
                "at least two fill time blocks and two bootstrap replicates required",
                "quote/base",
                h.excluded,
            );
        }
        let mut samples: Vec<f64> = h
            .means
            .iter()
            .filter(|m| m.weight.get() > 0.0)
            .map(|m| m.sum.get() / m.weight.get())
            .collect();
        samples.sort_by(f64::total_cmp);
        if samples.len() < 2 {
            return missing_band(
                "insufficient nonempty bootstrap replicates",
                "quote/base",
                h.excluded,
            );
        }
        ConfidenceBand {
            lower: Value::measured(quantile(&samples, 0.025), "quote/base", h.n, h.excluded),
            upper: Value::measured(quantile(&samples, 0.975), "quote/base", h.n, h.excluded),
            method: "95% percentile Poisson(1) clustered fixed-time-block bootstrap".into(),
        }
    }
}

/// One right-censored lifetime, measured from exchange arrival/resting in nanoseconds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SurvivalObservation {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub elapsed_ns: i64,
    pub event: bool,
}

/// Exact product-limit estimate. Events precede censor removals at a tied timestamp.
/// Greenwood normal intervals are clipped to [0,1]; an exhausted risk set is not
/// extrapolated beyond its last observation. A 0/0 Greenwood term at S=0 has CI [1,1].
pub fn kaplan_meier(observations: &[SurvivalObservation], horizon_ns: i64) -> KaplanMeier {
    let mut data: Vec<_> = observations
        .iter()
        .copied()
        .filter(|o| o.elapsed_ns >= 0 && horizon_ns >= 0)
        .map(|o| SurvivalObservation {
            elapsed_ns: o.elapsed_ns.min(horizon_ns),
            event: o.event && o.elapsed_ns <= horizon_ns,
        })
        .collect();
    data.sort_by_key(|o| o.elapsed_ns);
    let n = data.len() as u64;
    let excluded = (observations.len() - data.len()) as u64;
    let mut risk = n;
    let mut s = 1.0;
    let mut greenwood = 0.0;
    let mut curve = Vec::new();
    let mut i = 0;
    let mut censored = 0;
    let mut median = None;
    while i < data.len() {
        let t = data[i].elapsed_ns;
        let mut d = 0;
        let mut c = 0;
        while i < data.len() && data[i].elapsed_ns == t {
            if data[i].event {
                d += 1;
            } else {
                c += 1;
            }
            i += 1;
        }
        if d > 0 {
            s *= 1.0 - d as f64 / risk as f64;
            if risk > d {
                greenwood += d as f64 / (risk as f64 * (risk - d) as f64);
            }
            if s <= 0.5 && median.is_none() {
                median = Some(t);
            }
        }
        let se = if s == 0.0 { 0.0 } else { s * greenwood.sqrt() };
        curve.push(SurvivalPoint {
            elapsed_ns: t,
            survival: Value::measured(s, "probability", n, excluded),
            fill_probability: Value::measured(1.0 - s, "probability", n, excluded),
            probability_95: ConfidenceBand {
                lower: Value::measured(
                    (1.0 - s - 1.959963984540054 * se).clamp(0.0, 1.0),
                    "probability",
                    n,
                    excluded,
                ),
                upper: Value::measured(
                    (1.0 - s + 1.959963984540054 * se).clamp(0.0, 1.0),
                    "probability",
                    n,
                    excluded,
                ),
                method: "95% Greenwood normal, clipped to [0,1]".into(),
            },
            at_risk: risk,
            events: d,
            censored: c,
        });
        risk -= d + c;
        censored += c;
    }
    let supported = data.last().is_some_and(|o| o.elapsed_ns >= horizon_ns) || s == 0.0;
    let (fill_probability, probability_95) = if supported {
        let p = curve.last().expect("supported KM curve is nonempty");
        (p.fill_probability.clone(), p.probability_95.clone())
    } else {
        (
            Value::unavailable(
                "horizon beyond observed risk support",
                "probability",
                excluded,
            ),
            missing_band(
                "horizon beyond observed risk support",
                "probability",
                excluded,
            ),
        )
    };
    KaplanMeier {
        n,
        excluded,
        censored,
        curve,
        fill_probability,
        probability_95,
        median_ns: median
            .map(|m| Value::measured(m as f64, "ns", n, excluded))
            .unwrap_or_else(|| {
                Value::unavailable("KM survival did not reach one half", "ns", excluded)
            }),
    }
}
fn quantile(sorted: &[f64], p: f64) -> f64 {
    let index = p * (sorted.len() - 1) as f64;
    let lo = index.floor() as usize;
    let hi = index.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * index.fract()
}
fn distribution(mut values: Vec<f64>, unit: &str, excluded: u64) -> Distribution {
    values.sort_by(f64::total_cmp);
    let n = values.len() as u64;
    if values.is_empty() {
        let v = Value::unavailable("no eligible observations", unit, excluded);
        return Distribution {
            total: v.clone(),
            median: v.clone(),
            p05: v.clone(),
            p95: v,
        };
    }
    let mut sum = Sum::default();
    for x in &values {
        sum.add(*x);
    }
    Distribution {
        total: Value::measured(sum.get(), unit, n, excluded),
        median: Value::measured(quantile(&values, 0.5), unit, n, excluded),
        p05: Value::measured(quantile(&values, 0.05), unit, n, excluded),
        p95: Value::measured(quantile(&values, 0.95), unit, n, excluded),
    }
}
fn mid_at(book: &BboSeries, ts: i64, strict: bool, tick: f64) -> Option<f64> {
    let i = book
        .ts
        .partition_point(|t| if strict { *t < ts } else { *t <= ts })
        .checked_sub(1)?;
    let b = book.bid_tick[i]?;
    let a = book.ask_tick[i]?;
    (a > b)
        .then_some((a as f64 + b as f64) * 0.5 * tick)
        .filter(|m| m.is_finite() && *m > 0.0)
}
fn horizon_mid(book: &BboSeries, ts: i64, tick: f64) -> Option<f64> {
    if book.ts.last().is_none_or(|end| ts > *end) {
        return None;
    }
    mid_at(book, ts, false, tick)
}

struct CancelState {
    sent: i64,
    scheduled: Option<i64>,
    processed: Option<i64>,
    ack: Option<i64>,
    exposure: Sum,
    late: bool,
}
struct OrderState<'a> {
    order: &'a OrderRecord,
    spreads: LiquiditySplit<Spreads>,
    decision: Option<f64>,
    arrival: Option<i64>,
    resting: Option<i64>,
    ahead: Option<f64>,
    level: Option<f64>,
    first: Option<i64>,
    first_rest: Option<i64>,
    complete: Option<i64>,
    qty: Sum,
    notional: Sum,
    fees: Sum,
    cancels: Vec<CancelState>,
}
impl<'a> OrderState<'a> {
    fn new(order: &'a OrderRecord, h: usize) -> Self {
        Self {
            order,
            spreads: split_spreads(h),
            decision: None,
            arrival: order.exch_arrival_ts,
            resting: None,
            ahead: None,
            level: None,
            first: None,
            first_rest: None,
            complete: None,
            qty: Sum::default(),
            notional: Sum::default(),
            fees: Sum::default(),
            cancels: Vec::new(),
        }
    }
}

/// The tape remains borrowed. Links into it cost one machine word per event; the map holds
/// only routing metadata, never per-order spread/horizon/cancel accumulators.
struct OrderEvents<'a> {
    order: &'a OrderRecord,
    decision: Option<f64>,
    first: usize,
    last: usize,
}
#[derive(Default)]
struct PnlAcc {
    edge: Sum,
    drift: Sum,
    fees: Sum,
    n: u64,
    excluded: u64,
}
#[derive(Clone, Copy)]
struct FillTerms {
    qty: f64,
    sign: f64,
    price: f64,
}

impl PnlAcc {
    fn fill(
        &mut self,
        fill: FillTerms,
        mid: Option<f64>,
        end: Option<f64>,
        fee: f64,
        contract: f64,
    ) {
        let FillTerms { qty, sign, price } = fill;
        if let Some((m, end)) = mid.zip(end) {
            self.edge.add(sign * qty * (m - price) * contract);
            self.drift.add(sign * qty * (end - m) * contract);
            self.fees.add(fee);
            self.n += 1;
        } else {
            self.excluded += 1;
        }
    }
    fn report(&self) -> PnlDecomposition {
        let value = |x| {
            if self.excluded > 0 {
                Value::unavailable(
                    "incomplete decomposition: missing strict pre-fill or exchange end mark",
                    "quote",
                    self.excluded,
                )
            } else {
                Value::measured(x, "quote", self.n, 0)
            }
        };
        PnlDecomposition {
            edge: value(self.edge.get()),
            drift: value(self.drift.get()),
            fees: value(self.fees.get()),
            net: value(self.edge.get() + self.drift.get() - self.fees.get()),
        }
    }
}
fn duration(start: Option<i64>, finish: Option<i64>, reason: &str) -> Value {
    start
        .zip(finish)
        .and_then(|(s, t)| t.checked_sub(s))
        .filter(|t| *t >= 0)
        .map(|t| Value::measured(t as f64, "ns", 1, 0))
        .unwrap_or_else(|| Value::unavailable(reason, "ns", 1))
}
fn amount(value: Option<f64>, reason: &str) -> Value {
    value
        .map(|x| Value::measured(x, "quote", 1, 0))
        .unwrap_or_else(|| Value::unavailable(reason, "quote", 1))
}
fn price_metric(value: Option<f64>, reference: Option<f64>, tick: f64) -> PriceMetric {
    let mut a = PriceAcc::default();
    a.add(value, reference, 1.0);
    a.report(tick, 1.0)
}
fn shortfall(
    s: &OrderState<'_>,
    pd: Option<f64>,
    directional: bool,
    book: &BboSeries,
    end: Option<i64>,
    inst: &InstrumentMeta,
) -> Shortfall {
    let pd = if directional && s.arrival.is_some_and(|t| end.is_none_or(|e| t <= e)) {
        pd
    } else {
        None
    };
    let arrival = s
        .arrival
        .and_then(|t| mid_at(book, t, false, inst.tick_size));
    let abandonment = s
        .order
        .terminal_ts
        .into_iter()
        .chain(end)
        .min()
        .and_then(|t| mid_at(book, t, false, inst.tick_size));
    let q = s.qty.get();
    let unfilled = (s.order.qty - q).max(0.0);
    let sign = s.order.side.sign();
    let c = inst.contract_size;
    let execution = pd.map(|p| sign * (s.notional.get() - q * p) * c);
    let opportunity = if unfilled == 0.0 {
        pd.map(|_| 0.0)
    } else {
        pd.zip(abandonment)
            .map(|(p, m)| sign * unfilled * (m - p) * c)
    };
    let delay = pd.zip(arrival).map(|(p, m)| sign * q * (m - p) * c);
    let trading = pd
        .zip(arrival)
        .map(|(_, m)| sign * (s.notional.get() - q * m) * c);
    let total = execution
        .zip(opportunity)
        .map(|(e, o)| e + o + s.fees.get());
    let vwap = (q > 0.0).then(|| s.notional.get() / q);
    let reason = if !directional {
        "not explicitly designated a directional intent"
    } else {
        "missing decision price or evaluation mid"
    };
    Shortfall {
        execution: amount(execution, reason),
        delay: amount(delay, reason),
        trading: amount(trading, reason),
        opportunity: amount(opportunity, reason),
        fees: amount(pd.map(|_| s.fees.get()), reason),
        total: amount(total, reason),
        bps: total
            .zip(pd)
            .map(|(v, p)| Value::measured(v / (s.order.qty * p * c) * 10_000.0, "bps", 1, 0))
            .unwrap_or_else(|| Value::unavailable(reason, "bps", 1)),
        decision_price: pd
            .map(|p| Value::measured(p, "quote/base", 1, 0))
            .unwrap_or_else(|| Value::unavailable(reason, "quote/base", 1)),
        arrival_mid: arrival
            .map(|m| Value::measured(m, "quote/base", 1, 0))
            .unwrap_or_else(|| Value::unavailable("missing exchange arrival mid", "quote/base", 1)),
        // Arrival slippage needs no directional assumption; decision slippage/drift do.
        arrival_slippage: price_metric(
            vwap.zip(arrival).map(|(p, m)| sign * (p - m)),
            arrival,
            inst.tick_size,
        ),
        decision_slippage: price_metric(
            vwap.zip(pd).map(|(p, d)| sign * (p - d)),
            pd,
            inst.tick_size,
        ),
        latency_drift: price_metric(
            arrival.zip(pd).map(|(m, d)| sign * (m - d)),
            pd,
            inst.tick_size,
        ),
    }
}
fn lifetime(
    start: Option<i64>,
    event: Option<i64>,
    terminal: Option<i64>,
    end: Option<i64>,
) -> Option<SurvivalObservation> {
    let start = start?;
    let stop = terminal.into_iter().chain(end).min()?;
    let finish = event.filter(|t| *t <= stop);
    let t = finish.unwrap_or(stop);
    Some(SurvivalObservation {
        elapsed_ns: t.checked_sub(start).filter(|t| *t >= 0)?,
        event: finish.is_some(),
    })
}
fn queue_buckets(
    edges: &[f64],
    data: &[Vec<SurvivalObservation>],
    unit: &str,
    horizon: i64,
) -> Vec<QueueBucket> {
    data.iter()
        .enumerate()
        .map(|(i, observations)| QueueBucket {
            lower: i.checked_sub(1).map(|j| edges[j]),
            upper: edges.get(i).copied(),
            covariate_unit: unit.into(),
            estimate: kaplan_meier(observations, horizon),
        })
        .collect()
}

/// Compute E1–E10 without mutating the run or retaining fill/book copies. The book must be
/// the complete, timestamp-sorted, completed-exchange-batch BBO (not a downsampled replay).
/// E9 uses the exchange end mark, NOT the existing local-clock equity ledger.
pub fn compute_execution(
    run: &RunResult,
    book: &BboSeries,
    instrument: &InstrumentMeta,
    config: &ExecutionConfig,
) -> Result<ExecutionReport, ExecutionError> {
    compute_parts(&run.orders, &run.tape, book, instrument, config)
}

/// The same summary as a streaming computation with a discard sink, without constructing
/// per-order spread curves or detail reports. Exact distributions still include every order.
pub fn compute_execution_summary(
    run: &RunResult,
    book: &BboSeries,
    instrument: &InstrumentMeta,
    config: &ExecutionConfig,
) -> Result<ExecutionReport, ExecutionError> {
    compute_streaming_parts(&run.orders, &run.tape, book, instrument, config, None)
}

/// Bounded-output variant: every per-order result is delivered once to `on_order` and is not
/// retained in the returned report. The sink may persist or select details without retaining all
/// orders. Run summaries always include every order, regardless of what the sink keeps.
pub fn compute_execution_streaming(
    run: &RunResult,
    book: &BboSeries,
    instrument: &InstrumentMeta,
    config: &ExecutionConfig,
    mut on_order: impl FnMut(OrderExecution),
) -> Result<ExecutionReport, ExecutionError> {
    compute_streaming_parts(
        &run.orders,
        &run.tape,
        book,
        instrument,
        config,
        Some(&mut on_order),
    )
}
fn compute_parts(
    orders: &[OrderRecord],
    tape: &[FlightEvent],
    book: &BboSeries,
    inst: &InstrumentMeta,
    cfg: &ExecutionConfig,
) -> Result<ExecutionReport, ExecutionError> {
    let mut details = Vec::with_capacity(orders.len());
    let mut report = compute_streaming_parts(
        orders,
        tape,
        book,
        inst,
        cfg,
        Some(&mut |order| details.push(order)),
    )?;
    report.orders = details;
    report.order_details_streamed = false;
    Ok(report)
}

fn compute_streaming_parts(
    orders: &[OrderRecord],
    tape: &[FlightEvent],
    book: &BboSeries,
    inst: &InstrumentMeta,
    cfg: &ExecutionConfig,
    mut on_order: Option<&mut dyn FnMut(OrderExecution)>,
) -> Result<ExecutionReport, ExecutionError> {
    if [inst.tick_size, inst.lot_size, inst.contract_size]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err(ExecutionError::Instrument);
    }
    if book.ts.len() != book.bid_tick.len()
        || book.ts.len() != book.ask_tick.len()
        || book.ts.len() != book.bid_qty.len()
        || book.ts.len() != book.ask_qty.len()
    {
        return Err(ExecutionError::BookShape);
    }
    if cfg.horizons_ns.iter().any(|h| *h < 0)
        || cfg.queue_horizon_ns < 0
        || cfg.bootstrap_block_ns <= 0
    {
        return Err(ExecutionError::Config(
            "negative horizon or nonpositive block duration",
        ));
    }
    for edges in [&cfg.relative_queue_edges, &cfg.absolute_queue_lot_edges] {
        if edges.iter().any(|x| !x.is_finite() || *x < 0.0)
            || edges.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(ExecutionError::Config(
                "queue bucket edges must be finite, nonnegative and increasing",
            ));
        }
    }
    let mut directional = BTreeMap::new();
    for d in &cfg.directional_orders {
        if d.decision_price.is_some_and(|p| !p.is_finite() || p <= 0.0)
            || directional.insert(d.order_id, d.decision_price).is_some()
        {
            return Err(ExecutionError::Config(
                "duplicate directional order or invalid decision price",
            ));
        }
    }
    let mut end = cfg.observation_end_ns.or_else(|| book.ts.last().copied());
    let end_mid = end.and_then(|t| mid_at(book, t, false, inst.tick_size));
    let h = cfg.horizons_ns.len();
    let mut indexed_orders = BTreeMap::new();
    for order in orders {
        indexed_orders.insert(
            order.order_id,
            OrderEvents {
                order,
                decision: None,
                first: usize::MAX,
                last: usize::MAX,
            },
        );
    }
    let mut next_event = vec![usize::MAX; tape.len()];
    let mut spreads = split_spreads(h);
    let mut boot = LiquiditySplit {
        all: Bootstrap::new(cfg),
        maker: Bootstrap::new(cfg),
        taker: Bootstrap::new(cfg),
    };
    let mut pnl = LiquiditySplit {
        all: PnlAcc::default(),
        maker: PnlAcc::default(),
        taker: PnlAcc::default(),
    };
    let mut future = vec![None; h];
    let mut weights = vec![0.0; cfg.bootstrap_replicates];
    let mut weight_block = None;
    let mut unassociated = 0;
    // First pass: route events and accumulate global metrics in the original tape order.
    // Reordering fills by order would change compensated sums and bootstrap bands.
    for (event_index, event) in tape.iter().enumerate() {
        if cfg.observation_end_ns.is_some_and(|end| event.ts > end) {
            continue;
        }
        if cfg.observation_end_ns.is_none() && event.clock == bb_core::Clock::Exchange {
            end = Some(end.map_or(event.ts, |t| t.max(event.ts)));
        }
        let Some(events) = event.order_id.and_then(|id| indexed_orders.get_mut(&id)) else {
            if matches!(event.body, FlightBody::ExchFill { .. }) {
                unassociated += 1;
                pnl.all.excluded += 1;
            }
            continue;
        };
        if events.last == usize::MAX {
            events.first = event_index;
        } else {
            next_event[events.last] = event_index;
        }
        events.last = event_index;
        match &event.body {
            FlightBody::OrderIntent { market, .. } => {
                events.decision = market
                    .best_bid_tick
                    .zip(market.best_ask_tick)
                    .filter(|(b, a)| a > b)
                    .map(|(b, a)| (a as f64 + b as f64) * 0.5 * inst.tick_size)
                    .filter(|p| p.is_finite() && *p > 0.0);
            }
            FlightBody::ExchFill {
                price_tick,
                qty,
                maker,
                fee,
                ..
            } => {
                // Horizon marks may extend beyond the run, but never beyond the dataset.
                if !qty.is_finite() || *qty <= 0.0 || !fee.is_finite() {
                    return Err(ExecutionError::Config(
                        "non-finite fee or nonpositive fill quantity",
                    ));
                }
                let price = inst.price(*price_tick);
                let sign = events.order.side.sign();
                let mid = mid_at(book, event.ts, true, inst.tick_size);
                let arrival = events
                    .order
                    .exch_arrival_ts
                    .and_then(|t| mid_at(book, t, false, inst.tick_size));
                let decision = directional
                    .get(&events.order.order_id)
                    .and_then(|override_price| override_price.or(events.decision));
                for (slot, horizon) in future.iter_mut().zip(&cfg.horizons_ns) {
                    *slot = event
                        .ts
                        .checked_add(*horizon)
                        .and_then(|t| horizon_mid(book, t, inst.tick_size));
                }
                let block = event.ts.div_euclid(cfg.bootstrap_block_ns);
                if weight_block != Some(block) {
                    for (r, w) in weights.iter_mut().enumerate() {
                        *w = poisson(cfg.bootstrap_seed, block, r);
                    }
                    weight_block = Some(block);
                }
                let fill = FillTerms {
                    qty: *qty,
                    sign,
                    price,
                };
                spreads.all.fill(fill, mid, &future, arrival, decision);
                let global = if *maker {
                    &mut spreads.maker
                } else {
                    &mut spreads.taker
                };
                global.fill(fill, mid, &future, arrival, decision);
                boot.all.fill(block, &weights, *qty, sign, mid, &future);
                (if *maker {
                    &mut boot.maker
                } else {
                    &mut boot.taker
                })
                .fill(block, &weights, *qty, sign, mid, &future);
                pnl.all.fill(fill, mid, end_mid, *fee, inst.contract_size);
                (if *maker {
                    &mut pnl.maker
                } else {
                    &mut pnl.taker
                })
                .fill(fill, mid, end_mid, *fee, inst.contract_size);
            }
            _ => {}
        }
    }
    let mut component_values: [Vec<f64>; 5] = std::array::from_fn(|_| Vec::new());
    let mut cancel_values: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
    let (mut attempts, mut exchange_censored, mut acknowledgement_censored, mut late_cancels) =
        (0u64, 0u64, 0u64, 0u64);
    let mut rel = vec![Vec::new(); cfg.relative_queue_edges.len() + 1];
    let mut abs = vec![Vec::new(); cfg.absolute_queue_lot_edges.len() + 1];
    let mut excluded_rel = 0;
    let mut excluded_abs = 0;
    let mut first_obs = Vec::new();
    let mut complete_obs = Vec::new();
    let mut first_values = Vec::new();
    let mut complete_values = Vec::new();
    let mut shortfalls = Vec::new();
    let mut opportunity_values = vec![Vec::new(); h + 1];
    let mut opportunity_horizons = vec![0];
    opportunity_horizons.extend_from_slice(&cfg.horizons_ns);
    // Second pass: replay one order at a time, emit its detail, then free its accumulators.
    // BTreeMap order preserves detail ordering and the input order of exact distributions.
    for (_, events) in indexed_orders {
        let order = events.order;
        let mut state = OrderState::new(order, if on_order.is_some() { h } else { 0 });
        let mut event_index = events.first;
        while event_index != usize::MAX {
            let event = &tape[event_index];
            event_index = next_event[event_index];
            match &event.body {
                FlightBody::OrderIntent { market, .. } => {
                    state.decision = market
                        .best_bid_tick
                        .zip(market.best_ask_tick)
                        .filter(|(b, a)| a > b)
                        .map(|(b, a)| (a as f64 + b as f64) * 0.5 * inst.tick_size)
                        .filter(|p| p.is_finite() && *p > 0.0);
                }
                FlightBody::RequestSent {
                    req: ReqKind::Cancel,
                    exch_arrival_ts,
                    ..
                } => state.cancels.push(CancelState {
                    sent: event.ts,
                    scheduled: *exch_arrival_ts,
                    processed: None,
                    ack: None,
                    exposure: Sum::default(),
                    late: false,
                }),
                FlightBody::ExchQueued {
                    queue_ahead,
                    level_qty,
                    ..
                } => {
                    if state.resting.is_none() {
                        state.resting = Some(event.ts);
                        state.ahead = *queue_ahead;
                        state.level = Some(*level_qty);
                    }
                }
                FlightBody::ExchFill {
                    price_tick,
                    qty,
                    maker,
                    fee,
                    leaves_qty,
                    ..
                } => {
                    let price = inst.price(*price_tick);
                    let sign = order.side.sign();
                    if on_order.is_some() {
                        let mid = mid_at(book, event.ts, true, inst.tick_size);
                        let arrival = state
                            .arrival
                            .and_then(|t| mid_at(book, t, false, inst.tick_size));
                        let decision = directional
                            .get(&order.order_id)
                            .and_then(|override_price| override_price.or(state.decision));
                        for (slot, horizon) in future.iter_mut().zip(&cfg.horizons_ns) {
                            *slot = event
                                .ts
                                .checked_add(*horizon)
                                .and_then(|t| horizon_mid(book, t, inst.tick_size));
                        }
                        let fill = FillTerms {
                            qty: *qty,
                            sign,
                            price,
                        };
                        state
                            .spreads
                            .all
                            .fill(fill, mid, &future, arrival, decision);
                        (if *maker {
                            &mut state.spreads.maker
                        } else {
                            &mut state.spreads.taker
                        })
                        .fill(fill, mid, &future, arrival, decision);
                    }
                    state.qty.add(*qty);
                    state.notional.add(*qty * price);
                    state.fees.add(*fee);
                    state.first.get_or_insert(event.ts);
                    if state.resting.is_some_and(|t| event.ts >= t) {
                        state.first_rest.get_or_insert(event.ts);
                    }
                    if *leaves_qty <= 0.0 {
                        state.complete.get_or_insert(event.ts);
                    }
                    for c in &mut state.cancels {
                        if event.ts >= c.sent && c.processed.is_none_or(|t| event.ts <= t) {
                            c.exposure.add(*qty);
                        }
                    }
                }
                FlightBody::ExchResponse {
                    status,
                    request_rejected,
                    ..
                } => {
                    if let Some(c) = state.cancels.iter_mut().find(|c| {
                        c.processed.is_none()
                            && event.ts >= c.sent
                            && c.scheduled.is_some_and(|t| event.ts >= t)
                            && (*request_rejected || *status == OrderStatus::Canceled)
                    }) {
                        c.processed = Some(event.ts);
                        c.late = *request_rejected;
                    }
                }
                FlightBody::LocalResponse {
                    exch_ts,
                    request_rejected,
                    ..
                } => {
                    if let Some(c) = state.cancels.iter_mut().find(|c| {
                        c.ack.is_none() && c.processed == Some(*exch_ts) && event.ts >= c.sent
                    }) {
                        c.ack = Some(event.ts);
                        c.late |= *request_rejected;
                    }
                }
                _ => {}
            }
        }
        let eligibility = directional.get(&order.order_id);
        let pd = eligibility.and_then(|price| price.or(state.decision));
        let sf = shortfall(&state, pd, eligibility.is_some(), book, end, inst);
        if let Some(v) = sf.total.get() {
            shortfalls.push(v);
        }
        for (values, metric) in component_values.iter_mut().zip([
            &sf.execution,
            &sf.delay,
            &sf.trading,
            &sf.opportunity,
            &sf.fees,
        ]) {
            if let Some(value) = metric.get() {
                values.push(value);
            }
        }
        let mut opportunities = Vec::with_capacity(h + 1);
        let u = (order.qty - state.qty.get()).max(0.0);
        for (i, horizon) in opportunity_horizons.iter().enumerate() {
            let price = order
                .terminal_ts
                .into_iter()
                .chain(end)
                .min()
                .and_then(|t| t.checked_add(*horizon))
                .and_then(|t| {
                    if *horizon == 0 {
                        mid_at(book, t, false, inst.tick_size)
                    } else {
                        horizon_mid(book, t, inst.tick_size)
                    }
                });
            let cost = if eligibility.is_none()
                || !state.arrival.is_some_and(|t| end.is_none_or(|e| t <= e))
            {
                None
            } else if u == 0.0 {
                pd.map(|_| 0.0)
            } else {
                pd.zip(price)
                    .map(|(p, m)| order.side.sign() * u * (m - p) * inst.contract_size)
            };
            if let Some(v) = cost {
                opportunity_values[i].push(v);
            }
            opportunities.push(OpportunityCost {
                horizon_ns: *horizon,
                cost: amount(
                    cost,
                    "not directional, not arrived, or missing decision/evaluation price",
                ),
            });
        }
        let ff = lifetime(state.arrival, state.first, order.terminal_ts, end);
        let cf = lifetime(state.arrival, state.complete, order.terminal_ts, end);
        if let Some(o) = ff {
            first_obs.push(o);
            if o.event {
                first_values.push(o.elapsed_ns as f64);
            }
        }
        if let Some(o) = cf {
            complete_obs.push(o);
            if o.event {
                complete_values.push(o.elapsed_ns as f64);
            }
        }
        let queue_obs = lifetime(state.resting, state.first_rest, order.terminal_ts, end);
        let ahead = state.ahead.filter(|x| x.is_finite() && *x >= 0.0);
        if let Some((ahead, o)) = ahead.zip(queue_obs) {
            let lots = ahead / inst.lot_size;
            abs[cfg.absolute_queue_lot_edges.partition_point(|e| *e <= lots)].push(o);
        } else {
            excluded_abs += 1;
        }
        if let Some(((ahead, level), o)) = ahead
            .zip(state.level.filter(|l| l.is_finite() && *l > 0.0))
            .zip(queue_obs)
        {
            let rho = ahead / level;
            rel[cfg.relative_queue_edges.partition_point(|e| *e <= rho)].push(o);
        } else {
            excluded_rel += 1;
        }
        let cancels: Vec<CancelMetrics> = state
            .cancels
            .iter()
            .map(|c| CancelMetrics {
                sent_ns: c.sent,
                to_exchange: duration(
                    Some(c.sent),
                    c.processed,
                    "cancel not processed before observation end",
                ),
                round_trip: duration(Some(c.sent), c.ack, "cancel acknowledgement right-censored"),
                in_flight_quantity: if c.processed.is_some() {
                    Value::measured(c.exposure.get(), "base", 1, 0)
                } else {
                    Value::unavailable(
                        "cancel processing censored; exposure window incomplete",
                        "base",
                        1,
                    )
                },
                late_cancel: c.late,
                exchange_censored: c.processed.is_none(),
                acknowledgement_censored: c.ack.is_none(),
            })
            .collect();
        for cancel in &cancels {
            attempts += 1;
            exchange_censored += u64::from(cancel.exchange_censored);
            acknowledgement_censored += u64::from(cancel.acknowledgement_censored);
            late_cancels += u64::from(cancel.late_cancel);
            for (values, metric) in cancel_values.iter_mut().zip([
                &cancel.to_exchange,
                &cancel.round_trip,
                &cancel.in_flight_quantity,
            ]) {
                if let Some(value) = metric.get() {
                    values.push(value);
                }
            }
        }
        if let Some(on_order) = &mut on_order {
            on_order(OrderExecution {
                order_id: order.order_id,
                spreads: state.spreads.report(cfg, inst.tick_size, None),
                shortfall: sf,
                opportunities,
                time_to_first_fill: duration(
                    state.arrival,
                    state.first,
                    "first fill right-censored or no exchange arrival",
                ),
                time_to_complete_fill: duration(
                    state.arrival,
                    state.complete,
                    "complete fill right-censored or no exchange arrival",
                ),
                first_fill_censored: ff.is_some_and(|o| !o.event),
                complete_fill_censored: cf.is_some_and(|o| !o.event),
                cancels,
            });
        }
    }
    let n = orders.len() as u64;
    let times = |mut obs: Vec<SurvivalObservation>, values: Vec<f64>| {
        let horizon = obs.iter().map(|o| o.elapsed_ns).max().unwrap_or(0);
        obs.resize(
            orders.len(),
            SurvivalObservation {
                elapsed_ns: -1,
                event: false,
            },
        );
        let km = kaplan_meier(&obs, horizon);
        let excluded = n - values.len() as u64;
        FillTimes {
            observed_ns: distribution(values, "ns", excluded),
            km,
        }
    };
    let [execution, delay, trading, opportunity, fees] = component_values.map(|values| {
        let excluded = n - values.len() as u64;
        distribution(values, "quote", excluded)
    });
    let shortfall_components = ShortfallComponents {
        execution,
        delay,
        trading,
        opportunity,
        fees,
    };
    let [to_exchange, round_trip, in_flight] = cancel_values;
    let cancel_distribution = |values: Vec<f64>, unit: &str| {
        let excluded = attempts - values.len() as u64;
        distribution(values, unit, excluded)
    };
    let cancels = CancelSummary {
        attempts,
        exchange_censored,
        acknowledgement_censored,
        late_cancels,
        to_exchange_ns: cancel_distribution(to_exchange, "ns"),
        round_trip_ns: cancel_distribution(round_trip, "ns"),
        in_flight_quantity: cancel_distribution(in_flight, "base"),
    };
    Ok(ExecutionReport {
        analytics_version: crate::ANALYTICS_VERSION.into(),
        order_count: n,
        order_details_streamed: true,
        spreads: spreads.report(cfg, inst.tick_size, Some(&boot)), orders: Vec::new(),
        shortfall: { let excluded = n - shortfalls.len() as u64; distribution(shortfalls, "quote", excluded) },
        shortfall_components, cancels,
        opportunities: opportunity_horizons.into_iter().zip(opportunity_values).map(|(h, v)| { let excluded = n - v.len() as u64; (h, distribution(v, "quote", excluded)) }).collect(),
        queue: QueueMetrics { horizon_ns: cfg.queue_horizon_ns, modeled: true,
            relative: queue_buckets(&cfg.relative_queue_edges, &rel, "ratio", cfg.queue_horizon_ns),
            absolute_lots: queue_buckets(&cfg.absolute_queue_lot_edges, &abs, "lots", cfg.queue_horizon_ns), excluded_relative: excluded_rel, excluded_absolute: excluded_abs },
        first_fill: times(first_obs, first_values), complete_fill: times(complete_obs, complete_values),
        pnl: LiquiditySplit { all: pnl.all.report(), maker: pnl.maker.report(), taker: pnl.taker.report() },
        exchange_end_mid: end_mid.map(|m| Value::measured(m, "quote/base", 1, 0)).unwrap_or_else(|| Value::unavailable("missing exchange end mid", "quote/base", 1)),
        observation_end_ns: end, unassociated_fills: unassociated,
        caveats: vec!["Replay has no market reaction/impact; queue positions are modeled, not observed.".into(),
            "E9 uses the exchange-clock end mid and starts from zero inventory/cash; the existing local-clock ledger need not match.".into(),
            "IS and opportunity cost require explicit directional designation; one intent is one parent, not a market-maker quote.".into(),
            "Censoring-based estimates assume non-informative censoring; strategic cancellations can violate this.".into(),
            "Cancel responses have no request ID: rejected/canceled exchange responses are matched to outstanding cancel requests and local exch_ts.".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bb_core::{Clock, OrdType, Side, Source, Tif, result::OrderOutcome, strategy::MarketView};
    use proptest::prelude::*;

    const MS: i64 = 1_000_000;
    const SECOND: i64 = 1_000_000_000;
    fn inst() -> InstrumentMeta {
        InstrumentMeta {
            tick_size: 0.5,
            lot_size: 1.0,
            contract_size: 1.0,
        }
    }
    fn config() -> ExecutionConfig {
        ExecutionConfig {
            horizons_ns: vec![100 * MS, SECOND],
            bootstrap_replicates: 64,
            ..ExecutionConfig::default()
        }
    }
    fn book(rows: &[(i64, i64, i64)]) -> BboSeries {
        BboSeries {
            ts: rows.iter().map(|r| r.0).collect(),
            bid_tick: rows.iter().map(|r| Some(r.1)).collect(),
            ask_tick: rows.iter().map(|r| Some(r.2)).collect(),
            bid_qty: vec![10.0; rows.len()],
            ask_qty: vec![10.0; rows.len()],
        }
    }
    fn order(id: u64, side: Side, qty: f64, arrival: i64) -> OrderRecord {
        OrderRecord {
            order_id: id,
            tick_no: id,
            slot: 0,
            side,
            price_tick: 203,
            qty,
            tif: Tif::Ioc,
            ord_type: OrdType::Limit,
            reason: "fixture".into(),
            outcome: OrderOutcome::PartiallyFilled,
            decision_ts: 0,
            exch_arrival_ts: Some(arrival),
            first_response_ts: None,
            queue_ahead_at_entry: None,
            min_queue_ahead: None,
            traded_at_price_qty: 0.0,
            filled_qty: 0.0,
            maker_qty: 0.0,
            taker_qty: 0.0,
            avg_fill_tick: None,
            fees: 0.0,
            fill_count: 0,
            first_fill_ts: None,
            last_fill_ts: None,
            cancel_sent_ts: None,
            cancel_reason: None,
            cancel_rejected: false,
            terminal_ts: None,
            final_status: OrderStatus::New,
            first_seq: 0,
            last_seq: 0,
        }
    }
    fn event(id: u64, ts: i64, body: FlightBody) -> FlightEvent {
        let clock = if matches!(
            body,
            FlightBody::OrderIntent { .. }
                | FlightBody::RequestSent { .. }
                | FlightBody::LocalResponse { .. }
        ) {
            Clock::Local
        } else {
            Clock::Exchange
        };
        FlightEvent {
            seq: 0,
            ts,
            clock,
            source: Source::Engine,
            order_id: Some(id),
            body,
        }
    }
    fn fill(
        id: u64,
        ts: i64,
        price: i64,
        qty: f64,
        maker: bool,
        fee: f64,
        leaves: f64,
    ) -> FlightEvent {
        event(
            id,
            ts,
            FlightBody::ExchFill {
                price_tick: price,
                qty,
                maker,
                fee,
                value: price as f64 * 0.5 * qty,
                leaves_qty: leaves,
            },
        )
    }
    fn intent(id: u64) -> FlightEvent {
        event(
            id,
            0,
            FlightBody::OrderIntent {
                tick_no: 0,
                slot: 0,
                side: Side::Buy,
                price_tick: 203,
                qty: 5.0,
                tif: Tif::Ioc,
                ord_type: OrdType::Limit,
                reason: "fixture".into(),
                market: MarketView {
                    best_bid_tick: Some(200),
                    best_ask_tick: Some(202),
                    best_bid_qty: 2.0,
                    best_ask_qty: 3.0,
                    feed_exch_ts: Some(0),
                    feed_local_ts: Some(0),
                },
                known_position: 0.0,
            },
        )
    }
    fn near(v: &Value, expected: f64) {
        assert!(
            (v.get().expect("available metric") - expected).abs() < 1e-9,
            "{v:?} != {expected}"
        );
    }

    #[test]
    fn e1_e2_e4_e5_e10_directional_fixture() {
        // Buy target 5, fill 1@101 + 3@101.5, Pd=100.5, arrival=100.75.
        // ES=.625; RS(1s)=.125; AS=-.5. Execution=3.5 = delay1+trading2.5.
        // Abandon one unit at102: OC=1.5; fees=.1622; IS=5.1622.
        let mut o = order(1, Side::Buy, 5.0, MS);
        o.terminal_ts = Some(1100 * MS);
        let b = book(&[
            (0, 201, 202),
            (1001 * MS, 202, 203),
            (1100 * MS, 203, 205),
            (2 * SECOND, 203, 205),
        ]);
        let tape = [
            intent(1),
            fill(1, MS, 202, 1.0, false, 0.0404, 4.0),
            fill(1, MS, 203, 3.0, false, 0.1218, 1.0),
        ];
        let mut cfg = config();
        cfg.directional_orders.push(DirectionalOrder {
            order_id: 1,
            decision_price: None,
        });
        let r = compute_parts(&[o], &tape, &b, &inst(), &cfg).unwrap();
        near(&r.spreads.taker.effective_half.price, 0.625);
        near(&r.spreads.taker.effective_half.ticks, 1.25);
        near(
            &r.spreads.taker.effective_half.bps,
            0.625 / 100.75 * 10_000.0,
        );
        near(&r.spreads.taker.effective_full.price, 1.25);
        let h = &r.spreads.taker.horizons[1];
        near(&h.realised_half.price, 0.125);
        near(&h.markout.price, -0.125);
        near(&h.adverse_selection.price, -0.5);
        near(
            &h.matched_effective_half.price,
            h.realised_half.price.get().unwrap() - h.adverse_selection.price.get().unwrap(),
        );
        let sf = &r.orders[0].shortfall;
        near(&sf.execution, 3.5);
        near(&sf.delay, 1.0);
        near(&sf.trading, 2.5);
        near(&sf.opportunity, 1.5);
        near(&sf.fees, 0.1622);
        near(&sf.total, 5.1622);
        near(&sf.bps, 5.1622 / 502.5 * 10_000.0);
        near(&sf.arrival_slippage.price, 0.625);
        near(&sf.decision_slippage.price, 0.875);
        near(&sf.latency_drift.price, 0.25);
        near(&r.orders[0].opportunities[0].cost, 1.5);
        near(&r.opportunities[0].1.total, 1.5);
        assert!(r.orders[0].complete_fill_censored);
    }

    #[test]
    fn e1_e2_e3_maker_fixture() {
        // Buy2@100 against pre-fill100.5; 100ms mid100.25 and1s mid99.75.
        // ES=-.5; AS=.25/.75; RS(1s)=.25; markout=-.25.
        let b = book(&[(0, 200, 202), (101 * MS, 200, 201), (1001 * MS, 199, 200)]);
        let r = compute_parts(
            &[order(1, Side::Buy, 2.0, MS)],
            &[fill(1, MS, 200, 2.0, true, 0.0, 0.0)],
            &b,
            &inst(),
            &config(),
        )
        .unwrap();
        near(&r.spreads.maker.effective_half.price, -0.5);
        near(&r.spreads.maker.horizons[0].adverse_selection.price, 0.25);
        near(&r.spreads.maker.horizons[1].adverse_selection.price, 0.75);
        near(&r.spreads.maker.horizons[1].realised_half.price, 0.25);
        near(&r.spreads.maker.horizons[1].markout.price, -0.25);
        assert!(
            r.spreads.maker.horizons[1]
                .adverse_selection_95
                .lower
                .get()
                .is_none()
        );
    }

    #[test]
    fn strict_prefill_missing_reference_and_horizon_censoring() {
        // At the fill timestamp the book jumps from100.5 to110.5. Strict ES=-.5,
        // not-10.5; a fill at the first BBO timestamp has no pre-fill reference.
        let b = book(&[(0, 200, 202), (MS, 220, 222)]);
        let r = compute_parts(
            &[order(1, Side::Buy, 2.0, 0)],
            &[
                fill(1, 0, 200, 1.0, true, 0.0, 1.0),
                fill(1, MS, 200, 1.0, true, 0.0, 0.0),
            ],
            &b,
            &inst(),
            &config(),
        )
        .unwrap();
        near(&r.spreads.all.effective_half.price, -0.5);
        assert!(matches!(
            r.spreads.all.effective_half.price,
            Value::Available {
                n: 1,
                excluded: 1,
                ..
            }
        ));
        assert!(
            r.spreads.all.horizons[0]
                .realised_half
                .price
                .get()
                .is_none()
        );
        assert!(r.pnl.all.net.get().is_none());
    }

    #[test]
    fn directional_gating_and_missing_decision_never_use_limit() {
        let b = book(&[(0, 201, 202), (2 * SECOND, 203, 205)]);
        let o = order(1, Side::Buy, 5.0, MS);
        let tape = [fill(1, MS, 202, 1.0, false, 0.0, 4.0)];
        let r = compute_parts(std::slice::from_ref(&o), &tape, &b, &inst(), &config()).unwrap();
        assert!(r.orders[0].shortfall.total.get().is_none());
        assert!(r.orders[0].opportunities[0].cost.get().is_none());
        // E5 arrival slippage is defined even for a nondirectional order.
        near(&r.orders[0].shortfall.arrival_slippage.price, 0.25);
        let mut cfg = config();
        cfg.directional_orders.push(DirectionalOrder {
            order_id: 1,
            decision_price: None,
        });
        let r = compute_parts(std::slice::from_ref(&o), &tape, &b, &inst(), &cfg).unwrap();
        assert!(r.orders[0].shortfall.total.get().is_none());
        cfg.directional_orders[0].decision_price = Some(100.5);
        let r = compute_parts(&[o], &tape, &b, &inst(), &cfg).unwrap();
        near(&r.orders[0].shortfall.execution, 0.5);
        near(&r.orders[0].opportunities[0].cost, 6.0);
    }

    #[test]
    fn e6_km_fixture_greenwood_and_ties() {
        // S=.75*(1-.5)=.375; F=.625, not naive .5 or drop-censored2/3.
        // Greenwood=.083333+.5=7/12, SE=.375*sqrt(7/12).
        let observations = [
            SurvivalObservation {
                elapsed_ns: 200 * MS,
                event: true,
            },
            SurvivalObservation {
                elapsed_ns: 400 * MS,
                event: false,
            },
            SurvivalObservation {
                elapsed_ns: 800 * MS,
                event: true,
            },
            SurvivalObservation {
                elapsed_ns: SECOND,
                event: false,
            },
        ];
        let km = kaplan_meier(&observations, SECOND);
        near(&km.fill_probability, 0.625);
        near(&km.median_ns, 800_000_000.0);
        near(
            &km.probability_95.lower,
            (0.625 - 1.959963984540054 * 0.375 * (7.0_f64 / 12.0).sqrt()).max(0.0),
        );
        near(&km.probability_95.upper, 1.0);
        assert_eq!(km.censored, 2);
        assert_eq!(km.curve[2].at_risk, 2);
        let tied = kaplan_meier(
            &[
                SurvivalObservation {
                    elapsed_ns: 1,
                    event: true,
                },
                SurvivalObservation {
                    elapsed_ns: 1,
                    event: false,
                },
            ],
            1,
        );
        near(&tied.fill_probability, 0.5);
        assert!(
            kaplan_meier(&observations, 2 * SECOND)
                .fill_probability
                .get()
                .is_none()
        );
    }

    #[test]
    fn e6_queue_relative_and_absolute_bucket_integration() {
        let b = book(&[(0, 200, 202), (2 * SECOND, 200, 202)]);
        let mut orders = Vec::new();
        let mut tape = Vec::new();
        for id in 1..=4 {
            let mut o = order(id, Side::Buy, 1.0, 0);
            o.tif = Tif::Gtc;
            if id == 2 {
                o.terminal_ts = Some(400 * MS);
            }
            orders.push(o);
            tape.push(event(
                id,
                0,
                FlightBody::ExchQueued {
                    queue_ahead: Some(5.0),
                    level_qty: 10.0,
                    best_bid_tick: Some(200),
                    best_ask_tick: Some(202),
                },
            ));
        }
        tape.push(fill(1, 200 * MS, 200, 1.0, true, 0.0, 0.0));
        tape.push(fill(3, 800 * MS, 200, 1.0, true, 0.0, 0.0));
        let r = compute_parts(&orders, &tape, &b, &inst(), &config()).unwrap();
        near(&r.queue.relative[2].estimate.fill_probability, 0.625);
        near(&r.queue.absolute_lots[1].estimate.fill_probability, 0.625);
        assert_eq!(r.queue.excluded_relative, 0);
    }

    #[test]
    fn e7_first_complete_and_partial_censoring_fixture() {
        // Exchange arrival1000ms; 2@1300 and3@1750 => TTFF300ms, TTCF750ms.
        let b = book(&[(0, 200, 202), (2 * SECOND, 200, 202)]);
        let tape = [
            fill(1, 1300 * MS, 200, 2.0, true, 0.0, 3.0),
            fill(1, 1750 * MS, 200, 3.0, true, 0.0, 0.0),
        ];
        let r = compute_parts(
            &[order(1, Side::Buy, 5.0, SECOND)],
            &tape,
            &b,
            &inst(),
            &config(),
        )
        .unwrap();
        near(&r.orders[0].time_to_first_fill, 300_000_000.0);
        near(&r.orders[0].time_to_complete_fill, 750_000_000.0);
        near(&r.first_fill.km.median_ns, 300_000_000.0);
        near(&r.complete_fill.km.median_ns, 750_000_000.0);
        let r = compute_parts(
            &[order(1, Side::Buy, 5.0, SECOND)],
            &tape[..1],
            &b,
            &inst(),
            &config(),
        )
        .unwrap();
        assert!(r.orders[0].complete_fill_censored);
        assert_eq!(r.complete_fill.km.censored, 1);
        assert!(r.orders[0].time_to_complete_fill.get().is_none());
    }

    #[test]
    fn e8_cancel_latency_exposure_and_censoring_fixture() {
        // Send5000ms, fill1@5000.4ms, process5001ms, ack5002ms =>1ms/2ms/1unit.
        let b = book(&[(0, 200, 202), (6 * SECOND, 200, 202)]);
        let tape = [
            event(
                1,
                5000 * MS,
                FlightBody::RequestSent {
                    req: ReqKind::Cancel,
                    entry_latency_ns: MS,
                    exch_arrival_ts: Some(5001 * MS),
                },
            ),
            fill(1, 5000 * MS + 400_000, 200, 1.0, true, 0.0, 1.0),
            event(
                1,
                5001 * MS,
                FlightBody::ExchResponse {
                    status: OrderStatus::Canceled,
                    request_rejected: false,
                    leaves_qty: 1.0,
                    response_latency_ns: MS,
                    local_arrival_ts: 5002 * MS,
                },
            ),
            event(
                1,
                5002 * MS,
                FlightBody::LocalResponse {
                    status: OrderStatus::Canceled,
                    request_rejected: false,
                    exec_qty: 0.0,
                    exec_price_tick: 0,
                    leaves_qty: 1.0,
                    exch_ts: 5001 * MS,
                },
            ),
        ];
        let mut o = order(1, Side::Buy, 2.0, SECOND);
        o.terminal_ts = Some(5001 * MS);
        let r = compute_parts(&[o.clone()], &tape, &b, &inst(), &config()).unwrap();
        let c = &r.orders[0].cancels[0];
        near(&c.to_exchange, 1_000_000.0);
        near(&c.round_trip, 2_000_000.0);
        near(&c.in_flight_quantity, 1.0);
        assert!(!c.late_cancel && !c.exchange_censored && !c.acknowledgement_censored);
        let mut cfg = config();
        cfg.observation_end_ns = Some(5000 * MS + 500_000);
        let r = compute_parts(&[o], &tape, &b, &inst(), &cfg).unwrap();
        let c = &r.orders[0].cancels[0];
        assert!(c.exchange_censored && c.acknowledgement_censored);
        assert!(c.to_exchange.get().is_none() && c.in_flight_quantity.get().is_none());
    }

    #[test]
    fn e8_late_cancel_is_rejection_not_terminal_fill_latency() {
        let b = book(&[(0, 200, 202), (6 * SECOND, 200, 202)]);
        let tape = [
            event(
                1,
                5000 * MS,
                FlightBody::RequestSent {
                    req: ReqKind::Cancel,
                    entry_latency_ns: MS,
                    exch_arrival_ts: Some(5001 * MS),
                },
            ),
            fill(1, 5000 * MS + 400_000, 200, 1.0, true, 0.0, 0.0),
            event(
                1,
                5000 * MS + 400_000,
                FlightBody::ExchResponse {
                    status: OrderStatus::Filled,
                    request_rejected: false,
                    leaves_qty: 0.0,
                    response_latency_ns: MS,
                    local_arrival_ts: 5001 * MS + 400_000,
                },
            ),
            event(
                1,
                5001 * MS,
                FlightBody::ExchResponse {
                    status: OrderStatus::Filled,
                    request_rejected: true,
                    leaves_qty: 0.0,
                    response_latency_ns: MS,
                    local_arrival_ts: 5002 * MS,
                },
            ),
        ];
        let r = compute_parts(
            &[order(1, Side::Buy, 1.0, SECOND)],
            &tape,
            &b,
            &inst(),
            &config(),
        )
        .unwrap();
        assert!(r.orders[0].cancels[0].late_cancel);
        near(&r.orders[0].cancels[0].to_exchange, 1_000_000.0);
    }

    #[test]
    fn e9_exchange_mark_pnl_fixture_and_contract_scaling() {
        // Maker buy2@100:m0=100.5; taker sell2@100:m0=100.25; end100.25.
        // Edge1-.5, drift-.5+0, fees.04+.08 =>net-.12; zero terminal inventory.
        let b = book(&[(0, 200, 202), (MS + 500_000, 200, 201), (3 * MS, 200, 201)]);
        let orders = [
            order(1, Side::Buy, 2.0, MS),
            order(2, Side::Sell, 2.0, 2 * MS),
        ];
        let tape = [
            fill(1, MS, 200, 2.0, true, 0.04, 0.0),
            fill(2, 2 * MS, 200, 2.0, false, 0.08, 0.0),
        ];
        let r = compute_parts(&orders, &tape, &b, &inst(), &config()).unwrap();
        near(&r.pnl.maker.edge, 1.0);
        near(&r.pnl.taker.edge, -0.5);
        near(&r.pnl.maker.drift, -0.5);
        near(&r.pnl.taker.drift, 0.0);
        near(&r.pnl.all.fees, 0.12);
        near(&r.pnl.all.net, -0.12);
        let mut instrument = inst();
        instrument.contract_size = 10.0;
        let r = compute_parts(&orders, &tape, &b, &instrument, &config()).unwrap();
        near(&r.pnl.maker.edge, 10.0);
        near(&r.pnl.maker.drift, -5.0);
        near(&r.pnl.all.net, -0.12); // Fees are already quote-currency amounts.
    }

    #[test]
    fn e3_bootstrap_resamples_blocks_not_individual_fills() {
        // Duplicate a fill within its block, dividing size in half: every replicate and
        // confidence endpoint must stay identical. Two distinct blocks have AS=-.5,+.5.
        let b = book(&[
            (0, 200, 202),
            (MS + 10, 201, 203),
            (SECOND, 202, 204),
            (SECOND + MS + 10, 201, 203),
            (3 * SECOND, 201, 203),
        ]);
        let mut cfg = config();
        cfg.horizons_ns = vec![10];
        cfg.bootstrap_replicates = 256;
        let orders = [order(1, Side::Buy, 4.0, MS)];
        let a = [
            fill(1, MS, 200, 2.0, true, 0.0, 2.0),
            fill(1, SECOND + MS, 202, 2.0, true, 0.0, 0.0),
        ];
        let duplicated = [
            fill(1, MS, 200, 1.0, true, 0.0, 3.0),
            fill(1, MS, 200, 1.0, true, 0.0, 2.0),
            fill(1, SECOND + MS, 202, 2.0, true, 0.0, 0.0),
        ];
        let x = compute_parts(&orders, &a, &b, &inst(), &cfg).unwrap();
        let y = compute_parts(&orders, &duplicated, &b, &inst(), &cfg).unwrap();
        let xb = &x.spreads.all.horizons[0].adverse_selection_95;
        let yb = &y.spreads.all.horizons[0].adverse_selection_95;
        near(&xb.lower, -0.5);
        near(&xb.upper, 0.5);
        assert_eq!(xb.lower.get(), yb.lower.get());
        assert_eq!(xb.upper.get(), yb.upper.get());
        assert_eq!(
            xb.lower.get(),
            x.spreads.maker.horizons[0].adverse_selection_95.lower.get()
        );
    }

    #[test]
    fn final_unchanged_quote_does_not_drop_later_recorded_fills() {
        // BBO stores changes, not a row for every trade. A later fill still has a known
        // strict pre-fill reference and final exchange mark; future coverage is unknown.
        let b = book(&[(0, 200, 202)]);
        let r = compute_parts(
            &[order(1, Side::Buy, 1.0, MS)],
            &[fill(1, MS, 200, 1.0, true, 0.0, 0.0)],
            &b,
            &inst(),
            &config(),
        )
        .unwrap();
        near(&r.spreads.all.filled_quantity, 1.0);
        near(&r.spreads.all.effective_half.price, -0.5);
        near(&r.pnl.all.net, 0.5);
        near(&r.orders[0].time_to_first_fill, 0.0);
        assert!(
            r.spreads.all.horizons[0]
                .realised_half
                .price
                .get()
                .is_none()
        );
    }

    proptest! {
        #[test]
        fn spread_identity_and_exchange_mark_pnl_conservation(
            buy in any::<bool>(), p in 100i64..1000, m0 in 100i64..1000,
            mh in 100i64..1000, quantity in 1u32..100, fee in -10i32..10
        ) {
            let mut cfg = config(); cfg.horizons_ns = vec![MS]; cfg.bootstrap_replicates = 0;
            let side = if buy { Side::Buy } else { Side::Sell };
            let b = book(&[(0, m0, m0 + 2), (2 * MS, mh, mh + 2)]);
            let tape = [fill(1, MS, p, quantity as f64, true, fee as f64, 0.0)];
            let r = compute_parts(&[order(1, side, quantity as f64, MS)], &tape, &b, &inst(), &cfg).unwrap();
            let h = &r.spreads.all.horizons[0];
            let es = h.matched_effective_half.price.get().unwrap();
            let rs = h.realised_half.price.get().unwrap();
            let adverse = h.adverse_selection.price.get().unwrap();
            prop_assert!((es - (rs - adverse)).abs() < 1e-10);
            let expected = side.sign() * quantity as f64 * ((mh + 1) as f64 - p as f64) * 0.5 - fee as f64;
            prop_assert!((r.pnl.all.net.get().unwrap() - expected).abs() < 1e-8);
        }
        #[test]
        fn km_survival_non_increasing_and_bounded(rows in prop::collection::vec((0i64..10_000, any::<bool>()), 0..100)) {
            let observations: Vec<_> = rows.into_iter().map(|(elapsed_ns, event)| SurvivalObservation { elapsed_ns, event }).collect();
            let km = kaplan_meier(&observations, 10_000);
            let mut previous = 1.0;
            for point in &km.curve {
                let s = point.survival.get().unwrap();
                prop_assert!((0.0..=1.0).contains(&s)); prop_assert!(s <= previous + 1e-15);
                let lo = point.probability_95.lower.get().unwrap(); let hi = point.probability_95.upper.get().unwrap();
                prop_assert!(0.0 <= lo && lo <= hi && hi <= 1.0);
                previous = s;
            }
        }
    }
}
