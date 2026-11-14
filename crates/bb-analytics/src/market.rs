//! M1–M10 on completed exchange-time batches. All timestamps are ns relative to the dataset
//! origin. Series are last-observation-per-fixed-time-bin, never inputs to the exact summaries.
//! Depth is displayed L2 only; no trade-side inference or hidden-liquidity estimates are made.
use std::collections::{BTreeMap, BTreeSet};

use bb_core::{Side, dataset::InstrumentMeta};
use bb_data::{
    Event,
    book::{Bbo, L2Book},
};
use serde::{Deserialize, Serialize};

use crate::{Sum, Value};

// hftbacktest's stable engine-event wire flags. bb-data exposes Event, not its flag constants.
const EXCHANGE: u64 = 1 << 31;
const BUY: u64 = 1 << 29;
const SELL: u64 = 1 << 28;
const DEPTH: u64 = 1;
const TRADE: u64 = 2;
const CLEAR: u64 = 3;
const SNAPSHOT: u64 = 4;
const NS: f64 = 1_000_000_000.0;
const MAX_STATES: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketConfig {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub clock_step_ns: i64,
    /// Inclusive integer basis-point band, anchored to each current mid (M3).
    pub depth_band_bps: u32,
    pub imbalance_levels: Vec<usize>,
    pub strict_imbalance: bool,
    /// None disables series. Some(n) retains at most n points in each sampling mode.
    pub series_max_points: Option<usize>,
    /// Explicit resource limit: exceeding it returns an error, never a partial book.
    pub max_book_levels: usize,
    pub stoikov: Option<StoikovConfig>,
    pub liquidity_windows: Vec<LiquidityWindow>,
}
impl Default for MarketConfig {
    fn default() -> Self {
        Self {
            start_ns: None,
            end_ns: None,
            clock_step_ns: 100_000_000,
            depth_band_bps: 100,
            imbalance_levels: vec![1, 2, 3],
            strict_imbalance: true,
            series_max_points: None,
            max_book_levels: 100_000,
            stoikov: None,
            liquidity_windows: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MarketError {
    #[error("invalid market configuration: {0}")]
    Configuration(String),
    #[error("invalid exchange data: {0}")]
    Data(String),
    #[error("market resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("Stoikov calibration failed: {0}")]
    Calibration(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSummary {
    pub name: String,
    pub unit: String,
    pub event_mean: Value,
    pub clock_mean: Value,
    pub time_weighted_mean: Value,
    pub event_min: Value,
    pub event_max: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketPoint {
    pub ts_ns: i64,
    pub values: Vec<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketSeries {
    pub sampling: String,
    pub clock_step_ns: Option<i64>,
    pub reduction: String,
    pub max_points: usize,
    pub points: Vec<MarketPoint>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VarianceReport {
    pub realised_variance: Value,
    pub variance_rate: Value,
    pub volatility: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlowInterval {
    pub start_ns: i64,
    pub end_ns: i64,
    pub ofi: Value,
    pub flagged_invalid: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradeImbalance {
    pub buy_volume: Value,
    pub sell_volume: Value,
    pub unknown_volume: Value,
    pub imbalance: Value,
    pub coverage: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradeFlowInterval {
    pub start_ns: i64,
    pub end_ns: i64,
    pub trade_imbalance: TradeImbalance,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketReport {
    pub analytics_version: String,
    pub clock: String,
    pub batching: String,
    pub start_ns: i64,
    pub end_ns: i64,
    pub clock_step_ns: i64,
    pub metrics: Vec<MetricSummary>,
    pub event_series: Option<MarketSeries>,
    pub clock_series: Option<MarketSeries>,
    /// True if any evaluated M3 band reaches beyond the deepest published level seen so far.
    pub bid_depth_truncated: bool,
    pub ask_depth_truncated: bool,
    pub event_variance: VarianceReport,
    pub clock_variance: VarianceReport,
    pub order_flow_imbalance: Value,
    pub invalid_ofi_transitions: u64,
    /// Bounded last-interval-per-time-bin view; total OFI above includes every transition.
    pub ofi_intervals: Vec<FlowInterval>,
    pub trade_imbalance: TradeImbalance,
    /// Same bounded interval-view policy as ofi_intervals.
    pub trade_intervals: Vec<TradeFlowInterval>,
    pub stoikov: Option<StoikovModel>,
    pub liquidity: Vec<LiquidityReport>,
}

#[derive(Default)]
struct Average {
    sum: Sum,
    n: u64,
    excluded: u64,
    min: Option<f64>,
    max: Option<f64>,
}
impl Average {
    fn push(&mut self, x: Option<f64>) {
        if let Some(x) = x.filter(|x| x.is_finite()) {
            self.sum.add(x);
            self.n += 1;
            self.min = Some(self.min.map_or(x, |v| v.min(x)));
            self.max = Some(self.max.map_or(x, |v| v.max(x)));
        } else {
            self.excluded += 1;
        }
    }
    fn mean(&self, unit: &str) -> Value {
        if self.n == 0 {
            Value::unavailable("no valid observations", unit, self.excluded)
        } else {
            Value::measured(self.sum.get() / self.n as f64, unit, self.n, self.excluded)
        }
    }
}
#[derive(Default)]
struct Aggregate {
    event: Average,
    clock: Average,
    weighted: Sum,
    valid_ns: i128,
    segments: u64,
    excluded_segments: u64,
}
impl Aggregate {
    fn hold(&mut self, x: Option<f64>, duration: i64) {
        if duration <= 0 {
            return;
        }
        if let Some(x) = x {
            self.weighted.add(x * duration as f64);
            self.valid_ns += i128::from(duration);
            self.segments += 1;
        } else {
            self.excluded_segments += 1;
        }
    }
    fn report(&self, name: &str, unit: &str) -> MetricSummary {
        MetricSummary {
            name: name.into(),
            unit: unit.into(),
            event_mean: self.event.mean(unit),
            clock_mean: self.clock.mean(unit),
            time_weighted_mean: if self.valid_ns > 0 {
                Value::measured(
                    self.weighted.get() / self.valid_ns as f64,
                    unit,
                    self.segments,
                    self.excluded_segments,
                )
            } else {
                Value::unavailable("no valid holding time", unit, self.excluded_segments)
            },
            event_min: self.event.min.map_or_else(
                || Value::unavailable("no valid observations", unit, self.event.excluded),
                |v| Value::measured(v, unit, self.event.n, self.event.excluded),
            ),
            event_max: self.event.max.map_or_else(
                || Value::unavailable("no valid observations", unit, self.event.excluded),
                |v| Value::measured(v, unit, self.event.n, self.event.excluded),
            ),
        }
    }
}

struct Retained<T> {
    points: Vec<T>,
    bins: Vec<usize>,
    max: usize,
    start: i64,
    end: i64,
}
impl<T> Retained<T> {
    fn new(max: usize, start: i64, end: i64) -> Self {
        Self {
            points: Vec::new(),
            bins: Vec::new(),
            max,
            start,
            end,
        }
    }
    fn push(&mut self, ts: i64, value: impl FnOnce() -> T) {
        if self.max == 0 {
            return;
        }
        let value = value();
        let width = (i128::from(self.end) - i128::from(self.start) + 1).max(1);
        let bin = (((i128::from(ts) - i128::from(self.start)).max(0) * self.max as i128) / width)
            .min(self.max as i128 - 1) as usize;
        if self.bins.last() == Some(&bin) {
            if let Some(last) = self.points.last_mut() {
                *last = value;
            }
        } else {
            self.bins.push(bin);
            self.points.push(value);
        }
    }
}
#[derive(Default)]
struct Variance {
    sum: Sum,
    previous: Option<i128>,
    seen: bool,
    broken: bool,
    n: u64,
    excluded: u64,
}
impl Variance {
    fn invalidate(&mut self) {
        self.broken = true;
    }
    fn push(&mut self, mid: Option<i128>, event: bool) {
        if self.seen {
            if let (Some(previous), Some(now)) = (self.previous, mid) {
                if self.broken {
                    self.excluded += 1;
                } else if !event || previous != now {
                    self.sum.add(((now - previous) as f64 * 0.5).powi(2));
                    self.n += 1;
                }
            } else {
                self.excluded += 1;
            }
        }
        self.previous = mid;
        self.seen = true;
        self.broken = false;
    }
    fn report(&self, span_ns: i64, tick_size: f64) -> VarianceReport {
        let rv = if self.n == 0 && (self.excluded > 0 || self.previous.is_none()) {
            Value::unavailable("no valid mid increments", "price^2", self.excluded)
        } else {
            Value::measured(
                self.sum.get() * tick_size * tick_size,
                "price^2",
                self.n,
                self.excluded,
            )
        };
        let rate = if span_ns > 0 {
            rv.get().map(|v| v * NS / span_ns as f64)
        } else {
            None
        };
        VarianceReport {
            realised_variance: rv,
            variance_rate: rate.map_or_else(
                || {
                    Value::unavailable(
                        "no valid increments or zero duration",
                        "price^2/s",
                        self.excluded,
                    )
                },
                |v| Value::measured(v, "price^2/s", self.n, self.excluded),
            ),
            volatility: rate.map_or_else(
                || {
                    Value::unavailable(
                        "no valid increments or zero duration",
                        "price/sqrt(s)",
                        self.excluded,
                    )
                },
                |v| Value::measured(v.sqrt(), "price/sqrt(s)", self.n, self.excluded),
            ),
        }
    }
}

fn kind(e: &Event) -> u64 {
    e.ev & 0xff
}
fn exchange(e: &Event) -> bool {
    e.ev & EXCHANGE != 0
}
fn side(e: &Event) -> Option<Side> {
    if e.ev & BUY != 0 {
        Some(Side::Buy)
    } else if e.ev & SELL != 0 {
        Some(Side::Sell)
    } else {
        None
    }
}
fn mid2(bbo: Bbo) -> Option<i128> {
    if !bbo.is_valid() {
        None
    } else {
        Some(i128::from(bbo.bid?.0) + i128::from(bbo.ask?.0))
    }
}
fn spread(bbo: Bbo) -> Option<i128> {
    if !bbo.is_valid() {
        None
    } else {
        Some(i128::from(bbo.ask?.0) - i128::from(bbo.bid?.0))
    }
}
fn mid(bbo: Bbo, instrument: &InstrumentMeta) -> Option<f64> {
    mid2(bbo).map(|x| x as f64 * 0.5 * instrument.tick_size)
}
fn in_band(tick: i64, m2: i128, bps: u32) -> bool {
    (2 * i128::from(tick) - m2).abs() * 10_000 <= i128::from(bps) * m2
}
/// CKS completed-batch OFI, in exact lots; invalid transitions are absent, not zero.
pub fn order_flow_increment(
    previous: Bbo,
    current: Bbo,
    instrument: &InstrumentMeta,
) -> Option<i128> {
    if !previous.is_valid() || !current.is_valid() {
        return None;
    }
    let (pb, qb) = previous.bid?;
    let (pa, qa) = previous.ask?;
    let (b, q) = current.bid?;
    let (a, r) = current.ask?;
    Some(
        if b >= pb {
            i128::from(instrument.lots(q))
        } else {
            0
        } - if b <= pb {
            i128::from(instrument.lots(qb))
        } else {
            0
        } - if a <= pa {
            i128::from(instrument.lots(r))
        } else {
            0
        } + if a >= pa {
            i128::from(instrument.lots(qa))
        } else {
            0
        },
    )
}

/// Visits completed exchange batches without retaining historical frames. Local-only rows are ignored.
fn replay(
    mut visit: impl FnMut(i64, &L2Book, bool) -> Result<(), MarketError>,
    events: &[Event],
    instrument: &InstrumentMeta,
    origin: i64,
    max_levels: usize,
) -> Result<(), MarketError> {
    let mut book = L2Book::new();
    let mut ts = None;
    let mut dirty = false;
    for e in events.iter().filter(|e| exchange(e)) {
        let rel = e
            .exch_ts
            .checked_sub(origin)
            .ok_or_else(|| MarketError::Data("relative timestamp overflow".into()))?;
        if let Some(previous) = ts
            && previous != rel
        {
            visit(previous, &book, dirty)?;
            dirty = false;
        }
        ts = Some(rel);
        dirty |= book.apply(e, instrument);
        if book.bid_levels().saturating_add(book.ask_levels()) > max_levels {
            return Err(MarketError::ResourceLimit("max_book_levels".into()));
        }
    }
    if let Some(ts) = ts {
        visit(ts, &book, dirty)?;
    }
    Ok(())
}

fn labels(config: &MarketConfig) -> Vec<(String, &'static str)> {
    let mut labels = vec![
        ("quoted_spread".into(), "ticks"),
        ("relative_spread".into(), "bps"),
        ("bid_depth".into(), "base"),
        ("bid_notional".into(), "quote"),
        ("ask_depth".into(), "base"),
        ("ask_notional".into(), "quote"),
    ];
    for n in &config.imbalance_levels {
        labels.push((format!("imbalance_{n}"), "ratio"));
    }
    labels.push(("weighted_mid".into(), "price"));
    labels.push(("stoikov_microprice".into(), "price"));
    labels
}
#[derive(Default)]
struct BookInputs {
    bbo: Bbo,
    bids: Vec<(i64, f64)>,
    asks: Vec<(i64, f64)>,
}
struct Features {
    values: Vec<Option<f64>>,
    inputs: BookInputs,
    bid_truncated: bool,
    ask_truncated: bool,
}
fn features(
    book: &L2Book,
    instrument: &InstrumentMeta,
    config: &MarketConfig,
    model: Option<&StoikovModel>,
    deepest_bid: &mut Option<i64>,
    deepest_ask: &mut Option<i64>,
) -> Features {
    let mut values = vec![None; 8 + config.imbalance_levels.len()];
    let bbo = book.bbo();
    let bids = book.top_bids(book.bid_levels());
    let asks = book.top_asks(book.ask_levels());
    if let Some((b, _)) = bids.last() {
        *deepest_bid = Some(deepest_bid.map_or(*b, |old| old.min(*b)));
    }
    if let Some((a, _)) = asks.last() {
        *deepest_ask = Some(deepest_ask.map_or(*a, |old| old.max(*a)));
    }
    let Some(m2) = mid2(bbo) else {
        return Features {
            values,
            inputs: BookInputs { bbo, bids, asks },
            bid_truncated: false,
            ask_truncated: false,
        };
    };
    let (b, qb) = bbo.bid.unwrap();
    let (a, qa) = bbo.ask.unwrap();
    values[0] = Some((i128::from(a) - i128::from(b)) as f64);
    if m2 > 0 {
        values[1] = Some(2.0 * values[0].unwrap() * 10_000.0 / m2 as f64);
    }
    for (offset, levels) in [(2, &bids), (4, &asks)] {
        if m2 <= 0 {
            continue;
        }
        let mut lots = 0i128;
        let mut notional = Sum::default();
        for &(tick, qty) in levels {
            if in_band(tick, m2, config.depth_band_bps) {
                let q = instrument.lots(qty);
                lots += i128::from(q);
                notional.add(
                    instrument.price(tick)
                        * q as f64
                        * instrument.lot_size
                        * instrument.contract_size,
                );
            }
        }
        values[offset] = Some(lots as f64 * instrument.lot_size);
        values[offset + 1] = Some(notional.get());
    }
    for (i, &n) in config.imbalance_levels.iter().enumerate() {
        if config.strict_imbalance && (bids.len() < n || asks.len() < n) {
            continue;
        }
        let b: i128 = bids
            .iter()
            .take(n)
            .map(|(_, q)| i128::from(instrument.lots(*q)))
            .sum();
        let a: i128 = asks
            .iter()
            .take(n)
            .map(|(_, q)| i128::from(instrument.lots(*q)))
            .sum();
        if b + a > 0 {
            values[6 + i] = Some((b - a) as f64 / (b + a) as f64);
        }
    }
    let weighted = 6 + config.imbalance_levels.len();
    let qb = i128::from(instrument.lots(qb));
    let qa = i128::from(instrument.lots(qa));
    if qb + qa > 0 {
        values[weighted] = Some(
            (i128::from(b) * qa + i128::from(a) * qb) as f64 / (qb + qa) as f64
                * instrument.tick_size,
        );
    }
    if let Some(model) = model {
        values[weighted + 1] = model
            .evaluate(bbo, instrument, model.config.test_start_ns)
            .get();
    }
    let bound = i128::from(config.depth_band_bps) * m2;
    let bid_truncated =
        m2 > 0 && deepest_bid.is_none_or(|b| (m2 - 2 * i128::from(b)) * 10_000 < bound);
    let ask_truncated =
        m2 > 0 && deepest_ask.is_none_or(|a| (2 * i128::from(a) - m2) * 10_000 < bound);
    Features {
        values,
        inputs: BookInputs { bbo, bids, asks },
        bid_truncated,
        ask_truncated,
    }
}

fn inputs_changed(
    index: usize,
    previous: &BookInputs,
    next: &BookInputs,
    config: &MarketConfig,
    instrument: &InstrumentMeta,
) -> bool {
    if index < 2
        || index >= 6 + config.imbalance_levels.len()
        || !previous.bbo.is_valid()
        || !next.bbo.is_valid()
    {
        return previous.bbo != next.bbo;
    }
    if index >= 6 {
        let n = config.imbalance_levels[index - 6];
        let same = |left: &[(i64, f64)], right: &[(i64, f64)]| {
            left.iter()
                .take(n)
                .map(|(p, q)| (*p, instrument.lots(*q)))
                .eq(right.iter().take(n).map(|(p, q)| (*p, instrument.lots(*q))))
        };
        return !same(&previous.bids, &next.bids) || !same(&previous.asks, &next.asks);
    }
    let before = mid2(previous.bbo).unwrap();
    let after = mid2(next.bbo).unwrap();
    if before != after {
        return true;
    }
    let (left, right) = if index < 4 {
        (&previous.bids, &next.bids)
    } else {
        (&previous.asks, &next.asks)
    };
    !left
        .iter()
        .filter(|(p, _)| in_band(*p, before, config.depth_band_bps))
        .map(|(p, q)| (*p, instrument.lots(*q)))
        .eq(right
            .iter()
            .filter(|(p, _)| in_band(*p, after, config.depth_band_bps))
            .map(|(p, q)| (*p, instrument.lots(*q))))
}
fn point(
    ts: i64,
    values: &[Option<f64>],
    labels: &[(String, &str)],
    model: Option<&StoikovModel>,
) -> MarketPoint {
    MarketPoint {
        ts_ns: ts,
        values: values
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (value, (name, unit)))| {
                sampled(*value, i, values.len(), model, ts).map_or_else(
                    || {
                        Value::unavailable(
                            if name == "stoikov_microprice" {
                                if model.is_none() {
                                    "Stoikov model not configured"
                                } else {
                                    "outside held-out evaluation period or calibrated state"
                                }
                            } else {
                                "invalid book, insufficient depth, or zero denominator"
                            },
                            unit,
                            1,
                        )
                    },
                    |v| Value::measured(v, unit, 1, 0),
                )
            })
            .collect(),
    }
}

fn sampled(
    value: Option<f64>,
    index: usize,
    len: usize,
    model: Option<&StoikovModel>,
    ts: i64,
) -> Option<f64> {
    if index + 1 == len
        && model.is_some_and(|m| ts < m.config.test_start_ns || ts > m.config.test_end_ns)
    {
        None
    } else {
        value
    }
}
fn hold(
    aggregates: &mut [Aggregate],
    values: &[Option<f64>],
    from: i64,
    to: i64,
    model: Option<&StoikovModel>,
) {
    if to <= from {
        return;
    }
    let last = aggregates.len() - 1;
    for (i, (a, value)) in aggregates.iter_mut().zip(values).enumerate() {
        if i == last
            && let Some(model) = model
        {
            let overlap = (i128::from(to.min(model.config.test_end_ns))
                - i128::from(from.max(model.config.test_start_ns)))
            .max(0)
            .min(i128::from(to) - i128::from(from)) as i64;
            a.hold(*value, overlap);
            a.hold(None, to - from - overlap);
            continue;
        }
        a.hold(*value, to - from);
    }
}

/// Computes M1–M10 with memory bounded by configuration, live book size and model state count.
/// No state is extrapolated beyond the last published book batch. Explicit windows must fit there.
pub fn compute_market(
    events: &[Event],
    instrument: &InstrumentMeta,
    origin_ns: i64,
    config: &MarketConfig,
) -> Result<MarketReport, MarketError> {
    validate_config(config, instrument)?;
    let (first, last) = validate_events(events, instrument, origin_ns)?;
    let start = config.start_ns.unwrap_or(first);
    let end = config.end_ns.unwrap_or(last);
    if start < first || end > last || start > end || end.checked_sub(start).is_none() {
        return Err(MarketError::Configuration(
            "window must be ordered and inside published book coverage".into(),
        ));
    }
    let model = config
        .stoikov
        .as_ref()
        .map(|c| calibrate_stoikov(events, instrument, origin_ns, c, config.max_book_levels))
        .transpose()?;
    let labels = labels(config);
    let len = labels.len();
    let mut aggregates: Vec<Aggregate> = (0..len).map(|_| Aggregate::default()).collect();
    let max = config.series_max_points.unwrap_or(0);
    let mut event_series = Retained::new(max, start, end);
    let mut clock_series = Retained::new(max, start, end);
    let mut intervals = Retained::new(max, start, end);
    let mut event_rv = Variance::default();
    let mut clock_rv = Variance::default();
    let mut previous_bbo = None;
    let mut current_values = vec![None; len];
    let mut previous_inputs = BookInputs::default();
    let mut held_mid = None;
    let mut holding_since = start;
    let mut grid = Some(start);
    let mut bid_truncated = false;
    let mut ask_truncated = false;
    let mut deepest_bid: Option<i64> = None;
    let mut deepest_ask: Option<i64> = None;
    let mut ofi = 0i128;
    let mut ofi_n = 0;
    let mut ofi_bad = 0;
    let mut interval_start = start;
    let mut interval_end = start.saturating_add(config.clock_step_ns).min(end);
    let mut interval_ofi = 0i128;
    let mut interval_n = 0;
    let mut interval_bad = 0;
    let mut initialized = false;
    let mut held_bid_truncated = false;
    let mut held_ask_truncated = false;
    replay(
        |ts, book, dirty| {
            // Seed arbitrary window boundaries with the completed state strictly before them.
            if ts > start && !initialized {
                for (i, (a, v)) in aggregates.iter_mut().zip(&current_values).enumerate() {
                    a.event.push(sampled(*v, i, len, model.as_ref(), start));
                }
                event_rv.push(held_mid, true);
                event_series.push(start, || {
                    point(start, &current_values, &labels, model.as_ref())
                });
                bid_truncated |= held_bid_truncated;
                ask_truncated |= held_ask_truncated;
                initialized = true;
            }
            if ts > end {
                return Ok(());
            }
            let bbo = book.bbo();
            // Grid points strictly before this batch use the preceding state. An exact timestamp
            // is handled below, after the complete batch. Invalid sub-grid intervals break RV too.
            while let Some(t) = grid.filter(|t| *t < ts && *t <= end) {
                for (i, (a, v)) in aggregates.iter_mut().zip(&current_values).enumerate() {
                    a.clock.push(sampled(*v, i, len, model.as_ref(), t));
                }
                clock_rv.push(held_mid, false);
                clock_series.push(t, || point(t, &current_values, &labels, model.as_ref()));
                grid = t
                    .checked_add(config.clock_step_ns)
                    .filter(|next| *next <= end);
            }
            if ts >= start {
                hold(
                    &mut aggregates,
                    &current_values,
                    holding_since,
                    ts,
                    model.as_ref(),
                );
                holding_since = ts;
            }
            if !dirty && initialized {
                if grid == Some(ts) {
                    for (i, (a, v)) in aggregates.iter_mut().zip(&current_values).enumerate() {
                        a.clock.push(sampled(*v, i, len, model.as_ref(), ts));
                    }
                    clock_rv.push(held_mid, false);
                    clock_series.push(ts, || point(ts, &current_values, &labels, model.as_ref()));
                    grid = ts
                        .checked_add(config.clock_step_ns)
                        .filter(|next| *next <= end);
                }
                return Ok(());
            }
            let next = features(
                book,
                instrument,
                config,
                model.as_ref(),
                &mut deepest_bid,
                &mut deepest_ask,
            );
            held_bid_truncated = next.bid_truncated;
            held_ask_truncated = next.ask_truncated;
            if ts >= start {
                bid_truncated |= next.bid_truncated;
                ask_truncated |= next.ask_truncated;
                let changed = previous_bbo != Some(bbo);
                let initial = !initialized;
                let mut any_input_changed = false;
                for (i, aggregate) in aggregates.iter_mut().enumerate() {
                    if initial
                        || inputs_changed(i, &previous_inputs, &next.inputs, config, instrument)
                    {
                        aggregate
                            .event
                            .push(sampled(next.values[i], i, len, model.as_ref(), ts));
                        any_input_changed = true;
                    }
                }
                if any_input_changed {
                    event_series.push(ts, || point(ts, &next.values, &labels, model.as_ref()));
                }
                if initial || changed {
                    event_rv.push(mid2(bbo), true);
                }
                if !bbo.is_valid() {
                    clock_rv.invalidate();
                }
                if !initial && changed {
                    // Intervals are (left,right], so a transition exactly on the boundary belongs
                    // to the closing interval, not the next one.
                    while ts > interval_end && interval_end < end {
                        intervals.push(interval_end, || FlowInterval {
                            start_ns: interval_start,
                            end_ns: interval_end,
                            ofi: Value::measured(
                                interval_ofi as f64 * instrument.lot_size,
                                "base",
                                interval_n,
                                interval_bad,
                            ),
                            flagged_invalid: interval_bad > 0,
                        });
                        interval_start = interval_end;
                        interval_end = interval_end.saturating_add(config.clock_step_ns).min(end);
                        interval_ofi = 0;
                        interval_n = 0;
                        interval_bad = 0;
                    }
                    if let Some(e) =
                        previous_bbo.and_then(|p| order_flow_increment(p, bbo, instrument))
                    {
                        ofi += e;
                        ofi_n += 1;
                        interval_ofi += e;
                        interval_n += 1;
                    } else {
                        ofi_bad += 1;
                        interval_bad += 1;
                    }
                }
                initialized = true;
            }
            current_values = next.values;
            previous_inputs = next.inputs;
            held_mid = mid2(bbo);
            previous_bbo = Some(bbo);
            if ts < start {
                holding_since = start;
            }
            if grid == Some(ts) {
                for (i, (a, v)) in aggregates.iter_mut().zip(&current_values).enumerate() {
                    a.clock.push(sampled(*v, i, len, model.as_ref(), ts));
                }
                clock_rv.push(held_mid, false);
                clock_series.push(ts, || point(ts, &current_values, &labels, model.as_ref()));
                grid = ts
                    .checked_add(config.clock_step_ns)
                    .filter(|next| *next <= end);
            }
            Ok(())
        },
        events,
        instrument,
        origin_ns,
        config.max_book_levels,
    )?;
    // Last book coverage can finish on a non-grid timestamp. Complete only grid points inside it.
    while let Some(t) = grid.filter(|t| *t <= end) {
        for (i, (a, v)) in aggregates.iter_mut().zip(&current_values).enumerate() {
            a.clock.push(sampled(*v, i, len, model.as_ref(), t));
        }
        clock_rv.push(held_mid, false);
        clock_series.push(t, || point(t, &current_values, &labels, model.as_ref()));
        grid = t
            .checked_add(config.clock_step_ns)
            .filter(|next| *next <= end);
    }
    hold(
        &mut aggregates,
        &current_values,
        holding_since,
        end,
        model.as_ref(),
    );
    while interval_start < end {
        intervals.push(interval_end, || FlowInterval {
            start_ns: interval_start,
            end_ns: interval_end,
            ofi: Value::measured(
                interval_ofi as f64 * instrument.lot_size,
                "base",
                interval_n,
                interval_bad,
            ),
            flagged_invalid: interval_bad > 0,
        });
        interval_start = interval_end;
        interval_end = interval_end.saturating_add(config.clock_step_ns).min(end);
        interval_ofi = 0;
        interval_n = 0;
        interval_bad = 0;
    }
    let mut liquidity = Vec::with_capacity(config.liquidity_windows.len());
    for window in &config.liquidity_windows {
        liquidity.push(liquidity_window(
            events,
            instrument,
            origin_ns,
            config.max_book_levels,
            window,
            first,
            last,
        )?);
    }
    let (trade_imbalance, trade_intervals) = trade_imbalance(
        events,
        instrument,
        origin_ns,
        start,
        end,
        config.clock_step_ns,
        max,
    );
    Ok(MarketReport {
        analytics_version: crate::ANALYTICS_VERSION.into(),
        clock: "exchange".into(),
        batching: "completed equal-exchange-timestamp batches".into(),
        start_ns: start,
        end_ns: end,
        clock_step_ns: config.clock_step_ns,
        metrics: aggregates
            .iter()
            .zip(&labels)
            .map(|(a, (name, unit))| a.report(name, unit))
            .collect(),
        event_series: config.series_max_points.map(|max_points| MarketSeries {
            sampling: "event".into(),
            clock_step_ns: None,
            reduction: "last observation per fixed time bin".into(),
            max_points,
            points: event_series.points,
        }),
        clock_series: config.series_max_points.map(|max_points| MarketSeries {
            sampling: "clock_previous_tick".into(),
            clock_step_ns: Some(config.clock_step_ns),
            reduction: "last observation per fixed time bin".into(),
            max_points,
            points: clock_series.points,
        }),
        bid_depth_truncated: bid_truncated,
        ask_depth_truncated: ask_truncated,
        event_variance: event_rv.report(end - start, instrument.tick_size),
        clock_variance: clock_rv.report(end - start, instrument.tick_size),
        order_flow_imbalance: if ofi_n > 0 {
            Value::measured(ofi as f64 * instrument.lot_size, "base", ofi_n, ofi_bad)
        } else {
            Value::unavailable("no valid completed-batch transitions", "base", ofi_bad)
        },
        invalid_ofi_transitions: ofi_bad,
        ofi_intervals: intervals.points,
        trade_imbalance,
        trade_intervals,
        stoikov: model,
        liquidity,
    })
}

fn validate_config(c: &MarketConfig, i: &InstrumentMeta) -> Result<(), MarketError> {
    if ![i.tick_size, i.lot_size, i.contract_size]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0)
    {
        return Err(MarketError::Configuration(
            "instrument scales must be finite and positive".into(),
        ));
    }
    if c.clock_step_ns <= 0
        || c.max_book_levels == 0
        || c.imbalance_levels.contains(&0)
        || c.imbalance_levels.len() > 256
        || c.liquidity_windows.len() > 256
        || c.series_max_points.is_some_and(|n| n == 0 || n > 100_000)
    {
        return Err(MarketError::Configuration(
            "invalid sampling, depth, series, or window limit".into(),
        ));
    }
    Ok(())
}
fn validate_events(
    events: &[Event],
    instrument: &InstrumentMeta,
    origin: i64,
) -> Result<(i64, i64), MarketError> {
    let mut previous = None;
    let mut first = None;
    let mut last = None;
    for e in events.iter().filter(|e| exchange(e)) {
        if previous.is_some_and(|ts| ts > e.exch_ts) {
            return Err(MarketError::Data(
                "exchange rows must be time ordered".into(),
            ));
        }
        previous = Some(e.exch_ts);
        let ts = e
            .exch_ts
            .checked_sub(origin)
            .ok_or_else(|| MarketError::Data("relative timestamp overflow".into()))?;
        if matches!(kind(e), DEPTH | SNAPSHOT | TRADE)
            && (!e.px.is_finite()
                || !e.qty.is_finite()
                || e.qty < 0.0
                || (e.px / instrument.tick_size).abs() >= i64::MAX as f64
                || e.qty / instrument.lot_size >= i64::MAX as f64)
        {
            return Err(MarketError::Data(
                "non-finite, negative quantity, or unrepresentable tick/lot".into(),
            ));
        }
        if matches!(kind(e), DEPTH | SNAPSHOT | CLEAR) {
            first.get_or_insert(ts);
            last = Some(ts);
        }
    }
    let coverage = first
        .zip(last)
        .ok_or_else(|| MarketError::Data("no exchange book states".into()))?;
    if coverage.1.checked_sub(coverage.0).is_none() {
        return Err(MarketError::Data("book coverage duration overflow".into()));
    }
    Ok(coverage)
}
fn trade_imbalance(
    events: &[Event],
    instrument: &InstrumentMeta,
    origin: i64,
    start: i64,
    end: i64,
    step: i64,
    max: usize,
) -> (TradeImbalance, Vec<TradeFlowInterval>) {
    let mut volume = [0i128; 3];
    let mut counts = [0u64; 3];
    let mut interval_volume = [0i128; 3];
    let mut interval_counts = [0u64; 3];
    let mut retained = Retained::new(max, start, end);
    let mut left = start;
    let mut right = start.saturating_add(step).min(end);
    for e in events.iter().filter(|e| exchange(e) && kind(e) == TRADE) {
        let ts = e.exch_ts - origin;
        if ts < start {
            continue;
        }
        if ts > end {
            break;
        }
        while ts > right && right < end {
            retained.push(right, || TradeFlowInterval {
                start_ns: left,
                end_ns: right,
                trade_imbalance: trade_summary(interval_volume, interval_counts, instrument),
            });
            left = right;
            right = right.saturating_add(step).min(end);
            interval_volume = [0; 3];
            interval_counts = [0; 3];
        }
        let bucket = match side(e) {
            Some(Side::Buy) => 0,
            Some(Side::Sell) => 1,
            None => 2,
        };
        let lots = i128::from(instrument.lots(e.qty));
        volume[bucket] += lots;
        counts[bucket] += 1;
        interval_volume[bucket] += lots;
        interval_counts[bucket] += 1;
    }
    while left < end {
        retained.push(right, || TradeFlowInterval {
            start_ns: left,
            end_ns: right,
            trade_imbalance: trade_summary(interval_volume, interval_counts, instrument),
        });
        left = right;
        right = right.saturating_add(step).min(end);
        interval_volume = [0; 3];
        interval_counts = [0; 3];
    }
    (trade_summary(volume, counts, instrument), retained.points)
}
fn trade_summary(
    volume: [i128; 3],
    counts: [u64; 3],
    instrument: &InstrumentMeta,
) -> TradeImbalance {
    let known = volume[0] + volume[1];
    let total = known + volume[2];
    TradeImbalance {
        buy_volume: Value::measured(volume[0] as f64 * instrument.lot_size, "base", counts[0], 0),
        sell_volume: Value::measured(volume[1] as f64 * instrument.lot_size, "base", counts[1], 0),
        unknown_volume: Value::measured(
            volume[2] as f64 * instrument.lot_size,
            "base",
            counts[2],
            0,
        ),
        imbalance: if known > 0 {
            Value::measured(
                (volume[0] - volume[1]) as f64 / known as f64,
                "ratio",
                counts[0] + counts[1],
                counts[2],
            )
        } else {
            Value::unavailable("no recorded aggressor volume", "ratio", counts[2])
        },
        coverage: if total > 0 {
            Value::measured(known as f64 / total as f64, "ratio", counts.iter().sum(), 0)
        } else {
            Value::unavailable("no trade volume", "ratio", 0)
        },
    }
}

/// Sampling used to estimate the discrete Stoikov chain; distinct from report display sampling.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CalibrationSampling {
    Event,
    Clock { step_ns: i64 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoikovConfig {
    pub train_start_ns: i64,
    pub train_end_ns: i64,
    /// Must be strictly after train_end_ns; all evaluation is held out.
    pub test_start_ns: i64,
    pub test_end_ns: i64,
    pub imbalance_buckets: usize,
    pub spread_ticks: Vec<i64>,
    pub min_transitions_per_state: u64,
    pub max_iterations: usize,
    pub sampling: CalibrationSampling,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoikovModel {
    pub config: StoikovConfig,
    pub tick_size: f64,
    pub symmetrised: bool,
    pub state_transition_counts: Vec<u64>,
    pub q: Vec<Vec<f64>>,
    pub t: Vec<Vec<f64>>,
    /// R K in ticks; a full R table is unnecessary for the linear solve.
    pub expected_move_ticks: Vec<f64>,
    pub first_adjustment_ticks: Vec<f64>,
    pub move_transition: Vec<Vec<f64>>,
    pub adjustment_ticks: Vec<f64>,
    pub iterations: usize,
    pub residual_infinity_ticks: f64,
}
impl StoikovModel {
    pub fn evaluate(&self, bbo: Bbo, instrument: &InstrumentMeta, ts_ns: i64) -> Value {
        if instrument.tick_size != self.tick_size {
            return Value::unavailable("instrument tick size differs from calibration", "price", 1);
        }
        if ts_ns < self.config.test_start_ns || ts_ns > self.config.test_end_ns {
            return Value::unavailable("outside held-out evaluation period", "price", 1);
        }
        let Some(state) = state_index(bbo, instrument, &self.config) else {
            return Value::unavailable(
                "invalid book or outside calibrated imbalance/spread states",
                "price",
                1,
            );
        };
        if self.state_transition_counts[state] < self.config.min_transitions_per_state {
            return Value::unavailable("insufficient transitions in calibrated state", "price", 1);
        }
        Value::measured(
            mid(bbo, instrument).unwrap() + self.adjustment_ticks[state] * self.tick_size,
            "price",
            1,
            0,
        )
    }
}
fn state_index(bbo: Bbo, instrument: &InstrumentMeta, c: &StoikovConfig) -> Option<usize> {
    let s = i64::try_from(spread(bbo)?).ok()?;
    let spread_index = c.spread_ticks.iter().position(|x| *x == s)?;
    let qb = i128::from(instrument.lots(bbo.bid?.1));
    let qa = i128::from(instrument.lots(bbo.ask?.1));
    if qb + qa <= 0 {
        return None;
    }
    // Buckets are (left,right], with I=0 included in bucket zero. In particular I=.5 is L.
    let bucket = ((qb * c.imbalance_buckets as i128 + qb + qa - 1) / (qb + qa) - 1)
        .max(0)
        .min(c.imbalance_buckets as i128 - 1) as usize;
    Some(spread_index * c.imbalance_buckets + bucket)
}
fn mirror(index: usize, n: usize) -> usize {
    index / n * n + n - 1 - index % n
}

/// Fits only the explicitly bounded training interval, pairing every transition with its mirror.
pub fn calibrate_stoikov(
    events: &[Event],
    instrument: &InstrumentMeta,
    origin: i64,
    config: &StoikovConfig,
    max_book_levels: usize,
) -> Result<StoikovModel, MarketError> {
    let states = config
        .imbalance_buckets
        .saturating_mul(config.spread_ticks.len());
    if config.imbalance_buckets < 2
        || !config.imbalance_buckets.is_multiple_of(2)
        || states > MAX_STATES
        || states == 0
        || config.train_start_ns >= config.train_end_ns
        || config.test_start_ns <= config.train_end_ns
        || config.test_start_ns > config.test_end_ns
        || config.max_iterations == 0
        || config.max_iterations > 100_000
        || config.min_transitions_per_state == 0
        || config.spread_ticks.iter().any(|s| *s <= 0)
        || config
            .spread_ticks
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != config.spread_ticks.len()
        || matches!(config.sampling, CalibrationSampling::Clock { step_ns } if step_ns <= 0)
    {
        return Err(MarketError::Configuration(
            "invalid Stoikov states, sampling, train/test separation, or iteration limit".into(),
        ));
    }
    validate_config(
        &MarketConfig {
            max_book_levels,
            ..MarketConfig::default()
        },
        instrument,
    )?;
    let (first, last) = validate_events(events, instrument, origin)?;
    if config.train_start_ns < first || config.test_end_ns > last {
        return Err(MarketError::Configuration(
            "Stoikov train/test must fit published book coverage".into(),
        ));
    }
    let mut q = vec![vec![0.0; states]; states];
    let mut t = q.clone();
    let mut r: Vec<Sum> = vec![Sum::default(); states];
    let mut counts = vec![0u64; states];
    let mut previous: Option<(usize, i128)> = None;
    let mut observe = |bbo: Bbo| {
        let current = state_index(bbo, instrument, config).zip(mid2(bbo));
        if let (Some((x, old)), Some((y, now))) = (previous, current) {
            let change = (now - old) as f64 * 0.5;
            for (a, b, change) in [
                (x, y, change),
                (
                    mirror(x, config.imbalance_buckets),
                    mirror(y, config.imbalance_buckets),
                    -change,
                ),
            ] {
                counts[a] += 1;
                if change == 0.0 {
                    q[a][b] += 1.0;
                } else {
                    t[a][b] += 1.0;
                    r[a].add(change);
                }
            }
        }
        previous = current;
    };
    let mut grid = Some(config.train_start_ns);
    let mut held = Bbo::default();
    let mut last_observed = None;
    replay(
        |ts, book, _dirty| {
            if ts > config.train_end_ns {
                return Ok(());
            }
            match config.sampling {
                CalibrationSampling::Event => {
                    if ts > config.train_start_ns && last_observed.is_none() {
                        observe(held);
                        last_observed = Some(held);
                    }
                    if ts >= config.train_start_ns && last_observed != Some(book.bbo()) {
                        observe(book.bbo());
                        last_observed = Some(book.bbo());
                    }
                }
                CalibrationSampling::Clock { step_ns } => {
                    while let Some(tick) =
                        grid.filter(|tick| *tick < ts && *tick <= config.train_end_ns)
                    {
                        observe(held);
                        grid = tick
                            .checked_add(step_ns)
                            .filter(|next| *next <= config.train_end_ns);
                    }
                    if grid == Some(ts) {
                        observe(book.bbo());
                        grid = ts
                            .checked_add(step_ns)
                            .filter(|next| *next <= config.train_end_ns);
                    }
                    // An invalid interval must not be bridged even if it misses all clock samples.
                    if ts >= config.train_start_ns && !book.bbo().is_valid() {
                        observe(Bbo::default());
                    }
                }
            }
            held = book.bbo();
            Ok(())
        },
        events,
        instrument,
        origin,
        max_book_levels,
    )?;
    if let CalibrationSampling::Clock { step_ns } = config.sampling {
        while let Some(tick) = grid.filter(|tick| *tick <= config.train_end_ns) {
            observe(held);
            grid = tick
                .checked_add(step_ns)
                .filter(|next| *next <= config.train_end_ns);
        }
    }
    // Drop the closure's mutable borrows before normalising and solving.
    drop(observe);
    let mut rk = vec![0.0; states];
    for x in 0..states {
        if counts[x] < config.min_transitions_per_state {
            return Err(MarketError::Calibration(format!(
                "state {x} has {} transitions, needs {}",
                counts[x], config.min_transitions_per_state
            )));
        }
        for y in 0..states {
            q[x][y] /= counts[x] as f64;
            t[x][y] /= counts[x] as f64;
        }
        rk[x] = r[x].get() / counts[x] as f64;
    }
    solve_stoikov(config.clone(), instrument.tick_size, counts, q, t, rk)
}

// Partial-pivot LU; solve all right-hand sides with the same factorisation, never an inverse.
fn solve_stoikov(
    config: StoikovConfig,
    tick_size: f64,
    counts: Vec<u64>,
    q: Vec<Vec<f64>>,
    t: Vec<Vec<f64>>,
    rk: Vec<f64>,
) -> Result<StoikovModel, MarketError> {
    let n = q.len();
    let mut lu = q.clone();
    for i in 0..n {
        for j in 0..n {
            lu[i][j] = (if i == j { 1.0 } else { 0.0 }) - q[i][j];
        }
    }
    let mut permutation: Vec<usize> = (0..n).collect();
    for k in 0..n {
        let pivot = (k..n)
            .max_by(|&a, &b| lu[a][k].abs().total_cmp(&lu[b][k].abs()))
            .unwrap();
        if lu[pivot][k].abs() < 1e-14 {
            return Err(MarketError::Calibration(
                "singular I-Q (no eventual mid move)".into(),
            ));
        }
        lu.swap(k, pivot);
        permutation.swap(k, pivot);
        for i in k + 1..n {
            let (earlier, remaining) = lu.split_at_mut(i);
            let pivot_row = &earlier[k];
            let row = &mut remaining[0];
            row[k] /= pivot_row[k];
            let factor = row[k];
            for (value, pivot_value) in row[k + 1..].iter_mut().zip(&pivot_row[k + 1..]) {
                *value -= factor * pivot_value;
            }
        }
    }
    let solve = |rhs: &[f64]| -> Vec<f64> {
        let mut x: Vec<f64> = permutation.iter().map(|&i| rhs[i]).collect();
        for i in 0..n {
            for j in 0..i {
                let contribution = lu[i][j] * x[j];
                x[i] -= contribution;
            }
        }
        for i in (0..n).rev() {
            for j in i + 1..n {
                let contribution = lu[i][j] * x[j];
                x[i] -= contribution;
            }
            x[i] /= lu[i][i];
        }
        x
    };
    let g = solve(&rk);
    let mut b = vec![vec![0.0; n]; n];
    for j in 0..n {
        let rhs: Vec<f64> = t.iter().map(|row| row[j]).collect();
        let x = solve(&rhs);
        for i in 0..n {
            b[i][j] = x[i];
        }
    }
    let mut term = g.clone();
    let mut adjustment = vec![Sum::default(); n];
    let mut next = vec![0.0; n];
    for iteration in 0..config.max_iterations {
        let residual = term.iter().fold(0.0_f64, |v, x| v.max(x.abs()));
        if !residual.is_finite() {
            return Err(MarketError::Calibration("non-finite Stoikov series".into()));
        }
        if residual < 1e-12 {
            return Ok(StoikovModel {
                config,
                tick_size,
                symmetrised: true,
                state_transition_counts: counts,
                q,
                t,
                expected_move_ticks: rk,
                first_adjustment_ticks: g,
                move_transition: b,
                adjustment_ticks: adjustment.into_iter().map(Sum::get).collect(),
                iterations: iteration,
                residual_infinity_ticks: residual,
            });
        }
        for i in 0..n {
            adjustment[i].add(term[i]);
            let mut sum = Sum::default();
            for j in 0..n {
                sum.add(b[i][j] * term[j]);
            }
            next[i] = sum.get();
        }
        std::mem::swap(&mut term, &mut next);
    }
    Err(MarketError::Calibration(
        "Stoikov series did not converge within max_iterations".into(),
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiquidityBand {
    BasisPoints { bps: u32 },
    FirstLevels { levels: usize },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityWindow {
    pub start_ns: i64,
    pub end_ns: i64,
    pub side: Side,
    pub band: LiquidityBand,
    /// Absolute exchange timestamp distance allowed for price-matched trade attribution.
    pub trade_match_lag_ns: i64,
    /// Bounds attribution bookkeeping even under an arbitrarily dense trade stream.
    pub max_matched_trades: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recovery {
    /// Recovery time if observed, otherwise the right-censoring duration, both in ns.
    pub elapsed: Value,
    pub censored: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiquidityReport {
    pub window: LiquidityWindow,
    pub usable: bool,
    pub unavailable_reason: Option<String>,
    pub band_truncated: bool,
    pub pre_event_mid: Value,
    pub initial_depth: Value,
    pub final_depth: Value,
    pub trade_depletion: Value,
    pub cancel_depletion: Value,
    pub replenishment: Value,
    pub net_flow: Value,
    pub spread_recovery: Recovery,
    pub depth_recovery: Recovery,
}
fn side_levels(book: &L2Book, side: Side) -> Vec<(i64, f64)> {
    match side {
        Side::Buy => book.top_bids(book.bid_levels()),
        Side::Sell => book.top_asks(book.ask_levels()),
    }
}
fn fixed_band_contains(
    window: &LiquidityWindow,
    anchor: Option<i128>,
    boundary: Option<i64>,
    tick: i64,
) -> bool {
    let Some(mid) = anchor else {
        return false;
    };
    match window.band {
        LiquidityBand::BasisPoints { bps } => in_band(tick, mid, bps),
        LiquidityBand::FirstLevels { .. } => boundary.is_some_and(|edge| match window.side {
            Side::Buy => tick >= edge && 2 * i128::from(tick) <= mid,
            Side::Sell => tick <= edge && 2 * i128::from(tick) >= mid,
        }),
    }
}
fn liquidity_window(
    events: &[Event],
    instrument: &InstrumentMeta,
    origin: i64,
    max_levels: usize,
    window: &LiquidityWindow,
    first: i64,
    last: i64,
) -> Result<LiquidityReport, MarketError> {
    if window.start_ns <= first
        || window.start_ns > window.end_ns
        || window.end_ns > last
        || window.trade_match_lag_ns < 0
        || window.max_matched_trades == 0
        || matches!(window.band, LiquidityBand::FirstLevels { levels } if levels == 0 || levels > max_levels)
    {
        return Err(MarketError::Configuration("liquidity window requires a pre-event book, covered end, positive band and matching limit".into()));
    }
    let mut baseline = Bbo::default();
    let mut pre_levels = Vec::new();
    let mut previous = BTreeMap::<i64, i64>::new();
    let mut boundary = None;
    let mut anchor = None;
    let mut initial = 0i128;
    let mut final_depth = 0i128;
    let mut started = false;
    let mut usable = true;
    let mut reason = None;
    let mut truncated = false;
    let mut cancels = 0i128;
    let mut replenish = 0i128;
    let mut decreases = 0u64;
    let mut increases = 0u64;
    let mut matched: BTreeMap<usize, (i64, i64)> = BTreeMap::new();
    let mut trade_cursor = 0;
    let mut spread_start = None;
    let mut spread_recovered = None;
    let mut depth_start = None;
    let mut depth_recovered = None;
    let aggressor = match window.side {
        Side::Buy => Side::Sell,
        Side::Sell => Side::Buy,
    };
    let mut trades = 0i128;
    let mut trade_n = 0;
    replay(
        |ts, book, _dirty| {
            if ts < window.start_ns {
                baseline = book.bbo();
                pre_levels = side_levels(book, window.side);
                return Ok(());
            }
            if !started {
                anchor = mid2(baseline);
                if !baseline.is_valid() || anchor.is_none_or(|m| m <= 0) {
                    usable = false;
                    reason = Some("invalid pre-event book or non-positive mid".into());
                }
                if let LiquidityBand::FirstLevels { levels } = window.band {
                    boundary = pre_levels.iter().take(levels).next_back().map(|(p, _)| *p);
                    truncated = pre_levels.len() < levels;
                } else if let (LiquidityBand::BasisPoints { bps }, Some(m)) = (&window.band, anchor)
                {
                    truncated = pre_levels.last().is_none_or(|(p, _)| {
                        (2 * i128::from(*p) - m).abs() * 10_000 < i128::from(*bps) * m
                    });
                }
                for &(p, q) in &pre_levels {
                    let included = fixed_band_contains(window, anchor, boundary, p);
                    if included {
                        previous.insert(p, instrument.lots(q));
                        initial += i128::from(instrument.lots(q));
                    }
                }
                started = true;
                final_depth = initial;
            }
            if ts > window.end_ns {
                return Ok(());
            }
            if !book.bbo().is_valid() {
                usable = false;
                reason = Some("invalid book inside liquidity window".into());
            }
            let mut now = BTreeMap::new();
            for (p, q) in side_levels(book, window.side) {
                let included = fixed_band_contains(window, anchor, boundary, p);
                if included {
                    now.insert(p, instrument.lots(q));
                }
            }
            let oldest = ts.saturating_sub(window.trade_match_lag_ns);
            matched.retain(|_, (trade_ts, _)| *trade_ts >= oldest);
            while let Some(e) = events.get(trade_cursor) {
                if exchange(e) && e.exch_ts - origin >= oldest {
                    break;
                }
                trade_cursor += 1;
            }
            let latest = ts.saturating_add(window.trade_match_lag_ns);
            let mut touch_consumed = false;
            for (&p, &old) in &previous {
                let new = now.get(&p).copied().unwrap_or(0);
                if new >= old {
                    continue;
                }
                decreases += 1;
                let mut remaining = old - new;
                for (index, e) in events.iter().enumerate().skip(trade_cursor) {
                    if remaining == 0 {
                        break;
                    }
                    if exchange(e) && e.exch_ts - origin > latest {
                        break;
                    }
                    if !exchange(e)
                        || kind(e) != TRADE
                        || side(e) != Some(aggressor)
                        || instrument.tick(e.px) != p
                    {
                        continue;
                    }
                    let trade_ts = e.exch_ts - origin;
                    if trade_ts < window.start_ns
                        || trade_ts > window.end_ns
                        || (i128::from(trade_ts) - i128::from(ts)).abs()
                            > i128::from(window.trade_match_lag_ns)
                    {
                        continue;
                    }
                    let used = matched.get(&index).map_or(0, |(_, used)| *used);
                    let take = (instrument.lots(e.qty) - used).max(0).min(remaining);
                    if take == 0 {
                        continue;
                    }
                    if !matched.contains_key(&index) && matched.len() == window.max_matched_trades {
                        return Err(MarketError::ResourceLimit(
                            "liquidity max_matched_trades".into(),
                        ));
                    }
                    matched.insert(index, (trade_ts, used + take));
                    remaining -= take;
                }
                let touch = match window.side {
                    Side::Buy => baseline.bid,
                    Side::Sell => baseline.ask,
                };
                if new == 0 && remaining == 0 && touch.is_some_and(|(tick, _)| tick == p) {
                    touch_consumed = true;
                }
                cancels += i128::from(remaining);
            }
            for (&p, &new) in &now {
                let old = previous.get(&p).copied().unwrap_or(0);
                if new > old {
                    replenish += i128::from(new - old);
                    increases += 1;
                }
            }
            final_depth = now.values().map(|q| i128::from(*q)).sum();
            if depth_start.is_none() && final_depth < initial {
                depth_start = Some(ts);
            }
            if let Some(start) = depth_start
                && ts > start
                && depth_recovered.is_none()
                && final_depth >= initial
            {
                depth_recovered = Some(ts - start);
            }
            if spread_start.is_none()
                && touch_consumed
                && spread(book.bbo())
                    .zip(spread(baseline))
                    .is_some_and(|(s, pre)| s > pre)
            {
                spread_start = Some(ts);
            }
            if let Some(start) = spread_start
                && ts > start
                && spread_recovered.is_none()
                && spread(book.bbo())
                    .zip(spread(baseline))
                    .is_some_and(|(s, pre)| s <= pre)
            {
                spread_recovered = Some(ts - start);
            }
            previous = now;
            Ok(())
        },
        events,
        instrument,
        origin,
        max_levels,
    )?;
    if !started {
        return Err(MarketError::Configuration(
            "liquidity window contains no completed batch".into(),
        ));
    }
    for e in events
        .iter()
        .filter(|e| exchange(e) && kind(e) == TRADE && side(e) == Some(aggressor))
    {
        let ts = e.exch_ts - origin;
        if ts < window.start_ns || ts > window.end_ns {
            continue;
        }
        let p = instrument.tick(e.px);
        let included = fixed_band_contains(window, anchor, boundary, p);
        if included {
            trades += i128::from(instrument.lots(e.qty));
            trade_n += 1;
        }
    }
    let value = |x: f64, unit: &str, n| {
        if usable {
            Value::measured(x, unit, n, 0)
        } else {
            Value::unavailable(
                reason.as_deref().unwrap_or("unusable liquidity event"),
                unit,
                1,
            )
        }
    };
    let recovery = |start: Option<i64>, observed: Option<i64>| {
        if !usable {
            return Recovery {
                elapsed: value(0.0, "ns", 0),
                censored: false,
            };
        }
        match start {
            Some(start) => Recovery {
                elapsed: Value::measured(
                    observed.unwrap_or(window.end_ns - start) as f64,
                    "ns",
                    1,
                    0,
                ),
                censored: observed.is_none(),
            },
            None => Recovery {
                elapsed: Value::unavailable("no qualifying depletion/widening event", "ns", 0),
                censored: false,
            },
        }
    };
    Ok(LiquidityReport {
        window: window.clone(),
        usable,
        unavailable_reason: reason.clone(),
        band_truncated: truncated,
        pre_event_mid: anchor.map_or_else(
            || Value::unavailable("invalid pre-event mid", "price", 1),
            |m| value(m as f64 * 0.5 * instrument.tick_size, "price", 1),
        ),
        initial_depth: value(initial as f64 * instrument.lot_size, "base", 1),
        final_depth: value(final_depth as f64 * instrument.lot_size, "base", 1),
        trade_depletion: value(trades as f64 * instrument.lot_size, "base", trade_n),
        cancel_depletion: value(cancels as f64 * instrument.lot_size, "base", decreases),
        replenishment: value(replenish as f64 * instrument.lot_size, "base", increases),
        net_flow: value(
            (replenish - trades - cancels) as f64 * instrument.lot_size,
            "base",
            trade_n + decreases + increases,
        ),
        spread_recovery: recovery(spread_start, spread_recovered),
        depth_recovery: recovery(depth_start, depth_recovered),
    })
}

#[cfg(test)]
#[path = "market_tests.rs"]
mod tests;
