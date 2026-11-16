//! Bounded, local-private ITCH workbench. Reconstruction and L2 normalization remain the
//! existing ITCH engine's responsibility; this module records its accepted effects.
//! Checkpoints contain only flat resting-order rows in FIFO order, never whole engine clones.

use crate::{
    Event,
    itch::{
        self, FrameSink, ItchManifest, ItchOptions, Reconstructor, SourceFile, Stop,
        book::{Change, OrderBooks, OrderView},
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
        let index = usize::try_from(event_index).map_err(|_| "event index out of range")?;
        if index >= self.source_frames.len() {
            return Err("event index out of range".into());
        }
        let cp_index = self
            .checkpoints
            .partition_point(|c| c.frame_count <= index + 1)
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
        for (offset, source) in self.source_frames[cp.frame_count..index + 1]
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
        let side = |side| -> Vec<BookLevel> {
            books
                .top_levels(self.locate, side, levels)
                .into_iter()
                .map(|level| {
                    let queue =
                        books.level_queue_prefix(self.locate, side, level.price, orders_per_level);
                    let mut ahead = 0;
                    let orders = queue
                        .iter()
                        .take(orders_per_level)
                        .enumerate()
                        .map(|(i, order)| {
                            let birth = births
                                .get(&order.order_ref)
                                .expect("every accepted resting order has a creator");
                            let row = BookOrder {
                                order_ref: order.order_ref.to_string(),
                                shares: u64::from(order.shares),
                                queue_position: i as u32 + 1,
                                shares_ahead: ahead,
                                entered_ns: birth.ts.map(|ts| ts.to_string()),
                                entered_event_index: birth.event_index,
                            };
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
        };
        Ok(BookSnapshot {
            event_index,
            ts_ns: frame_ts(&self.source_frames[index].bytes).map(|t| t.to_string()),
            levels,
            bids: side(Side::Buy),
            asks: side(Side::Sell),
        })
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
}
