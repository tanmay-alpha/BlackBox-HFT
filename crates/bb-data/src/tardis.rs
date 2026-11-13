//! Ingestion of Tardis.dev CSV files (`trades` and `incremental_book_L2`).
//!
//! The conversion follows the py-hftbacktest reference converter (`hftbacktest/data/utils/
//! tardis.py`, Copyright (c) nkaz001, MIT License) with snapshot mode `process`: snapshot rows
//! become a clear row per side followed by snapshot-level rows, updates become depth rows, and
//! all trade rows are placed ahead of all depth rows before the shared normalization, so that a
//! trade precedes the depth update it caused when both carry the same exchange timestamp.
//!
//! Unlike the reference, nothing is accepted on trust: the header must match the Tardis schema
//! exactly, every row is checked, and any row that cannot be read is a hard error naming the
//! file and line. Rows are never skipped or repaired silently; what is unusual but readable is
//! kept and reported in the quality report.

use std::{
    collections::{HashMap, hash_map::Entry},
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use bb_core::{
    CONTRACT_VERSION,
    dataset::{
        DataLevel, DatasetKind, DatasetManifest, InputFile, InstrumentMeta, LocalTsProvenance,
        SourceInfo, TimestampInfo,
    },
    time::TimeUnit,
};
use flate2::read::MultiGzDecoder;
use hftbacktest::types::{
    BUY_EVENT, DEPTH_CLEAR_EVENT, DEPTH_EVENT, DEPTH_SNAPSHOT_EVENT, SELL_EVENT, TRADE_EVENT,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use crate::quality::DEFAULT_GAP_WARN_NS;
use crate::{
    BuiltDataset, DataError, Event,
    decimal::Decimal,
    is_valid_id, make_event,
    normalize::{DEFAULT_MIN_FEED_LATENCY_NS, NormalizeOptions, normalize},
    quality::{self, Finding, ReportInput, RowPrecision, SourceScan, TradeIdFindings},
};

const TRADES_HEADER: &str = "exchange,symbol,timestamp,local_timestamp,id,side,price,amount";
const BOOK_HEADER: &str =
    "exchange,symbol,timestamp,local_timestamp,is_snapshot,side,price,amount";
const FIELDS: usize = 8;
const READ_BUFFER_BYTES: usize = 1 << 16;

fn default_contract_size() -> f64 {
    1.0
}

fn default_time_unit() -> TimeUnit {
    TimeUnit::Micros
}

fn default_gap_warn_ns() -> i64 {
    DEFAULT_GAP_WARN_NS
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TardisIngestConfig {
    /// Dataset id (`[a-z0-9_-]+`).
    pub id: String,
    pub title: String,
    /// Tardis `trades` file, `.csv` or `.csv.gz`.
    pub trades_path: PathBuf,
    /// Tardis `incremental_book_L2` file, `.csv` or `.csv.gz`.
    pub book_path: PathBuf,
    /// Value every row's `exchange` column must have, e.g. `"binance-futures"`.
    pub exchange: String,
    /// Value every row's `symbol` column must have, e.g. `"BTCUSDT"`.
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    #[serde(default = "default_contract_size")]
    pub contract_size: f64,
    /// Unit of the `timestamp` and `local_timestamp` columns. Tardis CSV uses microseconds. A
    /// value that is not a plausible epoch time under this unit is an error, never a guess.
    #[serde(default = "default_time_unit")]
    pub time_unit: TimeUnit,
    /// Keep only events whose exchange time is at most this long after the first book row.
    ///
    /// The window always starts at the start of the files: the book can only be reconstructed
    /// from its initial snapshot, so a window beginning later would have to drop the snapshot
    /// and with it every level that is not updated afterwards. Both files are still read and
    /// validated to the end; rows beyond the window are just not converted.
    #[serde(default)]
    pub max_duration_ns: Option<i64>,
    /// Added to every local timestamp when the files contain negative feed latency (the
    /// reference converter's `base_latency`).
    #[serde(default)]
    pub base_latency_ns: i64,
    /// Gaps between consecutive exchange timestamps above this are reported.
    #[serde(default = "default_gap_warn_ns")]
    pub gap_warn_ns: i64,
}

fn unit_name(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Seconds => "seconds",
        TimeUnit::Millis => "milliseconds",
        TimeUnit::Micros => "microseconds",
        TimeUnit::Nanos => "nanoseconds",
    }
}

/// Passes bytes through while hashing and counting them, so the manifest records the hash of
/// the file exactly as it is on disk (compressed bytes for `.gz`).
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}

enum Lines {
    Plain(BufReader<HashingReader<File>>),
    Gz(BufReader<MultiGzDecoder<HashingReader<File>>>),
}

/// A Tardis CSV file open for streaming, positioned after its verified header.
struct CsvFile {
    name: String,
    lines: Lines,
    /// 1-based number of the line most recently read (the header is line 1).
    line_no: u64,
}

impl CsvFile {
    fn open(path: &Path, header: &str, description: &str) -> Result<CsvFile, DataError> {
        let file = File::open(path).map_err(|err| DataError::io(path, err))?;
        let raw = HashingReader {
            inner: file,
            hasher: Sha256::new(),
            bytes: 0,
        };
        let gzip = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"));
        let lines = if gzip {
            Lines::Gz(BufReader::with_capacity(
                READ_BUFFER_BYTES,
                MultiGzDecoder::new(raw),
            ))
        } else {
            Lines::Plain(BufReader::with_capacity(READ_BUFFER_BYTES, raw))
        };
        let mut csv = CsvFile {
            name: path.display().to_string(),
            lines,
            line_no: 0,
        };

        let mut line = Vec::new();
        if !csv.read_line(&mut line)? {
            return Err(DataError::Schema {
                file: csv.name,
                message: format!("the file is empty; expected the Tardis {description} header"),
            });
        }
        if line != header.as_bytes() {
            return Err(DataError::Schema {
                file: csv.name,
                message: format!(
                    "not a Tardis {description} file: the header is `{}`, expected `{header}`",
                    String::from_utf8_lossy(&line)
                ),
            });
        }
        Ok(csv)
    }

    /// Reads the next line into `buf` without its line ending. `false` at end of file.
    fn read_line(&mut self, buf: &mut Vec<u8>) -> Result<bool, DataError> {
        buf.clear();
        let read = match &mut self.lines {
            Lines::Plain(reader) => reader.read_until(b'\n', buf),
            Lines::Gz(reader) => reader.read_until(b'\n', buf),
        };
        let n = read.map_err(|source| DataError::Io {
            path: self.name.clone(),
            source,
        })?;
        if n == 0 {
            return Ok(false);
        }
        self.line_no += 1;
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Ok(true)
    }

    fn error(&self, message: String) -> DataError {
        DataError::Parse {
            file: self.name.clone(),
            line: self.line_no,
            message,
        }
    }

    /// Data rows read so far.
    fn data_rows(&self) -> u64 {
        self.line_no.saturating_sub(1)
    }

    /// Hash and size of the whole file as stored.
    fn finish(self) -> Result<(String, u64), DataError> {
        let CsvFile { name, lines, .. } = self;
        let mut raw = match lines {
            Lines::Plain(reader) => reader.into_inner(),
            Lines::Gz(reader) => reader.into_inner().into_inner(),
        };
        // The line reader stops at the end of the content; make sure every stored byte has
        // passed through the hasher.
        let mut sink = vec![0u8; READ_BUFFER_BYTES];
        loop {
            match raw.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => return Err(DataError::Io { path: name, source }),
            }
        }
        Ok((hex::encode(raw.hasher.finalize()), raw.bytes))
    }
}

/// Splits a line on commas (Tardis CSV has no quoting) into exactly [`FIELDS`] fields.
fn split_fields<'a>(file: &CsvFile, line: &'a [u8]) -> Result<[&'a str; FIELDS], DataError> {
    let text = std::str::from_utf8(line)
        .map_err(|_| file.error("the line is not valid UTF-8".to_string()))?;
    let found = text.split(',').count();
    if found != FIELDS {
        return Err(file.error(format!(
            "expected {FIELDS} comma-separated fields, found {found}"
        )));
    }
    let mut fields: [&'a str; FIELDS] = [""; FIELDS];
    for (slot, part) in fields.iter_mut().zip(text.split(',')) {
        *slot = part;
    }
    Ok(fields)
}

/// Per-run parsing context.
struct Ingest<'a> {
    cfg: &'a TardisIngestConfig,
    tick: Decimal,
    lot: Decimal,
    /// Last exchange timestamp that is converted, when a duration limit is set.
    window_end: Option<i64>,
}

impl Ingest<'_> {
    /// Checks the `exchange` and `symbol` columns and converts both timestamps to nanoseconds.
    fn timestamps(
        &self,
        file: &CsvFile,
        fields: &[&str; FIELDS],
    ) -> Result<(i64, i64), DataError> {
        if fields[0] != self.cfg.exchange {
            return Err(file.error(format!(
                "exchange is `{}`, expected `{}`",
                fields[0], self.cfg.exchange
            )));
        }
        if fields[1] != self.cfg.symbol {
            return Err(file.error(format!(
                "symbol is `{}`, expected `{}`",
                fields[1], self.cfg.symbol
            )));
        }
        let exch_ts = self.timestamp(file, "timestamp", fields[2])?;
        let local_ts = self.timestamp(file, "local_timestamp", fields[3])?;
        Ok((exch_ts, local_ts))
    }

    fn timestamp(&self, file: &CsvFile, column: &str, text: &str) -> Result<i64, DataError> {
        let raw: i64 = text
            .parse()
            .map_err(|_| file.error(format!("{column} `{text}` is not an integer")))?;
        let unit = self.cfg.time_unit;
        unit.to_plausible_ns(raw)
            .ok_or_else(|| DataError::Timestamp {
                file: file.name.clone(),
                line: file.line_no,
                message: format!(
                    "{column} {raw} is not a plausible epoch time (2010 to 2100) when read as \
                     {}; check `time_unit`",
                    unit_name(unit)
                ),
            })
    }

    /// Parses a price or amount: the `f64` the engine will see and whether the decimal text is
    /// an exact multiple of `unit`.
    fn number(
        &self,
        file: &CsvFile,
        column: &str,
        text: &str,
        unit: &Decimal,
    ) -> Result<(f64, bool), DataError> {
        let not_a_number = || file.error(format!("{column} `{text}` is not a decimal number"));
        let exact = Decimal::parse(text).ok_or_else(not_a_number)?;
        let value: f64 = text.parse().map_err(|_| not_a_number())?;
        Ok((value, exact.is_multiple_of(unit)))
    }

    fn in_window(&self, exch_ts: i64) -> bool {
        self.window_end.is_none_or(|end| exch_ts <= end)
    }
}

/// What makes two rows with the same trade id the same trade. The receipt time is left out: a
/// trade delivered twice is still one trade.
#[derive(Copy, Clone, PartialEq, Eq)]
struct TradeContent {
    exch_ts: i64,
    side: u64,
    px_bits: u64,
    qty_bits: u64,
}

/// Remembers every trade id of the file. The maps are only ever probed, never iterated, so
/// their unordered layout cannot influence the output.
#[derive(Default)]
struct TradeIds {
    integer: HashMap<u64, TradeContent>,
    text: HashMap<String, TradeContent>,
    last_integer: Option<u64>,
    findings: TradeIdFindings,
}

/// The id as an integer when it is written canonically (digits only, no leading zero), so that
/// `7` and `007` stay different ids.
fn canonical_integer(id: &str) -> Option<u64> {
    let canonical = !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && (id.len() == 1 || !id.starts_with('0'));
    if canonical { id.parse().ok() } else { None }
}

impl TradeIds {
    /// Records one trade row. Returns `false` when the row is an exact duplicate of an earlier
    /// one and must be dropped.
    fn observe(&mut self, id: &str, content: TradeContent, at: impl Fn() -> String) -> bool {
        if id.is_empty() {
            self.findings.rows_without_id += 1;
            return true;
        }
        self.findings.rows_with_id += 1;
        let integer = canonical_integer(id);
        let previous = match integer {
            Some(n) => match self.integer.entry(n) {
                Entry::Occupied(seen) => Some(*seen.get()),
                Entry::Vacant(slot) => {
                    slot.insert(content);
                    None
                }
            },
            None => {
                self.findings.non_integer_ids += 1;
                match self.text.get(id) {
                    Some(seen) => Some(*seen),
                    None => {
                        self.text.insert(id.to_string(), content);
                        None
                    }
                }
            }
        };
        match previous {
            Some(seen) if seen == content => {
                self.findings
                    .exact_duplicates
                    .hit(|| format!("{}: id {id} repeated (row dropped)", at()));
                return false;
            }
            Some(_) => {
                self.findings.conflicting.hit(|| {
                    format!("{}: id {id} repeated with different content (row kept)", at())
                });
            }
            None => {
                if let Some(n) = integer {
                    if let Some(last) = self.last_integer
                        && n != last.wrapping_add(1)
                    {
                        self.findings
                            .gaps
                            .hit(|| format!("{}: id {n} follows id {last}", at()));
                    }
                    self.last_integer = Some(n);
                }
            }
        }
        true
    }
}

struct TradesOutcome {
    rows: Vec<Event>,
    ids: TradeIdFindings,
    input: InputFile,
    /// Rows beyond the duration limit.
    beyond_window: u64,
}

fn read_trades(ingest: &Ingest<'_>, scan: &mut SourceScan) -> Result<TradesOutcome, DataError> {
    let mut file = CsvFile::open(&ingest.cfg.trades_path, TRADES_HEADER, "trades")?;
    let mut rows: Vec<Event> = Vec::new();
    let mut ids = TradeIds::default();
    let mut beyond_window = 0u64;
    let mut line = Vec::new();
    while file.read_line(&mut line)? {
        let fields = split_fields(&file, &line)?;
        let (exch_ts, local_ts) = ingest.timestamps(&file, &fields)?;
        // Tardis writes `buy`, `sell` or `unknown`; only the first two state an aggressor.
        let side = match fields[5] {
            "buy" => BUY_EVENT,
            "sell" => SELL_EVENT,
            _ => 0,
        };
        let (px, price_on_tick) = ingest.number(&file, "price", fields[6], &ingest.tick)?;
        let (qty, qty_on_lot) = ingest.number(&file, "amount", fields[7], &ingest.lot)?;
        if !ingest.in_window(exch_ts) {
            beyond_window += 1;
            continue;
        }
        let at = || format!("{}:{}", file.name, file.line_no);
        let content = TradeContent {
            exch_ts,
            side,
            px_bits: px.to_bits(),
            qty_bits: qty.to_bits(),
        };
        if !ids.observe(fields[4], content, at) {
            continue;
        }
        let ev = make_event(TRADE_EVENT | side, exch_ts, local_ts, px, qty);
        scan.trade(
            &ev,
            RowPrecision {
                price_on_tick,
                qty_on_lot,
            },
            at,
        );
        rows.push(ev);
    }
    let data_rows = file.data_rows();
    let name = file.name.clone();
    let (sha256, bytes) = file.finish()?;
    Ok(TradesOutcome {
        rows,
        ids: ids.findings,
        input: InputFile {
            name,
            role: "trades".to_string(),
            bytes,
            sha256,
            rows: data_rows,
        },
        beyond_window,
    })
}

/// Turns book rows into engine rows, converting each run of snapshot rows into clear rows plus
/// snapshot-level rows (the reference converter's `_convert_depth` in `process` mode).
struct DepthAssembler {
    /// Output; starts out holding the trade rows so the result is "trades, then depth".
    rows: Vec<Event>,
    in_snapshot: bool,
    /// Timestamps of the first row of the snapshot being collected.
    snapshot_ts: (i64, i64),
    snapshot_bids: Vec<Event>,
    snapshot_asks: Vec<Event>,
    snapshots: u64,
    book_rows: u64,
    starts_with_snapshot: bool,
    mid_file_snapshots: Finding,
}

impl DepthAssembler {
    fn new(rows: Vec<Event>) -> Self {
        Self {
            rows,
            in_snapshot: false,
            snapshot_ts: (0, 0),
            snapshot_bids: Vec::new(),
            snapshot_asks: Vec::new(),
            snapshots: 0,
            book_rows: 0,
            starts_with_snapshot: false,
            mid_file_snapshots: Finding::default(),
        }
    }

    /// `side` is `BUY_EVENT` (bid) or `SELL_EVENT` (ask).
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        is_snapshot: bool,
        side: u64,
        exch_ts: i64,
        local_ts: i64,
        px: f64,
        qty: f64,
        at: impl Fn() -> String,
    ) -> Event {
        let first_row = self.book_rows == 0;
        self.book_rows += 1;
        if is_snapshot {
            if !self.in_snapshot {
                self.in_snapshot = true;
                self.snapshots += 1;
                self.snapshot_ts = (exch_ts, local_ts);
                if first_row {
                    self.starts_with_snapshot = true;
                } else {
                    self.mid_file_snapshots
                        .hit(|| format!("{}: snapshot begins at exch_ts {exch_ts}", at()));
                }
            }
            let ev = make_event(DEPTH_SNAPSHOT_EVENT | side, exch_ts, local_ts, px, qty);
            if side == BUY_EVENT {
                self.snapshot_bids.push(ev.clone());
            } else {
                self.snapshot_asks.push(ev.clone());
            }
            ev
        } else {
            if self.in_snapshot {
                self.flush_snapshot();
            }
            let ev = make_event(DEPTH_EVENT | side, exch_ts, local_ts, px, qty);
            self.rows.push(ev.clone());
            ev
        }
    }

    /// Emits the collected snapshot: per side, one clear row covering the snapshot's price range
    /// and then its levels.
    ///
    /// Deviation from the reference: it takes the clear price from the side's last row, which
    /// is only the outermost price if the vendor sorted the snapshot. Here the bid clear uses
    /// the lowest bid price and the ask clear the highest ask price of the snapshot, whatever
    /// the row order. All rows take the clear's place in time from the snapshot's first row.
    fn flush_snapshot(&mut self) {
        self.in_snapshot = false;
        let (exch_ts, local_ts) = self.snapshot_ts;
        if !self.snapshot_bids.is_empty() {
            let lowest = self
                .snapshot_bids
                .iter()
                .map(|e| e.px)
                .fold(f64::INFINITY, f64::min);
            self.rows.push(make_event(
                DEPTH_CLEAR_EVENT | BUY_EVENT,
                exch_ts,
                local_ts,
                lowest,
                0.0,
            ));
            self.rows.append(&mut self.snapshot_bids);
        }
        if !self.snapshot_asks.is_empty() {
            let highest = self
                .snapshot_asks
                .iter()
                .map(|e| e.px)
                .fold(f64::NEG_INFINITY, f64::max);
            self.rows.push(make_event(
                DEPTH_CLEAR_EVENT | SELL_EVENT,
                exch_ts,
                local_ts,
                highest,
                0.0,
            ));
            self.rows.append(&mut self.snapshot_asks);
        }
    }

    /// The reference only emits a snapshot when a non-snapshot row follows it, so a snapshot at
    /// the very end of a file is lost there. Here it is emitted.
    fn finish(&mut self) {
        if self.in_snapshot {
            self.flush_snapshot();
        }
    }
}

struct BookOutcome {
    /// Trade rows followed by depth rows.
    rows: Vec<Event>,
    snapshots: u64,
    book_rows: u64,
    starts_with_snapshot: bool,
    mid_file_snapshots: Finding,
    input: InputFile,
    beyond_window: u64,
}

fn read_book(
    ingest: &Ingest<'_>,
    scan: &mut SourceScan,
    trade_rows: Vec<Event>,
) -> Result<BookOutcome, DataError> {
    let mut file = CsvFile::open(&ingest.cfg.book_path, BOOK_HEADER, "incremental_book_L2")?;
    let mut assembler = DepthAssembler::new(trade_rows);
    let mut beyond_window = 0u64;
    let mut line = Vec::new();
    while file.read_line(&mut line)? {
        let fields = split_fields(&file, &line)?;
        let (exch_ts, local_ts) = ingest.timestamps(&file, &fields)?;
        let is_snapshot = match fields[4] {
            "true" => true,
            "false" => false,
            other => {
                return Err(file.error(format!(
                    "is_snapshot is `{other}`, expected `true` or `false`"
                )));
            }
        };
        let side = match fields[5] {
            "bid" => BUY_EVENT,
            "ask" => SELL_EVENT,
            other => {
                return Err(file.error(format!("side is `{other}`, expected `bid` or `ask`")));
            }
        };
        let (px, price_on_tick) = ingest.number(&file, "price", fields[6], &ingest.tick)?;
        let (qty, qty_on_lot) = ingest.number(&file, "amount", fields[7], &ingest.lot)?;
        if !ingest.in_window(exch_ts) {
            beyond_window += 1;
            continue;
        }
        let at = || format!("{}:{}", file.name, file.line_no);
        let ev = assembler.push(is_snapshot, side, exch_ts, local_ts, px, qty, at);
        scan.book(
            &ev,
            RowPrecision {
                price_on_tick,
                qty_on_lot,
            },
            at,
        );
    }
    assembler.finish();

    let data_rows = file.data_rows();
    let name = file.name.clone();
    let (sha256, bytes) = file.finish()?;
    Ok(BookOutcome {
        rows: assembler.rows,
        snapshots: assembler.snapshots,
        book_rows: assembler.book_rows,
        starts_with_snapshot: assembler.starts_with_snapshot,
        mid_file_snapshots: assembler.mid_file_snapshots,
        input: InputFile {
            name,
            role: "incremental_book_l2".to_string(),
            bytes,
            sha256,
            rows: data_rows,
        },
        beyond_window,
    })
}

/// Exchange timestamp of the first book row, which anchors the duration limit.
fn first_book_exch_ts(ingest: &Ingest<'_>) -> Result<i64, DataError> {
    let mut file = CsvFile::open(&ingest.cfg.book_path, BOOK_HEADER, "incremental_book_L2")?;
    let mut line = Vec::new();
    if !file.read_line(&mut line)? {
        return Err(DataError::Invalid(format!(
            "{}: the book file has no data rows",
            file.name
        )));
    }
    let fields = split_fields(&file, &line)?;
    Ok(ingest.timestamps(&file, &fields)?.0)
}

fn validate(cfg: &TardisIngestConfig) -> Result<(Decimal, Decimal), DataError> {
    let mut problems: Vec<String> = Vec::new();
    if !is_valid_id(&cfg.id) {
        problems.push(format!("id `{}` must match [a-z0-9_-]+", cfg.id));
    }
    if cfg.exchange.is_empty() || cfg.symbol.is_empty() {
        problems.push("exchange and symbol must not be empty".to_string());
    }
    let tick = Decimal::from_positive_f64(cfg.tick_size);
    let lot = Decimal::from_positive_f64(cfg.lot_size);
    if tick.is_none() {
        problems.push("tick_size must be a positive finite number".to_string());
    }
    if lot.is_none() {
        problems.push("lot_size must be a positive finite number".to_string());
    }
    if !cfg.contract_size.is_finite() || cfg.contract_size <= 0.0 {
        problems.push("contract_size must be a positive finite number".to_string());
    }
    if cfg.max_duration_ns.is_some_and(|d| d <= 0) {
        problems.push("max_duration_ns must be positive when given".to_string());
    }
    if cfg.base_latency_ns < 0 {
        problems.push("base_latency_ns must not be negative".to_string());
    }
    if cfg.gap_warn_ns <= 0 {
        problems.push("gap_warn_ns must be positive".to_string());
    }
    match (tick, lot) {
        (Some(tick), Some(lot)) if problems.is_empty() => Ok((tick, lot)),
        _ => Err(DataError::Invalid(format!(
            "invalid Tardis ingest configuration: {}",
            problems.join("; ")
        ))),
    }
}

/// Converts one pair of Tardis files into a dataset.
pub fn ingest(cfg: &TardisIngestConfig) -> Result<BuiltDataset, DataError> {
    let (tick, lot) = validate(cfg)?;
    let instrument = InstrumentMeta {
        tick_size: cfg.tick_size,
        lot_size: cfg.lot_size,
        contract_size: cfg.contract_size,
    };
    let mut context = Ingest {
        cfg,
        tick,
        lot,
        window_end: None,
    };
    if let Some(max_duration_ns) = cfg.max_duration_ns {
        let first = first_book_exch_ts(&context)?;
        context.window_end = Some(first.saturating_add(max_duration_ns));
    }

    let mut scan = SourceScan::default();
    let trades = read_trades(&context, &mut scan)?;
    let book = read_book(&context, &mut scan, trades.rows)?;
    if book.book_rows == 0 {
        return Err(DataError::Invalid(format!(
            "{}: no book rows to convert; a dataset needs an order book",
            book.input.name
        )));
    }

    let normalized = normalize(
        book.rows,
        &NormalizeOptions {
            base_latency_ns: cfg.base_latency_ns,
            min_feed_latency_ns: Some(DEFAULT_MIN_FEED_LATENCY_NS),
        },
    );
    let analysis = quality::analyze(&ReportInput {
        events: &normalized.events,
        instrument: &instrument,
        scan: &scan,
        trade_ids: Some(&trades.ids),
        mid_file_snapshots: &book.mid_file_snapshots,
        snapshots: book.snapshots,
        starts_with_snapshot: book.starts_with_snapshot,
        local_ts_shift_ns: normalized.local_ts_shift_ns,
        gap_warn_ns: cfg.gap_warn_ns,
        precision_basis: "exact decimal comparison of the source text",
    })?;

    let mut note = "Local timestamps are the Tardis `local_timestamp` column: the arrival time \
                    measured at the vendor's capture location, not at the place a strategy \
                    would run. Feed latency in this dataset is a measurement for that location \
                    only."
        .to_string();
    if let Some(max_duration_ns) = cfg.max_duration_ns {
        note.push_str(&format!(
            " Only events up to {max_duration_ns} ns after the first book row were converted; \
             {} trade rows and {} book rows after that were left out.",
            trades.beyond_window, book.beyond_window
        ));
    }

    let origin_ns = analysis.first_exch_ts;
    let manifest = DatasetManifest {
        contract_version: CONTRACT_VERSION,
        id: cfg.id.clone(),
        title: cfg.title.clone(),
        kind: DatasetKind::Recorded,
        level: DataLevel::L2MbpWithTrades,
        source: SourceInfo {
            vendor: "tardis.dev".to_string(),
            exchange: cfg.exchange.clone(),
            symbol: cfg.symbol.clone(),
            inputs: vec![trades.input, book.input],
            license_note: "Vendor data. Not redistributable; obtain it from the vendor under \
                           its terms. This dataset directory must not be committed or \
                           published."
                .to_string(),
            exch_ts_semantics: "Tardis `timestamp` column (exchange-provided event time; for \
                                Binance Futures this is the message event time 'E', not the \
                                matching-engine transaction time 'T')"
                .to_string(),
        },
        instrument,
        origin_ns: origin_ns.to_string(),
        start: 0,
        end: analysis.last_exch_ts - origin_ns,
        timestamps: TimestampInfo {
            input_unit: cfg.time_unit,
            local_ts: LocalTsProvenance::Measured,
            local_ts_shift_ns: normalized.local_ts_shift_ns,
            note,
        },
        counts: analysis.counts,
        events_file: String::new(),
        events_sha256: String::new(),
        events_bytes: 0,
        generator: None,
        quality: analysis.report,
    };

    Ok(BuiltDataset {
        manifest,
        events: normalized.events,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use bb_core::{
        dataset::{QualityCheck, Severity},
        hash::sha256_hex,
    };
    use hftbacktest::types::{EXCH_EVENT, LOCAL_EVENT};

    use super::*;
    use crate::{book::L2Book, has, kind, normalize::validate_event_order, testutil::TempDir};

    /// 2020-02-01T00:00:00Z in microseconds, the unit Tardis CSV uses.
    const T0_US: i64 = 1_580_515_200_000_000;
    const T0_NS: i64 = T0_US * 1_000;

    /// A book row `us` microseconds after `T0`, received 5 ms later.
    fn book_row(us: i64, snapshot: bool, side: &str, price: &str, amount: &str) -> String {
        format!(
            "binance-futures,BTCUSDT,{},{},{snapshot},{side},{price},{amount}\n",
            T0_US + us,
            T0_US + us + 5_000
        )
    }

    /// A trade row `us` microseconds after `T0`, received 3 ms later.
    fn trade_row(us: i64, id: &str, side: &str, price: &str, amount: &str) -> String {
        format!(
            "binance-futures,BTCUSDT,{},{},{id},{side},{price},{amount}\n",
            T0_US + us,
            T0_US + us + 3_000
        )
    }

    fn with_header(header: &str, rows: &[String]) -> String {
        let mut text = format!("{header}\n");
        for row in rows {
            text.push_str(row);
        }
        text
    }

    fn initial_snapshot() -> Vec<String> {
        vec![
            book_row(0, true, "bid", "9349.5", "2.5"),
            book_row(0, true, "bid", "9349", "1"),
            book_row(0, true, "ask", "9350", "3"),
            book_row(0, true, "ask", "9350.5", "4.25"),
        ]
    }

    fn base_book() -> String {
        let mut rows = initial_snapshot();
        rows.push(book_row(100_000, false, "ask", "9350", "2.5"));
        rows.push(book_row(200_000, false, "bid", "9349.5", "0"));
        with_header(BOOK_HEADER, &rows)
    }

    fn base_trades() -> String {
        with_header(
            TRADES_HEADER,
            &[
                trade_row(100_000, "1001", "buy", "9350", "0.5"),
                trade_row(150_000, "1002", "sell", "9349.5", "0.25"),
            ],
        )
    }

    fn config(trades_path: PathBuf, book_path: PathBuf) -> TardisIngestConfig {
        TardisIngestConfig {
            id: "tardis-test".to_string(),
            title: "Tardis test".to_string(),
            trades_path,
            book_path,
            exchange: "binance-futures".to_string(),
            symbol: "BTCUSDT".to_string(),
            tick_size: 0.5,
            lot_size: 0.001,
            contract_size: 1.0,
            time_unit: TimeUnit::Micros,
            max_duration_ns: None,
            base_latency_ns: 0,
            gap_warn_ns: DEFAULT_GAP_WARN_NS,
        }
    }

    /// Writes both files as plain CSV into `dir` and returns a configuration for them.
    fn plain(dir: &TempDir, trades: &str, book: &str) -> TardisIngestConfig {
        let trades_path = dir.path().join("trades.csv");
        let book_path = dir.path().join("book.csv");
        std::fs::write(&trades_path, trades).unwrap();
        std::fs::write(&book_path, book).unwrap();
        config(trades_path, book_path)
    }

    fn write_gz(path: &Path, text: &str) {
        let file = File::create(path).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap();
    }

    fn find<'a>(ds: &'a BuiltDataset, id: &str) -> &'a QualityCheck {
        ds.manifest
            .quality
            .checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("check {id} missing"))
    }

    fn replay(ds: &BuiltDataset) -> L2Book {
        let mut book = L2Book::new();
        for ev in ds.events.iter().filter(|e| has(e, EXCH_EVENT)) {
            book.apply(ev, &ds.manifest.instrument);
        }
        book
    }

    #[test]
    fn snapshot_updates_and_trades_are_converted() {
        let dir = TempDir::new("tardis-basic");
        let cfg = plain(&dir, &base_trades(), &base_book());
        let ds = ingest(&cfg).unwrap();
        let both = EXCH_EVENT | LOCAL_EVENT;

        // 2 trades + (clear + 2 levels) per side + 2 updates, none split.
        assert_eq!(ds.events.len(), 10);
        validate_event_order(&ds.events).unwrap();
        let row = |i: usize| {
            let e = &ds.events[i];
            (e.ev, e.exch_ts - T0_NS, e.local_ts - T0_NS, e.px, e.qty)
        };
        // Microseconds became nanoseconds.
        assert_eq!(ds.events[0].exch_ts, 1_580_515_200_000_000_000);
        assert_eq!(row(0), (DEPTH_CLEAR_EVENT | BUY_EVENT | both, 0, 5_000_000, 9349.0, 0.0));
        assert_eq!(row(1), (DEPTH_SNAPSHOT_EVENT | BUY_EVENT | both, 0, 5_000_000, 9349.5, 2.5));
        assert_eq!(row(2), (DEPTH_SNAPSHOT_EVENT | BUY_EVENT | both, 0, 5_000_000, 9349.0, 1.0));
        assert_eq!(row(3), (DEPTH_CLEAR_EVENT | SELL_EVENT | both, 0, 5_000_000, 9350.5, 0.0));
        assert_eq!(row(4), (DEPTH_SNAPSHOT_EVENT | SELL_EVENT | both, 0, 5_000_000, 9350.0, 3.0));
        assert_eq!(row(5), (DEPTH_SNAPSHOT_EVENT | SELL_EVENT | both, 0, 5_000_000, 9350.5, 4.25));
        // Same exchange timestamp: the trade comes before the depth update it caused.
        assert_eq!(
            row(6),
            (TRADE_EVENT | BUY_EVENT | both, 100_000_000, 103_000_000, 9350.0, 0.5)
        );
        assert_eq!(
            row(7),
            (DEPTH_EVENT | SELL_EVENT | both, 100_000_000, 105_000_000, 9350.0, 2.5)
        );
        assert_eq!(
            row(8),
            (TRADE_EVENT | SELL_EVENT | both, 150_000_000, 153_000_000, 9349.5, 0.25)
        );
        assert_eq!(
            row(9),
            (DEPTH_EVENT | BUY_EVENT | both, 200_000_000, 205_000_000, 9349.5, 0.0)
        );

        let m = &ds.manifest;
        assert_eq!(m.kind, DatasetKind::Recorded);
        assert_eq!(m.origin_ns, "1580515200000000000");
        assert_eq!((m.start, m.end), (0, 200_000_000));
        assert_eq!(m.timestamps.input_unit, TimeUnit::Micros);
        assert_eq!(m.timestamps.local_ts, LocalTsProvenance::Measured);
        assert_eq!(m.timestamps.local_ts_shift_ns, 0);
        assert!(m.timestamps.note.contains("vendor's capture location"));
        assert!(m.source.license_note.contains("Not redistributable"));
        assert!(m.source.exch_ts_semantics.contains("'E'"));
        assert_eq!(m.generator, None);
        assert_eq!(
            (m.counts.rows, m.counts.trades, m.counts.depth_updates),
            (10, 2, 2)
        );
        assert_eq!(
            (m.counts.snapshot_levels, m.counts.snapshots, m.counts.depth_clears),
            (4, 1, 2)
        );

        // Inputs are identified by the hash of the bytes on disk.
        let trades_bytes = std::fs::read(&cfg.trades_path).unwrap();
        let book_bytes = std::fs::read(&cfg.book_path).unwrap();
        assert_eq!(m.source.inputs.len(), 2);
        assert_eq!(m.source.inputs[0].role, "trades");
        assert_eq!(m.source.inputs[0].rows, 2);
        assert_eq!(m.source.inputs[0].bytes, trades_bytes.len() as u64);
        assert_eq!(m.source.inputs[0].sha256, sha256_hex(&trades_bytes));
        assert_eq!(m.source.inputs[1].role, "incremental_book_l2");
        assert_eq!(m.source.inputs[1].rows, 6);
        assert_eq!(m.source.inputs[1].sha256, sha256_hex(&book_bytes));

        assert!(m.quality.checks.iter().all(|c| c.passed), "{:#?}", m.quality.checks);
        assert_eq!(m.quality.book_valid_fraction, 1.0);
        let latency = m.quality.feed_latency_ns.unwrap();
        assert_eq!((latency.min, latency.max), (3_000_000.0, 5_000_000.0));

        let book = replay(&ds);
        assert_eq!(book.best_bid(), Some((18_698, 1.0)));
        assert_eq!(book.best_ask(), Some((18_700, 2.5)));
    }

    #[test]
    fn gzip_input_gives_the_same_events_and_hashes_the_compressed_bytes() {
        let dir = TempDir::new("tardis-gz");
        let plain_ds = ingest(&plain(&dir, &base_trades(), &base_book())).unwrap();

        let trades_path = dir.path().join("trades.csv.gz");
        let book_path = dir.path().join("book.csv.gz");
        write_gz(&trades_path, &base_trades());
        write_gz(&book_path, &base_book());
        let gz_ds = ingest(&config(trades_path.clone(), book_path.clone())).unwrap();

        assert_eq!(gz_ds.events, plain_ds.events);
        assert_eq!(gz_ds.manifest.counts, plain_ds.manifest.counts);
        for (input, path) in gz_ds.manifest.source.inputs.iter().zip([&trades_path, &book_path]) {
            let stored = std::fs::read(path).unwrap();
            assert_eq!(input.bytes, stored.len() as u64);
            assert_eq!(input.sha256, sha256_hex(&stored));
        }
        assert_eq!(gz_ds.manifest.source.inputs[1].rows, 6);
    }

    #[test]
    fn declaring_the_wrong_time_unit_is_an_error() {
        let dir = TempDir::new("tardis-unit");
        for (unit, name) in [
            (TimeUnit::Millis, "milliseconds"),
            (TimeUnit::Nanos, "nanoseconds"),
            (TimeUnit::Seconds, "seconds"),
        ] {
            let mut cfg = plain(&dir, &base_trades(), &base_book());
            cfg.time_unit = unit;
            match ingest(&cfg) {
                Err(DataError::Timestamp {
                    file,
                    line,
                    message,
                }) => {
                    assert!(file.ends_with("trades.csv"), "{file}");
                    assert_eq!(line, 2);
                    assert!(message.contains(name), "{message}");
                }
                other => panic!("expected a timestamp error, got {:?}", other.map(|_| ())),
            }
        }
    }

    fn ingest_error(trades: &str, book: &str) -> DataError {
        let dir = TempDir::new("tardis-bad");
        match ingest(&plain(&dir, trades, book)) {
            Err(err) => err,
            Ok(_) => panic!("ingestion should have failed"),
        }
    }

    fn assert_parse_error(err: DataError, file_name: &str, expected_line: u64, needle: &str) {
        match err {
            DataError::Parse {
                file,
                line,
                message,
            } => {
                assert!(file.ends_with(file_name), "{file}");
                assert_eq!(line, expected_line, "{message}");
                assert!(message.contains(needle), "{message}");
            }
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn malformed_rows_report_file_and_line() {
        // Wrong field count on line 3 of the trades file.
        let trades = with_header(
            TRADES_HEADER,
            &[
                trade_row(100_000, "1001", "buy", "9350", "0.5"),
                "binance-futures,BTCUSDT,1580515200150000,1580515200153000,1002,sell,9349.5\n"
                    .to_string(),
            ],
        );
        assert_parse_error(ingest_error(&trades, &base_book()), "trades.csv", 3, "found 7");

        // Unparseable numbers.
        for bad in ["12a", "nan", "", "1,5"] {
            let trades = with_header(
                TRADES_HEADER,
                &[trade_row(100_000, "1001", "buy", bad, "0.5")],
            );
            let err = ingest_error(&trades, &base_book());
            assert!(matches!(err, DataError::Parse { line: 2, .. }), "{bad}: {err:?}");
        }
        let trades = with_header(
            TRADES_HEADER,
            &[trade_row(100_000, "1001", "buy", "9350", "lots")],
        );
        assert_parse_error(ingest_error(&trades, &base_book()), "trades.csv", 2, "amount");

        // Unknown book side on line 4, unknown is_snapshot value on line 6.
        let mut rows = initial_snapshot();
        rows[2] = book_row(0, true, "middle", "9350", "3");
        let book = with_header(BOOK_HEADER, &rows);
        assert_parse_error(ingest_error(&base_trades(), &book), "book.csv", 4, "side");

        let mut rows = initial_snapshot();
        rows.push(book_row(100_000, false, "ask", "9350", "2.5").replace(",false,", ",no,"));
        let book = with_header(BOOK_HEADER, &rows);
        assert_parse_error(ingest_error(&base_trades(), &book), "book.csv", 6, "is_snapshot");

        // A row of another instrument.
        let mut rows = initial_snapshot();
        rows.push(book_row(100_000, false, "ask", "9350", "2.5").replace("BTCUSDT", "ETHUSDT"));
        let book = with_header(BOOK_HEADER, &rows);
        assert_parse_error(ingest_error(&base_trades(), &book), "book.csv", 6, "symbol");

        // A non-integer timestamp.
        let trades = base_trades().replace("1580515200150000,", "15805152001500.5,");
        assert_parse_error(ingest_error(&trades, &base_book()), "trades.csv", 3, "timestamp");
    }

    #[test]
    fn a_file_with_the_wrong_header_is_rejected_by_name() {
        // The two files swapped.
        match ingest_error(&base_book(), &base_trades()) {
            DataError::Schema { file, message } => {
                assert!(file.ends_with("trades.csv"), "{file}");
                assert!(message.contains("is_snapshot"), "{message}");
            }
            other => panic!("expected a schema error, got {other:?}"),
        }
        match ingest_error(&base_trades(), "") {
            DataError::Schema { file, .. } => assert!(file.ends_with("book.csv"), "{file}"),
            other => panic!("expected a schema error, got {other:?}"),
        }
        let dir = TempDir::new("tardis-missing");
        let mut cfg = plain(&dir, &base_trades(), &base_book());
        cfg.book_path = dir.path().join("absent.csv");
        assert!(matches!(ingest(&cfg), Err(DataError::Io { .. })));
    }

    #[test]
    fn precision_and_trade_id_problems_reach_the_quality_report() {
        let dir = TempDir::new("tardis-quality");
        let trades = with_header(
            TRADES_HEADER,
            &[
                trade_row(100_000, "1001", "buy", "9350", "0.5"),
                // The same trade delivered again, later: dropped.
                trade_row(100_000, "1001", "buy", "9350", "0.5").replace("103000,", "109000,"),
                // Id 1002 is missing; the price is between ticks.
                trade_row(120_000, "1003", "sell", "9349.75", "0.25"),
                // Same id, different content: kept and reported as an error.
                trade_row(130_000, "1003", "sell", "9349.5", "0.25"),
                // No aggressor side; half a lot.
                trade_row(140_000, "1004", "unknown", "9349.5", "0.0005"),
            ],
        );
        let ds = ingest(&plain(&dir, &trades, &base_book())).unwrap();
        assert_eq!(ds.manifest.counts.trades, 4);
        assert_eq!(ds.manifest.source.inputs[0].rows, 5);
        let kept_1001 = ds
            .events
            .iter()
            .filter(|e| has(e, EXCH_EVENT) && kind(e) == TRADE_EVENT && e.px == 9350.0)
            .count();
        assert_eq!(kept_1001, 1);

        let off_tick = find(&ds, "precision.price_off_tick");
        assert_eq!((off_tick.count, off_tick.severity, off_tick.passed), (1, Severity::Error, false));
        assert!(off_tick.examples[0].contains("trades.csv:4"), "{:?}", off_tick.examples);
        let off_lot = find(&ds, "precision.qty_off_lot");
        assert_eq!((off_lot.count, off_lot.passed), (1, false));

        let dup = find(&ds, "trades.duplicate_id");
        assert_eq!((dup.count, dup.severity, dup.passed), (2, Severity::Error, false));
        assert!(dup.detail.contains("1 rows repeat an earlier trade id with the same"));
        assert!(dup.examples.iter().any(|e| e.contains("trades.csv:3")));
        assert!(dup.examples.iter().any(|e| e.contains("trades.csv:5")));

        let gap = find(&ds, "trades.id_gap");
        assert_eq!((gap.count, gap.severity), (1, Severity::Warn));
        assert!(gap.examples[0].contains("id 1003 follows id 1001"));

        let missing_side = find(&ds, "trades.missing_side");
        assert_eq!((missing_side.count, missing_side.severity), (1, Severity::Info));
        let sideless = ds
            .events
            .iter()
            .filter(|e| kind(e) == TRADE_EVENT && !has(e, BUY_EVENT) && !has(e, SELL_EVENT))
            .count();
        assert_eq!(sideless, 1);
        assert_eq!(ds.manifest.quality.worst(), Some(Severity::Error));
    }

    fn resnapshot_rows() -> Vec<String> {
        vec![
            // Deliberately not sorted outermost-last: the clear prices must not depend on it.
            book_row(300_000, true, "bid", "9348", "1"),
            book_row(300_000, true, "bid", "9348.5", "2"),
            book_row(300_000, true, "ask", "9351.5", "1"),
            book_row(300_000, true, "ask", "9351", "2"),
        ]
    }

    #[test]
    fn a_mid_file_snapshot_clears_its_range_and_is_reported() {
        let dir = TempDir::new("tardis-resnapshot");
        let mut rows = initial_snapshot();
        rows.push(book_row(100_000, false, "ask", "9350", "2.5"));
        rows.extend(resnapshot_rows());
        rows.push(book_row(400_000, false, "bid", "9348.5", "3"));
        let book = with_header(BOOK_HEADER, &rows);
        // A trades file with no rows is valid.
        let ds = ingest(&plain(&dir, &with_header(TRADES_HEADER, &[]), &book)).unwrap();

        let c = ds.manifest.counts;
        assert_eq!((c.trades, c.depth_updates, c.snapshot_levels), (0, 2, 8));
        assert_eq!((c.snapshots, c.depth_clears, c.rows), (2, 4, 14));
        let clears: Vec<(u64, i64, f64)> = ds
            .events
            .iter()
            .filter(|e| has(e, EXCH_EVENT) && kind(e) == DEPTH_CLEAR_EVENT)
            .map(|e| (e.ev & (BUY_EVENT | SELL_EVENT), e.exch_ts - T0_NS, e.px))
            .collect();
        assert_eq!(
            clears,
            vec![
                (BUY_EVENT, 0, 9349.0),
                (SELL_EVENT, 0, 9350.5),
                // Lowest bid and highest ask of the second snapshot.
                (BUY_EVENT, 300_000_000, 9348.0),
                (SELL_EVENT, 300_000_000, 9351.5),
            ]
        );

        // The old levels inside the snapshot's range are gone.
        let book = replay(&ds);
        assert_eq!(book.top_bids(10), vec![(18_697, 3.0), (18_696, 1.0)]);
        assert_eq!(book.top_asks(10), vec![(18_702, 2.0), (18_703, 1.0)]);

        let check = find(&ds, "depth.mid_file_snapshot");
        assert_eq!((check.count, check.severity, check.passed), (1, Severity::Warn, false));
        assert!(check.examples[0].contains("book.csv:7"), "{:?}", check.examples);
        assert!(find(&ds, "depth.initial_snapshot").passed);
        assert!(find(&ds, "trades.id_gap").detail.contains("Not evaluated"));
    }

    #[test]
    fn a_snapshot_at_the_end_of_the_file_is_not_lost() {
        let dir = TempDir::new("tardis-tail");
        let mut rows = initial_snapshot();
        rows.push(book_row(100_000, false, "ask", "9350", "2.5"));
        rows.extend(resnapshot_rows());
        let book = with_header(BOOK_HEADER, &rows);
        let ds = ingest(&plain(&dir, &base_trades(), &book)).unwrap();

        let c = ds.manifest.counts;
        assert_eq!((c.snapshot_levels, c.snapshots, c.depth_clears), (8, 2, 4));
        assert_eq!(ds.manifest.end, 300_000_000);
        let last = ds.events.last().unwrap();
        assert_eq!(kind(last), DEPTH_SNAPSHOT_EVENT);
        assert_eq!((last.exch_ts - T0_NS, last.px, last.qty), (300_000_000, 9351.0, 2.0));
        let book = replay(&ds);
        assert_eq!(book.top_bids(10), vec![(18_697, 2.0), (18_696, 1.0)]);
        assert_eq!(book.top_asks(10), vec![(18_702, 2.0), (18_703, 1.0)]);
    }

    #[test]
    fn the_duration_limit_keeps_the_start_of_the_files() {
        let dir = TempDir::new("tardis-window");
        let mut cfg = plain(&dir, &base_trades(), &base_book());
        cfg.max_duration_ns = Some(150_000_000);
        let ds = ingest(&cfg).unwrap();
        let m = &ds.manifest;
        // The update at +200 ms is beyond the limit; the trade exactly at +150 ms is inside.
        assert_eq!((m.counts.trades, m.counts.depth_updates), (2, 1));
        assert_eq!(m.end, 150_000_000);
        // The files are still described in full.
        assert_eq!(m.source.inputs[1].rows, 6);
        assert!(m.timestamps.note.contains("0 trade rows and 1 book rows"));

        cfg.max_duration_ns = Some(0);
        assert!(matches!(ingest(&cfg), Err(DataError::Invalid(_))));
    }

    #[test]
    fn a_book_without_an_initial_snapshot_and_skewed_clocks_are_reported() {
        let dir = TempDir::new("tardis-skew");
        // Updates only, and the second row was "received" 2 ms before it happened.
        let book = with_header(
            BOOK_HEADER,
            &[
                book_row(0, false, "bid", "9349.5", "2.5"),
                book_row(100_000, false, "ask", "9350", "3").replace(
                    &(T0_US + 105_000).to_string(),
                    &(T0_US + 98_000).to_string(),
                ),
            ],
        );
        let ds = ingest(&plain(&dir, &with_header(TRADES_HEADER, &[]), &book)).unwrap();

        let snapshot = find(&ds, "depth.initial_snapshot");
        assert_eq!((snapshot.passed, snapshot.severity), (false, Severity::Warn));
        let skew = find(&ds, "time.local_before_exch");
        assert_eq!((skew.count, skew.passed), (1, false));
        // Worst latency was -2 ms; the shift lifts it to +1 ns.
        assert_eq!(ds.manifest.timestamps.local_ts_shift_ns, 2_000_001);
        assert!(skew.detail.contains("2000001"));
        assert!(ds.events.iter().all(|e| e.local_ts > e.exch_ts));
        validate_event_order(&ds.events).unwrap();
    }

    #[test]
    fn configuration_json_defaults_match_the_documented_ones() {
        let cfg: TardisIngestConfig = serde_json::from_str(
            r#"{"id":"x","title":"X","trades_path":"t.csv","book_path":"b.csv",
                "exchange":"binance-futures","symbol":"BTCUSDT","tick_size":0.1,"lot_size":0.001}"#,
        )
        .unwrap();
        assert_eq!(cfg.contract_size, 1.0);
        assert_eq!(cfg.time_unit, TimeUnit::Micros);
        assert_eq!(cfg.max_duration_ns, None);
        assert_eq!(cfg.base_latency_ns, 0);
        assert_eq!(cfg.gap_warn_ns, 5_000_000_000);
    }
}
