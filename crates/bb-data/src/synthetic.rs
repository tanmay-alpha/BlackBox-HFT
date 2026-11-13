//! Deterministic synthetic Level-2 market.
//!
//! The generator keeps an integer book (ticks and lots) and emits the feed a venue would publish
//! for it: an initial snapshot, then market trades, cancellations and limit additions. All
//! randomness comes from named [`bb_core::rng::Rng`] streams of the configured seed and all
//! state is integer, so a configuration always produces the same bytes on every platform.
//!
//! This is not market data and has no predictive content. It exists so the platform can be
//! exercised without vendor files; the manifest says so (`DatasetKind::Synthetic`).
//!
//! Guarantees, at every row on the exchange clock: the book is neither crossed nor locked, each
//! side has at least [`MIN_LEVELS_PER_SIDE`] levels, every quantity is a whole number of lots and
//! every price is a positive whole number of ticks. A trade row is always followed by the depth
//! row that reduces the level it hit, at the same exchange timestamp.

use std::collections::BTreeMap;

use bb_core::{
    CONTRACT_VERSION,
    dataset::{
        DataLevel, DatasetKind, DatasetManifest, InputFile, InstrumentMeta, LocalTsProvenance,
        SourceInfo, TimestampInfo,
    },
    hash::sha256_hex,
    rng::Rng,
    time::{TimeUnit, is_plausible_epoch_ns},
};
use hftbacktest::types::{
    BUY_EVENT, DEPTH_CLEAR_EVENT, DEPTH_EVENT, DEPTH_SNAPSHOT_EVENT, SELL_EVENT, TRADE_EVENT,
};
use serde::{Deserialize, Serialize};

use crate::{
    BuiltDataset, DataError, Event, is_valid_id, make_event,
    normalize::{NormalizeOptions, normalize},
    quality::{self, DEFAULT_GAP_WARN_NS, Finding, ReportInput, RowPrecision, SourceScan},
};

/// Levels each side is guaranteed to hold at every row.
pub const MIN_LEVELS_PER_SIDE: usize = 5;

/// A side that drops below this many levels gets a new far level in the same batch. One above
/// the guarantee, so the guarantee also holds on the row that removes a level.
const REPLENISH_BELOW: usize = MIN_LEVELS_PER_SIDE + 1;

/// The best bid never moves below this tick, which keeps every price positive and leaves room
/// for the guaranteed levels beneath it.
const MIN_BID_TICK: i64 = 16;

/// Bound of the order-flow pressure walk.
const PRESSURE_LIMIT: i64 = 40;

/// Levels from the touch a cancellation may pick.
const NEAR_LEVELS: usize = 10;

/// Chance (percent) that the other side re-quotes inside the spread after a level is consumed.
const FOLLOW_PCT: u64 = 60;

/// Chance (percent) that a limit addition improves the best price when the spread allows it.
const IMPROVE_PCT: u64 = 30;

/// Above this spread every limit addition improves the best price.
const WIDE_SPREAD_TICKS: i64 = 3;

/// Refuses configurations that would generate more events than this.
const MAX_EVENTS: i64 = 20_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyntheticConfig {
    /// Dataset id (`[a-z0-9_-]+`).
    pub id: String,
    pub title: String,
    pub seed: u64,
    /// Exchange timestamp of the initial snapshot, absolute epoch nanoseconds.
    pub origin_ns: i64,
    /// Exchange-clock length of the generated session.
    pub duration_ns: i64,
    pub tick_size: f64,
    pub lot_size: f64,
    pub contract_size: f64,
    /// The initial book is centred here: best bid one tick below, best ask one tick above.
    pub start_mid_tick: i64,
    /// Levels per side in the initial snapshot, and how far from the touch limit orders land.
    pub levels: u32,
    /// Typical size of a level in lots.
    pub base_level_lots: i64,
    /// Mean exchange-time gap between events; gaps are uniform in `[1, 2 * mean]`.
    pub mean_event_gap_ns: i64,
    /// Probability that an event is a market trade.
    pub trade_prob: f64,
    /// Probability that an event is a cancellation. The remainder are limit additions.
    pub cancel_prob: f64,
    /// Constant part of the feed latency.
    pub feed_latency_ns: i64,
    /// Feed latency jitter, uniform in `[0, feed_jitter_ns]`.
    pub feed_jitter_ns: i64,
}

impl Default for SyntheticConfig {
    fn default() -> Self {
        Self {
            id: "synthetic-demo".to_string(),
            title: "Synthetic demo market".to_string(),
            seed: 7,
            origin_ns: 1_700_000_000_000_000_000,
            duration_ns: 120_000_000_000,
            tick_size: 0.1,
            lot_size: 0.001,
            contract_size: 1.0,
            start_mid_tick: 300_000,
            levels: 20,
            base_level_lots: 2_000,
            mean_event_gap_ns: 2_500_000,
            trade_prob: 0.18,
            cancel_prob: 0.38,
            feed_latency_ns: 2_000_000,
            feed_jitter_ns: 1_500_000,
        }
    }
}

fn validate(cfg: &SyntheticConfig) -> Result<(), DataError> {
    let mut problems: Vec<String> = Vec::new();
    if !is_valid_id(&cfg.id) {
        problems.push(format!("id `{}` must match [a-z0-9_-]+", cfg.id));
    }
    for (name, value) in [
        ("tick_size", cfg.tick_size),
        ("lot_size", cfg.lot_size),
        ("contract_size", cfg.contract_size),
    ] {
        if !value.is_finite() || value <= 0.0 {
            problems.push(format!("{name} must be a positive finite number"));
        }
    }
    if cfg.duration_ns < 1 {
        problems.push("duration_ns must be at least 1".to_string());
    }
    if !(1..=3_600_000_000_000).contains(&cfg.mean_event_gap_ns) {
        problems.push("mean_event_gap_ns must be between 1 ns and 1 hour".to_string());
    } else if cfg.duration_ns / cfg.mean_event_gap_ns > MAX_EVENTS {
        problems.push(format!(
            "duration_ns / mean_event_gap_ns must not exceed {MAX_EVENTS} events"
        ));
    }
    if !(REPLENISH_BELOW as u32..=1_000).contains(&cfg.levels) {
        problems.push(format!("levels must be between {REPLENISH_BELOW} and 1000"));
    }
    if !(1..=1_000_000_000).contains(&cfg.base_level_lots) {
        problems.push("base_level_lots must be between 1 and 1e9".to_string());
    }
    let lowest_bid = cfg.start_mid_tick.saturating_sub(cfg.levels as i64);
    if lowest_bid < MIN_BID_TICK || cfg.start_mid_tick > 1_000_000_000_000 {
        problems.push(format!(
            "start_mid_tick must be at least levels + {MIN_BID_TICK} and at most 1e12"
        ));
    }
    for (name, p) in [
        ("trade_prob", cfg.trade_prob),
        ("cancel_prob", cfg.cancel_prob),
    ] {
        if !(0.0..=1.0).contains(&p) {
            problems.push(format!("{name} must be between 0 and 1"));
        }
    }
    if cfg.trade_prob + cfg.cancel_prob > 1.0 {
        problems.push("trade_prob + cancel_prob must not exceed 1".to_string());
    }
    for (name, v) in [
        ("feed_latency_ns", cfg.feed_latency_ns),
        ("feed_jitter_ns", cfg.feed_jitter_ns),
    ] {
        if !(0..=3_600_000_000_000).contains(&v) {
            problems.push(format!("{name} must be between 0 and 1 hour"));
        }
    }
    let end = cfg.origin_ns.checked_add(cfg.duration_ns.max(0));
    if !is_plausible_epoch_ns(cfg.origin_ns) || !end.is_some_and(is_plausible_epoch_ns) {
        problems.push(
            "origin_ns and origin_ns + duration_ns must be epoch nanoseconds between 2010 and 2100"
                .to_string(),
        );
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(DataError::Invalid(format!(
            "invalid synthetic configuration: {}",
            problems.join("; ")
        )))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BookSide {
    Bid,
    Ask,
}

impl BookSide {
    fn flag(self) -> u64 {
        match self {
            BookSide::Bid => BUY_EVENT,
            BookSide::Ask => SELL_EVENT,
        }
    }
}

#[derive(Copy, Clone)]
enum Stream {
    Depth,
    Trade,
}

struct Generator<'a> {
    cfg: &'a SyntheticConfig,
    instrument: InstrumentMeta,
    /// Tick -> lots.
    bids: BTreeMap<i64, i64>,
    asks: BTreeMap<i64, i64>,
    rows: Vec<Event>,
    scan: SourceScan,
    /// Order-flow imbalance in `[-PRESSURE_LIMIT, PRESSURE_LIMIT]`; positive favours buyers.
    pressure: i64,
    last_depth_local: i64,
    last_trade_local: i64,
    rng_time: Rng,
    rng_kind: Rng,
    rng_pressure: Rng,
    rng_trade: Rng,
    rng_cancel: Rng,
    rng_add: Rng,
    rng_latency: Rng,
    rng_book: Rng,
}

impl<'a> Generator<'a> {
    fn new(cfg: &'a SyntheticConfig, instrument: InstrumentMeta) -> Self {
        let stream = |name: &str| Rng::for_stream(cfg.seed, name);
        Self {
            cfg,
            instrument,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            rows: Vec::new(),
            scan: SourceScan::default(),
            pressure: 0,
            last_depth_local: i64::MIN,
            last_trade_local: i64::MIN,
            rng_time: stream("synthetic.time"),
            rng_kind: stream("synthetic.kind"),
            rng_pressure: stream("synthetic.pressure"),
            rng_trade: stream("synthetic.trade"),
            rng_cancel: stream("synthetic.cancel"),
            rng_add: stream("synthetic.add"),
            rng_latency: stream("synthetic.latency"),
            rng_book: stream("synthetic.book"),
        }
    }

    fn levels(&self, side: BookSide) -> &BTreeMap<i64, i64> {
        match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        }
    }

    /// `(tick, lots)` of the `k`-th level counted from the touch.
    fn nth_from_best(&self, side: BookSide, k: usize) -> (i64, i64) {
        let level = match side {
            BookSide::Bid => self.bids.iter().rev().nth(k),
            BookSide::Ask => self.asks.iter().nth(k),
        };
        let (tick, lots) = level.expect("synthetic book: requested level exists");
        (*tick, *lots)
    }

    fn best_bid(&self) -> i64 {
        self.nth_from_best(BookSide::Bid, 0).0
    }

    fn best_ask(&self) -> i64 {
        self.nth_from_best(BookSide::Ask, 0).0
    }

    /// Whether the touch level of `side` may disappear: another level must exist, and the bid
    /// must not fall below [`MIN_BID_TICK`].
    fn can_remove_best(&self, side: BookSide) -> bool {
        match side {
            BookSide::Bid => self
                .bids
                .keys()
                .rev()
                .nth(1)
                .is_some_and(|next| *next >= MIN_BID_TICK),
            BookSide::Ask => self.asks.len() >= 2,
        }
    }

    /// Receipt time of a row: exchange time plus latency plus jitter, never earlier than the
    /// previous row of the same stream. Depth updates carry absolute quantities, so a feed that
    /// delivered them out of order would leave the receiver's book wrong for good; real feeds
    /// are ordered per stream and so is this one. Trades and depth are separate streams and do
    /// overtake each other, which is what makes local order differ from exchange order.
    fn local_ts(&mut self, exch_ts: i64, stream: Stream) -> i64 {
        let jitter = self.rng_latency.range_i64(0, self.cfg.feed_jitter_ns);
        let raw = exch_ts + self.cfg.feed_latency_ns + jitter;
        let last = match stream {
            Stream::Depth => &mut self.last_depth_local,
            Stream::Trade => &mut self.last_trade_local,
        };
        *last = raw.max(*last);
        *last
    }

    fn emit(&mut self, flags: u64, exch_ts: i64, local_ts: i64, tick: i64, lots: i64) {
        let px = self.instrument.price(tick);
        let qty = lots as f64 * self.instrument.lot_size;
        let ev = make_event(flags, exch_ts, local_ts, px, qty);
        // The stored floats must be exactly the canonical value of an integer tick / lot count.
        let precision = RowPrecision {
            price_on_tick: self.instrument.price(self.instrument.tick(px)).to_bits()
                == px.to_bits(),
            qty_on_lot: (self.instrument.lots(qty) as f64 * self.instrument.lot_size).to_bits()
                == qty.to_bits(),
        };
        let row = self.rows.len();
        let at = || format!("generated row {row}");
        if flags & 0xff == TRADE_EVENT {
            self.scan.trade(&ev, precision, at);
        } else {
            self.scan.book(&ev, precision, at);
        }
        self.rows.push(ev);
    }

    /// Sets the total quantity of a level in the book and publishes the depth row.
    fn set_level(&mut self, side: BookSide, tick: i64, lots: i64, exch_ts: i64) {
        debug_assert!(lots >= 0 && tick >= 1);
        let levels = match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        };
        if lots > 0 {
            levels.insert(tick, lots);
        } else {
            levels.remove(&tick);
        }
        let local_ts = self.local_ts(exch_ts, Stream::Depth);
        self.emit(DEPTH_EVENT | side.flag(), exch_ts, local_ts, tick, lots);
        debug_assert!(self.best_bid() < self.best_ask(), "synthetic book crossed");
    }

    /// Size of a freshly posted full level: between half and one and a half times the base.
    fn full_level_lots(&mut self) -> i64 {
        let base = self.cfg.base_level_lots;
        base / 2 + 1 + self.rng_book.below(base as u64) as i64
    }

    /// Size of one limit order.
    fn order_lots(rng: &mut Rng, base: i64) -> i64 {
        1 + rng.below((base / 2).max(1) as u64) as i64
    }

    /// Posts new far levels until `side` is back at its working minimum.
    fn replenish(&mut self, side: BookSide, exch_ts: i64) {
        while self.levels(side).len() < REPLENISH_BELOW {
            let tick = match side {
                BookSide::Ask => {
                    *self
                        .asks
                        .keys()
                        .next_back()
                        .expect("synthetic book: asks are never empty")
                        + 1
                }
                BookSide::Bid => {
                    let lowest = *self
                        .bids
                        .keys()
                        .next()
                        .expect("synthetic book: bids are never empty");
                    if lowest > 1 {
                        lowest - 1
                    } else {
                        // The bids already reach tick 1: use the nearest free tick below the
                        // touch. One exists because the touch is at MIN_BID_TICK or above.
                        (1..self.best_bid())
                            .rev()
                            .find(|t| !self.bids.contains_key(t))
                            .expect("synthetic book: a free bid tick exists below the touch")
                    }
                }
            };
            let lots = self.full_level_lots();
            self.set_level(side, tick, lots, exch_ts);
        }
    }

    /// Posts a new best level one tick inside the spread, if the spread has room.
    fn improve(&mut self, side: BookSide, lots: i64, exch_ts: i64) -> bool {
        let (bid, ask) = (self.best_bid(), self.best_ask());
        if ask - bid < 2 {
            return false;
        }
        let tick = match side {
            BookSide::Bid => bid + 1,
            BookSide::Ask => ask - 1,
        };
        self.set_level(side, tick, lots, exch_ts);
        true
    }

    fn snapshot(&mut self) {
        let cfg = self.cfg;
        let levels = cfg.levels as i64;
        for k in 0..levels {
            let lots = self.full_level_lots();
            self.bids.insert(cfg.start_mid_tick - 1 - k, lots);
            let lots = self.full_level_lots();
            self.asks.insert(cfg.start_mid_tick + 1 + k, lots);
        }
        // One message: the clear and every level share a receipt time, otherwise the clear
        // could arrive after levels it is meant to precede.
        let exch_ts = cfg.origin_ns;
        let local_ts = self.local_ts(exch_ts, Stream::Depth);
        self.rows
            .push(make_event(DEPTH_CLEAR_EVENT, exch_ts, local_ts, 0.0, 0.0));
        let bids: Vec<(i64, i64)> = self.bids.iter().rev().map(|(t, l)| (*t, *l)).collect();
        let asks: Vec<(i64, i64)> = self.asks.iter().map(|(t, l)| (*t, *l)).collect();
        for (tick, lots) in bids {
            self.emit(
                DEPTH_SNAPSHOT_EVENT | BUY_EVENT,
                exch_ts,
                local_ts,
                tick,
                lots,
            );
        }
        for (tick, lots) in asks {
            self.emit(
                DEPTH_SNAPSHOT_EVENT | SELL_EVENT,
                exch_ts,
                local_ts,
                tick,
                lots,
            );
        }
    }

    /// One step of a mean-reverting bounded walk: the further from zero, the likelier a step
    /// back. Order flow therefore trends for a while and then fades.
    fn step_pressure(&mut self) {
        let p = self.pressure;
        let pulled_back = self.rng_pressure.below(4 * PRESSURE_LIMIT as u64) < p.unsigned_abs();
        let step = if pulled_back {
            -p.signum()
        } else if self.rng_pressure.below(2) == 0 {
            1
        } else {
            -1
        };
        self.pressure = (p + step).clamp(-PRESSURE_LIMIT, PRESSURE_LIMIT);
    }

    /// A market order: hits the opposite touch. Emits the trade, then the reduced level.
    fn trade(&mut self, exch_ts: i64) {
        // Buy probability between 5% and 95%, set by the pressure.
        let buy_threshold = (1_000 + self.pressure * 900 / PRESSURE_LIMIT) as u64;
        let mut buyer_initiated = self.rng_trade.below(2_000) < buy_threshold;
        if !buyer_initiated && !self.can_remove_best(BookSide::Bid) {
            // The bid may not retreat any further; only buyers trade until it recovers.
            buyer_initiated = true;
        }
        let (resting, aggressor_flag, follower) = if buyer_initiated {
            (BookSide::Ask, BUY_EVENT, BookSide::Bid)
        } else {
            (BookSide::Bid, SELL_EVENT, BookSide::Ask)
        };
        let (tick, level) = self.nth_from_best(resting, 0);
        let cap = level.min(2 * self.cfg.base_level_lots);
        // A quarter of the orders take everything on offer (up to the cap); the rest take a
        // uniform part of it.
        let lots = if self.rng_trade.below(4) == 0 {
            cap
        } else {
            1 + self.rng_trade.below(cap as u64) as i64
        };

        let local_ts = self.local_ts(exch_ts, Stream::Trade);
        self.emit(TRADE_EVENT | aggressor_flag, exch_ts, local_ts, tick, lots);
        self.set_level(resting, tick, level - lots, exch_ts);

        if lots == level {
            self.replenish(resting, exch_ts);
            if self.rng_trade.below(100) < FOLLOW_PCT {
                let quote = Self::order_lots(&mut self.rng_trade, self.cfg.base_level_lots);
                self.improve(follower, quote, exch_ts);
            }
        }
    }

    /// Reduces, or occasionally removes, a level near the touch.
    fn cancel(&mut self, exch_ts: i64) {
        let side = if self.rng_cancel.below(2) == 0 {
            BookSide::Bid
        } else {
            BookSide::Ask
        };
        let near = self.levels(side).len().min(NEAR_LEVELS);
        let k = self.rng_cancel.below(near as u64) as usize;
        let (tick, level) = self.nth_from_best(side, k);
        let mut lots = if self.rng_cancel.below(16) == 0 {
            level
        } else {
            // Up to half the level, rounded up so a one-lot level can still be cancelled.
            1 + self.rng_cancel.below(((level + 1) / 2) as u64) as i64
        };
        if lots == level && k == 0 && !self.can_remove_best(side) {
            lots = level - 1;
        }
        if lots == 0 {
            // A one-lot touch that must stay: nothing can be cancelled this time.
            return;
        }
        self.set_level(side, tick, level - lots, exch_ts);
        if lots == level {
            self.replenish(side, exch_ts);
        }
    }

    /// A limit order: joins or creates a level within `levels` ticks of the touch, or improves
    /// the best price when the spread is open.
    fn add(&mut self, exch_ts: i64) {
        let side = if self.rng_add.below(2) == 0 {
            BookSide::Bid
        } else {
            BookSide::Ask
        };
        let lots = Self::order_lots(&mut self.rng_add, self.cfg.base_level_lots);
        let spread = self.best_ask() - self.best_bid();
        if spread > 1
            && (spread > WIDE_SPREAD_TICKS || self.rng_add.below(100) < IMPROVE_PCT)
            && self.improve(side, lots, exch_ts)
        {
            return;
        }
        // Minimum of two uniforms: orders cluster at the touch.
        let reach = self.cfg.levels as u64;
        let k = self.rng_add.below(reach).min(self.rng_add.below(reach)) as i64;
        let tick = match side {
            BookSide::Bid => (self.best_bid() - k).max(1),
            BookSide::Ask => self.best_ask() + k,
        };
        let existing = self.levels(side).get(&tick).copied().unwrap_or(0);
        self.set_level(side, tick, existing + lots, exch_ts);
    }

    fn run(&mut self) {
        let cfg = self.cfg;
        let end = cfg.origin_ns + cfg.duration_ns;
        let mut exch_ts = cfg.origin_ns;
        loop {
            exch_ts += self.rng_time.range_i64(1, 2 * cfg.mean_event_gap_ns);
            if exch_ts > end {
                break;
            }
            self.step_pressure();
            let u = self.rng_kind.next_f64();
            if u < cfg.trade_prob {
                self.trade(exch_ts);
            } else if u < cfg.trade_prob + cfg.cancel_prob {
                self.cancel(exch_ts);
            } else {
                self.add(exch_ts);
            }
        }
    }
}

/// Generates the dataset described by `cfg`. Same configuration, same output, bit for bit.
pub fn generate(cfg: &SyntheticConfig) -> Result<BuiltDataset, DataError> {
    validate(cfg)?;
    let instrument = InstrumentMeta {
        tick_size: cfg.tick_size,
        lot_size: cfg.lot_size,
        contract_size: cfg.contract_size,
    };

    let mut generator = Generator::new(cfg, instrument);
    generator.snapshot();
    generator.run();
    let Generator { rows, scan, .. } = generator;
    let source_rows = rows.len() as u64;

    // Rows were generated in exchange order with a trade ahead of its depth row, which is the
    // source order the normalization's stable sorts need.
    let normalized = normalize(rows, &NormalizeOptions::default());
    let analysis = quality::analyze(&ReportInput {
        events: &normalized.events,
        instrument: &instrument,
        scan: &scan,
        trade_ids: None,
        mid_file_snapshots: &Finding::default(),
        snapshots: 1,
        starts_with_snapshot: true,
        local_ts_shift_ns: normalized.local_ts_shift_ns,
        gap_warn_ns: DEFAULT_GAP_WARN_NS,
        precision_basis: "each stored value was compared with the exact product of its integer \
                          tick or lot count",
    })?;

    let generator_json = serde_json::to_value(cfg).map_err(|err| {
        DataError::Invalid(format!(
            "synthetic configuration cannot be serialized: {err}"
        ))
    })?;
    let config_bytes = generator_json.to_string().into_bytes();
    let origin_ns = analysis.first_exch_ts;

    let manifest = DatasetManifest {
        contract_version: CONTRACT_VERSION,
        id: cfg.id.clone(),
        title: cfg.title.clone(),
        kind: DatasetKind::Synthetic,
        level: DataLevel::L2MbpWithTrades,
        source: SourceInfo {
            vendor: "blackbox-synthetic".to_string(),
            exchange: "synthetic".to_string(),
            symbol: "SYNTH".to_string(),
            inputs: vec![InputFile {
                name: "generator configuration (see `generator`)".to_string(),
                role: "synthetic".to_string(),
                bytes: config_bytes.len() as u64,
                sha256: sha256_hex(&config_bytes),
                rows: source_rows,
            }],
            license_note: "Generated by BlackBox HFT from a seed. Not market data. Freely \
                           redistributable."
                .to_string(),
            exch_ts_semantics: "Generator event time: the instant the synthetic book changed. \
                                Not an exchange measurement."
                .to_string(),
        },
        instrument,
        origin_ns: origin_ns.to_string(),
        start: 0,
        end: analysis.last_exch_ts - origin_ns,
        timestamps: TimestampInfo {
            input_unit: TimeUnit::Nanos,
            local_ts: LocalTsProvenance::Synthetic,
            local_ts_shift_ns: normalized.local_ts_shift_ns,
            note: format!(
                "Local timestamps are generated, not measured: exch_ts + {} ns + uniform jitter \
                 in [0, {}] ns, kept in order within the depth stream and within the trade \
                 stream.",
                cfg.feed_latency_ns, cfg.feed_jitter_ns
            ),
        },
        counts: analysis.counts,
        events_file: String::new(),
        events_sha256: String::new(),
        events_bytes: 0,
        generator: Some(generator_json),
        quality: analysis.report,
    };

    Ok(BuiltDataset {
        manifest,
        events: normalized.events,
    })
}

#[cfg(test)]
mod tests {
    use bb_core::dataset::Severity;
    use hftbacktest::types::{EXCH_EVENT, LOCAL_EVENT};

    use super::*;
    use crate::{book::L2Book, has, kind, normalize::validate_event_order};

    fn short(seed: u64) -> SyntheticConfig {
        SyntheticConfig {
            seed,
            duration_ns: 20_000_000_000,
            ..SyntheticConfig::default()
        }
    }

    fn assert_same_events(a: &[Event], b: &[Event]) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert_eq!(x.ev, y.ev, "ev differs at row {i}");
            assert_eq!(x.exch_ts, y.exch_ts, "exch_ts differs at row {i}");
            assert_eq!(x.local_ts, y.local_ts, "local_ts differs at row {i}");
            assert_eq!(x.px.to_bits(), y.px.to_bits(), "px differs at row {i}");
            assert_eq!(x.qty.to_bits(), y.qty.to_bits(), "qty differs at row {i}");
            assert_eq!(x.order_id, y.order_id, "order_id differs at row {i}");
            assert_eq!(x.ival, y.ival, "ival differs at row {i}");
            assert_eq!(
                x.fval.to_bits(),
                y.fval.to_bits(),
                "fval differs at row {i}"
            );
        }
    }

    #[test]
    fn same_configuration_gives_identical_output() {
        let a = generate(&short(7)).unwrap();
        let b = generate(&short(7)).unwrap();
        assert_same_events(&a.events, &b.events);
        assert_eq!(a.manifest, b.manifest);
    }

    #[test]
    fn a_different_seed_gives_a_different_market() {
        let a = generate(&short(7)).unwrap();
        let b = generate(&short(8)).unwrap();
        let identical = a.events.len() == b.events.len()
            && a.events.iter().zip(&b.events).all(|(x, y)| {
                x.ev == y.ev
                    && x.exch_ts == y.exch_ts
                    && x.local_ts == y.local_ts
                    && x.px.to_bits() == y.px.to_bits()
                    && x.qty.to_bits() == y.qty.to_bits()
            });
        assert!(!identical);
    }

    #[test]
    fn book_invariants_hold_at_every_row_on_the_exchange_clock() {
        let cfg = SyntheticConfig::default();
        let ds = generate(&cfg).unwrap();
        let inst = ds.manifest.instrument;
        let mut book = L2Book::new();
        let mut checked = 0u64;
        for ev in ds.events.iter().filter(|e| has(e, EXCH_EVENT)) {
            book.apply(ev, &inst);
            if kind(ev) != DEPTH_CLEAR_EVENT {
                let tick = inst.tick(ev.px);
                assert!(ev.px > 0.0 && tick >= 1, "price must be positive");
                assert_eq!(
                    inst.price(tick).to_bits(),
                    ev.px.to_bits(),
                    "price off tick"
                );
                let lots = inst.lots(ev.qty);
                assert!(lots >= 0);
                assert_eq!(
                    (lots as f64 * inst.lot_size).to_bits(),
                    ev.qty.to_bits(),
                    "quantity is not a whole number of lots"
                );
            }
            // The opening snapshot is one message; the book exists once it is complete.
            if ev.exch_ts == cfg.origin_ns {
                continue;
            }
            let (bid, ask) = (book.best_bid().unwrap().0, book.best_ask().unwrap().0);
            assert!(bid < ask, "book crossed or locked: {bid} / {ask}");
            assert!(book.bid_levels() >= MIN_LEVELS_PER_SIDE);
            assert!(book.ask_levels() >= MIN_LEVELS_PER_SIDE);
            checked += 1;
        }
        assert!(checked > 10_000, "only {checked} rows were generated");
        assert!(!book.is_crossed());
    }

    #[test]
    fn every_trade_is_followed_by_the_depth_row_it_caused() {
        let ds = generate(&SyntheticConfig::default()).unwrap();
        let inst = ds.manifest.instrument;
        let exch: Vec<&Event> = ds.events.iter().filter(|e| has(e, EXCH_EVENT)).collect();
        let mut book = L2Book::new();
        let mut trades = 0u64;
        for (i, ev) in exch.iter().enumerate() {
            if kind(ev) == TRADE_EVENT {
                trades += 1;
                let tick = inst.tick(ev.px);
                let buyer = has(ev, BUY_EVENT);
                assert!(
                    buyer != has(ev, SELL_EVENT),
                    "trade must name one aggressor"
                );
                // A buyer lifts the best ask, a seller hits the best bid.
                let (best, before) = if buyer {
                    book.best_ask().unwrap()
                } else {
                    book.best_bid().unwrap()
                };
                assert_eq!(tick, best, "trade away from the touch");
                let next = exch.get(i + 1).expect("a depth row follows the trade");
                assert_eq!(kind(next), DEPTH_EVENT);
                assert_eq!(next.exch_ts, ev.exch_ts);
                assert_eq!(inst.tick(next.px), tick);
                assert!(has(next, if buyer { SELL_EVENT } else { BUY_EVENT }));
                assert_eq!(
                    inst.lots(next.qty),
                    inst.lots(before) - inst.lots(ev.qty),
                    "depth row must reduce the level by the traded quantity"
                );
                assert!(inst.lots(ev.qty) >= 1);
            }
            book.apply(ev, &inst);
        }
        assert!(trades > 1_000, "only {trades} trades were generated");
    }

    #[test]
    fn manifest_describes_a_synthetic_dataset_honestly() {
        let cfg = SyntheticConfig::default();
        let ds = generate(&cfg).unwrap();
        let m = &ds.manifest;
        assert_eq!(m.kind, DatasetKind::Synthetic);
        assert_eq!(m.timestamps.local_ts, LocalTsProvenance::Synthetic);
        assert_eq!(m.source.vendor, "blackbox-synthetic");
        assert!(m.source.license_note.contains("redistributable"));
        assert_eq!(m.origin(), Some(cfg.origin_ns));
        assert_eq!(m.start, 0);
        assert!(m.end > cfg.duration_ns / 2 && m.end <= cfg.duration_ns);
        assert_eq!(
            serde_json::from_value::<SyntheticConfig>(m.generator.clone().unwrap()).unwrap(),
            cfg
        );
        assert!(m.events_file.is_empty() && m.events_sha256.is_empty());

        assert_eq!(m.counts.rows, ds.events.len() as u64);
        assert_eq!(m.counts.snapshots, 1);
        assert_eq!(m.counts.depth_clears, 1);
        assert_eq!(m.counts.snapshot_levels, 2 * cfg.levels as u64);
        assert!(m.counts.trades > 1_000 && m.counts.depth_updates > m.counts.trades);

        validate_event_order(&ds.events).unwrap();
        // Trades and depth overtake each other on the way to the receiver, so some rows must
        // have been split; this is what exercises the reorder path downstream.
        let split = m
            .quality
            .checks
            .iter()
            .find(|c| c.id == "time.rows_split_for_ordering")
            .unwrap();
        assert!(split.count > 0);
        let source_rows = ds.events.iter().filter(|e| has(e, EXCH_EVENT)).count() as u64;
        assert_eq!(m.counts.rows, source_rows + split.count);
        assert_eq!(
            ds.events.iter().filter(|e| has(e, LOCAL_EVENT)).count() as u64,
            source_rows
        );
        assert_eq!(m.source.inputs[0].rows, source_rows);

        // Nothing worse than the informational split-row note, and no trade-id checks.
        assert!(!matches!(
            m.quality.worst(),
            Some(Severity::Warn | Severity::Error)
        ));
        assert!(
            m.quality
                .checks
                .iter()
                .all(|c| !c.id.starts_with("trades.duplicate") && c.id != "trades.id_gap")
        );
        assert_eq!(m.quality.book_valid_fraction, 1.0);
        let latency = m.quality.feed_latency_ns.unwrap();
        assert!(latency.min >= cfg.feed_latency_ns as f64);
        let spread = m.quality.spread_ticks.unwrap();
        assert!(spread.min >= 1.0 && spread.p50 <= 3.0, "{spread:?}");
    }

    #[test]
    fn invalid_configurations_are_rejected_with_every_problem_named() {
        let cfg = SyntheticConfig {
            id: "Not Valid".to_string(),
            tick_size: 0.0,
            trade_prob: 0.9,
            cancel_prob: 0.9,
            levels: 2,
            ..SyntheticConfig::default()
        };
        let Err(DataError::Invalid(message)) = generate(&cfg) else {
            panic!("configuration must be rejected");
        };
        for needle in ["id", "tick_size", "trade_prob + cancel_prob", "levels"] {
            assert!(message.contains(needle), "{message}");
        }
    }

    #[test]
    fn partial_json_configuration_uses_defaults() {
        let cfg: SyntheticConfig = serde_json::from_str(r#"{"seed": 99, "levels": 12}"#).unwrap();
        assert_eq!(
            cfg,
            SyntheticConfig {
                seed: 99,
                levels: 12,
                ..SyntheticConfig::default()
            }
        );
    }

    #[test]
    fn zero_latency_configuration_still_yields_strictly_positive_feed_latency() {
        let cfg = SyntheticConfig {
            feed_latency_ns: 0,
            feed_jitter_ns: 0,
            duration_ns: 2_000_000_000,
            ..SyntheticConfig::default()
        };
        let ds = generate(&cfg).unwrap();
        assert_eq!(ds.manifest.timestamps.local_ts_shift_ns, 1);
        assert!(ds.events.iter().all(|e| e.local_ts > e.exch_ts));
    }
}
