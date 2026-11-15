//! Normalization of selected stocks' L3 book changes into BlackBox Level-2 engine rows.
//!
//! Rows follow the event format in docs/MARKET_DATA.md: depth rows carry the new total displayed
//! shares at a price (0 removes the level) and trade rows the traded quantity, both on the
//! exchange clock. ITCH has no receipt clock, so every row carries `EXCH_EVENT` only and its
//! `local_ts` is a copy of `exch_ts` that must not be read as an arrival time. The rules are
//! listed in [`DECISIONS`] and copied into the manifest.

use hftbacktest::types::{BUY_EVENT, DEPTH_EVENT, EXCH_EVENT, SELL_EVENT, TRADE_EVENT};

use super::{
    book::{Change, Print, PrintKind},
    decode::{Price4, Side, Stock},
};
use crate::{Event, make_event, quality::Finding};

/// Normalization rules, recorded verbatim in every manifest.
pub const DECISIONS: &[&str] = &[
    "The book starts empty at the first message and no snapshot rows are emitted: an ITCH day \
     file begins before any order is entered, so a file that does not start with System Event \
     'O' is reported (itch.start_of_messages).",
    "Prices stay integers in 1/10,000 USD (Price(4)) through decoding and reconstruction; an L2 \
     row's px is that integer divided by 10,000 as f64 (tick_size 0.0001, lot_size 1 share).",
    "exch_ts = midnight_epoch_ns + the ITCH timestamp (nanoseconds since midnight, US Eastern \
     time). ITCH files carry no date, so midnight_epoch_ns is supplied by the caller; 0 keeps \
     raw nanoseconds since midnight.",
    "There is no local receipt clock: rows carry EXCH_EVENT only, and local_ts is a copy of \
     exch_ts, not an arrival time. A simulation would need an explicitly assumed feed latency.",
    "Each message emits one depth row per price level it changed, carrying the new total \
     displayed shares (0 removes the level); a replace at an unchanged price emits one row.",
    "An execution of a displayed order (E, printable C) emits a trade row before the depth row \
     it caused, at the execution price, with the aggressor side opposite to the resting order.",
    "A non-printable C execution changes the book but emits no trade row: its shares are part of \
     the later cross print (spec 1.4.2).",
    "A non-cross trade (P) emits a trade row without a side and does not change the displayed \
     book: the Buy/Sell indicator is always 'B' since 2014-07-14 (spec 1.5.1).",
    "A cross trade (Q) emits a trade row without a side and does not change the book (the \
     crossed orders were executed by C messages); a cross with zero shares emits nothing.",
    "A broken trade (B) is counted; trade rows already emitted are not retracted.",
    "A quarantined message changes nothing and emits nothing; no order, side or quantity is ever \
     invented.",
    "A row whose message timestamp is below the previous row of its stock is emitted at that \
     previous timestamp (l2.timestamp_held_back), so the rows stay in exchange-time order; the \
     book itself follows file order.",
    "A row with an overflowing epoch timestamp or a quantity above the exact f64 integer range \
     is refused and reported (l2.invalid_value); no wrapped timestamp or rounded size is emitted.",
    "The displayed book excludes hidden and reserve interest, which ITCH does not publish.",
];

/// Level-2 rows of one stock.
#[derive(Clone, Debug)]
pub struct L2Stream {
    pub symbol: String,
    pub locate: u16,
    pub events: Vec<Event>,
    pub depth_rows: u64,
    pub trade_rows: u64,
    /// Non-printable executions whose shares appear in a later cross print.
    pub non_printable_skipped: u64,
    pub zero_share_crosses_skipped: u64,
    /// Rows emitted at the previous row's timestamp because their own was earlier.
    pub held_back: Finding,
    last_ts: Option<i64>,
}

/// Turns book changes of the selected symbols into engine rows.
#[derive(Debug)]
pub struct L2Normalizer {
    wanted: Vec<String>,
    midnight_epoch_ns: i64,
    /// Stream index per locate.
    by_locate: Vec<Option<usize>>,
    streams: Vec<L2Stream>,
    pub(crate) invalid_values: Finding,
}

impl L2Normalizer {
    /// `symbols` are matched against Stock Directory symbols without their padding.
    pub fn new(symbols: &[String], midnight_epoch_ns: i64) -> Self {
        Self {
            wanted: symbols.to_vec(),
            midnight_epoch_ns,
            by_locate: Vec::new(),
            streams: Vec::new(),
            invalid_values: Finding::default(),
        }
    }

    /// Selects `locate` when its directory symbol was requested. A symbol is bound to the first
    /// locate the directory gives it.
    pub fn on_directory(&mut self, locate: u16, stock: Stock<'_>) {
        let symbol = stock.trimmed();
        if !self.wanted.contains(&symbol) || self.streams.iter().any(|s| s.symbol == symbol) {
            return;
        }
        let i = usize::from(locate);
        if self.by_locate.len() <= i {
            self.by_locate.resize(i + 1, None);
        }
        if self.by_locate[i].is_some() {
            return;
        }
        self.by_locate[i] = Some(self.streams.len());
        self.streams.push(L2Stream {
            symbol,
            locate,
            events: Vec::new(),
            depth_rows: 0,
            trade_rows: 0,
            non_printable_skipped: 0,
            zero_share_crosses_skipped: 0,
            held_back: Finding::default(),
            last_ts: None,
        });
    }

    /// Emits the rows for one message's changes. `timestamp_ns` is the message timestamp.
    pub fn on_changes(&mut self, timestamp_ns: u64, changes: &[Change]) {
        for change in changes {
            let locate = match change {
                Change::Level { locate, .. } => *locate,
                Change::Print(p) => p.locate,
            };
            let Some(Some(index)) = self.by_locate.get(usize::from(locate)) else {
                continue;
            };
            let stream = &mut self.streams[*index];
            let Some(ts) = i64::try_from(timestamp_ns)
                .ok()
                .and_then(|ts| self.midnight_epoch_ns.checked_add(ts))
            else {
                self.invalid_values.hit(|| {
                    format!("locate {locate}: epoch timestamp overflows i64 at {timestamp_ns}")
                });
                continue;
            };
            let shares = match change {
                Change::Level { shares, .. } => *shares,
                Change::Print(p) => p.shares,
            };
            if shares > (1u64 << 53) - 1 {
                self.invalid_values
                    .hit(|| format!("locate {locate}: quantity {shares} exceeds 2^53-1"));
                continue;
            }
            match *change {
                Change::Level {
                    side,
                    price,
                    shares,
                    ..
                } => {
                    let ts = stream.timestamp(ts);
                    stream.events.push(make_event(
                        DEPTH_EVENT | side_flag(side) | EXCH_EVENT,
                        ts,
                        ts,
                        px(price),
                        shares as f64,
                    ));
                    stream.depth_rows += 1;
                }
                Change::Print(p) => stream.print(ts, &p),
            }
        }
    }

    /// Requested symbols the directory never named.
    pub fn unmatched(&self) -> Vec<String> {
        self.wanted
            .iter()
            .filter(|w| !self.streams.iter().any(|s| &s.symbol == *w))
            .cloned()
            .collect()
    }

    pub fn into_streams(self) -> Vec<L2Stream> {
        self.streams
    }

    pub fn streams(&self) -> &[L2Stream] {
        &self.streams
    }
}

impl L2Stream {
    /// The row timestamp for a message: held at the previous row's when the message's is earlier.
    fn timestamp(&mut self, ts: i64) -> i64 {
        match self.last_ts {
            Some(last) if ts < last => {
                let symbol = &self.symbol;
                self.held_back
                    .hit(|| format!("{symbol}: message at {ts} ns emitted at {last} (exch_ts)"));
                last
            }
            _ => {
                self.last_ts = Some(ts);
                ts
            }
        }
    }

    fn print(&mut self, ts: i64, p: &Print) {
        let side = match p.kind {
            PrintKind::Execution {
                printable: false, ..
            } => {
                self.non_printable_skipped += 1;
                return;
            }
            PrintKind::Cross { .. } if p.shares == 0 => {
                self.zero_share_crosses_skipped += 1;
                return;
            }
            PrintKind::Execution { resting, .. } => side_flag(resting.opposite()),
            PrintKind::NonCross | PrintKind::Cross { .. } => 0,
        };
        let ts = self.timestamp(ts);
        self.events.push(make_event(
            TRADE_EVENT | side | EXCH_EVENT,
            ts,
            ts,
            px(p.price),
            p.shares as f64,
        ));
        self.trade_rows += 1;
    }
}

fn side_flag(side: Side) -> u64 {
    match side {
        Side::Buy => BUY_EVENT,
        Side::Sell => SELL_EVENT,
    }
}

/// The only place a price leaves integer form.
fn px(price: Price4) -> f64 {
    f64::from(price.0) / f64::from(Price4::SCALE)
}
