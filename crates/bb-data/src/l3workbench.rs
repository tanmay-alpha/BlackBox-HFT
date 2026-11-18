//! Bounded, local-private ITCH workbench. Reconstruction and L2 normalization remain the
//! existing ITCH engine's responsibility; this module records its accepted effects.
//! Checkpoints contain only flat resting-order rows in FIFO order, never whole engine clones.

use crate::{
    Event,
    itch::{
        self, FrameSink, ItchManifest, ItchOptions, Reconstructor, SourceFile, Stop,
        book::{Change, OrderBooks, OrderView, QueuedOrder},
        decode::{self, AddOrder, FrameReader, Header, Message, Price4, Side, Stock},
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
};

pub const MAX_FRAMES: u64 = 100_000;
pub const MAX_CHECKPOINT_INTERVAL: usize = 4_096;
// At most ~64 MiB of flat order rows (excluding timestamp string storage).
const MAX_CHECKPOINT_ORDERS: usize = 1_000_000;
const MIN_SNAPSHOT_SPACING: usize = 32;
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
/// Event-detail bounds: depth levels per side and listed orders per level.
pub const MAX_DETAIL_LEVELS: usize = 10;
pub const MAX_DETAIL_ORDERS: usize = 100;
/// Same-timestamp events listed in an event detail (the most recent ones up to the event).
const MAX_SAME_TS_EVENTS: usize = 32;
const EXAMPLE_MIN_QUEUE: u32 = 6;
const EXAMPLE_MIN_SAME_TS: usize = 3;
/// Curated examples start once each side of the book holds this many resting orders.
const EXAMPLE_WARM_ORDERS: u64 = 100;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    pub event_index: u64,
    pub message_type: String,
    pub ts_ns: Option<String>,
    pub order_ref: Option<String>,
    pub new_order_ref: Option<String>,
    pub side: Option<String>,
    pub price4: Option<u32>,
    pub shares: Option<u64>,
    pub best_bid_price4: Option<u32>,
    pub best_ask_price4: Option<u32>,
    pub applied: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub event_index: u64,
    pub ts_ns: Option<String>,
    pub live_orders: u64,
    pub bid_shares: u64,
    pub ask_shares: u64,
    pub best_bid_price4: Option<u32>,
    pub best_ask_price4: Option<u32>,
    pub order_count_imbalance: Option<f64>,
    pub signed_added_shares: i64,
    pub signed_cancelled_shares: i64,
    pub signed_executed_shares: i64,
    pub touch_depleted_shares: u64,
    pub touch_replenished_shares: u64,
    pub lifetime_ns: Option<u64>,
    pub executed_queue_position: Option<u32>,
    pub cancel_location: Option<i8>,
    pub order_identity_observations: u64,
    pub touch_queue_observations: u64,
    pub accepted_add: bool,
    pub accepted_cancel: bool,
    pub level_key: Option<(u8, u32)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookSnapshot {
    pub event_index: u64,
    pub ts_ns: Option<String>,
    pub levels: usize,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookLevel {
    pub price4: u32,
    pub shares: u64,
    pub order_count: u32,
    pub orders: Vec<BookOrder>,
    pub orders_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookOrder {
    pub order_ref: String,
    pub shares: u64,
    pub queue_position: u32,
    pub shares_ahead: u64,
    pub entered_ns: Option<String>,
    pub entered_event_index: u64,
}

/// One resting order in a compact FIFO depth view; its queue position is its array index + 1.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueSlot {
    pub order_ref: String,
    pub shares: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DepthLevel {
    pub price4: u32,
    pub shares: u64,
    pub order_count: u32,
    pub orders: Vec<QueueSlot>,
    pub orders_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Depth {
    pub bids: Vec<DepthLevel>,
    pub asks: Vec<DepthLevel>,
}

/// A price level the event changed, complete before and after the event. `role` is `order`, or
/// for a replace `replaced` (same price), `replaced_from` and `replaced_to`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AffectedLevel {
    pub side: String,
    pub price4: u32,
    pub role: String,
    pub before: BookLevel,
    pub after: BookLevel,
}

/// One resting order's place in its price queue at one instant. `age_ns` is the event timestamp
/// minus the order's accepted add/replace timestamp.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderPosition {
    pub order_ref: String,
    pub side: String,
    pub price4: u32,
    pub shares: u64,
    pub queue_position: u32,
    pub level_orders: u32,
    pub shares_ahead: u64,
    pub level_shares: u64,
    pub entered_ns: Option<String>,
    pub entered_event_index: u64,
    pub age_ns: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LevelChange {
    pub side: String,
    pub price4: u32,
    pub before_shares: u64,
    pub after_shares: u64,
    pub before_orders: u32,
    pub after_orders: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SameTimestampEvent {
    pub event_index: u64,
    pub message_type: String,
    pub applied: String,
    pub level_changes: Vec<LevelChange>,
}

/// Events up to and including the selected one that share its source timestamp, in source order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SameTimestamp {
    pub ts_ns: Option<String>,
    /// One-based position of the selected event among them.
    pub position: u32,
    pub first_event_index: u64,
    pub events: Vec<SameTimestampEvent>,
    /// Earlier same-timestamp events exist beyond the listed ones.
    pub events_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventDetail {
    pub event: EventRecord,
    /// The `before` state is after this frame (`None`: before the first frame).
    pub before_event_index: Option<u64>,
    pub levels: usize,
    pub orders_per_level: usize,
    pub before: Depth,
    pub after: Depth,
    pub affected: Vec<AffectedLevel>,
    /// The resting order an E/C/X/D/U message referenced, before the event.
    pub order_before: Option<OrderPosition>,
    /// The added, replacing, or still-resting referenced order after the event.
    pub order_after: Option<OrderPosition>,
    /// Completed lifetime when this event removed or replaced the referenced order.
    pub lifetime_ns: Option<String>,
    pub same_timestamp: SameTimestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CuratedExample {
    pub id: String,
    pub title: String,
    pub event_index: u64,
    pub rule: String,
}

#[derive(Clone, Copy, Debug)]
struct Birth {
    ts: Option<u64>,
    event_index: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RestingOrder {
    order_ref: u64,
    side: u8,
    price4: u32,
    shares: u32,
    entered_ns: Option<String>,
    entered_event_index: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Checkpoint {
    /// State after this many physical source frames, not after this many filtered events.
    frame_count: usize,
    orders: Vec<RestingOrder>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceFrame {
    bytes: Vec<u8>,
    /// Apply only original accepted selected-symbol book effects during checkpoint replay.
    /// This also preserves rejections caused by orders in other symbols.
    accepted_book: bool,
}

#[derive(Debug)]
pub struct Workbench {
    pub manifest: ItchManifest,
    pub symbol: String,
    pub events: Vec<EventRecord>,
    pub observations: Vec<Observation>,
    pub l2: Vec<Event>,
    locate: u16,
    stock: [u8; 8],
    checkpoint_interval: usize,
    checkpoints: Vec<Checkpoint>,
    source_frames: Vec<SourceFrame>,
}

impl Workbench {
    pub fn from_bytes(
        name: &str,
        bytes: &[u8],
        symbol: &str,
        checkpoint_interval: usize,
    ) -> Result<Self, String> {
        let mut sink = Recorder::new(symbol, MAX_FRAMES, checkpoint_interval)?;
        let mut frames = FrameReader::new(bytes);
        let stop = itch::run(&mut frames, &mut sink).map_err(|e| e.to_string())?;
        let source = SourceFile {
            name: name.into(),
            compression: "none".into(),
            bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        };
        sink.finish(source, stop, frames.offset())
    }

    /// Reads plain or gzip input through the existing hashing/decompression runner.
    /// A larger requested prefix is capped at MAX_FRAMES; zero is rejected.
    pub fn from_file(
        path: &Path,
        symbol: &str,
        max_messages: u64,
        checkpoint_interval: usize,
    ) -> Result<Self, String> {
        if max_messages == 0 {
            return Err("max_messages must be positive".into());
        }
        let mut sink = Recorder::new(symbol, max_messages.min(MAX_FRAMES), checkpoint_interval)?;
        let (source, stop, decoded) = itch::run_file(path, &mut sink).map_err(|e| e.to_string())?;
        sink.finish(source, stop, decoded)
    }

    /// Returns as-of physical-frame state, including frames omitted by the symbol filter.
    /// Spacing below 32 is rounded up to avoid per-event full snapshots; forward replay is
    /// always bounded by 4096 source frames.
    pub fn book(
        &self,
        event_index: u64,
        levels: usize,
        orders_per_level: usize,
    ) -> Result<BookSnapshot, String> {
        if !(1..=20).contains(&levels) || !(1..=100).contains(&orders_per_level) {
            return Err("levels must be 1..20 and orders_per_level must be 1..100".into());
        }
        let index = self.frame_index(event_index)?;
        let (books, births) = self.replay(index + 1)?;
        Ok(BookSnapshot {
            event_index,
            ts_ns: frame_ts(&self.source_frames[index].bytes).map(|t| t.to_string()),
            levels,
            bids: self.book_side(&books, &births, Side::Buy, levels, orders_per_level),
            asks: self.book_side(&books, &births, Side::Sell, levels, orders_per_level),
        })
    }

    fn frame_index(&self, event_index: u64) -> Result<usize, String> {
        usize::try_from(event_index)
            .ok()
            .filter(|&index| index < self.source_frames.len())
            .ok_or_else(|| "event index out of range".into())
    }

    /// Selected-symbol state after the first `frames` physical source frames (0: before any).
    /// Reads no frame at or beyond `frames`.
    fn replay(&self, frames: usize) -> Result<(OrderBooks, HashMap<u64, Birth>), String> {
        let cp_index = self
            .checkpoints
            .partition_point(|c| c.frame_count <= frames)
            - 1;
        let cp = &self.checkpoints[cp_index];
        let mut books = OrderBooks::new();
        let mut births = HashMap::with_capacity(cp.orders.len());
        for order in &cp.orders {
            let side = if order.side == b'B' {
                Side::Buy
            } else {
                Side::Sell
            };
            books.apply(&Message::AddOrder(AddOrder {
                header: Header {
                    stock_locate: self.locate,
                    tracking_number: 0,
                    timestamp_ns: 0,
                },
                order_ref: order.order_ref,
                side,
                shares: order.shares,
                stock: Stock(&self.stock),
                price: Price4(order.price4),
                attribution: None,
            }));
            births.insert(
                order.order_ref,
                Birth {
                    ts: order.entered_ns.as_deref().and_then(|s| s.parse().ok()),
                    event_index: order.entered_event_index,
                },
            );
        }
        for (offset, source) in self.source_frames[cp.frame_count..frames]
            .iter()
            .enumerate()
        {
            if source.accepted_book {
                let message = decode::decode(&source.bytes).map_err(|e| e.to_string())?;
                books.apply(&message);
                update_births(
                    &mut births,
                    &books,
                    &message,
                    (cp.frame_count + offset) as u64,
                );
            }
        }
        Ok((books, births))
    }

    fn book_side(
        &self,
        books: &OrderBooks,
        births: &HashMap<u64, Birth>,
        side: Side,
        levels: usize,
        orders_per_level: usize,
    ) -> Vec<BookLevel> {
        books
            .top_levels(self.locate, side, levels)
            .into_iter()
            .map(|level| {
                let queue =
                    books.level_queue_prefix(self.locate, side, level.price, orders_per_level);
                let mut ahead = 0;
                let orders = queue
                    .iter()
                    .enumerate()
                    .map(|(i, order)| {
                        let row = book_order(births, order, i, ahead);
                        ahead += u64::from(order.shares);
                        row
                    })
                    .collect();
                BookLevel {
                    price4: level.price.0,
                    shares: level.shares,
                    order_count: level.orders,
                    orders,
                    orders_truncated: level.orders as usize > orders_per_level,
                }
            })
            .collect()
    }

    fn depth(&self, books: &OrderBooks, levels: usize, orders_per_level: usize) -> Depth {
        let side = |side| {
            books
                .top_levels(self.locate, side, levels)
                .into_iter()
                .map(|level| DepthLevel {
                    price4: level.price.0,
                    shares: level.shares,
                    order_count: level.orders,
                    orders: books
                        .level_queue_prefix(self.locate, side, level.price, orders_per_level)
                        .into_iter()
                        .map(|o| QueueSlot {
                            order_ref: o.order_ref.to_string(),
                            shares: u64::from(o.shares),
                        })
                        .collect(),
                    orders_truncated: level.orders as usize > orders_per_level,
                })
                .collect()
        };
        Depth {
            bids: side(Side::Buy),
            asks: side(Side::Sell),
        }
    }

    /// One complete price level: the first `limit` FIFO orders plus every order in `include`,
    /// with exact queue positions and shares ahead computed over the whole queue.
    fn level_detail(
        &self,
        books: &OrderBooks,
        births: &HashMap<u64, Birth>,
        side: Side,
        price: Price4,
        limit: usize,
        include: &[u64],
    ) -> BookLevel {
        let queue = books.level_queue(self.locate, side, price);
        let mut ahead = 0;
        let mut orders = Vec::with_capacity(queue.len().min(limit + include.len()));
        for (i, order) in queue.iter().enumerate() {
            if i < limit || include.contains(&order.order_ref) {
                orders.push(book_order(births, order, i, ahead));
            }
            ahead += u64::from(order.shares);
        }
        BookLevel {
            price4: price.0,
            shares: ahead,
            order_count: queue.len() as u32,
            orders_truncated: orders.len() < queue.len(),
            orders,
        }
    }

    fn level_totals(&self, books: &OrderBooks, side: Side, price: Price4) -> (u64, u32) {
        books
            .level_queue_prefix(self.locate, side, price, usize::MAX)
            .iter()
            .fold((0, 0), |(shares, orders), o| {
                (shares + u64::from(o.shares), orders + 1)
            })
    }

    fn order_position(
        &self,
        books: &OrderBooks,
        births: &HashMap<u64, Birth>,
        order_ref: u64,
        at_ns: Option<u64>,
    ) -> Option<OrderPosition> {
        let queue = books.observed_queue(order_ref)?;
        let birth = births.get(&order_ref).copied();
        let entered = birth.and_then(|b| b.ts);
        Some(OrderPosition {
            order_ref: order_ref.to_string(),
            side: side_name(queue.order.side).into(),
            price4: queue.order.price.0,
            shares: u64::from(queue.order.shares),
            queue_position: queue.ahead.len() as u32 + 1,
            level_orders: queue.level_orders,
            shares_ahead: queue.shares_ahead,
            level_shares: queue.level_shares,
            entered_ns: entered.map(|t| t.to_string()),
            entered_event_index: birth.map_or(0, |b| b.event_index),
            age_ns: at_ns
                .zip(entered)
                .and_then(|(at, entered)| at.checked_sub(entered))
                .map(|ns| ns.to_string()),
        })
    }

    /// The book immediately before (after frame `i - 1`) and after (frame `i`) one selected-symbol
    /// event, from a single forward replay that reads no frame beyond `i`: a later message,
    /// including a later message with the same timestamp, never affects the result.
    /// `Ok(None)` when the frame belongs to another symbol.
    pub fn event_detail(
        &self,
        event_index: u64,
        levels: usize,
        orders_per_level: usize,
    ) -> Result<Option<EventDetail>, String> {
        if !(1..=MAX_DETAIL_LEVELS).contains(&levels)
            || !(1..=MAX_DETAIL_ORDERS).contains(&orders_per_level)
        {
            return Err(format!(
                "levels must be 1..{MAX_DETAIL_LEVELS} and orders_per_level must be 1..{MAX_DETAIL_ORDERS}"
            ));
        }
        let index = self.frame_index(event_index)?;
        let Ok(position) = self
            .events
            .binary_search_by_key(&event_index, |e| e.event_index)
        else {
            return Ok(None);
        };
        let event = &self.events[position];
        let ts = event.ts_ns.as_deref().and_then(|s| s.parse::<u64>().ok());
        let mut first = position;
        if event.ts_ns.is_some() {
            while first > 0 && self.events[first - 1].ts_ns == event.ts_ns {
                first -= 1;
            }
        }
        let listed_from = first.max((position + 1).saturating_sub(MAX_SAME_TS_EVENTS));
        let listed = &self.events[listed_from..=position];
        let start = listed[0].event_index as usize;
        let (mut books, mut births) = self.replay(start)?;
        let mut same_ts = Vec::with_capacity(listed.len());
        let mut pending = listed.iter().peekable();
        let parse_ref = |s: &Option<String>| s.as_deref().and_then(|s| s.parse::<u64>().ok());
        let kind = event.message_type.as_str();
        let old_ref = parse_ref(&event.order_ref);
        let new_ref = parse_ref(&event.new_order_ref);
        let mut detail = None;
        for frame in start..=index {
            let source = &self.source_frames[frame];
            let listed_event = pending.next_if(|e| e.event_index == frame as u64);
            let message = if source.accepted_book {
                Some(decode::decode(&source.bytes).map_err(|e| e.to_string())?)
            } else {
                None
            };
            let touched = message
                .as_ref()
                .map(|m| touched_levels(&books, m))
                .unwrap_or_default();
            let pre_totals: Vec<_> = touched
                .iter()
                .map(|&(side, price, _)| self.level_totals(&books, side, price))
                .collect();
            let mut before = None;
            if frame == index {
                let include: Vec<u64> = old_ref.into_iter().chain(new_ref).collect();
                let order_before = if matches!(kind, "E" | "C" | "X" | "D" | "U") {
                    old_ref.and_then(|id| self.order_position(&books, &births, id, ts))
                } else {
                    None
                };
                let levels_before: Vec<_> = touched
                    .iter()
                    .map(|&(side, price, _)| {
                        self.level_detail(&books, &births, side, price, orders_per_level, &include)
                    })
                    .collect();
                before = Some((
                    self.depth(&books, levels, orders_per_level),
                    levels_before,
                    order_before,
                    include,
                ));
            }
            if let Some(message) = &message {
                books.apply(message);
                update_births(&mut births, &books, message, frame as u64);
            }
            if let Some(listed_event) = listed_event {
                same_ts.push(SameTimestampEvent {
                    event_index: listed_event.event_index,
                    message_type: listed_event.message_type.clone(),
                    applied: listed_event.applied.clone(),
                    level_changes: touched
                        .iter()
                        .zip(&pre_totals)
                        .map(|(&(side, price, _), &(before_shares, before_orders))| {
                            let (after_shares, after_orders) =
                                self.level_totals(&books, side, price);
                            LevelChange {
                                side: side_name(side).into(),
                                price4: price.0,
                                before_shares,
                                after_shares,
                                before_orders,
                                after_orders,
                            }
                        })
                        .collect(),
                });
            }
            if let Some((depth_before, levels_before, order_before, include)) = before {
                let accepted = message.is_some();
                let after_ref = match kind {
                    "A" | "F" if accepted => old_ref,
                    "U" if accepted => new_ref,
                    "E" | "C" | "X" | "D" | "U" => old_ref,
                    _ => None,
                };
                let order_after =
                    after_ref.and_then(|id| self.order_position(&books, &births, id, ts));
                let terminal = accepted
                    && order_before.is_some()
                    && (kind == "U" || old_ref.is_some_and(|id| books.order(id).is_none()));
                let lifetime_ns = order_before
                    .as_ref()
                    .filter(|_| terminal)
                    .and_then(|o| ts?.checked_sub(o.entered_ns.as_deref()?.parse().ok()?))
                    .map(|ns| ns.to_string());
                let affected = touched
                    .iter()
                    .zip(levels_before)
                    .map(|(&(side, price, role), before)| AffectedLevel {
                        side: side_name(side).into(),
                        price4: price.0,
                        role: role.into(),
                        before,
                        after: self.level_detail(
                            &books,
                            &births,
                            side,
                            price,
                            orders_per_level,
                            &include,
                        ),
                    })
                    .collect();
                detail = Some(EventDetail {
                    event: event.clone(),
                    before_event_index: event_index.checked_sub(1),
                    levels,
                    orders_per_level,
                    before: depth_before,
                    after: self.depth(&books, levels, orders_per_level),
                    affected,
                    order_before,
                    order_after,
                    lifetime_ns,
                    same_timestamp: SameTimestamp {
                        ts_ns: event.ts_ns.clone(),
                        position: (position - first + 1) as u32,
                        first_event_index: self.events[first].event_index,
                        events: Vec::new(),
                        events_truncated: listed_from > first,
                    },
                });
            }
        }
        Ok(detail.map(|mut d| {
            d.same_timestamp.events = same_ts;
            d
        }))
    }

    /// Deterministic illustrative events, each the earliest accepted event in source order with
    /// the property its `rule` states. Selection reads the whole session; the detail view of each
    /// example remains causal.
    pub fn curated_examples(&self) -> Result<Vec<CuratedExample>, String> {
        let mut books = OrderBooks::new();
        let mut found: [Option<u64>; 4] = [None; 4];
        let mut warm_from = None;
        for (index, source) in self.source_frames.iter().enumerate() {
            if found.iter().all(Option::is_some) {
                break;
            }
            if !source.accepted_book {
                continue;
            }
            let message = decode::decode(&source.bytes).map_err(|e| e.to_string())?;
            if warm_from.is_none() {
                let (_, bids) = books.side_totals(self.locate, Side::Buy);
                let (_, asks) = books.side_totals(self.locate, Side::Sell);
                if bids.min(asks) >= EXAMPLE_WARM_ORDERS {
                    warm_from = Some(index as u64);
                } else {
                    books.apply(&message);
                    continue;
                }
            }
            let pre = match &message {
                Message::AddOrder(_) | Message::Trade(_) => None,
                m => order_reference(m).and_then(|id| books.observed_queue(id)),
            };
            let touch = |books: &OrderBooks, side| {
                if side == Side::Buy {
                    books.best_bid(self.locate)
                } else {
                    books.best_ask(self.locate)
                }
                .map(|l| l.price)
            };
            let pre_touch = pre.as_ref().and_then(|q| touch(&books, q.order.side));
            let at_touch = pre
                .as_ref()
                .is_some_and(|q| Some(q.order.price) == pre_touch);
            let replace_target = match &message {
                Message::OrderReplace(m) => pre.as_ref().map(|q| {
                    m.price != q.order.price
                        && !books
                            .level_queue_prefix(self.locate, q.order.side, m.price, 1)
                            .is_empty()
                }),
                _ => None,
            };
            books.apply(&message);
            let Some(q) = pre else { continue };
            let position = q.ahead.len() as u32 + 1;
            let remains = books.order(q.order.order_ref).is_some();
            let deep = q.level_orders >= EXAMPLE_MIN_QUEUE;
            let slot = match &message {
                Message::OrderDelete(_)
                    if at_touch && deep && position > 1 && position < q.level_orders =>
                {
                    Some(0)
                }
                Message::OrderExecuted(_) | Message::OrderExecutedWithPrice(_)
                    if at_touch && deep && remains =>
                {
                    Some(1)
                }
                Message::OrderReplace(_)
                    if deep && position < q.level_orders && replace_target == Some(true) =>
                {
                    Some(2)
                }
                Message::OrderDelete(_)
                | Message::OrderExecuted(_)
                | Message::OrderExecutedWithPrice(_)
                    if at_touch
                        && q.level_orders == 1
                        && !remains
                        && touch(&books, q.order.side).is_some_and(|t| Some(t) != pre_touch) =>
                {
                    Some(3)
                }
                _ => None,
            };
            if let Some(slot) = slot
                && found[slot].is_none()
            {
                found[slot] = Some(index as u64);
            }
        }
        // Same-timestamp sequence: the last event of the first maximal run of consecutive
        // selected-symbol events sharing one timestamp, all with accepted book effects.
        let mut same_ts = None;
        let mut run_start = 0;
        for i in 1..=self.events.len() {
            let continues = i < self.events.len()
                && self.events[i].ts_ns.is_some()
                && self.events[i].ts_ns == self.events[run_start].ts_ns;
            if continues {
                continue;
            }
            let run = &self.events[run_start..i];
            let mut kinds: Vec<&str> = run.iter().map(|e| e.message_type.as_str()).collect();
            kinds.sort_unstable();
            kinds.dedup();
            if warm_from.is_some_and(|warm| run[0].event_index >= warm)
                && run
                    .last()
                    .is_some_and(|e| !found.contains(&Some(e.event_index)))
                && run.len() >= EXAMPLE_MIN_SAME_TS
                && run.iter().all(|e| e.applied == "book")
                && kinds.len() >= 2
            {
                same_ts = run.last().map(|e| e.event_index);
                break;
            }
            run_start = i;
        }
        let (q, warm) = (EXAMPLE_MIN_QUEUE, EXAMPLE_WARM_ORDERS);
        let specs = [
            (
                "cancel-ahead",
                "Cancel ahead of other orders",
                found[0],
                format!(
                    "First accepted D (delete), once each side holds at least {warm} resting orders, of an order at the pre-event best price of its side whose queue holds at least {q} orders, with at least one order ahead of it and one behind it."
                ),
            ),
            (
                "partial-execution",
                "Partial execution at the touch",
                found[1],
                format!(
                    "First accepted E/C execution, once each side holds at least {warm} resting orders, at the pre-event best price of its side in a queue of at least {q} orders, after which the executed order still rests."
                ),
            ),
            (
                "replace",
                "Replace loses queue priority",
                found[2],
                format!(
                    "First accepted U (replace), once each side holds at least {warm} resting orders, to a different price that already has resting orders, of an order with at least one order behind it in a queue of at least {q} orders."
                ),
            ),
            (
                "same-timestamp",
                "Same-timestamp sequence",
                same_ts,
                format!(
                    "Last event of the first run, starting once each side holds at least {warm} resting orders, of at least {EXAMPLE_MIN_SAME_TS} consecutive events with one source timestamp, all with accepted book effects and at least 2 message types, whose last event is not one of the other examples."
                ),
            ),
            (
                "level-change",
                "Touch level removed",
                found[3],
                format!(
                    "First accepted D/E/C, once each side holds at least {warm} resting orders, that removes the only order at the pre-event best price of its side, after which that side's best price is another existing level."
                ),
            ),
        ];
        Ok(specs
            .into_iter()
            .filter_map(|(id, title, index, rule)| {
                index.map(|event_index| CuratedExample {
                    id: id.into(),
                    title: title.into(),
                    event_index,
                    rule,
                })
            })
            .collect())
    }

    /// Creates a new directory with a self-contained JSON artifact. Local artifacts are always
    /// classified private, even if their input happened to be synthetically generated. Neither
    /// an existing directory nor any existing file is overwritten.
    pub fn save(&self, out: &Path) -> Result<(), String> {
        #[derive(Serialize)]
        struct Artifact<'a> {
            format: &'static str,
            data_class: &'static str,
            symbol: &'a str,
            manifest: &'a ItchManifest,
            events: &'a [EventRecord],
            observations: &'a [Observation],
            checkpoint_interval: usize,
            locate: u16,
            checkpoints: &'a [Checkpoint],
            source_frames: &'a [SourceFrame],
        }
        fs::create_dir(out).map_err(|e| format!("{}: {e}", out.display()))?;
        let path = out.join("workbench.json");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(
            &mut writer,
            &Artifact {
                format: "blackbox.l3-workbench.v1",
                data_class: "private",
                symbol: &self.symbol,
                manifest: &self.manifest,
                events: &self.events,
                observations: &self.observations,
                checkpoint_interval: self.checkpoint_interval,
                locate: self.locate,
                checkpoints: &self.checkpoints,
                source_frames: &self.source_frames,
            },
        )
        .map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())
    }
}

struct Recorder {
    rec: Reconstructor,
    symbol: String,
    stock: [u8; 8],
    locate: Option<u16>,
    max_frames: u64,
    interval: usize,
    events: Vec<EventRecord>,
    observations: Vec<Observation>,
    frames: Vec<SourceFrame>,
    checkpoints: Vec<Checkpoint>,
    checkpoint_orders: usize,
    source_bytes: usize,
    births: HashMap<u64, Birth>,
    error: Option<String>,
}

impl Recorder {
    fn new(symbol: &str, max_frames: u64, interval: usize) -> Result<Self, String> {
        let stock = crate::l3synth::stock8(symbol)?;
        if !(1..=MAX_CHECKPOINT_INTERVAL).contains(&interval) {
            return Err(format!(
                "checkpoint_interval must be 1..={MAX_CHECKPOINT_INTERVAL}"
            ));
        }
        Ok(Self {
            rec: Reconstructor::new(ItchOptions {
                max_messages: Some(max_frames),
                symbols: vec![symbol.into()],
                midnight_epoch_ns: 0,
            }),
            symbol: symbol.into(),
            stock,
            locate: None,
            max_frames,
            interval: interval.max(MIN_SNAPSHOT_SPACING),
            events: Vec::new(),
            observations: Vec::new(),
            frames: Vec::new(),
            checkpoints: vec![Checkpoint {
                frame_count: 0,
                orders: Vec::new(),
            }],
            checkpoint_orders: 0,
            source_bytes: 0,
            births: HashMap::new(),
            error: None,
        })
    }

    fn checkpoint(&mut self) {
        if self.checkpoint_orders + self.births.len() > MAX_CHECKPOINT_ORDERS {
            self.error = Some(
                "resting-order checkpoint memory limit exceeded; increase checkpoint_interval"
                    .into(),
            );
            return;
        }
        let mut orders = Vec::with_capacity(self.births.len());
        if let Some(locate) = self.locate {
            for side in [Side::Buy, Side::Sell] {
                for level in self.rec.books().levels(locate, side) {
                    for order in self.rec.books().level_queue(locate, side, level.price) {
                        let birth = self.births[&order.order_ref];
                        orders.push(RestingOrder {
                            order_ref: order.order_ref,
                            side: side_byte(side),
                            price4: level.price.0,
                            shares: order.shares,
                            entered_ns: birth.ts.map(|t| t.to_string()),
                            entered_event_index: birth.event_index,
                        });
                    }
                }
            }
        }
        self.checkpoint_orders += orders.len();
        self.checkpoints.push(Checkpoint {
            frame_count: self.frames.len(),
            orders,
        });
    }

    fn finish(self, source: SourceFile, stop: Stop, decoded: u64) -> Result<Workbench, String> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let locate = self
            .locate
            .ok_or_else(|| format!("symbol {} not found in source directory", self.symbol))?;
        let reconstruction = self.rec.finish(source, stop, decoded);
        let l2 = reconstruction
            .l2
            .into_iter()
            .find(|s| s.symbol == self.symbol)
            .map(|s| s.events)
            .unwrap_or_default();
        Ok(Workbench {
            manifest: reconstruction.manifest,
            symbol: self.symbol,
            events: self.events,
            observations: self.observations,
            l2,
            locate,
            stock: self.stock,
            checkpoint_interval: self.interval,
            checkpoints: self.checkpoints,
            source_frames: self.frames,
        })
    }
}

impl FrameSink for Recorder {
    fn limit_reached(&self) -> bool {
        self.error.is_some() || self.frames.len() as u64 >= self.max_frames
    }

    fn push(&mut self, frame: &[u8]) {
        let index = self.frames.len() as u64;
        if frame.len() > MAX_SOURCE_BYTES - self.source_bytes {
            self.error = Some("source-frame storage exceeds 32 MiB workbench limit".into());
            return;
        }
        self.source_bytes += frame.len();
        let decoded = decode::decode(frame);
        let message = decoded.as_ref().ok();
        if let Some(Message::StockDirectory(directory)) = message
            && directory.stock.0 == &self.stock
        {
            self.locate = Some(directory.header.stock_locate);
        }
        let header = message.and_then(|m| m.header());
        let selected =
            header.is_none_or(|h| h.stock_locate == 0 || Some(h.stock_locate) == self.locate);
        let ts = header.map(|h| h.timestamp_ns);
        let reference = message.and_then(order_reference);
        let pre = reference.and_then(|id| self.rec.books().order(id));
        let pre_queue = if selected && matches!(frame.first(), Some(b'E' | b'C' | b'X' | b'D')) {
            reference.and_then(|id| self.rec.books().queue_position(id))
        } else {
            None
        };
        let pre_bid = self.locate.and_then(|l| self.rec.books().best_bid(l));
        let pre_ask = self.locate.and_then(|l| self.rec.books().best_ask(l));
        let quarantined_before = self.rec.books().quarantined_total();
        self.rec.push(frame);
        let books = self.rec.books();
        let quarantined = books.quarantined_total() != quarantined_before;
        let book_effect = !quarantined
            && decoded.is_ok()
            && books
                .changes()
                .iter()
                .any(|c| matches!(c, Change::Level { .. }));
        self.frames.push(SourceFrame {
            bytes: frame.to_vec(),
            accepted_book: selected && book_effect,
        });
        if selected {
            let applied = if decoded.is_err() {
                "invalid"
            } else if quarantined {
                "quarantined"
            } else if book_effect {
                "book"
            } else if books
                .changes()
                .iter()
                .any(|c| matches!(c, Change::Print(_)))
            {
                "print"
            } else {
                "no_effect"
            };
            let (bid_shares, bid_orders) = self
                .locate
                .map_or((0, 0), |l| books.side_totals(l, Side::Buy));
            let (ask_shares, ask_orders) = self
                .locate
                .map_or((0, 0), |l| books.side_totals(l, Side::Sell));
            let bid = self.locate.and_then(|l| books.best_bid(l));
            let ask = self.locate.and_then(|l| books.best_ask(l));
            let mut event = EventRecord {
                event_index: index,
                message_type: frame
                    .first()
                    .map_or_else(|| "empty".into(), |b| decode::show_kind(*b)),
                ts_ns: ts.map(|t| t.to_string()),
                order_ref: reference.map(|id| id.to_string()),
                new_order_ref: None,
                side: pre.map(|o| side_name(o.side).into()),
                price4: pre.map(|o| o.price.0),
                shares: None,
                best_bid_price4: bid.map(|l| l.price.0),
                best_ask_price4: ask.map(|l| l.price.0),
                applied: applied.into(),
            };
            if let Some(message) = message {
                fill_event(&mut event, message, pre);
            }
            let count = bid_orders + ask_orders;
            let mut observation = Observation {
                event_index: index,
                ts_ns: event.ts_ns.clone(),
                live_orders: count,
                bid_shares,
                ask_shares,
                best_bid_price4: event.best_bid_price4,
                best_ask_price4: event.best_ask_price4,
                order_count_imbalance: (count != 0)
                    .then(|| (bid_orders as f64 - ask_orders as f64) / count as f64),
                signed_added_shares: 0,
                signed_cancelled_shares: 0,
                signed_executed_shares: 0,
                touch_depleted_shares: 0,
                touch_replenished_shares: 0,
                lifetime_ns: None,
                executed_queue_position: None,
                cancel_location: None,
                order_identity_observations: count,
                touch_queue_observations: bid.map_or(0, |l| u64::from(l.orders))
                    + ask.map_or(0, |l| u64::from(l.orders)),
                accepted_add: false,
                accepted_cancel: false,
                level_key: None,
            };
            if book_effect {
                if let Some(message) = message {
                    match message {
                        Message::AddOrder(m) => {
                            observation.signed_added_shares = signed(m.side, u64::from(m.shares));
                            observation.accepted_add = true;
                            observation.level_key = Some((side_byte(m.side), m.price.0));
                        }
                        Message::OrderExecuted(_) | Message::OrderExecutedWithPrice(_) => {
                            if let Some(order) = pre {
                                observation.signed_executed_shares =
                                    signed(order.side, event.shares.unwrap_or(0));
                                observation.executed_queue_position = pre_queue.map(|q| q.0);
                            }
                        }
                        Message::OrderCancel(_) | Message::OrderDelete(_) => {
                            if let Some(order) = pre {
                                observation.signed_cancelled_shares =
                                    signed(order.side, event.shares.unwrap_or(0));
                                observation.accepted_cancel = true;
                                observation.level_key =
                                    Some((side_byte(order.side), order.price.0));
                                observation.cancel_location = pre_queue.map(|(pos, n)| {
                                    let middle = (n - 1) / 2;
                                    match (pos - 1).cmp(&middle) {
                                        std::cmp::Ordering::Less => -1,
                                        std::cmp::Ordering::Equal => 0,
                                        std::cmp::Ordering::Greater => 1,
                                    }
                                });
                            }
                        }
                        Message::OrderReplace(m) => {
                            if let Some(order) = pre {
                                observation.signed_cancelled_shares =
                                    signed(order.side, u64::from(order.shares));
                                observation.signed_added_shares =
                                    signed(order.side, u64::from(m.shares));
                            }
                        }
                        _ => {}
                    }
                    if let Some(order) = pre {
                        let replaced = matches!(message, Message::OrderReplace(_));
                        if replaced || books.order(order.order_ref).is_none() {
                            observation.lifetime_ns = self
                                .births
                                .get(&order.order_ref)
                                .and_then(|birth| ts?.checked_sub(birth.ts?));
                        }
                    }
                    update_births(&mut self.births, books, message, index);
                }
                // Fixed pre-event touch prices: disappearing or moving touch is not itself flow.
                for change in books.changes() {
                    if let Change::Level {
                        locate,
                        side,
                        price,
                        shares,
                        ..
                    } = change
                    {
                        if Some(*locate) != self.locate {
                            continue;
                        }
                        let touch = if *side == Side::Buy { pre_bid } else { pre_ask };
                        if let Some(touch) = touch.filter(|t| t.price == *price) {
                            observation.touch_depleted_shares +=
                                touch.shares.saturating_sub(*shares);
                            observation.touch_replenished_shares +=
                                shares.saturating_sub(touch.shares);
                        }
                    }
                }
            }
            self.events.push(event);
            self.observations.push(observation);
        }
        if self.frames.len().is_multiple_of(self.interval) {
            self.checkpoint();
        }
    }
}

fn side_byte(side: Side) -> u8 {
    if side == Side::Buy { b'B' } else { b'S' }
}
fn side_name(side: Side) -> &'static str {
    if side == Side::Buy { "buy" } else { "sell" }
}
fn signed(side: Side, shares: u64) -> i64 {
    if side == Side::Buy {
        shares as i64
    } else {
        -(shares as i64)
    }
}
fn frame_ts(frame: &[u8]) -> Option<u64> {
    decode::decode(frame).ok()?.header().map(|h| h.timestamp_ns)
}
fn order_reference(message: &Message<'_>) -> Option<u64> {
    match message {
        Message::AddOrder(m) => Some(m.order_ref),
        Message::OrderExecuted(m) => Some(m.order_ref),
        Message::OrderExecutedWithPrice(m) => Some(m.order_ref),
        Message::OrderCancel(m) => Some(m.order_ref),
        Message::OrderDelete(m) => Some(m.order_ref),
        Message::OrderReplace(m) => Some(m.original_ref),
        Message::Trade(m) => Some(m.order_ref),
        _ => None,
    }
}

fn fill_event(event: &mut EventRecord, message: &Message<'_>, pre: Option<OrderView>) {
    match message {
        Message::AddOrder(m) => {
            event.side = Some(side_name(m.side).into());
            event.price4 = Some(m.price.0);
            event.shares = Some(u64::from(m.shares));
        }
        Message::OrderExecuted(m) => {
            event.shares = Some(u64::from(m.shares));
        }
        Message::OrderExecutedWithPrice(m) => {
            event.price4 = Some(m.price.0);
            event.shares = Some(u64::from(m.shares));
        }
        Message::OrderCancel(m) => {
            event.shares = Some(u64::from(m.shares));
        }
        Message::OrderDelete(_) => {
            event.shares = pre.map(|o| u64::from(o.shares));
        }
        Message::OrderReplace(m) => {
            event.new_order_ref = Some(m.new_ref.to_string());
            event.price4 = Some(m.price.0);
            event.shares = Some(u64::from(m.shares));
        }
        Message::Trade(m) => {
            event.side = Some(side_name(m.side).into());
            event.price4 = Some(m.price.0);
            event.shares = Some(u64::from(m.shares));
        }
        Message::CrossTrade(m) => {
            event.price4 = Some(m.price.0);
            event.shares = Some(m.shares);
        }
        _ => {}
    }
}

fn book_order(
    births: &HashMap<u64, Birth>,
    order: &QueuedOrder,
    index: usize,
    shares_ahead: u64,
) -> BookOrder {
    let birth = births
        .get(&order.order_ref)
        .expect("every accepted resting order has a creator");
    BookOrder {
        order_ref: order.order_ref.to_string(),
        shares: u64::from(order.shares),
        queue_position: index as u32 + 1,
        shares_ahead,
        entered_ns: birth.ts.map(|ts| ts.to_string()),
        entered_event_index: birth.event_index,
    }
}

/// Price levels an accepted message changes, resolved against the pre-event book.
fn touched_levels(books: &OrderBooks, message: &Message<'_>) -> Vec<(Side, Price4, &'static str)> {
    match message {
        Message::AddOrder(m) => vec![(m.side, m.price, "order")],
        Message::OrderReplace(m) => books.order(m.original_ref).map_or_else(Vec::new, |o| {
            if o.price == m.price {
                vec![(o.side, o.price, "replaced")]
            } else {
                vec![
                    (o.side, o.price, "replaced_from"),
                    (o.side, m.price, "replaced_to"),
                ]
            }
        }),
        Message::OrderExecuted(_)
        | Message::OrderExecutedWithPrice(_)
        | Message::OrderCancel(_)
        | Message::OrderDelete(_) => order_reference(message)
            .and_then(|id| books.order(id))
            .map_or_else(Vec::new, |o| vec![(o.side, o.price, "order")]),
        _ => Vec::new(),
    }
}

fn update_births(
    births: &mut HashMap<u64, Birth>,
    books: &OrderBooks,
    message: &Message<'_>,
    index: u64,
) {
    let birth = Birth {
        ts: message.header().map(|h| h.timestamp_ns),
        event_index: index,
    };
    match message {
        Message::AddOrder(m) => {
            births.insert(m.order_ref, birth);
        }
        Message::OrderReplace(m) => {
            births.remove(&m.original_ref);
            births.insert(m.new_ref, birth);
        }
        _ => {
            if let Some(id) = order_reference(message)
                && books.order(id).is_none()
            {
                births.remove(&id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        l3synth::{self, L3SynthConfig},
        testutil::TempDir,
    };
    use bb_core::dataset::InstrumentMeta;
    use std::io::Write;

    fn reference(kind: u8, ts: u64, id: u64, shares: Option<u32>) -> Vec<u8> {
        let mut frame = l3synth::header(kind, ts);
        frame.extend(id.to_be_bytes());
        if let Some(shares) = shares {
            frame.extend(shares.to_be_bytes());
        }
        if kind == b'E' || kind == b'C' {
            frame.extend(ts.to_be_bytes());
        }
        if kind == b'C' {
            frame.push(b'Y');
            frame.extend(100_001u32.to_be_bytes());
        }
        frame
    }

    fn fixture() -> Vec<u8> {
        let stock = l3synth::stock8("TEST").unwrap();
        let mut start = l3synth::header(b'S', 0);
        start.push(b'O');
        let mut frames = vec![start, l3synth::directory(1, &stock)];
        for (id, ts, side, shares, price) in [
            (1, 10, b'B', 100, 100_000),
            (2, 20, b'B', 200, 100_000),
            (3, 30, b'B', 300, 100_000),
            (4, 40, b'S', 80, 110_000),
        ] {
            frames.push(l3synth::add(ts, id, side, shares, price, &stock));
        }
        frames.push(reference(b'X', 45, 1, Some(10)));
        frames.push(reference(b'X', 50, 2, Some(50)));
        frames.push(reference(b'E', 60, 3, Some(100)));
        frames.push(reference(b'C', 70, 1, Some(90)));
        frames.push(reference(b'X', 80, 3, Some(200)));
        let mut replace = reference(b'U', 90, 2, None);
        replace.extend(5u64.to_be_bytes());
        replace.extend(75u32.to_be_bytes());
        replace.extend(99_000u32.to_be_bytes());
        frames.push(replace);
        frames.push(reference(b'D', 100, 5, None));
        let mut print = l3synth::add(110, 0, b'B', 25, 105_000, &stock);
        print[0] = b'P';
        print.extend(1000u64.to_be_bytes());
        frames.push(print);
        frames.push(l3synth::add(120, 4, b'S', 1, 110_000, &stock));
        frames.push(reference(b'E', 130, 999, Some(1)));
        let mut bytes = Vec::new();
        for frame in frames {
            l3synth::pack(&mut bytes, &frame);
        }
        bytes
    }

    #[test]
    fn hand_computed_identity_flow_queue_and_terminal_lifetime() {
        let w = Workbench::from_bytes("fixture", &fixture(), "TEST", 32).unwrap();
        let at = |ts: &str| {
            w.observations
                .iter()
                .find(|o| o.ts_ns.as_deref() == Some(ts))
                .unwrap()
        };
        assert_eq!(
            (
                at("40").bid_shares,
                at("40").ask_shares,
                at("40").live_orders
            ),
            (600, 80, 4)
        );
        assert_eq!(at("40").order_count_imbalance, Some(0.5));
        assert_eq!(at("20").touch_replenished_shares, 200);
        assert_eq!(at("40").signed_added_shares, -80);
        assert_eq!(at("40").touch_queue_observations, 4);
        assert_eq!(at("45").cancel_location, Some(-1));
        assert_eq!(at("50").cancel_location, Some(0));
        assert_eq!(at("80").cancel_location, Some(1));
        assert_eq!(at("50").signed_cancelled_shares, 50);
        assert_eq!(at("60").executed_queue_position, Some(3));
        assert_eq!(at("60").signed_executed_shares, 100);
        assert_eq!(at("60").lifetime_ns, None);
        assert_eq!(at("70").executed_queue_position, Some(1));
        assert_eq!(at("70").lifetime_ns, Some(60));
        assert_eq!(at("70").touch_depleted_shares, 90);
        assert_eq!(at("80").lifetime_ns, Some(50));
        assert_eq!(
            (
                at("90").signed_cancelled_shares,
                at("90").signed_added_shares
            ),
            (150, 75)
        );
        assert_eq!(at("90").lifetime_ns, Some(70));
        assert!(!at("90").accepted_add && !at("90").accepted_cancel);
        assert_eq!(at("90").touch_depleted_shares, 150);
        assert_eq!(at("100").lifetime_ns, Some(10));
        assert_eq!(at("100").signed_cancelled_shares, 75);
        assert_eq!(at("110").signed_executed_shares, 0);
        for ts in ["120", "130"] {
            let o = at(ts);
            assert_eq!(
                (
                    o.signed_added_shares,
                    o.signed_cancelled_shares,
                    o.signed_executed_shares
                ),
                (0, 0, 0)
            );
            assert!(!o.accepted_add && !o.accepted_cancel);
            assert_eq!(o.executed_queue_position, None);
            assert_eq!(o.lifetime_ns, None);
        }
        let book = w.book(5, 1, 2).unwrap();
        assert_eq!((book.bids[0].shares, book.bids[0].order_count), (600, 3));
        assert!(book.bids[0].orders_truncated);
        assert_eq!(book.bids[0].orders[1].queue_position, 2);
        assert_eq!(book.bids[0].orders[1].shares_ahead, 100);
        assert_eq!(book.bids[0].orders[1].entered_ns.as_deref(), Some("20"));
        assert_eq!(book.bids[0].orders[1].entered_event_index, 3);
        let replaced = w.book(11, 1, 100).unwrap();
        assert_eq!(replaced.bids[0].orders[0].entered_event_index, 11);
        assert_eq!(replaced.bids[0].orders[0].entered_ns.as_deref(), Some("90"));
        assert_eq!(w.events[9].price4, Some(100_001));
        assert_eq!(w.events[12].shares, Some(75));
        assert_eq!(w.events[13].applied, "print");
        assert_eq!(w.events[14].applied, "quarantined");
    }

    #[test]
    fn checkpoints_match_linear_reconstruction_and_l2_at_every_event() {
        let bytes = l3synth::generate(&L3SynthConfig {
            events: 400,
            ..Default::default()
        })
        .unwrap();
        let w = Workbench::from_bytes("seed7", &bytes, "SYNTH", 32).unwrap();
        let sparse = Workbench::from_bytes("seed7", &bytes, "SYNTH", 4096).unwrap();
        let instrument = InstrumentMeta {
            tick_size: 0.0001,
            lot_size: 1.0,
            contract_size: 1.0,
        };
        let mut rec = Reconstructor::new(ItchOptions {
            symbols: vec!["SYNTH".into()],
            ..Default::default()
        });
        let mut l2 = crate::book::L2Book::new();
        let mut replayed = 0;
        for (index, frame) in w.source_frames.iter().enumerate() {
            rec.push(&frame.bytes);
            if let Some(stream) = rec.l2().first() {
                for event in &stream.events[replayed..] {
                    l2.apply(event, &instrument);
                }
                replayed = stream.events.len();
            }
            let snapshot = w.book(index as u64, 20, 100).unwrap();
            for (side, levels) in [(Side::Buy, &snapshot.bids), (Side::Sell, &snapshot.asks)] {
                let expected = rec.books().levels(1, side);
                assert_eq!(levels.len(), expected.len());
                for (level, expected) in levels.iter().zip(&expected) {
                    assert_eq!(
                        (level.price4, level.shares, level.order_count),
                        (expected.price.0, expected.shares, expected.orders)
                    );
                    let queue = rec.books().level_queue(1, side, expected.price);
                    for (position, (order, expected)) in level.orders.iter().zip(queue).enumerate()
                    {
                        assert_eq!(order.order_ref, expected.order_ref.to_string());
                        assert_eq!(order.shares, u64::from(expected.shares));
                        assert_eq!(order.queue_position, position as u32 + 1);
                    }
                }
                let actual_l2 = if side == Side::Buy {
                    l2.top_bids(usize::MAX)
                } else {
                    l2.top_asks(usize::MAX)
                };
                let expected_l2: Vec<_> = expected
                    .iter()
                    .map(|l| (i64::from(l.price.0), l.shares as f64))
                    .collect();
                assert_eq!(actual_l2, expected_l2);
            }
        }
        // Deterministic random access straddles checkpoint boundaries and compares creator metadata too.
        let mut state = 17u64;
        for _ in 0..100 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let index = state % w.source_frames.len() as u64;
            assert_eq!(
                w.book(index, 20, 100).unwrap(),
                sparse.book(index, 20, 100).unwrap()
            );
        }
        assert!(w.book(w.source_frames.len() as u64, 5, 20).is_err());
        assert!(w.book(0, 0, 20).is_err());
        assert!(w.book(0, 5, 101).is_err());
        assert!(Workbench::from_bytes("bad", &bytes, "SYNTH", 0).is_err());
    }

    #[test]
    fn gzip_prefix_and_private_artifact_never_overwrite() {
        let dir = TempDir::new("l3-workbench");
        let path = dir.path().join("source.gz");
        let bytes = fixture();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        fs::write(&path, encoder.finish().unwrap()).unwrap();
        let w = Workbench::from_file(&path, "TEST", 10, 32).unwrap();
        assert_eq!(w.manifest.frames, 10);
        assert_eq!(w.manifest.source.compression, "gzip");
        assert_eq!(w.manifest.stop.reason, "message_limit");
        let out = dir.path().join("artifact");
        w.save(&out).unwrap();
        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("workbench.json")).unwrap()).unwrap();
        assert_eq!(artifact["data_class"], "private");
        assert_eq!(artifact["source_frames"].as_array().unwrap().len(), 10);
        assert!(artifact["checkpoints"].is_array());
        assert!(w.save(&out).is_err());
        assert!(w.save(&path).is_err());
        assert!(Workbench::from_file(&path, "TEST", 0, 32).is_err());
    }

    #[test]
    fn source_frame_gaps_keep_physical_indices_and_as_of_book() {
        let stock = l3synth::stock8("OTHER").unwrap();
        let mut bytes = fixture();
        let mut other = l3synth::directory(140, &stock);
        other[1..3].copy_from_slice(&2u16.to_be_bytes());
        l3synth::pack(&mut bytes, &other);
        let mut other = l3synth::add(150, 888, b'B', 500, 90_000, &stock);
        other[1..3].copy_from_slice(&2u16.to_be_bytes());
        l3synth::pack(&mut bytes, &other);
        let mut last = l3synth::header(b'S', 160);
        last.push(b'C');
        l3synth::pack(&mut bytes, &last);
        let w = Workbench::from_bytes("multi", &bytes, "TEST", 32).unwrap();
        assert_eq!(w.events.last().unwrap().event_index, 18);
        assert!(
            !w.events
                .iter()
                .any(|e| e.event_index == 16 || e.event_index == 17)
        );
        assert_eq!(w.book(17, 5, 20).unwrap().asks[0].shares, 80);
        assert_eq!(w.observations.last().unwrap().live_orders, 1);
    }

    fn depth_of(levels: &[BookLevel]) -> Vec<DepthLevel> {
        levels
            .iter()
            .map(|l| DepthLevel {
                price4: l.price4,
                shares: l.shares,
                order_count: l.order_count,
                orders: l
                    .orders
                    .iter()
                    .map(|o| QueueSlot {
                        order_ref: o.order_ref.clone(),
                        shares: o.shares,
                    })
                    .collect(),
                orders_truncated: l.orders_truncated,
            })
            .collect()
    }

    fn frame_prefix(bytes: &[u8], frames: usize) -> &[u8] {
        let mut end = 0;
        for _ in 0..frames {
            end += 2 + usize::from(u16::from_be_bytes([bytes[end], bytes[end + 1]]));
        }
        &bytes[..end]
    }

    #[test]
    fn event_detail_hand_computed_partial_cancel_execution_replace_and_bounds() {
        let w = Workbench::from_bytes("fixture", &fixture(), "TEST", 32).unwrap();
        // #7: X cancels 50 of order 2 (200 shares, second of three at 10.0000).
        let d = w.event_detail(7, 5, 100).unwrap().unwrap();
        assert_eq!(d.event.message_type, "X");
        assert_eq!(d.before_event_index, Some(6));
        let before = d.order_before.as_ref().unwrap();
        assert_eq!(
            (
                before.order_ref.as_str(),
                before.shares,
                before.queue_position,
                before.level_orders,
                before.shares_ahead,
                before.level_shares,
            ),
            ("2", 200, 2, 3, 90, 590)
        );
        assert_eq!(before.age_ns.as_deref(), Some("30"));
        let after = d.order_after.as_ref().unwrap();
        assert_eq!(
            (after.shares, after.queue_position, after.shares_ahead),
            (150, 2, 90)
        );
        assert_eq!(d.lifetime_ns, None);
        assert_eq!(d.affected.len(), 1);
        let level = &d.affected[0];
        assert_eq!(
            (level.side.as_str(), level.price4, level.role.as_str()),
            ("buy", 100_000, "order")
        );
        assert_eq!((level.before.shares, level.after.shares), (590, 540));
        assert_eq!(level.before.orders[2].shares_ahead, 290);
        assert_eq!(level.after.orders[2].shares_ahead, 240);
        assert_eq!(d.same_timestamp.position, 1);
        assert_eq!(
            d.same_timestamp.events[0].level_changes,
            vec![LevelChange {
                side: "buy".into(),
                price4: 100_000,
                before_shares: 590,
                after_shares: 540,
                before_orders: 3,
                after_orders: 3,
            }]
        );
        // #9: C executes the remaining 90 of order 1 at the head: terminal, lifetime 70 - 10.
        let d = w.event_detail(9, 5, 100).unwrap().unwrap();
        assert_eq!(d.order_before.as_ref().unwrap().queue_position, 1);
        assert!(d.order_after.is_none());
        assert_eq!(d.lifetime_ns.as_deref(), Some("60"));
        assert_eq!(d.affected[0].after.order_count, 2);
        // #11: U replaces order 2 (alone at 10.0000) by order 5 at 9.9000: two levels change,
        // the old lifetime ends and the new order starts at the back of its new queue.
        let d = w.event_detail(11, 5, 100).unwrap().unwrap();
        let roles: Vec<_> = d
            .affected
            .iter()
            .map(|l| (l.role.as_str(), l.price4, l.before.shares, l.after.shares))
            .collect();
        assert_eq!(
            roles,
            vec![
                ("replaced_from", 100_000, 150, 0),
                ("replaced_to", 99_000, 0, 75)
            ]
        );
        assert_eq!(d.lifetime_ns.as_deref(), Some("70"));
        let after = d.order_after.as_ref().unwrap();
        assert_eq!(
            (
                after.order_ref.as_str(),
                after.queue_position,
                after.entered_event_index,
                after.age_ns.as_deref()
            ),
            ("5", 1, 11, Some("0"))
        );
        // #0 has no earlier state; quarantined messages change nothing and name no new order.
        let d = w.event_detail(0, 5, 100).unwrap().unwrap();
        assert_eq!(d.before_event_index, None);
        assert!(d.before.bids.is_empty() && d.after.bids.is_empty());
        for index in [14, 15] {
            let d = w.event_detail(index, 5, 100).unwrap().unwrap();
            assert_eq!(d.event.applied, "quarantined");
            assert!(d.affected.is_empty() && d.order_after.is_none());
            assert_eq!(d.before, d.after);
        }
        // Truncation keeps the selected order and says so.
        let d = w.event_detail(7, 5, 1).unwrap().unwrap();
        let level = &d.affected[0].before;
        assert!(level.orders_truncated);
        assert_eq!(
            level
                .orders
                .iter()
                .map(|o| o.queue_position)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(d.before.bids[0].orders_truncated);
        assert_eq!(d.before.bids[0].orders.len(), 1);
        assert!(w.event_detail(16, 5, 20).is_err());
        assert!(w.event_detail(0, 11, 20).is_err());
        assert!(w.event_detail(0, 5, 101).is_err());
    }

    #[test]
    fn event_detail_is_causal_and_agrees_with_book_and_observations() {
        let bytes = l3synth::generate(&L3SynthConfig {
            events: 400,
            ..Default::default()
        })
        .unwrap();
        let w = Workbench::from_bytes("seed7", &bytes, "SYNTH", 32).unwrap();
        let mut same_ts_checked = 0;
        for (k, event) in w.events.iter().enumerate() {
            let i = event.event_index;
            let d = w.event_detail(i, 10, 100).unwrap().unwrap();
            assert_eq!(&d.event, event);
            let after = w.book(i, 10, 100).unwrap();
            assert_eq!(d.after.bids, depth_of(&after.bids));
            assert_eq!(d.after.asks, depth_of(&after.asks));
            if let Some(previous) = i.checked_sub(1) {
                let before = w.book(previous, 10, 100).unwrap();
                assert_eq!(d.before.bids, depth_of(&before.bids));
                assert_eq!(d.before.asks, depth_of(&before.asks));
            }
            let o = &w.observations[k];
            assert_eq!(
                d.lifetime_ns,
                o.lifetime_ns.map(|ns| ns.to_string()),
                "#{i}"
            );
            if o.executed_queue_position.is_some() {
                assert_eq!(
                    d.order_before.as_ref().map(|q| q.queue_position),
                    o.executed_queue_position
                );
            }
            for level in &d.affected {
                for state in [&level.before, &level.after] {
                    let mut ahead = 0;
                    for order in &state.orders {
                        assert_eq!(order.shares_ahead, ahead);
                        ahead += order.shares;
                    }
                    assert_eq!(ahead, state.shares);
                }
            }
            // Same-timestamp context never reaches past the event and keeps source order.
            let st = &d.same_timestamp;
            assert_eq!(st.events.last().unwrap().event_index, i);
            assert_eq!(st.events.len(), st.position as usize);
            assert!(
                st.events
                    .windows(2)
                    .all(|p| p[0].event_index < p[1].event_index)
            );
            if st.position > 1 {
                same_ts_checked += 1;
            }
        }
        assert!(same_ts_checked > 0);
        // No look-ahead: a source cut immediately after the event gives the identical detail,
        // including where the next message shares the event's timestamp.
        let mut cuts: Vec<usize> = w
            .events
            .windows(2)
            .filter(|p| p[0].ts_ns == p[1].ts_ns)
            .map(|p| p[0].event_index as usize)
            .take(5)
            .collect();
        cuts.extend([40, 129, 200, 333]);
        assert!(cuts.len() > 4);
        for cut in cuts {
            let prefix =
                Workbench::from_bytes("prefix", frame_prefix(&bytes, cut + 1), "SYNTH", 32)
                    .unwrap();
            assert_eq!(
                prefix.event_detail(cut as u64, 10, 100).unwrap(),
                w.event_detail(cut as u64, 10, 100).unwrap(),
                "#{cut}"
            );
        }
    }

    #[test]
    fn curated_examples_have_their_stated_properties() {
        let bytes = l3synth::generate(&L3SynthConfig::default()).unwrap();
        let w = Workbench::from_bytes("seed7", &bytes, "SYNTH", 1024).unwrap();
        let examples = w.curated_examples().unwrap();
        assert_eq!(
            examples.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec![
                "cancel-ahead",
                "partial-execution",
                "replace",
                "same-timestamp",
                "level-change"
            ]
        );
        assert_eq!(examples, w.curated_examples().unwrap());
        let mut distinct: Vec<_> = examples.iter().map(|e| e.event_index).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), examples.len());
        let detail = |id: &str| {
            let example = examples.iter().find(|e| e.id == id).unwrap();
            println!("{id}: #{}", example.event_index);
            // Not an early, thin book: both sides already hold at least 100 orders.
            let before = w.book(example.event_index - 1, 20, 1).unwrap();
            for side in [&before.bids, &before.asks] {
                assert!(
                    side.iter().map(|l| l.order_count).sum::<u32>() >= 100,
                    "{id}"
                );
            }
            w.event_detail(example.event_index, 5, 100)
                .unwrap()
                .unwrap()
        };
        let best = |depth: &Depth, side: &str| {
            if side == "buy" {
                &depth.bids
            } else {
                &depth.asks
            }
            .first()
            .map(|l| l.price4)
        };

        let d = detail("cancel-ahead");
        assert_eq!(
            (d.event.message_type.as_str(), d.event.applied.as_str()),
            ("D", "book")
        );
        let o = d.order_before.as_ref().unwrap();
        assert!(o.level_orders >= 6 && o.queue_position > 1 && o.queue_position < o.level_orders);
        assert_eq!(best(&d.before, &o.side), Some(o.price4));
        assert!(d.order_after.is_none());
        let level = &d.affected[0];
        assert_eq!(level.after.order_count + 1, level.before.order_count);
        for behind in level
            .before
            .orders
            .iter()
            .filter(|r| r.queue_position > o.queue_position)
        {
            let moved = level
                .after
                .orders
                .iter()
                .find(|r| r.order_ref == behind.order_ref)
                .unwrap();
            assert_eq!(moved.queue_position + 1, behind.queue_position);
            assert_eq!(moved.shares_ahead + o.shares, behind.shares_ahead);
        }

        let d = detail("partial-execution");
        assert!(matches!(d.event.message_type.as_str(), "E" | "C"));
        let (o, a) = (
            d.order_before.as_ref().unwrap(),
            d.order_after.as_ref().unwrap(),
        );
        assert_eq!(o.order_ref, a.order_ref);
        assert!(o.level_orders >= 6);
        assert_eq!(best(&d.before, &o.side), Some(o.price4));
        assert_eq!(a.shares + d.event.shares.unwrap(), o.shares);
        assert!(a.shares > 0);
        assert_eq!(
            (a.queue_position, a.shares_ahead),
            (o.queue_position, o.shares_ahead)
        );

        let d = detail("replace");
        assert_eq!(d.event.message_type, "U");
        let roles: Vec<_> = d.affected.iter().map(|l| l.role.as_str()).collect();
        assert_eq!(roles, vec!["replaced_from", "replaced_to"]);
        let (o, a) = (
            d.order_before.as_ref().unwrap(),
            d.order_after.as_ref().unwrap(),
        );
        assert!(o.level_orders >= 6 && o.queue_position < o.level_orders);
        let target = &d.affected[1];
        assert!(target.before.order_count >= 1);
        assert_eq!(a.queue_position, target.before.order_count + 1);
        assert_eq!(a.shares_ahead, target.before.shares);
        assert_eq!(Some(&a.order_ref), d.event.new_order_ref.as_ref());

        let d = detail("same-timestamp");
        let st = &d.same_timestamp;
        assert!(st.events.len() >= 3 && !st.events_truncated);
        assert_eq!(st.position as usize, st.events.len());
        let k = w
            .events
            .binary_search_by_key(&d.event.event_index, |e| e.event_index)
            .unwrap();
        let run = &w.events[k + 1 - st.events.len()..=k];
        assert!(
            run.iter()
                .all(|e| e.ts_ns == d.event.ts_ns && e.applied == "book")
        );
        assert_ne!(w.events.get(k + 1).map(|e| &e.ts_ns), Some(&d.event.ts_ns));
        let mut kinds: Vec<_> = st.events.iter().map(|e| &e.message_type).collect();
        kinds.sort();
        kinds.dedup();
        assert!(kinds.len() >= 2);

        let d = detail("level-change");
        assert!(matches!(d.event.message_type.as_str(), "D" | "E" | "C"));
        let o = d.order_before.as_ref().unwrap();
        let level = &d.affected[0];
        assert_eq!((level.before.order_count, level.after.order_count), (1, 0));
        assert_eq!(best(&d.before, &o.side), Some(o.price4));
        let moved = best(&d.after, &o.side).unwrap();
        assert_ne!(moved, o.price4);
    }
}
