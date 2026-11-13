//! Level-2 book reconstruction from an engine event array, on the exchange clock.
//!
//! The book is rebuilt only from rows flagged `EXCH_EVENT`, in array order. Rows that share one
//! exchange timestamp form a batch (one exchange message, or several that cannot be told apart
//! in time); only the state after a complete batch is ever reported, because the states in
//! between are artifacts of row order and never existed at the exchange.

use std::collections::BTreeMap;

use bb_core::{
    RelNs, Side,
    dataset::InstrumentMeta,
    market::{BboSeries, DepthFrame, ReplayWindow, TradeSeries},
};
use hftbacktest::types::{
    DEPTH_CLEAR_EVENT, DEPTH_EVENT, DEPTH_SNAPSHOT_EVENT, EXCH_EVENT, TRADE_EVENT,
};

use crate::{Event, has, kind, side_of};

/// Upper bound on depth frames in one [`ReplayWindow`]. A request asking for more gets a wider
/// step, which the response states in `step_ns`.
pub const MAX_REPLAY_FRAMES: i64 = 5_000;

/// Top of book: `(price_tick, qty)` per side, `None` while the side is empty.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Bbo {
    pub bid: Option<(i64, f64)>,
    pub ask: Option<(i64, f64)>,
}

impl Bbo {
    pub fn is_empty(&self) -> bool {
        self.bid.is_none() && self.ask.is_none()
    }

    /// Both sides present with the best bid at or above the best ask.
    pub fn is_crossed(&self) -> bool {
        matches!((self.bid, self.ask), (Some((b, _)), Some((a, _))) if b >= a)
    }

    /// Both sides present and not crossed.
    pub fn is_valid(&self) -> bool {
        matches!((self.bid, self.ask), (Some((b, _)), Some((a, _))) if b < a)
    }
}

/// Price-ordered Level-2 book keyed by integer tick.
///
/// Rows are applied literally: a bid posted at or above the best ask leaves both levels in place
/// and the book reports itself crossed. (The engine's hash-map depth instead moves its best-price
/// pointer past the stale level; this type is for inspection and quality checks, where a crossed
/// feed must stay visible.)
#[derive(Clone, Debug, Default)]
pub struct L2Book {
    bids: BTreeMap<i64, f64>,
    asks: BTreeMap<i64, f64>,
}

impl L2Book {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one engine row. Depth and snapshot-level rows set the total quantity at a price
    /// (a quantity that rounds to zero lots removes the level, as in the engine); clear rows
    /// remove bids at or above `px`, asks at or below `px`, or both sides when the row names no
    /// side. Returns `false` for rows that do not describe the book (trades, depth rows without a
    /// side).
    pub fn apply(&mut self, ev: &Event, instrument: &InstrumentMeta) -> bool {
        match kind(ev) {
            DEPTH_EVENT | DEPTH_SNAPSHOT_EVENT => {
                let Some(side) = side_of(ev) else {
                    return false;
                };
                let tick = instrument.tick(ev.px);
                let levels = match side {
                    Side::Buy => &mut self.bids,
                    Side::Sell => &mut self.asks,
                };
                if instrument.lots(ev.qty) > 0 {
                    levels.insert(tick, ev.qty);
                } else {
                    levels.remove(&tick);
                }
                true
            }
            DEPTH_CLEAR_EVENT => {
                match side_of(ev) {
                    Some(Side::Buy) => {
                        if ev.px.is_finite() {
                            // `split_off` returns the keys >= tick, which are the ones to drop.
                            drop(self.bids.split_off(&instrument.tick(ev.px)));
                        } else {
                            self.bids.clear();
                        }
                    }
                    Some(Side::Sell) => {
                        if ev.px.is_finite() {
                            let first_kept = instrument.tick(ev.px).saturating_add(1);
                            self.asks = self.asks.split_off(&first_kept);
                        } else {
                            self.asks.clear();
                        }
                    }
                    None => {
                        self.bids.clear();
                        self.asks.clear();
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub fn best_bid(&self) -> Option<(i64, f64)> {
        self.bids.iter().next_back().map(|(t, q)| (*t, *q))
    }

    pub fn best_ask(&self) -> Option<(i64, f64)> {
        self.asks.iter().next().map(|(t, q)| (*t, *q))
    }

    pub fn bbo(&self) -> Bbo {
        Bbo {
            bid: self.best_bid(),
            ask: self.best_ask(),
        }
    }

    /// Up to `n` bid levels, best (highest) first.
    pub fn top_bids(&self, n: usize) -> Vec<(i64, f64)> {
        self.bids.iter().rev().take(n).map(|(t, q)| (*t, *q)).collect()
    }

    /// Up to `n` ask levels, best (lowest) first.
    pub fn top_asks(&self, n: usize) -> Vec<(i64, f64)> {
        self.asks.iter().take(n).map(|(t, q)| (*t, *q)).collect()
    }

    pub fn bid_qty(&self, tick: i64) -> Option<f64> {
        self.bids.get(&tick).copied()
    }

    pub fn ask_qty(&self, tick: i64) -> Option<f64> {
        self.asks.get(&tick).copied()
    }

    pub fn bid_levels(&self) -> usize {
        self.bids.len()
    }

    pub fn ask_levels(&self) -> usize {
        self.asks.len()
    }

    /// Both sides present with the best bid at or above the best ask (crossed or locked).
    pub fn is_crossed(&self) -> bool {
        self.bbo().is_crossed()
    }
}

fn push_bbo(series: &mut BboSeries, ts: RelNs, bbo: &Bbo) {
    series.ts.push(ts);
    series.bid_tick.push(bbo.bid.map(|(t, _)| t));
    series.ask_tick.push(bbo.ask.map(|(t, _)| t));
    series.bid_qty.push(bbo.bid.map_or(0.0, |(_, q)| q));
    series.ask_qty.push(bbo.ask.map_or(0.0, |(_, q)| q));
}

/// Top of book at every change, for the whole dataset.
///
/// An entry is appended when the best bid or ask (tick or quantity) differs from the previous
/// entry after all rows of one exchange timestamp have been applied. The series starts with the
/// first non-empty book.
pub fn bbo_series(events: &[Event], instrument: &InstrumentMeta, origin_ns: i64) -> BboSeries {
    let mut series = BboSeries::default();
    let mut book = L2Book::new();
    let mut recorded = Bbo::default();
    let mut batch_ts: Option<i64> = None;
    let mut dirty = false;

    let mut close = |ts: i64, book: &L2Book, dirty: &mut bool| {
        if std::mem::take(dirty) {
            let now = book.bbo();
            if now != recorded {
                push_bbo(&mut series, ts - origin_ns, &now);
                recorded = now;
            }
        }
    };

    for ev in events.iter().filter(|e| has(e, EXCH_EVENT)) {
        if let Some(ts) = batch_ts
            && ts != ev.exch_ts
        {
            close(ts, &book, &mut dirty);
        }
        batch_ts = Some(ev.exch_ts);
        if book.apply(ev, instrument) {
            dirty = true;
        }
    }
    if let Some(ts) = batch_ts {
        close(ts, &book, &mut dirty);
    }
    series
}

/// Parameters of [`replay_window`]. Times are relative to the dataset origin.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ReplayRequest {
    pub t0: RelNs,
    pub t1: RelNs,
    /// Spacing of depth frames. Values below 1 are treated as 1, and the step is widened when the
    /// window would otherwise hold more than [`MAX_REPLAY_FRAMES`] frames.
    pub step_ns: i64,
    /// Price levels per side in each depth frame.
    pub levels: u32,
    /// Cap on returned trades; beyond it only the largest by quantity are kept.
    pub max_trades: u32,
}

struct WindowTrade {
    ts: RelNs,
    tick: i64,
    qty: f64,
    side: Option<Side>,
}

/// Book, top of book and trades for `[t0, t1]`, built in one pass over `events`.
///
/// * `bbo` starts with the state in force at `t0` (when the book was non-empty) and then holds
///   every change inside the window. If that is more than four entries per depth frame it is
///   reduced to the first entry plus the last state of each step and `bbo_downsampled` is set.
/// * `depth` holds one frame at `t0, t0 + step, ...` up to `t1`, each with the book as it stood
///   after every batch at or before the frame time.
/// * `trades` holds the trades inside the window in time order; when there are more than
///   `max_trades`, the largest by quantity are kept and `trades_truncated` is set.
pub fn replay_window(
    events: &[Event],
    instrument: &InstrumentMeta,
    origin_ns: i64,
    req: &ReplayRequest,
) -> ReplayWindow {
    let (t0, t1) = (req.t0, req.t1);
    let mut step = req.step_ns.max(1);
    let mut window = ReplayWindow {
        t0,
        t1,
        step_ns: step,
        levels: req.levels,
        bbo: BboSeries::default(),
        bbo_downsampled: false,
        trades: TradeSeries::default(),
        trades_truncated: false,
        depth: Vec::new(),
    };
    if t1 < t0 {
        return window;
    }
    let span = t1.saturating_sub(t0);
    if span / step >= MAX_REPLAY_FRAMES {
        // Smallest step for which `span / step + 1` frames fit under the cap.
        step = span / MAX_REPLAY_FRAMES + 1;
        window.step_ns = step;
    }
    let frame_count = (span / step + 1) as usize;
    let levels = req.levels as usize;

    let mut book = L2Book::new();
    let mut bbo = BboSeries::default();
    // State after the last completed batch, and the state of the last `bbo` entry.
    let mut current = Bbo::default();
    let mut recorded = Bbo::default();
    let mut initial_done = false;
    let mut trades: Vec<WindowTrade> = Vec::new();
    let mut frames: Vec<DepthFrame> = Vec::with_capacity(frame_count);
    let mut next_frame = Some(t0);

    let mut i = 0usize;
    let n = events.len();
    while i < n {
        if !has(&events[i], EXCH_EVENT) {
            i += 1;
            continue;
        }
        let abs_ts = events[i].exch_ts;
        let ts = abs_ts.saturating_sub(origin_ns);
        if ts > t1 {
            break;
        }

        // Frames strictly before this batch see the book as it was before the batch.
        while let Some(frame_ts) = next_frame
            && frame_ts < ts
        {
            frames.push(DepthFrame {
                ts: frame_ts,
                bids: book.top_bids(levels),
                asks: book.top_asks(levels),
            });
            next_frame = frame_ts.checked_add(step).filter(|t| *t <= t1);
        }
        if ts > t0 && !initial_done {
            initial_done = true;
            if !current.is_empty() {
                push_bbo(&mut bbo, t0, &current);
            }
            recorded = current;
        }

        // Apply the whole batch before looking at the book again.
        while i < n {
            let ev = &events[i];
            if !has(ev, EXCH_EVENT) {
                i += 1;
                continue;
            }
            if ev.exch_ts != abs_ts {
                break;
            }
            if kind(ev) == TRADE_EVENT {
                if ts >= t0 {
                    trades.push(WindowTrade {
                        ts,
                        tick: instrument.tick(ev.px),
                        qty: ev.qty,
                        side: side_of(ev),
                    });
                }
            } else {
                book.apply(ev, instrument);
            }
            i += 1;
        }

        current = book.bbo();
        if ts >= t0 {
            if !initial_done {
                // A batch exactly at `t0` defines the state in force at `t0`.
                initial_done = true;
                if !current.is_empty() {
                    push_bbo(&mut bbo, ts, &current);
                }
                recorded = current;
            } else if current != recorded {
                push_bbo(&mut bbo, ts, &current);
                recorded = current;
            }
        }
    }

    if !initial_done && !current.is_empty() {
        push_bbo(&mut bbo, t0, &current);
    }
    while let Some(frame_ts) = next_frame {
        frames.push(DepthFrame {
            ts: frame_ts,
            bids: book.top_bids(levels),
            asks: book.top_asks(levels),
        });
        next_frame = frame_ts.checked_add(step).filter(|t| *t <= t1);
    }

    if bbo.len() > 4 * frames.len() {
        bbo = downsample_bbo(&bbo, t0, step);
        window.bbo_downsampled = true;
    }

    let max_trades = req.max_trades as usize;
    if trades.len() > max_trades {
        window.trades_truncated = true;
        let mut order: Vec<usize> = (0..trades.len()).collect();
        // Largest first; equal sizes keep the earlier trade so the choice is deterministic.
        order.sort_by(|a, b| trades[*b].qty.total_cmp(&trades[*a].qty).then(a.cmp(b)));
        order.truncate(max_trades);
        order.sort_unstable();
        trades = {
            let mut slots: Vec<Option<WindowTrade>> = trades.into_iter().map(Some).collect();
            order.iter().filter_map(|i| slots[*i].take()).collect()
        };
    }
    for t in trades {
        window.trades.ts.push(t.ts);
        window.trades.price_tick.push(t.tick);
        window.trades.qty.push(t.qty);
        window.trades.side.push(t.side);
    }

    window.bbo = bbo;
    window.depth = frames;
    window
}

/// Keeps the first entry and, for each step, the last entry that began inside it. Entries keep
/// their own timestamps and values; nothing is averaged or interpolated.
fn downsample_bbo(bbo: &BboSeries, t0: RelNs, step: i64) -> BboSeries {
    let mut out = BboSeries::default();
    let copy = |out: &mut BboSeries, i: usize| {
        out.ts.push(bbo.ts[i]);
        out.bid_tick.push(bbo.bid_tick[i]);
        out.ask_tick.push(bbo.ask_tick[i]);
        out.bid_qty.push(bbo.bid_qty[i]);
        out.ask_qty.push(bbo.ask_qty[i]);
    };
    let n = bbo.len();
    for i in 0..n {
        let bucket = (bbo.ts[i] - t0) / step;
        let last_of_bucket = i + 1 == n || (bbo.ts[i + 1] - t0) / step != bucket;
        if i == 0 || last_of_bucket {
            copy(&mut out, i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use hftbacktest::types::{BUY_EVENT, LOCAL_EVENT, SELL_EVENT};

    use super::*;
    use crate::make_event;

    const ORIGIN: i64 = 1_700_000_000_000_000_000;

    fn instrument() -> InstrumentMeta {
        InstrumentMeta {
            tick_size: 0.5,
            lot_size: 0.001,
            contract_size: 1.0,
        }
    }

    fn ev(flags: u64, rel: i64, tick: i64, lots: i64) -> Event {
        make_event(
            flags | EXCH_EVENT | LOCAL_EVENT,
            ORIGIN + rel,
            ORIGIN + rel + 1_000,
            tick as f64 * 0.5,
            lots as f64 * 0.001,
        )
    }

    fn bid(rel: i64, tick: i64, lots: i64) -> Event {
        ev(DEPTH_EVENT | BUY_EVENT, rel, tick, lots)
    }

    fn ask(rel: i64, tick: i64, lots: i64) -> Event {
        ev(DEPTH_EVENT | SELL_EVENT, rel, tick, lots)
    }

    fn q(lots: i64) -> f64 {
        lots as f64 * 0.001
    }

    /// Snapshot at 0 (bids 100/99, asks 102/103), then:
    /// * 100: the bid moves up from 100 to 101 in one batch (remove 100, add 101),
    /// * 150: a buy trade of 3 lots at 102 and the ask reduction it caused,
    /// * 250: the ask at 102 is removed.
    fn sequence() -> Vec<Event> {
        vec![
            ev(DEPTH_CLEAR_EVENT, 0, 0, 0),
            ev(DEPTH_SNAPSHOT_EVENT | BUY_EVENT, 0, 100, 10),
            ev(DEPTH_SNAPSHOT_EVENT | BUY_EVENT, 0, 99, 20),
            ev(DEPTH_SNAPSHOT_EVENT | SELL_EVENT, 0, 102, 7),
            ev(DEPTH_SNAPSHOT_EVENT | SELL_EVENT, 0, 103, 9),
            bid(100, 100, 0),
            bid(100, 101, 5),
            ev(TRADE_EVENT | BUY_EVENT, 150, 102, 3),
            ask(150, 102, 4),
            ask(250, 102, 0),
        ]
    }

    #[test]
    fn book_applies_depth_snapshot_and_clear_rows() {
        let inst = instrument();
        let mut book = L2Book::new();
        for e in sequence().iter().take(5) {
            book.apply(e, &inst);
        }
        assert_eq!(book.best_bid(), Some((100, q(10))));
        assert_eq!(book.best_ask(), Some((102, q(7))));
        assert_eq!(book.top_bids(5), vec![(100, q(10)), (99, q(20))]);
        assert_eq!(book.top_asks(1), vec![(102, q(7))]);
        assert!(!book.is_crossed());

        // A trade row is not a book row.
        assert!(!book.apply(&ev(TRADE_EVENT | BUY_EVENT, 1, 102, 1), &inst));
        assert_eq!(book.ask_qty(102), Some(q(7)));

        // Locked book counts as crossed.
        book.apply(&bid(2, 102, 1), &inst);
        assert!(book.is_crossed());

        // Bid clear removes bids at or above its price only.
        book.apply(&ev(DEPTH_CLEAR_EVENT | BUY_EVENT, 3, 100, 0), &inst);
        assert_eq!(book.top_bids(5), vec![(99, q(20))]);
        assert_eq!(book.ask_levels(), 2);

        // Ask clear removes asks at or below its price only.
        book.apply(&ev(DEPTH_CLEAR_EVENT | SELL_EVENT, 4, 102, 0), &inst);
        assert_eq!(book.top_asks(5), vec![(103, q(9))]);

        // A clear without a side empties the book.
        book.apply(&ev(DEPTH_CLEAR_EVENT, 5, 0, 0), &inst);
        assert_eq!(book.bbo(), Bbo::default());
        assert_eq!((book.bid_levels(), book.ask_levels()), (0, 0));
    }

    #[test]
    fn bbo_series_records_only_completed_batches() {
        let series = bbo_series(&sequence(), &instrument(), ORIGIN);
        assert_eq!(series.ts, vec![0, 100, 150, 250]);
        // The batch at 100 removes bid 100 before adding bid 101; the transient best bid of 99
        // must never appear.
        assert_eq!(
            series.bid_tick,
            vec![Some(100), Some(101), Some(101), Some(101)]
        );
        assert_eq!(
            series.ask_tick,
            vec![Some(102), Some(102), Some(102), Some(103)]
        );
        assert_eq!(series.bid_qty, vec![q(10), q(5), q(5), q(5)]);
        assert_eq!(series.ask_qty, vec![q(7), q(7), q(4), q(9)]);
        assert_eq!(series.mid_tick_at(120), Some(101.5));
    }

    #[test]
    fn bbo_series_ignores_rows_without_the_exchange_flag() {
        let mut events = sequence();
        // A local-only copy of a depth row must not be applied a second time or out of order.
        let mut stray = bid(300, 150, 1);
        stray.ev &= !EXCH_EVENT;
        events.insert(6, stray);
        let series = bbo_series(&events, &instrument(), ORIGIN);
        assert_eq!(series.ts, vec![0, 100, 150, 250]);
        assert!(series.bid_tick.iter().all(|t| *t != Some(150)));
    }

    #[test]
    fn replay_window_returns_frames_bbo_and_trades() {
        let req = ReplayRequest {
            t0: 50,
            t1: 250,
            step_ns: 50,
            levels: 1,
            max_trades: 10,
        };
        let w = replay_window(&sequence(), &instrument(), ORIGIN, &req);
        assert_eq!((w.t0, w.t1, w.step_ns, w.levels), (50, 250, 50, 1));

        // The first entry is the state in force at t0, stamped t0.
        assert_eq!(w.bbo.ts, vec![50, 100, 150, 250]);
        assert_eq!(
            w.bbo.bid_tick,
            vec![Some(100), Some(101), Some(101), Some(101)]
        );
        assert_eq!(
            w.bbo.ask_tick,
            vec![Some(102), Some(102), Some(102), Some(103)]
        );
        assert_eq!(w.bbo.ask_qty, vec![q(7), q(7), q(4), q(9)]);
        assert!(!w.bbo_downsampled);
        // The intra-batch best bid of 99 at t=100 is never exposed, in the BBO or in a frame.
        assert!(w.bbo.bid_tick.iter().all(|t| *t != Some(99)));

        let frame_ts: Vec<i64> = w.depth.iter().map(|f| f.ts).collect();
        assert_eq!(frame_ts, vec![50, 100, 150, 200, 250]);
        assert_eq!(w.depth[0].bids, vec![(100, q(10))]);
        assert_eq!(w.depth[0].asks, vec![(102, q(7))]);
        // A frame at a batch's own timestamp shows the book after that batch.
        assert_eq!(w.depth[1].bids, vec![(101, q(5))]);
        assert_eq!(w.depth[2].asks, vec![(102, q(4))]);
        assert_eq!(w.depth[3].asks, vec![(102, q(4))]);
        assert_eq!(w.depth[4].asks, vec![(103, q(9))]);

        assert_eq!(w.trades.ts, vec![150]);
        assert_eq!(w.trades.price_tick, vec![102]);
        assert_eq!(w.trades.qty, vec![q(3)]);
        assert_eq!(w.trades.side, vec![Some(Side::Buy)]);
        assert!(!w.trades_truncated);
    }

    #[test]
    fn replay_window_frames_hold_requested_levels_and_stop_at_t1() {
        let req = ReplayRequest {
            t0: 0,
            t1: 120,
            step_ns: 100,
            levels: 5,
            max_trades: 10,
        };
        let w = replay_window(&sequence(), &instrument(), ORIGIN, &req);
        assert_eq!(w.depth.len(), 2);
        assert_eq!(w.depth[0].ts, 0);
        assert_eq!(w.depth[0].bids, vec![(100, q(10)), (99, q(20))]);
        assert_eq!(w.depth[0].asks, vec![(102, q(7)), (103, q(9))]);
        assert_eq!(w.depth[1].bids, vec![(101, q(5)), (99, q(20))]);
        // Events after t1 are not part of the window.
        assert_eq!(w.bbo.ts, vec![0, 100]);
        assert!(w.trades.ts.is_empty());

        // A window after the last event still reports the state in force.
        let late = ReplayRequest {
            t0: 1_000,
            t1: 1_100,
            step_ns: 100,
            levels: 1,
            max_trades: 10,
        };
        let w = replay_window(&sequence(), &instrument(), ORIGIN, &late);
        assert_eq!(w.bbo.ts, vec![1_000]);
        assert_eq!(w.bbo.bid_tick, vec![Some(101)]);
        assert_eq!(w.bbo.ask_tick, vec![Some(103)]);
        assert_eq!(w.depth.len(), 2);
        assert_eq!(w.depth[1].asks, vec![(103, q(9))]);

        // An inverted window is empty.
        let inverted = ReplayRequest {
            t0: 10,
            t1: 5,
            step_ns: 1,
            levels: 1,
            max_trades: 1,
        };
        let w = replay_window(&sequence(), &instrument(), ORIGIN, &inverted);
        assert!(w.depth.is_empty() && w.bbo.is_empty() && w.trades.ts.is_empty());
    }

    #[test]
    fn replay_window_keeps_the_largest_trades_in_time_order() {
        let mut events = sequence();
        for (k, lots) in [4i64, 9, 2, 9, 6].iter().enumerate() {
            events.push(ev(
                TRADE_EVENT | SELL_EVENT,
                300 + k as i64 * 10,
                101,
                *lots,
            ));
        }
        events.push(ev(TRADE_EVENT, 400, 101, 1));
        let req = ReplayRequest {
            t0: 0,
            t1: 1_000,
            step_ns: 500,
            levels: 1,
            max_trades: 3,
        };
        let w = replay_window(&events, &instrument(), ORIGIN, &req);
        assert!(w.trades_truncated);
        // Seven trades in the window; the three largest are 9, 9 and 6 lots.
        assert_eq!(w.trades.ts, vec![310, 330, 340]);
        assert_eq!(w.trades.qty, vec![q(9), q(9), q(6)]);
        assert_eq!(w.trades.side, vec![Some(Side::Sell); 3]);

        let all = ReplayRequest {
            max_trades: 7,
            ..req
        };
        let w = replay_window(&events, &instrument(), ORIGIN, &all);
        assert!(!w.trades_truncated);
        assert_eq!(w.trades.ts.len(), 7);
        assert_eq!(w.trades.side[6], None);
    }

    #[test]
    fn replay_window_downsamples_dense_bbo_to_the_last_state_per_step() {
        // One frame step of 1000 ns holding 20 top-of-book changes.
        let mut events = vec![bid(0, 100, 1), ask(0, 200, 1)];
        for k in 1..=20i64 {
            events.push(bid(k * 40, 100, 1 + k));
        }
        let req = ReplayRequest {
            t0: 0,
            t1: 1_999,
            step_ns: 1_000,
            levels: 1,
            max_trades: 0,
        };
        let w = replay_window(&events, &instrument(), ORIGIN, &req);
        assert_eq!(w.depth.len(), 2);
        assert!(w.bbo_downsampled);
        // First entry kept, then the last state that began in the only populated step.
        assert_eq!(w.bbo.ts, vec![0, 800]);
        assert_eq!(w.bbo.bid_qty, vec![q(1), q(21)]);
        assert_eq!(w.depth[1].bids, vec![(100, q(21))]);
    }

    #[test]
    fn replay_window_caps_the_number_of_frames() {
        let req = ReplayRequest {
            t0: 0,
            t1: 60_000_000_000,
            step_ns: 1,
            levels: 1,
            max_trades: 0,
        };
        let w = replay_window(&sequence(), &instrument(), ORIGIN, &req);
        assert!(w.depth.len() as i64 <= MAX_REPLAY_FRAMES);
        assert!(w.step_ns > 1);
        assert_eq!(w.depth[1].ts - w.depth[0].ts, w.step_ns);
        assert_eq!(w.depth[0].ts, 0);
    }
}
