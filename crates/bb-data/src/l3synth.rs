//! Redistributable, wholly invented ITCH 5.0 messages, not observed exchange data.
//! A latent fair value follows a bounded half-tick random walk; passive adds rest around it,
//! cancels concentrate near the touch, and marketable orders sweep FIFO queues at the touch.
//! The dynamics are illustrative, not calibrated. Seed and configuration determine the bytes.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};

pub const MAX_EVENTS: u32 = 50_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct L3SynthConfig {
    pub seed: u64,
    pub symbol: String,
    /// Exact number of order-flow messages after the session/directory/seed-book prologue
    /// (excluding opt-in injected faults and the closing system event).
    pub events: u32,
    pub inject_unknown_execute: bool,
    pub inject_duplicate_add: bool,
}

impl Default for L3SynthConfig {
    fn default() -> Self {
        Self {
            seed: 7,
            symbol: "SYNTH".into(),
            events: 30_000,
            inject_unknown_execute: false,
            inject_duplicate_add: false,
        }
    }
}

pub(crate) fn stock8(symbol: &str) -> Result<[u8; 8], String> {
    if symbol.is_empty() || symbol.len() > 8 || !symbol.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("symbol must contain 1..8 printable non-space ASCII characters".into());
    }
    let mut stock = [b' '; 8];
    stock[..symbol.len()].copy_from_slice(symbol.as_bytes());
    Ok(stock)
}

/// SplitMix64: fixed integer arithmetic, independent of platform and library versions.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn gap(&mut self) -> u64 {
        // Geometric waiting time on a 100-us lattice (discrete Poisson approximation).
        let mut slots = 1;
        while self.below(4) != 0 {
            slots += 1;
        }
        slots * 100_000
    }
}

pub(crate) fn header(kind: u8, ts: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(50);
    out.push(kind);
    out.extend((if kind == b'S' { 0u16 } else { 1u16 }).to_be_bytes());
    out.extend(0u16.to_be_bytes());
    out.extend(&ts.to_be_bytes()[2..]);
    out
}

pub(crate) fn pack(out: &mut Vec<u8>, frame: &[u8]) {
    debug_assert_eq!(
        Some(frame.len()),
        crate::itch::decode::spec_length(frame[0])
    );
    out.extend((frame.len() as u16).to_be_bytes());
    out.extend_from_slice(frame);
}

pub(crate) fn directory(ts: u64, stock: &[u8; 8]) -> Vec<u8> {
    let mut f = header(b'R', ts);
    f.extend(stock);
    f.extend(*b"QN");
    f.extend(100u32.to_be_bytes());
    f.extend(*b"NCZ PNN1N");
    f.extend(0u32.to_be_bytes());
    f.push(b'N');
    f
}

pub(crate) fn add(ts: u64, id: u64, side: u8, shares: u32, price: u32, stock: &[u8; 8]) -> Vec<u8> {
    let mut f = header(b'A', ts);
    f.extend(id.to_be_bytes());
    f.push(side);
    f.extend(shares.to_be_bytes());
    f.extend(stock);
    f.extend(price.to_be_bytes());
    f
}

/// One cent in ITCH price4 units.
const TICK: u32 = 100;
/// Round lot in shares.
const LOT: u32 = 100;
/// Latent fair value starts at $100.005 (half-tick units), bounded to +/- $2.00.
const FAIR_START: i64 = 2 * 10_000 + 1;
const FAIR_BOUND: i64 = 400;
/// Marketable orders walk at most this many price levels.
const SWEEP_LEVELS: usize = 3;
/// Fair value moves one half-tick with probability 1/FAIR_STEP per step.
const FAIR_STEP: u64 = 16;
/// While fair value lies beyond a touch, a step is a marketable order with probability 1/STALE_HIT.
const STALE_HIT: u64 = 4;
/// Percent probability that a passive add's geometric depth continues one more tick out.
const ADD_DEPTH_CONTINUE: u64 = 60;
/// Percent probability that a cancel/replace target moves one more level away from the touch.
const CANCEL_DEPTH_CONTINUE: u64 = 35;

#[derive(Clone, Copy)]
struct Live {
    side: usize,
    shares: u32,
    price: u32,
}

/// Generator-side mirror of the resting book. HashMap is used for lookups only; every
/// random choice walks the ordered price maps, so iteration order never affects bytes.
#[derive(Default)]
struct Book {
    orders: HashMap<u64, Live>,
    /// Index 0 = bids, 1 = asks; FIFO queues of order ids per price.
    levels: [BTreeMap<u32, VecDeque<u64>>; 2],
    counts: [usize; 2],
}

impl Book {
    fn best(&self, side: usize) -> Option<u32> {
        if side == 0 {
            self.levels[0].last_key_value().map(|(p, _)| *p)
        } else {
            self.levels[1].first_key_value().map(|(p, _)| *p)
        }
    }
    /// Price `depth` levels away from the touch (0 = touch).
    fn level(&self, side: usize, depth: usize) -> Option<u32> {
        if side == 0 {
            self.levels[0].keys().rev().nth(depth).copied()
        } else {
            self.levels[1].keys().nth(depth).copied()
        }
    }
    fn insert(&mut self, id: u64, order: Live) {
        self.orders.insert(id, order);
        self.levels[order.side]
            .entry(order.price)
            .or_default()
            .push_back(id);
        self.counts[order.side] += 1;
    }
    fn remove(&mut self, id: u64) -> Live {
        let order = self.orders.remove(&id).expect("live order");
        let queue = self.levels[order.side]
            .get_mut(&order.price)
            .expect("live level");
        queue.retain(|x| *x != id);
        if queue.is_empty() {
            self.levels[order.side].remove(&order.price);
        }
        self.counts[order.side] -= 1;
        order
    }
    /// Reduces resting shares; removes the order when it reaches zero.
    fn reduce(&mut self, id: u64, shares: u32) {
        let order = self.orders.get_mut(&id).expect("live order");
        order.shares -= shares;
        if order.shares == 0 {
            self.remove(id);
        }
    }
}

#[derive(Clone, Copy)]
enum Op {
    Add { mpid: bool },
    Delete,
    PartialCancel,
    Marketable,
    Replace,
    Print,
}

fn side_byte(side: usize) -> u8 {
    if side == 0 { b'B' } else { b'S' }
}

/// True when a `side` limit at `price` would trade against `opposite_best`.
fn crosses(side: usize, price: u32, opposite_best: Option<u32>) -> bool {
    opposite_best.is_some_and(|p| if side == 0 { price >= p } else { price <= p })
}

/// Output buffer plus the generator's mirror of every accepted effect.
struct Emitter {
    out: Vec<u8>,
    stock: [u8; 8],
    book: Book,
    next_id: u64,
    match_no: u64,
    emitted: u32,
    budget: u32,
}

impl Emitter {
    fn room(&self) -> bool {
        self.emitted < self.budget
    }
    fn emit(&mut self, frame: &[u8]) {
        pack(&mut self.out, frame);
        self.emitted += 1;
    }
    fn post(&mut self, ts: u64, side: usize, shares: u32, price: u32, mpid: bool) {
        let id = self.next_id;
        let mut frame = add(ts, id, side_byte(side), shares, price, &self.stock);
        if mpid {
            frame[0] = b'F';
            frame.extend(*b"SYNT");
        }
        self.emit(&frame);
        self.book.insert(
            id,
            Live {
                side,
                shares,
                price,
            },
        );
        self.next_id += 1;
    }
    /// Executes FIFO against the `resting` side's touch while it is within `limit` (if any),
    /// across at most `SWEEP_LEVELS` levels, never consuming a side's last order. One `E`
    /// (1 in 10: `C`) per resting order hit, all at the aggressor's timestamp. Returns the
    /// unfilled quantity.
    fn take(
        &mut self,
        rng: &mut Rng,
        ts: u64,
        resting: usize,
        limit: Option<u32>,
        mut remaining: u32,
    ) -> u32 {
        let mut walked = 0;
        while remaining > 0 && self.room() && walked < SWEEP_LEVELS && self.book.counts[resting] > 1
        {
            let price = self.book.best(resting).expect("non-empty side");
            if limit.is_some_and(|l| !crosses(1 - resting, l, Some(price))) {
                break;
            }
            let id = self.book.levels[resting][&price][0];
            let fill = remaining.min(self.book.orders[&id].shares);
            self.match_no += 1;
            let with_price = rng.below(10) == 0;
            let mut frame = header(if with_price { b'C' } else { b'E' }, ts);
            frame.extend(id.to_be_bytes());
            frame.extend(fill.to_be_bytes());
            frame.extend(self.match_no.to_be_bytes());
            if with_price {
                frame.push(b'Y');
                frame.extend(price.to_be_bytes());
            }
            self.emit(&frame);
            remaining -= fill;
            self.book.reduce(id, fill);
            if self.book.best(resting) != Some(price) {
                walked += 1;
            }
        }
        remaining
    }
}

/// Generates raw, two-byte-length-prefixed ITCH frames. No external data is used.
/// `events` must be 8..=50_000.
///
/// Per-step dynamics (integer arithmetic on one SplitMix64 stream):
/// - a latent fair value moves one half-tick with probability `1/FAIR_STEP`; passive
///   reference prices are the ticks just inside it (1-tick reference spread when fair
///   value sits between ticks, 2 ticks when on a tick);
/// - base mix per step: 50% `A`, 4% `F`, 31% `D`, 4% `X`, 5% marketable orders, 4% `U`,
///   2% non-cross `P`;
/// - add prices are drawn below/above the reference (geometric depth plus a uniform
///   0..10-tick tail); an add whose price reaches the opposite touch is a marketable limit
///   order: it executes FIFO up to its limit and any remainder rests at that limit, moving
///   the touch toward fair value;
/// - marketable (unpriced) orders sweep up to `SWEEP_LEVELS` touch levels; their direction
///   leans toward fair value relative to the book mid;
/// - when fair value has moved through a displayed touch, that side is stale: a step is a
///   marketable order against it with probability `1/STALE_HIT`, and half of the cancels
///   and replaces pull its quotes;
/// - add sides lean toward the side with fewer orders, cancels toward the side with more;
///   cancel levels are geometric from the touch, with occasional cleanup of depth more than
///   ten ticks beyond the reference. Replaces reprice toward the reference without crossing.
pub fn generate(config: &L3SynthConfig) -> Result<Vec<u8>, String> {
    if !(8..=MAX_EVENTS).contains(&config.events) {
        return Err(format!("events must be 8..={MAX_EVENTS}"));
    }
    let stock = stock8(&config.symbol)?;
    let mut rng = Rng(config.seed);
    let mut g = Emitter {
        out: Vec::with_capacity((config.events as usize + 64) * 40),
        stock,
        book: Book::default(),
        next_id: 1,
        match_no: 0,
        emitted: 0,
        budget: u32::MAX,
    };
    let mut ts = 9 * 3_600_000_000_000 + 30 * 60_000_000_000;
    let mut f = header(b'S', ts);
    f.push(b'O');
    pack(&mut g.out, &f);
    ts += 1;
    pack(&mut g.out, &directory(ts, &stock));
    ts += 1;
    let mut f = header(b'H', ts);
    f.extend(stock);
    f.extend(*b"T     ");
    pack(&mut g.out, &f);
    let mut fair2 = FAIR_START;
    let refs = |fair2: i64| -> [u32; 2] {
        [
            ((fair2 - 1).div_euclid(2) as u32) * TICK,
            ((fair2 + 2).div_euclid(2) as u32) * TICK,
        ]
    };
    // Seed book: five levels per side, three orders each, so FIFO effects are visible.
    let seed_refs = refs(fair2);
    for side in 0..2 {
        for depth in 0..5u32 {
            for _ in 0..3 {
                ts += 1;
                let price = if side == 0 {
                    seed_refs[0] - depth * TICK
                } else {
                    seed_refs[1] + depth * TICK
                };
                let shares = LOT * (1 + rng.below(8) as u32);
                g.post(ts, side, shares, price, false);
            }
        }
    }
    g.emitted = 0;
    g.budget = config.events;
    while g.room() {
        // Same-timestamp bursts (1/8) preserve strict source order.
        if rng.below(8) != 0 {
            ts += rng.gap();
        }
        if rng.below(FAIR_STEP) == 0 {
            let step = if rng.below(2) == 0 { -1 } else { 1 };
            fair2 = (fair2 + step).clamp(FAIR_START - FAIR_BOUND, FAIR_START + FAIR_BOUND);
        }
        let reference = refs(fair2);
        let touch = [g.book.best(0), g.book.best(1)];
        let counts = g.book.counts;
        // Side leaning toward balance: probability 7/10 for the favoured side.
        let lean = |rng: &mut Rng, favour_bids: bool| -> usize {
            let p_bid = match counts[0].cmp(&counts[1]) {
                std::cmp::Ordering::Equal => 5,
                _ if favour_bids => 7,
                _ => 3,
            };
            usize::from(rng.below(10) >= p_bid)
        };
        let stale = if touch[1].is_some_and(|a| reference[0] >= a) {
            Some(1)
        } else if touch[0].is_some_and(|b| reference[1] <= b) {
            Some(0)
        } else {
            None
        };
        let mut op = if stale.is_some() && rng.below(STALE_HIT) == 0 {
            Op::Marketable
        } else {
            match rng.below(100) {
                0..=49 => Op::Add { mpid: false },
                50..=53 => Op::Add { mpid: true },
                54..=84 => Op::Delete,
                85..=88 => Op::PartialCancel,
                89..=93 => Op::Marketable,
                94..=97 => Op::Replace,
                _ => Op::Print,
            }
        };
        // A side with fewer than two orders is replenished before anything else.
        let thin = (0..2).find(|&s| counts[s] < 2);
        if thin.is_some() {
            op = Op::Add { mpid: false };
        }
        let place = |rng: &mut Rng, side: usize| -> u32 {
            let depth = if rng.below(4) != 0 {
                let mut d = 0;
                while d < 15 && rng.below(100) < ADD_DEPTH_CONTINUE {
                    d += 1;
                }
                d
            } else {
                rng.below(11) as u32
            };
            if side == 0 {
                reference[0].saturating_sub(depth * TICK).max(TICK)
            } else {
                reference[1] + depth * TICK
            }
        };
        let size = |rng: &mut Rng| -> u32 {
            if rng.below(16) == 0 {
                LOT * (5 + rng.below(16) as u32)
            } else {
                LOT * (1 + rng.below(8) as u32)
            }
        };
        // Cancel/replace target: geometric level depth from the touch, uniform position in
        // the queue; 1/3 of the time the farthest level is cleaned up instead, when it lies
        // more than ten ticks beyond the passive reference.
        let target = |rng: &mut Rng, book: &Book, side: usize| -> u64 {
            let levels = book.levels[side].len();
            let far = book.level(side, levels - 1).expect("level exists");
            let depth = if rng.below(3) == 0 && far.abs_diff(reference[side]) > 10 * TICK {
                levels - 1
            } else {
                let mut d = 0;
                while d + 1 < levels && rng.below(100) < CANCEL_DEPTH_CONTINUE {
                    d += 1;
                }
                d
            };
            let price = book.level(side, depth).expect("level exists");
            let queue = &book.levels[side][&price];
            queue[rng.below(queue.len() as u64) as usize]
        };
        match op {
            Op::Add { mpid } => {
                let side = thin.unwrap_or_else(|| lean(&mut rng, counts[0] < counts[1]));
                let price = place(&mut rng, side);
                let mut shares = size(&mut rng);
                if crosses(side, price, touch[1 - side]) {
                    shares = g.take(&mut rng, ts, 1 - side, Some(price), shares);
                }
                // A crossing remainder that could not trade (sweep bound) is cancelled (IOC).
                if shares > 0 && g.room() && !crosses(side, price, g.book.best(1 - side)) {
                    g.post(ts, side, shares, price, mpid);
                }
            }
            Op::Delete | Op::PartialCancel | Op::Replace => {
                let side = match stale {
                    Some(side) if rng.below(2) == 0 => side,
                    _ => lean(&mut rng, counts[0] > counts[1]),
                };
                let id = target(&mut rng, &g.book, side);
                let order = g.book.orders[&id];
                let mut frame;
                if matches!(op, Op::Replace) {
                    // Reprice toward the reference with a fresh size (new priority), passive only.
                    g.book.remove(id);
                    let opposite = g.book.best(1 - side);
                    let mut price = place(&mut rng, side);
                    if crosses(side, price, opposite) {
                        let p = opposite.expect("crossing implies a touch");
                        price = if side == 0 { p - TICK } else { p + TICK };
                    }
                    let shares = size(&mut rng);
                    frame = header(b'U', ts);
                    frame.extend(id.to_be_bytes());
                    frame.extend(g.next_id.to_be_bytes());
                    frame.extend(shares.to_be_bytes());
                    frame.extend(price.to_be_bytes());
                    g.book.insert(
                        g.next_id,
                        Live {
                            side,
                            shares,
                            price,
                        },
                    );
                    g.next_id += 1;
                } else if matches!(op, Op::PartialCancel) && order.shares >= 2 * LOT {
                    let lots = order.shares / LOT;
                    let cancelled = LOT * (1 + rng.below(u64::from(lots - 1)) as u32);
                    frame = header(b'X', ts);
                    frame.extend(id.to_be_bytes());
                    frame.extend(cancelled.to_be_bytes());
                    g.book.reduce(id, cancelled);
                } else {
                    frame = header(b'D', ts);
                    frame.extend(id.to_be_bytes());
                    g.book.remove(id);
                }
                g.emit(&frame);
            }
            Op::Marketable => {
                // Stale quotes are hit; otherwise the aggressor leans toward fair value
                // relative to the book mid. A buyer consumes asks (side 1), a seller bids.
                let (Some(bid), Some(ask)) = (touch[0], touch[1]) else {
                    unreachable!("both sides hold >= 2 orders here")
                };
                let book_mid2 = i64::from((bid + ask) / TICK);
                let buy_bias = match fair2.cmp(&book_mid2) {
                    std::cmp::Ordering::Greater => 7,
                    std::cmp::Ordering::Less => 3,
                    std::cmp::Ordering::Equal => 5,
                };
                let resting = stale.unwrap_or_else(|| usize::from(rng.below(10) < buy_bias));
                let shares = if rng.below(8) == 0 {
                    LOT * (5 + rng.below(20) as u32)
                } else {
                    LOT * (1 + rng.below(6) as u32)
                };
                g.take(&mut rng, ts, resting, None, shares);
            }
            Op::Print => {
                // Non-displayed execution: a trade print inside the spread, no book effect.
                let (Some(bid), Some(ask)) = (touch[0], touch[1]) else {
                    unreachable!("both sides hold >= 2 orders here")
                };
                g.match_no += 1;
                let mut frame = header(b'P', ts);
                frame.extend(0u64.to_be_bytes());
                frame.push(b'B');
                frame.extend((LOT * (1 + rng.below(5) as u32)).to_be_bytes());
                frame.extend(stock);
                frame.extend(((bid + ask) / 2 / TICK * TICK).to_be_bytes());
                frame.extend(g.match_no.to_be_bytes());
                g.emit(&frame);
            }
        }
    }
    if config.inject_unknown_execute {
        ts += 1;
        let mut f = header(b'E', ts);
        f.extend(u64::MAX.to_be_bytes());
        f.extend(1u32.to_be_bytes());
        f.extend(u64::MAX.to_be_bytes());
        pack(&mut g.out, &f);
    }
    if config.inject_duplicate_add {
        ts += 1;
        let price = g.book.best(0).expect("bids are never emptied");
        let id = g.book.levels[0][&price][0];
        let o = g.book.orders[&id];
        pack(&mut g.out, &add(ts, id, b'B', o.shares, o.price, &stock));
    }
    ts += 1;
    let mut f = header(b'S', ts);
    f.push(b'C');
    pack(&mut g.out, &f);
    Ok(g.out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::itch::{ItchOptions, reconstruct_bytes};
    use sha2::{Digest, Sha256};

    #[test]
    fn deterministic_bytes_and_clean_default_cover_all_operations() {
        let config = L3SynthConfig::default();
        let a = generate(&config).unwrap();
        let b = generate(&config).unwrap();
        assert_eq!(Sha256::digest(&a), Sha256::digest(&b));
        assert_eq!(a, b);
        let rec = reconstruct_bytes(
            "synthetic.itch",
            &a,
            ItchOptions {
                symbols: vec!["SYNTH".into()],
                ..Default::default()
            },
        );
        assert!(
            rec.manifest.checks.iter().all(|c| c.passed),
            "{:?}",
            rec.manifest.checks
        );
        assert_eq!(rec.manifest.book.quarantined_total, 0);
        for kind in ["S", "R", "A", "F", "E", "C", "X", "D", "U", "P"] {
            assert!(rec.manifest.messages_by_type[kind] > 0);
        }
        let changed = generate(&L3SynthConfig { seed: 8, ..config }).unwrap();
        assert_ne!(Sha256::digest(&a), Sha256::digest(&changed));
    }

    #[test]
    fn faults_are_explicit_and_bounded() {
        let config = L3SynthConfig {
            inject_unknown_execute: true,
            inject_duplicate_add: true,
            ..Default::default()
        };
        let rec = reconstruct_bytes(
            "faults.itch",
            &generate(&config).unwrap(),
            ItchOptions::default(),
        );
        assert_eq!(rec.manifest.book.quarantined_total, 2);
        assert_eq!(rec.manifest.book.quarantined["unknown_order"], 1);
        assert_eq!(rec.manifest.book.quarantined["duplicate_order_ref"], 1);
        assert!(
            generate(&L3SynthConfig {
                events: MAX_EVENTS + 1,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            generate(&L3SynthConfig {
                symbol: "TOO_LONG_".into(),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            generate(&L3SynthConfig {
                events: 0,
                ..Default::default()
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod dynamics_tests {
    use super::*;
    use crate::itch::{
        ItchOptions, Reconstructor,
        decode::{FrameReader, Message, Side},
    };

    /// Statistics measured through the unchanged ITCH reconstructor, not the generator mirror.
    #[derive(Debug, Default)]
    struct Stats {
        by_type: BTreeMap<char, usize>,
        bid_changes: usize,
        ask_changes: usize,
        distinct_bids: usize,
        bid_range_ticks: u32,
        /// Spread in ticks -> number of post-event observations with a two-sided book.
        spreads: BTreeMap<u32, usize>,
        /// Execution messages (E/C) by resting side: [buy, sell].
        executed: [usize; 2],
        max_order_gap: u64,
        mean_orders_per_side: f64,
        /// Resting orders [bid, ask] after the closing message.
        final_orders: [u64; 2],
        /// Mean resting orders at the best bid/ask over two-sided observations.
        mean_touch_orders: f64,
    }

    fn measure(config: &L3SynthConfig) -> Stats {
        let bytes = generate(config).unwrap();
        let mut frames = FrameReader::new(bytes.as_slice());
        let mut rec = Reconstructor::new(ItchOptions::default());
        let mut stats = Stats::default();
        let (mut last_bid, mut last_ask) = (None, None);
        let mut bids = std::collections::BTreeSet::new();
        let (mut observations, mut order_sum, mut touch_sum) = (0u64, 0u64, 0u64);
        while let Some(frame) = frames.next_frame().unwrap() {
            *stats.by_type.entry(char::from(frame[0])).or_default() += 1;
            let executed = match crate::itch::decode::decode(frame).unwrap() {
                Message::OrderExecuted(m) => Some(m.order_ref),
                Message::OrderExecutedWithPrice(m) => Some(m.order_ref),
                _ => None,
            };
            if let Some(order) = executed.and_then(|id| rec.books().order(id)) {
                stats.executed[usize::from(order.side == Side::Sell)] += 1;
            }
            rec.push(frame);
            let best_bid = rec.books().best_bid(1);
            let best_ask = rec.books().best_ask(1);
            let bid = best_bid.as_ref().map(|l| l.price.0);
            let ask = best_ask.as_ref().map(|l| l.price.0);
            if let (Some(b), Some(a)) = (&best_bid, &best_ask) {
                touch_sum += u64::from(b.orders) + u64::from(a.orders);
            }
            stats.bid_changes += usize::from(last_bid.zip(bid).is_some_and(|(a, b)| a != b));
            stats.ask_changes += usize::from(last_ask.zip(ask).is_some_and(|(a, b)| a != b));
            if let (Some(b), Some(a)) = (bid, ask) {
                assert_eq!((b % TICK, a % TICK), (0, 0), "cent grid");
                assert!(a > b, "crossed or locked book");
                *stats.spreads.entry((a - b) / TICK).or_default() += 1;
                bids.insert(b);
                let buy = rec.books().side_totals(1, Side::Buy).1;
                let sell = rec.books().side_totals(1, Side::Sell).1;
                stats.max_order_gap = stats.max_order_gap.max(buy.abs_diff(sell));
                observations += 2;
                order_sum += buy + sell;
            }
            (last_bid, last_ask) = (bid, ask);
        }
        assert_eq!(rec.books().quarantined_total(), 0);
        stats.distinct_bids = bids.len();
        stats.bid_range_ticks = (bids.last().unwrap() - bids.first().unwrap()) / TICK;
        stats.mean_orders_per_side = order_sum as f64 / observations as f64;
        stats.final_orders = [Side::Buy, Side::Sell].map(|s| rec.books().side_totals(1, s).1);
        stats.mean_touch_orders = touch_sum as f64 / observations as f64;
        stats
    }

    #[test]
    fn default_session_is_not_degenerate() {
        let config = L3SynthConfig::default();
        let s = measure(&config);
        eprintln!("{s:#?}");
        let n = |k: char| s.by_type.get(&k).copied().unwrap_or(0);
        // Prologue: S, R, H and 30 seed adds; epilogue: S.
        assert_eq!(
            s.by_type.values().sum::<usize>(),
            config.events as usize + 34
        );
        assert!(s.bid_changes >= 750, "bid changes: {}", s.bid_changes);
        assert!(s.ask_changes >= 750, "ask changes: {}", s.ask_changes);
        assert!(s.distinct_bids >= 10, "distinct bids: {}", s.distinct_bids);
        assert!(s.bid_range_ticks >= 5, "bid range: {}", s.bid_range_ticks);
        assert!(s.spreads.len() >= 3, "spreads: {:?}", s.spreads);
        let one_tick = s.spreads.get(&1).copied().unwrap_or(0);
        let total: usize = s.spreads.values().sum();
        assert!(
            one_tick * 10 > total && one_tick * 10 < total * 9,
            "spreads: {:?}",
            s.spreads
        );
        assert!(s.executed.iter().all(|n| *n >= 500), "{:?}", s.executed);
        let adds = n('A') + n('F');
        let cancels = n('X') + n('D');
        let executions = n('E') + n('C');
        assert!(adds > 3 * cancels / 2, "adds={adds} cancels={cancels}");
        assert!(
            cancels > executions,
            "cancels={cancels} executions={executions}"
        );
        assert!(n('U') > 0 && n('P') > 0);
        assert!(
            (s.max_order_gap as f64) <= s.mean_orders_per_side,
            "order-count gap {} vs mean {}",
            s.max_order_gap,
            s.mean_orders_per_side
        );
    }

    #[test]
    fn other_seeds_and_short_sessions_stay_clean() {
        for seed in [1, 2, 3] {
            let s = measure(&L3SynthConfig {
                seed,
                events: 5_000,
                ..Default::default()
            });
            assert!(s.bid_changes >= 100 && s.spreads.len() >= 2, "{s:?}");
        }
        measure(&L3SynthConfig {
            events: 8,
            ..Default::default()
        });
    }
}
