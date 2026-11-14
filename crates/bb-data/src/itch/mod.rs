//! NASDAQ TotalView-ITCH 5.0: decoding, order-by-order (L3) book reconstruction and
//! normalization to the BlackBox Level-2 event model, for local validation.
//!
//! Written from the public specification "Nasdaq TotalView-ITCH 5.0" (nasdaqtrader.com, revision
//! log ending 2023-04-28). itch-rs and itch-book were read as references only; no code was copied
//! (docs/rebuild/REUSE_MATRIX.md).
//!
//! **Local use only.** NASDAQ's sample files carry no redistribution grant. They, and everything
//! derived from them event by event (books, rows, run tapes), must never be committed, bundled or
//! uploaded. Tests build their own bytes.
//!
//! [`decode`] splits the 2-byte length-prefixed file format and parses messages without copying;
//! [`book`] rebuilds every stock's order book and quarantines what it cannot apply; [`l2`]
//! aggregates selected stocks into engine rows. [`Reconstructor`] drives the three per frame and
//! [`reconstruct_file`] runs a whole file into an [`ItchManifest`].

pub mod book;
pub mod decode;
pub mod l2;

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use bb_core::dataset::{QualityCheck, Severity};
use flate2::read::MultiGzDecoder;
use serde::Serialize;
use sha2::{Digest, Sha256};

use self::{
    book::{OrderBooks, Quarantine},
    decode::{FrameError, FrameReader, Message, Truncated, decode, show_kind},
    l2::{DECISIONS, L2Normalizer, L2Stream},
};
use crate::{DataError, HashingReader, quality::Finding, quality::check};

/// Recorded in every manifest.
pub const LICENSE_NOTE: &str = "Local use only. NASDAQ ITCH sample files carry no redistribution \
     grant: the source file and anything derived from it event by event (books, rows, run tapes) \
     must never be committed, bundled or uploaded.";

/// Nanoseconds in a day; a timestamp at or beyond it is not "since midnight" of one day.
pub const NS_PER_DAY: u64 = 86_400_000_000_000;

const READ_BUFFER_BYTES: usize = 1 << 20;

/// System events kept in the manifest (a day has six).
const MAX_SYSTEM_EVENTS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ItchOptions {
    /// Stop after this many frames (valid or not).
    pub max_messages: Option<u64>,
    /// Symbols to normalize into Level-2 rows, as in the Stock Directory without padding.
    pub symbols: Vec<String>,
    /// Added to every ITCH timestamp to form `exch_ts`; 0 keeps nanoseconds since midnight.
    pub midnight_epoch_ns: i64,
}

/// Why reading stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    EndOfInput,
    MessageLimit,
    Truncated(Truncated),
}

impl Stop {
    /// The manifest form; `frames` is the number of frames read.
    pub fn record(&self, frames: u64) -> StopRecord {
        match self {
            Stop::EndOfInput => StopRecord {
                reason: "end_of_input".to_string(),
                detail: None,
            },
            Stop::MessageLimit => StopRecord {
                reason: "message_limit".to_string(),
                detail: Some(format!("stopped after {frames} frames")),
            },
            Stop::Truncated(t) => StopRecord {
                reason: "truncated".to_string(),
                detail: Some(t.to_string()),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceFile {
    /// File name without its directory.
    pub name: String,
    /// `"gzip"` or `"none"`, detected from the content.
    pub compression: String,
    /// Size of the file as stored.
    pub bytes: u64,
    /// SHA-256 of every stored byte, read to the end even when decoding stopped early.
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StopRecord {
    /// `end_of_input`, `message_limit` or `truncated`.
    pub reason: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimestampSummary {
    /// `"nanoseconds since midnight (ITCH)"`.
    pub clock: String,
    pub first_ns: Option<u64>,
    pub last_ns: Option<u64>,
    /// Messages whose timestamp is below the previous message's.
    pub regressions: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SystemEventRecord {
    pub code: String,
    pub timestamp_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BookSummary {
    pub stocks_in_directory: u64,
    pub stocks_with_orders: u64,
    pub orders_added: u64,
    pub orders_replaced: u64,
    pub orders_deleted: u64,
    pub orders_fully_executed: u64,
    pub orders_cancelled_to_zero: u64,
    pub live_orders_at_end: u64,
    pub executions: u64,
    pub executed_shares: u64,
    pub non_printable_executions: u64,
    pub non_cross_trades: u64,
    pub non_cross_shares: u64,
    pub cross_trades: u64,
    pub cross_shares: u64,
    pub zero_share_crosses: u64,
    pub broken_trades: u64,
    pub crossed_entries: u64,
    /// Crossed entries while the stock's last Trading Action was `T` (trading).
    pub crossed_entries_while_trading: u64,
    pub crossed_ns: u64,
    pub locked_entries: u64,
    pub locked_entries_while_trading: u64,
    pub locked_ns: u64,
    pub quarantined_total: u64,
    /// Per [`Quarantine::key`].
    pub quarantined: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StreamSummary {
    pub symbol: String,
    pub locate: u16,
    pub rows: u64,
    pub depth_rows: u64,
    pub trade_rows: u64,
    pub non_printable_skipped: u64,
    pub zero_share_crosses_skipped: u64,
    pub timestamps_held_back: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NormalizationSummary {
    /// Matches `DataLevel::L2MbpWithTrades`.
    pub level: String,
    pub tick_size: f64,
    pub lot_size: f64,
    pub price_unit: String,
    /// `"absent"`: ITCH has no receipt clock.
    pub local_clock: String,
    pub midnight_epoch_ns: i64,
    pub streams: Vec<StreamSummary>,
    /// Requested symbols the directory never named.
    pub unmatched_symbols: Vec<String>,
    pub decisions: Vec<String>,
}

/// Everything a reconstruction run did, deterministic for one input and one set of options.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ItchManifest {
    pub format: String,
    pub source: SourceFile,
    pub license_note: String,
    pub options: ItchOptions,
    pub stop: StopRecord,
    pub frames: u64,
    /// Decompressed bytes consumed by the frames read.
    pub decoded_bytes: u64,
    /// Valid frames per message type, including types that were only length-checked.
    pub messages_by_type: BTreeMap<String, u64>,
    /// Types in `messages_by_type` the decoder does not interpret (skipped, counted).
    pub uninterpreted_types: Vec<String>,
    /// Frames whose length or fields contradict the specification, per type byte.
    pub invalid_by_type: BTreeMap<String, u64>,
    /// Type bytes the specification does not define.
    pub unknown_by_type: BTreeMap<String, u64>,
    pub timestamps: TimestampSummary,
    pub system_events: Vec<SystemEventRecord>,
    pub book: BookSummary,
    pub normalization: NormalizationSummary,
    pub checks: Vec<QualityCheck>,
}

impl ItchManifest {
    /// Worst severity among failed checks.
    pub fn worst(&self) -> Option<Severity> {
        let rank = |s: Severity| match s {
            Severity::Info => 0,
            Severity::Warn => 1,
            Severity::Error => 2,
        };
        self.checks
            .iter()
            .filter(|c| !c.passed)
            .map(|c| c.severity)
            .max_by_key(|&s| rank(s))
    }
}

/// The result of a run: manifest, final books and the selected stocks' rows.
#[derive(Debug)]
pub struct Reconstruction {
    pub manifest: ItchManifest,
    pub books: OrderBooks,
    pub l2: Vec<L2Stream>,
}

/// Decodes, applies and normalizes one frame at a time.
#[derive(Debug)]
pub struct Reconstructor {
    options: ItchOptions,
    frames: u64,
    by_type: [u64; 256],
    invalid_by_type: [u64; 256],
    empty_frames: u64,
    invalid: Finding,
    unknown: Finding,
    first_message: Option<(u8, Option<u8>)>,
    first_ts: Option<u64>,
    prev_ts: Option<u64>,
    regressions: Finding,
    beyond_day: Finding,
    system_events: Vec<SystemEventRecord>,
    books: OrderBooks,
    l2: L2Normalizer,
}

impl Reconstructor {
    pub fn new(options: ItchOptions) -> Self {
        let l2 = L2Normalizer::new(&options.symbols, options.midnight_epoch_ns);
        Self {
            options,
            frames: 0,
            by_type: [0; 256],
            invalid_by_type: [0; 256],
            empty_frames: 0,
            invalid: Finding::default(),
            unknown: Finding::default(),
            first_message: None,
            first_ts: None,
            prev_ts: None,
            regressions: Finding::default(),
            beyond_day: Finding::default(),
            system_events: Vec::new(),
            books: OrderBooks::new(),
            l2,
        }
    }

    pub fn books(&self) -> &OrderBooks {
        &self.books
    }

    pub fn l2(&self) -> &[L2Stream] {
        self.l2.streams()
    }

    pub fn limit_reached(&self) -> bool {
        self.options
            .max_messages
            .is_some_and(|max| self.frames >= max)
    }

    /// Processes one message body (the bytes after its length prefix).
    pub fn push(&mut self, frame: &[u8]) {
        self.frames += 1;
        let n = self.frames;
        let msg = match decode(frame) {
            Ok(msg) => msg,
            Err(err) => {
                match frame.first() {
                    Some(&kind) => self.invalid_by_type[usize::from(kind)] += 1,
                    None => self.empty_frames += 1,
                }
                self.invalid.hit(|| format!("frame {n}: {err}"));
                return;
            }
        };
        let kind = frame[0];
        let Some(header) = msg.header() else {
            self.unknown.hit(|| {
                format!(
                    "frame {n}: type {} ({} bytes)",
                    show_kind(kind),
                    frame.len()
                )
            });
            self.by_type[usize::from(kind)] += 1;
            return;
        };
        self.by_type[usize::from(kind)] += 1;

        let ts = header.timestamp_ns;
        if self.first_message.is_none() {
            let code = match msg {
                Message::SystemEvent(e) => Some(e.event_code),
                _ => None,
            };
            self.first_message = Some((kind, code));
        }
        self.first_ts.get_or_insert(ts);
        if let Some(prev) = self.prev_ts
            && ts < prev
        {
            self.regressions.hit(|| {
                format!(
                    "frame {n} (type {}): {ts} ns after {prev} ns",
                    show_kind(kind)
                )
            });
        }
        self.prev_ts = Some(ts);
        if ts >= NS_PER_DAY {
            self.beyond_day
                .hit(|| format!("frame {n} (type {}): {ts} ns", show_kind(kind)));
        }

        match msg {
            Message::SystemEvent(e) if self.system_events.len() < MAX_SYSTEM_EVENTS => {
                self.system_events.push(SystemEventRecord {
                    code: show_kind(e.event_code),
                    timestamp_ns: ts,
                });
            }
            Message::StockDirectory(d) => self.l2.on_directory(header.stock_locate, d.stock),
            _ => {}
        }
        self.books.apply(&msg);
        self.l2.on_changes(ts, self.books.changes());
    }

    /// Closes the books and assembles the manifest.
    pub fn finish(mut self, source: SourceFile, stop: Stop, decoded_bytes: u64) -> Reconstruction {
        self.books.finish(self.prev_ts.unwrap_or(0));
        let counts = |a: &[u64; 256]| -> BTreeMap<String, u64> {
            (0..=255u8)
                .filter(|&k| a[usize::from(k)] > 0)
                .map(|k| (show_kind(k), a[usize::from(k)]))
                .collect()
        };
        let mut known = [0u64; 256];
        let mut unknown = [0u64; 256];
        for k in 0..=255u8 {
            let slot = if decode::spec_length(k).is_some() {
                &mut known
            } else {
                &mut unknown
            };
            slot[usize::from(k)] = self.by_type[usize::from(k)];
        }
        let mut invalid_by_type = counts(&self.invalid_by_type);
        if self.empty_frames > 0 {
            invalid_by_type.insert("empty".to_string(), self.empty_frames);
        }
        let uninterpreted_types = (0..=255u8)
            .filter(|&k| known[usize::from(k)] > 0 && !interpreted(k))
            .map(show_kind)
            .collect();

        let books = &self.books;
        let stats = books.stats();
        let quarantined = Quarantine::ALL
            .iter()
            .map(|&q| (q.key().to_string(), books.quarantined(q).count))
            .collect();
        let book = BookSummary {
            stocks_in_directory: books.stocks_in_directory() as u64,
            stocks_with_orders: books.stocks_with_orders() as u64,
            orders_added: stats.orders_added,
            orders_replaced: stats.orders_replaced,
            orders_deleted: stats.orders_deleted,
            orders_fully_executed: stats.orders_fully_executed,
            orders_cancelled_to_zero: stats.orders_cancelled_to_zero,
            live_orders_at_end: books.live_orders() as u64,
            executions: stats.executions,
            executed_shares: stats.executed_shares,
            non_printable_executions: stats.non_printable_executions,
            non_cross_trades: stats.non_cross_trades,
            non_cross_shares: stats.non_cross_shares,
            cross_trades: stats.cross_trades,
            cross_shares: stats.cross_shares,
            zero_share_crosses: stats.zero_share_crosses,
            broken_trades: stats.broken_trades,
            crossed_entries: books.crossed().count,
            crossed_entries_while_trading: stats.crossed_entries_while_trading,
            crossed_ns: stats.crossed_ns,
            locked_entries: books.locked().count,
            locked_entries_while_trading: stats.locked_entries_while_trading,
            locked_ns: stats.locked_ns,
            quarantined_total: books.quarantined_total(),
            quarantined,
        };

        let streams: Vec<StreamSummary> = self
            .l2
            .streams()
            .iter()
            .map(|s| StreamSummary {
                symbol: s.symbol.clone(),
                locate: s.locate,
                rows: s.events.len() as u64,
                depth_rows: s.depth_rows,
                trade_rows: s.trade_rows,
                non_printable_skipped: s.non_printable_skipped,
                zero_share_crosses_skipped: s.zero_share_crosses_skipped,
                timestamps_held_back: s.held_back.count,
            })
            .collect();
        let mut held_back = Finding::default();
        for s in self.l2.streams() {
            held_back.count += s.held_back.count;
            for e in &s.held_back.examples {
                if held_back.examples.len() < crate::quality::MAX_EXAMPLES {
                    held_back.examples.push(e.clone());
                }
            }
        }
        let normalization = NormalizationSummary {
            level: "l2_mbp_with_trades".to_string(),
            tick_size: 0.0001,
            lot_size: 1.0,
            price_unit: "1/10,000 USD (ITCH Price(4) integer)".to_string(),
            local_clock: "absent".to_string(),
            midnight_epoch_ns: self.options.midnight_epoch_ns,
            streams,
            unmatched_symbols: self.l2.unmatched(),
            decisions: DECISIONS.iter().map(|d| d.to_string()).collect(),
        };

        let checks = self.checks(&stop, &held_back);
        let stop = stop.record(self.frames);
        let manifest = ItchManifest {
            format: "NASDAQ TotalView-ITCH 5.0, 2-byte big-endian length-prefixed messages"
                .to_string(),
            source,
            license_note: LICENSE_NOTE.to_string(),
            options: self.options.clone(),
            stop,
            frames: self.frames,
            decoded_bytes,
            messages_by_type: counts(&known),
            uninterpreted_types,
            invalid_by_type,
            unknown_by_type: counts(&unknown),
            timestamps: TimestampSummary {
                clock: "nanoseconds since midnight (ITCH)".to_string(),
                first_ns: self.first_ts,
                last_ns: self.prev_ts,
                regressions: self.regressions.count,
            },
            system_events: self.system_events.clone(),
            book,
            normalization,
            checks,
        };
        Reconstruction {
            manifest,
            books: self.books,
            l2: self.l2.into_streams(),
        }
    }

    fn checks(&self, stop: &Stop, held_back: &Finding) -> Vec<QualityCheck> {
        let books = &self.books;
        let mut framing = Finding::default();
        if let Stop::Truncated(t) = stop {
            framing.hit(|| t.to_string());
        }
        let mut start = Finding::default();
        match self.first_message {
            Some((b'S', Some(b'O'))) | None => {}
            Some((kind, code)) => start.hit(|| match code {
                Some(code) => format!("first message is System Event '{}'", show_kind(code)),
                None => format!("first message has type {}", show_kind(kind)),
            }),
        }
        let stats = books.stats();
        let mut checks = vec![
            check(
                "itch.framing",
                "Input ends inside a frame",
                Severity::Error,
                &framing,
                "A truncated input stops decoding; everything after the last complete frame is \
                 missing."
                    .to_string(),
            ),
            check(
                "itch.invalid_message",
                "Frames contradicting the specification",
                Severity::Error,
                &self.invalid,
                "Frames whose length or a coded field does not match the specification are \
                 skipped (the length prefix keeps framing intact) and counted."
                    .to_string(),
            ),
            check(
                "itch.unknown_type",
                "Message types the specification does not define",
                Severity::Warn,
                &self.unknown,
                "Skipped and counted per type byte.".to_string(),
            ),
            check(
                "itch.start_of_messages",
                "Input does not begin with System Event 'O'",
                Severity::Warn,
                &start,
                "The book starts empty at the first message; orders entered before it are \
                 missing and later messages naming them are quarantined."
                    .to_string(),
            ),
            check(
                "time.regression",
                "Timestamps below the previous message's",
                Severity::Warn,
                &self.regressions,
                "Messages are applied in file order regardless; Level-2 rows hold the previous \
                 timestamp (l2.timestamp_held_back)."
                    .to_string(),
            ),
            check(
                "time.beyond_day",
                "Timestamps of 24 hours or more",
                Severity::Warn,
                &self.beyond_day,
                "ITCH timestamps count nanoseconds since midnight of one day.".to_string(),
            ),
        ];
        for q in Quarantine::ALL {
            let f = books.quarantined(q);
            checks.push(check(
                &format!("book.{}", q.key()),
                q.title(),
                Severity::Error,
                f,
                "Quarantined: the message was not applied and nothing was invented in its \
                 place."
                    .to_string(),
            ));
        }
        checks.extend([
            check(
                "book.crossed",
                "Crossed books (best bid above best ask)",
                Severity::Warn,
                books.crossed(),
                format!(
                    "Entries into a crossed state, judged per stock after each batch of messages \
                     sharing one timestamp; {} of them while the stock's trading state was T \
                     (the others came while it was halted, paused, quotation-only or not yet \
                     released, when Nasdaq does not match continuously); {} ns crossed in total \
                     over all stocks. Reported, not repaired.",
                    stats.crossed_entries_while_trading, stats.crossed_ns
                ),
            ),
            check(
                "book.locked",
                "Locked books (best bid equal to best ask)",
                Severity::Warn,
                books.locked(),
                format!(
                    "Entries into a locked state, judged like book.crossed; {} of them while \
                     trading; {} ns locked in total. Reported, not repaired.",
                    stats.locked_entries_while_trading, stats.locked_ns
                ),
            ),
            check(
                "trades.broken",
                "Broken trades",
                Severity::Info,
                &Finding {
                    count: stats.broken_trades,
                    examples: Vec::new(),
                },
                "Broken executions do not change the book (spec 1.5.3); rows already emitted \
                 are not retracted."
                    .to_string(),
            ),
            check(
                "trades.broken_unknown_match",
                "Broken trades naming a match number never printed",
                Severity::Warn,
                books.broken_unknown_match(),
                "The broken execution was not in the input read (a bounded prefix can cause \
                 this)."
                    .to_string(),
            ),
            check(
                "l2.timestamp_held_back",
                "Level-2 rows held at an earlier row's timestamp",
                Severity::Warn,
                held_back,
                "Rows of a stock keep exchange-time order; a row whose message timestamp is \
                 below its predecessor's is emitted at the predecessor's."
                    .to_string(),
            ),
        ]);
        checks
    }
}

/// Types [`decode`] turns into a specific [`Message`] variant.
fn interpreted(kind: u8) -> bool {
    matches!(
        kind,
        b'S' | b'R' | b'H' | b'A' | b'F' | b'E' | b'C' | b'X' | b'D' | b'U' | b'P' | b'Q' | b'B'
    )
}

/// Consumer of message bodies for [`run`].
pub trait FrameSink {
    fn limit_reached(&self) -> bool;
    fn push(&mut self, frame: &[u8]);
}

impl FrameSink for Reconstructor {
    fn limit_reached(&self) -> bool {
        Reconstructor::limit_reached(self)
    }

    fn push(&mut self, frame: &[u8]) {
        Reconstructor::push(self, frame);
    }
}

/// Framing and decoding without any book: per-type counts, for quick file scans and for
/// measuring the decoder alone.
#[derive(Clone, Debug)]
pub struct DecodeScan {
    pub max_messages: Option<u64>,
    pub frames: u64,
    pub invalid: u64,
    by_type: [u64; 256],
}

impl DecodeScan {
    pub fn new(max_messages: Option<u64>) -> Self {
        Self {
            max_messages,
            frames: 0,
            invalid: 0,
            by_type: [0; 256],
        }
    }

    /// Valid frames per type byte (unknown types included).
    pub fn by_type(&self) -> BTreeMap<String, u64> {
        (0..=255u8)
            .filter(|&k| self.by_type[usize::from(k)] > 0)
            .map(|k| (show_kind(k), self.by_type[usize::from(k)]))
            .collect()
    }
}

impl FrameSink for DecodeScan {
    fn limit_reached(&self) -> bool {
        self.max_messages.is_some_and(|max| self.frames >= max)
    }

    fn push(&mut self, frame: &[u8]) {
        self.frames += 1;
        match decode(frame) {
            Ok(_) => self.by_type[usize::from(frame[0])] += 1,
            Err(_) => self.invalid += 1,
        }
    }
}

/// Feeds frames to `sink` until the input ends, the sink's message limit is reached or framing
/// breaks. Returns why it stopped; only I/O failures are errors.
pub fn run<R: BufRead, S: FrameSink>(
    frames: &mut FrameReader<R>,
    sink: &mut S,
) -> Result<Stop, std::io::Error> {
    loop {
        if sink.limit_reached() {
            return Ok(Stop::MessageLimit);
        }
        match frames.next_frame() {
            Ok(Some(frame)) => sink.push(frame),
            Ok(None) => return Ok(Stop::EndOfInput),
            Err(FrameError::Truncated(t)) => return Ok(Stop::Truncated(t)),
            Err(FrameError::Io(err)) => return Err(err),
        }
    }
}

/// Reconstructs from uncompressed bytes in memory (no copy of any frame).
pub fn reconstruct_bytes(name: &str, bytes: &[u8], options: ItchOptions) -> Reconstruction {
    let mut rec = Reconstructor::new(options);
    let mut frames = FrameReader::new(bytes);
    let stop = run(&mut frames, &mut rec).expect("reading a slice cannot fail");
    let decoded = frames.offset();
    let source = SourceFile {
        name: name.to_string(),
        compression: "none".to_string(),
        bytes: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(bytes)),
    };
    rec.finish(source, stop, decoded)
}

/// Reconstructs from a file, plain or gzip-compressed (detected from its first bytes).
pub fn reconstruct_file(path: &Path, options: ItchOptions) -> Result<Reconstruction, DataError> {
    let mut rec = Reconstructor::new(options);
    let (source, stop, decoded) = run_file(path, &mut rec)?;
    Ok(rec.finish(source, stop, decoded))
}

/// Runs a file, plain or gzip-compressed (detected from its first bytes), through `sink`.
/// Returns the source description (hash of the whole stored file), why reading stopped and the
/// decompressed bytes consumed.
pub fn run_file<S: FrameSink>(
    path: &Path,
    sink: &mut S,
) -> Result<(SourceFile, Stop, u64), DataError> {
    let io_err = |source| DataError::io(path, source);
    let mut file = File::open(path).map_err(io_err)?;
    let mut magic = [0u8; 2];
    let mut got = 0;
    while got < magic.len() {
        match file.read(&mut magic[got..]).map_err(io_err)? {
            0 => break,
            n => got += n,
        }
    }
    file.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let gzip = got == 2 && magic == [0x1f, 0x8b];
    let raw = HashingReader::new(file);
    let (stop, decoded, raw) = if gzip {
        let reader = BufReader::with_capacity(READ_BUFFER_BYTES, MultiGzDecoder::new(raw));
        let mut frames = FrameReader::new(reader);
        let stop = run(&mut frames, sink).map_err(io_err)?;
        let decoded = frames.offset();
        (stop, decoded, frames.into_inner().into_inner().into_inner())
    } else {
        let mut frames = FrameReader::new(BufReader::with_capacity(READ_BUFFER_BYTES, raw));
        let stop = run(&mut frames, sink).map_err(io_err)?;
        let decoded = frames.offset();
        (stop, decoded, frames.into_inner().into_inner())
    };
    let (sha256, bytes) = raw.finish().map_err(io_err)?;
    let source = SourceFile {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        compression: if gzip { "gzip" } else { "none" }.to_string(),
        bytes,
        sha256,
    };
    Ok((source, stop, decoded))
}
