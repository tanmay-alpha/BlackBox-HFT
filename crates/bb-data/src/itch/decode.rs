//! ITCH 5.0 framing and zero-copy message decoding.
//!
//! Layouts are the offset/length tables of "Nasdaq TotalView-ITCH 5.0" (§ numbers below refer to
//! the revision whose log ends 2023-04-28). From its "Data Types" section: every integer is big
//! endian and unsigned; alpha fields are ASCII, left justified and padded with spaces; `Price (4)`
//! is an integer with four implied decimal places; timestamps are nanoseconds since midnight.
//!
//! Every message starts with the same 11 bytes (offsets 0..11 of each table): message type (1),
//! stock locate (2), tracking number (2) and a 6-byte timestamp. Decoded messages borrow their
//! alpha fields from the frame and copy only integers out of it; prices stay integers.
//!
//! Files: NASDAQ's `*.NASDAQ_ITCH50` files store each message behind a 2-byte big-endian length,
//! the same framing as a SoupBinTCP/MoldUDP64 message block. [`FrameReader`] splits that format.

use std::{
    fmt,
    io::{self, BufRead},
};

/// Length of the header shared by every message: type, locate, tracking number, timestamp.
pub const HEADER_LEN: usize = 11;

/// Message length in bytes for every type the specification defines, `None` for any other byte.
pub fn spec_length(kind: u8) -> Option<usize> {
    Some(match kind {
        b'S' => 12, // §1.1   System Event
        b'R' => 39, // §1.2.1 Stock Directory
        b'H' => 25, // §1.2.2 Stock Trading Action
        b'Y' => 20, // §1.2.3 Reg SHO Short Sale Price Test Restricted Indicator
        b'L' => 26, // §1.2.4 Market Participant Position
        b'V' => 35, // §1.2.5.1 MWCB Decline Level
        b'W' => 12, // §1.2.5.2 MWCB Status
        b'K' => 28, // §1.2.6 IPO Quoting Period Update
        b'J' => 35, // §1.2.7 LULD Auction Collar
        b'h' => 21, // §1.2.8 Operational Halt
        b'A' => 36, // §1.3.1 Add Order, no MPID attribution
        b'F' => 40, // §1.3.2 Add Order with MPID attribution
        b'E' => 31, // §1.4.1 Order Executed
        b'C' => 36, // §1.4.2 Order Executed With Price
        b'X' => 23, // §1.4.3 Order Cancel
        b'D' => 19, // §1.4.4 Order Delete
        b'U' => 35, // §1.4.5 Order Replace
        b'P' => 44, // §1.5.1 Trade (non-cross)
        b'Q' => 40, // §1.5.2 Cross Trade
        b'B' => 19, // §1.5.3 Broken Trade
        b'I' => 50, // §1.6   Net Order Imbalance Indicator
        b'N' => 20, // §1.7   Retail Price Improvement Indicator
        b'O' => 48, // §1.8   Direct Listing with Capital Raise Price Discovery
        _ => return None,
    })
}

/// `Price (4)`: an unsigned integer in units of 1/10,000 (spec "Data Types"). Never converted to
/// floating point by the decoder or the book.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price4(pub u32);

impl Price4 {
    /// Units per whole currency unit.
    pub const SCALE: u32 = 10_000;
}

impl fmt::Display for Price4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:04}", self.0 / Self::SCALE, self.0 % Self::SCALE)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// Buy/Sell Indicator: `"B"` or `"S"` (§1.3.1, §1.5.1).
    fn from_byte(b: u8) -> Option<Side> {
        match b {
            b'B' => Some(Side::Buy),
            b'S' => Some(Side::Sell),
            _ => None,
        }
    }

    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

/// Stock symbol: 8 ASCII bytes, right padded with spaces, borrowed from the frame.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Stock<'a>(pub &'a [u8; 8]);

impl<'a> Stock<'a> {
    /// The symbol without its padding; non-ASCII bytes (never valid) are shown as `?`.
    pub fn trimmed(&self) -> String {
        let end = self.0.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
        self.0[..end]
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
            .collect()
    }
}

impl fmt::Debug for Stock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Stock({:?})", self.trimmed())
    }
}

/// The fields at offsets 1..11 of every message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Locate code of the security, 0 for messages that are not stock specific. Assigned per
    /// day by the Stock Directory message (spec "Architecture").
    pub stock_locate: u16,
    /// Nasdaq internal tracking number.
    pub tracking_number: u16,
    /// Nanoseconds since midnight (48 bits on the wire).
    pub timestamp_ns: u64,
}

/// §1.1. `event_code`: `O` start of messages, `S` start of system hours, `Q` start of market
/// hours, `M` end of market hours, `E` end of system hours, `C` end of messages.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SystemEvent {
    pub header: Header,
    pub event_code: u8,
}

/// §1.2.1.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StockDirectory<'a> {
    pub header: Header,
    pub stock: Stock<'a>,
    pub market_category: u8,
    pub financial_status: u8,
    pub round_lot_size: u32,
    pub round_lots_only: u8,
    pub issue_classification: u8,
    pub issue_sub_type: &'a [u8; 2],
    pub authenticity: u8,
    pub short_sale_threshold: u8,
    pub ipo_flag: u8,
    pub luld_tier: u8,
    pub etp_flag: u8,
    pub etp_leverage_factor: u32,
    pub inverse: u8,
}

/// §1.2.2. `trading_state`: `H` halted, `P` paused, `Q` quotation only, `T` trading.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TradingAction<'a> {
    pub header: Header,
    pub stock: Stock<'a>,
    pub trading_state: u8,
    pub reason: &'a [u8; 4],
}

/// §1.3.1 (`A`) and §1.3.2 (`F`, which adds `attribution`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AddOrder<'a> {
    pub header: Header,
    pub order_ref: u64,
    pub side: Side,
    pub shares: u32,
    pub stock: Stock<'a>,
    pub price: Price4,
    pub attribution: Option<&'a [u8; 4]>,
}

/// §1.4.1: executed at the order's display price.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderExecuted {
    pub header: Header,
    pub order_ref: u64,
    pub shares: u32,
    pub match_number: u64,
}

/// §1.4.2: executed at a price different from the display price. Non-printable executions are
/// reported again in a later bulk print (a cross).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderExecutedWithPrice {
    pub header: Header,
    pub order_ref: u64,
    pub shares: u32,
    pub match_number: u64,
    pub printable: bool,
    pub price: Price4,
}

/// §1.4.3: partial cancellation of `shares`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderCancel {
    pub header: Header,
    pub order_ref: u64,
    pub shares: u32,
}

/// §1.4.4: all remaining shares are removed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderDelete {
    pub header: Header,
    pub order_ref: u64,
}

/// §1.4.5: the original order is removed and a new one with a new reference number is added. Side,
/// stock and attribution carry over from the original.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderReplace {
    pub header: Header,
    pub original_ref: u64,
    pub new_ref: u64,
    pub shares: u32,
    pub price: Price4,
}

/// §1.5.1: execution of a non-displayed order. The order reference is zero since 2010-12-06 and
/// the Buy/Sell indicator is always `B` since 2014-07-14, whatever the resting side.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Trade<'a> {
    pub header: Header,
    pub order_ref: u64,
    pub side: Side,
    pub shares: u32,
    pub stock: Stock<'a>,
    pub price: Price4,
    pub match_number: u64,
}

/// §1.5.2: the bulk print of a cross. `shares` is 8 bytes wide here and may be zero.
/// `cross_type`: `O` opening, `C` closing, `H` IPO or halted/paused security (older files also
/// carry `I`, intraday/post-close).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CrossTrade<'a> {
    pub header: Header,
    pub shares: u64,
    pub stock: Stock<'a>,
    pub price: Price4,
    pub match_number: u64,
    pub cross_type: u8,
}

/// §1.5.3: a previously reported execution was broken. It has no effect on the book.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BrokenTrade {
    pub header: Header,
    pub match_number: u64,
}

/// One decoded message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Message<'a> {
    SystemEvent(SystemEvent),
    StockDirectory(StockDirectory<'a>),
    TradingAction(TradingAction<'a>),
    AddOrder(AddOrder<'a>),
    OrderExecuted(OrderExecuted),
    OrderExecutedWithPrice(OrderExecutedWithPrice),
    OrderCancel(OrderCancel),
    OrderDelete(OrderDelete),
    OrderReplace(OrderReplace),
    Trade(Trade<'a>),
    CrossTrade(CrossTrade<'a>),
    BrokenTrade(BrokenTrade),
    /// A type the specification defines but this decoder does not interpret (its length was
    /// checked and its common header read).
    Skipped {
        kind: u8,
        header: Header,
    },
    /// A type byte the specification does not define. Nothing else about the frame is known.
    Unknown {
        kind: u8,
    },
}

impl Message<'_> {
    /// The header, for every type the specification defines.
    pub fn header(&self) -> Option<Header> {
        Some(match self {
            Message::SystemEvent(m) => m.header,
            Message::StockDirectory(m) => m.header,
            Message::TradingAction(m) => m.header,
            Message::AddOrder(m) => m.header,
            Message::OrderExecuted(m) => m.header,
            Message::OrderExecutedWithPrice(m) => m.header,
            Message::OrderCancel(m) => m.header,
            Message::OrderDelete(m) => m.header,
            Message::OrderReplace(m) => m.header,
            Message::Trade(m) => m.header,
            Message::CrossTrade(m) => m.header,
            Message::BrokenTrade(m) => m.header,
            Message::Skipped { header, .. } => *header,
            Message::Unknown { .. } => return None,
        })
    }
}

/// A frame whose content does not match the specification. The frame boundary itself is intact,
/// so decoding continues with the next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Empty,
    Length {
        kind: u8,
        expected: usize,
        actual: usize,
    },
    Field {
        kind: u8,
        field: &'static str,
        value: u8,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Empty => write!(f, "empty frame"),
            DecodeError::Length {
                kind,
                expected,
                actual,
            } => write!(
                f,
                "message type {} must be {expected} bytes, the frame holds {actual}",
                show_kind(*kind)
            ),
            DecodeError::Field { kind, field, value } => write!(
                f,
                "message type {}: invalid {field} byte 0x{value:02x}",
                show_kind(*kind)
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A message type for display and as a count key: the ASCII letter, or `0x..` when the byte is
/// not printable.
pub fn show_kind(kind: u8) -> String {
    if kind.is_ascii_graphic() {
        (kind as char).to_string()
    } else {
        format!("0x{kind:02x}")
    }
}

#[inline]
fn be_u16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

#[inline]
fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// 6-byte timestamp (spec "Data Types": nanoseconds since midnight).
#[inline]
fn be_u48(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes([
        0,
        0,
        b[at],
        b[at + 1],
        b[at + 2],
        b[at + 3],
        b[at + 4],
        b[at + 5],
    ])
}

#[inline]
fn be_u64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_be_bytes(a)
}

#[inline]
fn alpha<const N: usize>(b: &[u8], at: usize) -> &[u8; N] {
    b[at..]
        .first_chunk::<N>()
        .expect("frame length was checked against the specification")
}

fn side(kind: u8, b: u8) -> Result<Side, DecodeError> {
    Side::from_byte(b).ok_or(DecodeError::Field {
        kind,
        field: "buy/sell indicator",
        value: b,
    })
}

/// Decodes one message (the bytes after the length prefix). Allocation free; alpha fields
/// borrow from `frame`.
pub fn decode(frame: &[u8]) -> Result<Message<'_>, DecodeError> {
    let Some(&kind) = frame.first() else {
        return Err(DecodeError::Empty);
    };
    let Some(expected) = spec_length(kind) else {
        return Ok(Message::Unknown { kind });
    };
    if frame.len() != expected {
        return Err(DecodeError::Length {
            kind,
            expected,
            actual: frame.len(),
        });
    }
    let b = frame;
    let header = Header {
        stock_locate: be_u16(b, 1),
        tracking_number: be_u16(b, 3),
        timestamp_ns: be_u48(b, 5),
    };
    Ok(match kind {
        b'S' => Message::SystemEvent(SystemEvent {
            header,
            event_code: b[11],
        }),
        b'R' => Message::StockDirectory(StockDirectory {
            header,
            stock: Stock(alpha(b, 11)),
            market_category: b[19],
            financial_status: b[20],
            round_lot_size: be_u32(b, 21),
            round_lots_only: b[25],
            issue_classification: b[26],
            issue_sub_type: alpha(b, 27),
            authenticity: b[29],
            short_sale_threshold: b[30],
            ipo_flag: b[31],
            luld_tier: b[32],
            etp_flag: b[33],
            etp_leverage_factor: be_u32(b, 34),
            inverse: b[38],
        }),
        b'H' => Message::TradingAction(TradingAction {
            header,
            stock: Stock(alpha(b, 11)),
            trading_state: b[19],
            reason: alpha(b, 21),
        }),
        b'A' | b'F' => Message::AddOrder(AddOrder {
            header,
            order_ref: be_u64(b, 11),
            side: side(kind, b[19])?,
            shares: be_u32(b, 20),
            stock: Stock(alpha(b, 24)),
            price: Price4(be_u32(b, 32)),
            attribution: (kind == b'F').then(|| alpha(b, 36)),
        }),
        b'E' => Message::OrderExecuted(OrderExecuted {
            header,
            order_ref: be_u64(b, 11),
            shares: be_u32(b, 19),
            match_number: be_u64(b, 23),
        }),
        b'C' => Message::OrderExecutedWithPrice(OrderExecutedWithPrice {
            header,
            order_ref: be_u64(b, 11),
            shares: be_u32(b, 19),
            match_number: be_u64(b, 23),
            printable: match b[31] {
                b'Y' => true,
                b'N' => false,
                value => {
                    return Err(DecodeError::Field {
                        kind,
                        field: "printable",
                        value,
                    });
                }
            },
            price: Price4(be_u32(b, 32)),
        }),
        b'X' => Message::OrderCancel(OrderCancel {
            header,
            order_ref: be_u64(b, 11),
            shares: be_u32(b, 19),
        }),
        b'D' => Message::OrderDelete(OrderDelete {
            header,
            order_ref: be_u64(b, 11),
        }),
        b'U' => Message::OrderReplace(OrderReplace {
            header,
            original_ref: be_u64(b, 11),
            new_ref: be_u64(b, 19),
            shares: be_u32(b, 27),
            price: Price4(be_u32(b, 31)),
        }),
        b'P' => Message::Trade(Trade {
            header,
            order_ref: be_u64(b, 11),
            side: side(kind, b[19])?,
            shares: be_u32(b, 20),
            stock: Stock(alpha(b, 24)),
            price: Price4(be_u32(b, 32)),
            match_number: be_u64(b, 36),
        }),
        b'Q' => Message::CrossTrade(CrossTrade {
            header,
            shares: be_u64(b, 11),
            stock: Stock(alpha(b, 19)),
            price: Price4(be_u32(b, 27)),
            match_number: be_u64(b, 31),
            cross_type: b[39],
        }),
        b'B' => Message::BrokenTrade(BrokenTrade {
            header,
            match_number: be_u64(b, 11),
        }),
        _ => Message::Skipped { kind, header },
    })
}

/// The input ended inside a length prefix or a message body. Framing cannot continue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Truncated {
    /// Byte offset (in the decompressed stream) of the incomplete frame's length prefix.
    pub offset: u64,
    /// Bytes the frame needs, prefix included; `None` when a compressed stream ended early and
    /// the frame length is not known.
    pub needed: Option<usize>,
    /// Bytes of the frame that were present.
    pub available: usize,
}

impl fmt::Display for Truncated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "input ends inside the frame at byte {}: ", self.offset)?;
        match self.needed {
            Some(needed) => write!(f, "{needed} bytes needed, {} present", self.available),
            None => write!(
                f,
                "the compressed stream ended early after {} bytes of the frame",
                self.available
            ),
        }
    }
}

impl std::error::Error for Truncated {}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error(transparent)]
    Truncated(#[from] Truncated),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Splits the 2-byte big-endian length-prefixed format.
///
/// A frame that lies entirely inside the reader's buffer is returned as a slice of that buffer
/// (no copy); only a frame straddling a buffer refill is assembled in a scratch buffer. Reading
/// `&[u8]` therefore never copies.
pub struct FrameReader<R> {
    inner: R,
    /// Bytes of the frame returned last, consumed at the next call.
    pending: usize,
    /// Byte offset of the next frame.
    offset: u64,
    scratch: Vec<u8>,
}

impl<R: BufRead> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            pending: 0,
            offset: 0,
            scratch: Vec::new(),
        }
    }

    /// Bytes consumed so far, including the frame returned last.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn into_inner(mut self) -> R {
        self.inner.consume(std::mem::take(&mut self.pending));
        self.inner
    }

    /// The next message body, `Ok(None)` at a clean end of input.
    ///
    /// An input that ends inside a frame (including a compressed stream reporting an unexpected
    /// end) is [`FrameError::Truncated`]; nothing more can be read after it.
    pub fn next_frame(&mut self) -> Result<Option<&[u8]>, FrameError> {
        self.inner.consume(std::mem::take(&mut self.pending));
        let start = self.offset;
        let (buffered, len) = {
            let avail = fill(&mut self.inner, start)?;
            match avail {
                [] => return Ok(None),
                [hi, lo, ..] => {
                    let len = usize::from(u16::from_be_bytes([*hi, *lo]));
                    (avail.len() >= 2 + len, len)
                }
                [_] => (false, 0),
            }
        };
        if buffered {
            self.pending = 2 + len;
            self.offset += (2 + len) as u64;
            let avail = fill(&mut self.inner, start)?;
            return Ok(Some(&avail[2..2 + len]));
        }

        // The frame continues past the buffered bytes: assemble it.
        let mut prefix = [0u8; 2];
        let got = read_full(&mut self.inner, &mut prefix, start, 0)?;
        if got < 2 {
            return Err(Truncated {
                offset: start,
                needed: Some(2),
                available: got,
            }
            .into());
        }
        let len = usize::from(u16::from_be_bytes(prefix));
        self.scratch.resize(len, 0);
        let got = read_full(&mut self.inner, &mut self.scratch, start, 2)?;
        if got < len {
            return Err(Truncated {
                offset: start,
                needed: Some(2 + len),
                available: 2 + got,
            }
            .into());
        }
        self.offset += (2 + len) as u64;
        Ok(Some(&self.scratch))
    }
}

/// `fill_buf`, retrying interrupted reads. A second `fill_buf` on a non-empty buffer returns the
/// same bytes without reading (the `BufRead` contract).
fn fill<R: BufRead>(inner: &mut R, offset: u64) -> Result<&[u8], FrameError> {
    loop {
        match inner.fill_buf() {
            Ok(_) => break,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(eof_as_truncated(err, offset, 0)),
        }
    }
    inner
        .fill_buf()
        .map_err(|err| eof_as_truncated(err, offset, 0))
}

/// Reads until `buf` is full or the input ends; returns the number of bytes read. `before` is
/// the number of bytes of this frame read earlier, for the truncation report.
fn read_full<R: BufRead>(
    inner: &mut R,
    buf: &mut [u8],
    offset: u64,
    before: usize,
) -> Result<usize, FrameError> {
    let mut got = 0;
    while got < buf.len() {
        match inner.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(eof_as_truncated(err, offset, before + got)),
        }
    }
    Ok(got)
}

/// A decompressor reports an input that stops mid-stream as `UnexpectedEof`: that is a truncated
/// file, not an I/O failure.
fn eof_as_truncated(err: io::Error, offset: u64, available: usize) -> FrameError {
    if err.kind() == io::ErrorKind::UnexpectedEof {
        Truncated {
            offset,
            needed: None,
            available,
        }
        .into()
    } else {
        FrameError::Io(err)
    }
}
