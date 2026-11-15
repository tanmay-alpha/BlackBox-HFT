//! Hand-built ITCH 5.0 fixtures. Every byte is produced here from the specification's layouts;
//! no NASDAQ data is used.

use std::io::{BufReader, Write};

use bb_core::dataset::{InstrumentMeta, Severity};
use hftbacktest::types::{
    BUY_EVENT, DEPTH_EVENT, EXCH_EVENT, LOCAL_EVENT, SELL_EVENT, TRADE_EVENT,
};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

use super::{
    DecodeScan, ItchManifest, ItchOptions, Reconstruction, Reconstructor,
    book::{Applied, OrderBooks, Quarantine},
    decode::*,
    reconstruct_bytes, reconstruct_file, run_file,
};
use crate::{Event, book::L2Book, normalize::validate_event_order, testutil::TempDir};

// ---- fixture encoder ------------------------------------------------------------------------

const LOCATE: u16 = 1;
const SYMBOL: &str = "TEST";
/// $10.0000 in Price(4) units.
const P10: u32 = 100_000;
/// 04:00:00 in nanoseconds since midnight.
const T0: u64 = 4 * 3_600 * 1_000_000_000;

fn stock8(symbol: &str) -> [u8; 8] {
    let mut s = [b' '; 8];
    s[..symbol.len()].copy_from_slice(symbol.as_bytes());
    s
}

/// Message type, locate, tracking number 0 and the 48-bit timestamp (offsets 0..11).
fn header(kind: u8, locate: u16, ts: u64) -> Vec<u8> {
    let mut b = vec![kind];
    b.extend(locate.to_be_bytes());
    b.extend(7u16.to_be_bytes());
    b.extend(&ts.to_be_bytes()[2..]);
    b
}

fn done(b: Vec<u8>) -> Vec<u8> {
    assert_eq!(
        Some(b.len()),
        spec_length(b[0]),
        "fixture length for {}",
        b[0] as char
    );
    b
}

fn system(ts: u64, code: u8) -> Vec<u8> {
    let mut b = header(b'S', 0, ts);
    b.push(code);
    done(b)
}

fn directory(ts: u64, locate: u16, symbol: &str) -> Vec<u8> {
    let mut b = header(b'R', locate, ts);
    b.extend(stock8(symbol));
    b.extend(*b"QN"); // market category, financial status
    b.extend(100u32.to_be_bytes()); // round lot size
    b.extend(*b"NC"); // round lots only, issue classification
    b.extend(*b"Z "); // issue sub-type
    b.extend(*b"PNN1N"); // authenticity .. ETP flag
    b.extend(0u32.to_be_bytes()); // ETP leverage factor
    b.push(b'N'); // inverse indicator
    done(b)
}

fn trading_action(ts: u64, locate: u16, symbol: &str, state: u8) -> Vec<u8> {
    let mut b = header(b'H', locate, ts);
    b.extend(stock8(symbol));
    b.extend([state, b' ']);
    b.extend(*b"    ");
    done(b)
}

fn add(ts: u64, order_ref: u64, side: u8, shares: u32, price: u32) -> Vec<u8> {
    add_for(ts, LOCATE, SYMBOL, order_ref, side, shares, price)
}

fn add_for(
    ts: u64,
    locate: u16,
    symbol: &str,
    order_ref: u64,
    side: u8,
    shares: u32,
    price: u32,
) -> Vec<u8> {
    let mut b = header(b'A', locate, ts);
    b.extend(order_ref.to_be_bytes());
    b.push(side);
    b.extend(shares.to_be_bytes());
    b.extend(stock8(symbol));
    b.extend(price.to_be_bytes());
    done(b)
}

fn add_attributed(ts: u64, order_ref: u64, side: u8, shares: u32, price: u32) -> Vec<u8> {
    let mut b = add(ts, order_ref, side, shares, price);
    b[0] = b'F';
    b.extend(*b"MPID");
    done(b)
}

fn executed(ts: u64, order_ref: u64, shares: u32, match_number: u64) -> Vec<u8> {
    let mut b = header(b'E', LOCATE, ts);
    b.extend(order_ref.to_be_bytes());
    b.extend(shares.to_be_bytes());
    b.extend(match_number.to_be_bytes());
    done(b)
}

fn executed_at(
    ts: u64,
    order_ref: u64,
    shares: u32,
    match_number: u64,
    printable: bool,
    price: u32,
) -> Vec<u8> {
    let mut b = header(b'C', LOCATE, ts);
    b.extend(order_ref.to_be_bytes());
    b.extend(shares.to_be_bytes());
    b.extend(match_number.to_be_bytes());
    b.push(if printable { b'Y' } else { b'N' });
    b.extend(price.to_be_bytes());
    done(b)
}

fn cancel(ts: u64, order_ref: u64, shares: u32) -> Vec<u8> {
    let mut b = header(b'X', LOCATE, ts);
    b.extend(order_ref.to_be_bytes());
    b.extend(shares.to_be_bytes());
    done(b)
}

fn delete_on(ts: u64, locate: u16, order_ref: u64) -> Vec<u8> {
    let mut b = header(b'D', locate, ts);
    b.extend(order_ref.to_be_bytes());
    done(b)
}

fn delete(ts: u64, order_ref: u64) -> Vec<u8> {
    delete_on(ts, LOCATE, order_ref)
}

fn replace(ts: u64, original: u64, new: u64, shares: u32, price: u32) -> Vec<u8> {
    let mut b = header(b'U', LOCATE, ts);
    b.extend(original.to_be_bytes());
    b.extend(new.to_be_bytes());
    b.extend(shares.to_be_bytes());
    b.extend(price.to_be_bytes());
    done(b)
}

fn trade(ts: u64, shares: u32, price: u32, match_number: u64) -> Vec<u8> {
    let mut b = header(b'P', LOCATE, ts);
    b.extend(0u64.to_be_bytes());
    b.push(b'B');
    b.extend(shares.to_be_bytes());
    b.extend(stock8(SYMBOL));
    b.extend(price.to_be_bytes());
    b.extend(match_number.to_be_bytes());
    done(b)
}

fn cross(ts: u64, shares: u64, price: u32, match_number: u64, cross_type: u8) -> Vec<u8> {
    let mut b = header(b'Q', LOCATE, ts);
    b.extend(shares.to_be_bytes());
    b.extend(stock8(SYMBOL));
    b.extend(price.to_be_bytes());
    b.extend(match_number.to_be_bytes());
    b.push(cross_type);
    done(b)
}

fn broken(ts: u64, match_number: u64) -> Vec<u8> {
    let mut b = header(b'B', LOCATE, ts);
    b.extend(match_number.to_be_bytes());
    done(b)
}

/// A spec-defined type the decoder does not interpret: an all-zero NOII body.
fn noii(ts: u64) -> Vec<u8> {
    let mut b = header(b'I', LOCATE, ts);
    b.resize(50, 0);
    done(b)
}

/// The 2-byte length-prefixed file format.
fn file(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in messages {
        out.extend(u16::try_from(m.len()).unwrap().to_be_bytes());
        out.extend(m);
    }
    out
}

/// Start of messages plus the directory entry of the test stock.
fn preamble() -> Vec<Vec<u8>> {
    vec![system(T0 - 2, b'O'), directory(T0 - 1, LOCATE, SYMBOL)]
}

fn options() -> ItchOptions {
    ItchOptions {
        symbols: vec![SYMBOL.to_string()],
        ..ItchOptions::default()
    }
}

fn run(body: &[Vec<u8>]) -> Reconstruction {
    let mut messages = preamble();
    messages.extend_from_slice(body);
    let r = reconstruct_bytes("fixture", &file(&messages), options());
    r.books.check_invariants().unwrap();
    r
}

fn rows(r: &Reconstruction) -> &[Event] {
    &r.l2[0].events
}

fn check<'a>(m: &'a ItchManifest, id: &str) -> &'a bb_core::dataset::QualityCheck {
    m.checks.iter().find(|c| c.id == id).unwrap()
}

fn failed(m: &ItchManifest) -> Vec<&str> {
    m.checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| c.id.as_str())
        .collect()
}

fn depth(side: u64, px: f64, qty: f64) -> (u64, f64, f64) {
    (DEPTH_EVENT | side | EXCH_EVENT, px, qty)
}

fn trade_row(side: u64, px: f64, qty: f64) -> (u64, f64, f64) {
    (TRADE_EVENT | side | EXCH_EVENT, px, qty)
}

fn shape(events: &[Event]) -> Vec<(u64, f64, f64)> {
    events.iter().map(|e| (e.ev, e.px, e.qty)).collect()
}

fn queue(r: &Reconstruction, side: Side, price: u32) -> Vec<(u64, u32)> {
    r.books
        .level_queue(LOCATE, side, Price4(price))
        .iter()
        .map(|q| (q.order_ref, q.shares))
        .collect()
}

// ---- decoder ----------------------------------------------------------------------------------

#[test]
fn decoder_reads_every_field_of_each_interpreted_type() {
    let max_ts = (1u64 << 48) - 1;
    let h = |ts| Header {
        stock_locate: LOCATE,
        tracking_number: 7,
        timestamp_ns: ts,
    };
    let sys = system(max_ts, b'Q');
    assert_eq!(
        decode(&sys).unwrap(),
        Message::SystemEvent(SystemEvent {
            header: Header {
                stock_locate: 0,
                ..h(max_ts)
            },
            event_code: b'Q'
        })
    );

    let dir = directory(5, LOCATE, "ABCDEFGH");
    let Message::StockDirectory(d) = decode(&dir).unwrap() else {
        panic!("not a directory message")
    };
    assert_eq!(d.header, h(5));
    assert_eq!(d.stock.trimmed(), "ABCDEFGH");
    assert_eq!((d.market_category, d.financial_status), (b'Q', b'N'));
    assert_eq!(d.round_lot_size, 100);
    assert_eq!(d.issue_sub_type, b"Z ");
    assert_eq!((d.luld_tier, d.inverse), (b'1', b'N'));

    let ta = trading_action(6, LOCATE, SYMBOL, b'T');
    let Message::TradingAction(t) = decode(&ta).unwrap() else {
        panic!("not a trading action")
    };
    assert_eq!(
        (t.stock.trimmed(), t.trading_state),
        ("TEST".to_string(), b'T')
    );

    let a = add(7, 0x0102_0304_0506_0708, b'S', 4_000_000_000, 1_234_500);
    let Message::AddOrder(a) = decode(&a).unwrap() else {
        panic!("not an add")
    };
    assert_eq!(
        (
            a.header,
            a.order_ref,
            a.side,
            a.shares,
            a.price,
            a.attribution
        ),
        (
            h(7),
            0x0102_0304_0506_0708,
            Side::Sell,
            4_000_000_000,
            Price4(1_234_500),
            None
        )
    );
    assert_eq!(a.price.to_string(), "123.4500");
    let f = add_attributed(7, 9, b'B', 1, 1);
    let Message::AddOrder(f) = decode(&f).unwrap() else {
        panic!("not an add")
    };
    assert_eq!((f.side, f.attribution), (Side::Buy, Some(b"MPID")));

    assert_eq!(
        decode(&executed(8, 9, 10, u64::MAX)).unwrap(),
        Message::OrderExecuted(OrderExecuted {
            header: h(8),
            order_ref: 9,
            shares: 10,
            match_number: u64::MAX
        })
    );
    assert_eq!(
        decode(&executed_at(8, 9, 10, 11, false, 99)).unwrap(),
        Message::OrderExecutedWithPrice(OrderExecutedWithPrice {
            header: h(8),
            order_ref: 9,
            shares: 10,
            match_number: 11,
            printable: false,
            price: Price4(99)
        })
    );
    assert_eq!(
        decode(&cancel(8, 9, 3)).unwrap(),
        Message::OrderCancel(OrderCancel {
            header: h(8),
            order_ref: 9,
            shares: 3
        })
    );
    assert_eq!(
        decode(&delete(8, 9)).unwrap(),
        Message::OrderDelete(OrderDelete {
            header: h(8),
            order_ref: 9
        })
    );
    assert_eq!(
        decode(&replace(8, 9, 10, 11, 12)).unwrap(),
        Message::OrderReplace(OrderReplace {
            header: h(8),
            original_ref: 9,
            new_ref: 10,
            shares: 11,
            price: Price4(12)
        })
    );
    let p = trade(8, 5, 6, 7);
    let Message::Trade(p) = decode(&p).unwrap() else {
        panic!("not a trade")
    };
    assert_eq!(
        (p.order_ref, p.side, p.shares, p.price, p.match_number),
        (0, Side::Buy, 5, Price4(6), 7)
    );
    let big = u64::from(u32::MAX) + 5;
    let q = cross(8, big, 6, 7, b'C');
    let Message::CrossTrade(q) = decode(&q).unwrap() else {
        panic!("not a cross")
    };
    assert_eq!(
        (
            q.shares,
            q.stock.trimmed(),
            q.price,
            q.match_number,
            q.cross_type
        ),
        (big, "TEST".to_string(), Price4(6), 7, b'C')
    );
    assert_eq!(
        decode(&broken(8, 42)).unwrap(),
        Message::BrokenTrade(BrokenTrade {
            header: h(8),
            match_number: 42
        })
    );
    assert_eq!(
        decode(&noii(8)).unwrap(),
        Message::Skipped {
            kind: b'I',
            header: h(8)
        }
    );
    assert_eq!(decode(b"z123").unwrap(), Message::Unknown { kind: b'z' });
}

#[test]
fn decoder_rejects_wrong_lengths_and_coded_fields() {
    let a = add(1, 1, b'B', 1, 1);
    assert_eq!(
        decode(&a[..30]),
        Err(DecodeError::Length {
            kind: b'A',
            expected: 36,
            actual: 30
        })
    );
    let mut bad_side = a.clone();
    bad_side[19] = b'Z';
    assert_eq!(
        decode(&bad_side),
        Err(DecodeError::Field {
            kind: b'A',
            field: "buy/sell indicator",
            value: b'Z'
        })
    );
    let mut bad_printable = executed_at(1, 1, 1, 1, true, 1);
    bad_printable[31] = b'?';
    assert!(matches!(
        decode(&bad_printable),
        Err(DecodeError::Field {
            field: "printable",
            ..
        })
    ));
    assert_eq!(decode(&[]), Err(DecodeError::Empty));
}

#[test]
fn frame_reader_reassembles_frames_split_across_buffer_refills() {
    let messages = vec![
        system(1, b'O'),
        add(2, 1, b'B', 10, P10),
        executed(3, 1, 4, 1),
        noii(4),
    ];
    let bytes = file(&messages);
    for capacity in [1, 3, 7, 40, 4096] {
        let mut reader = FrameReader::new(BufReader::with_capacity(capacity, bytes.as_slice()));
        let mut got = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            got.push(frame.to_vec());
        }
        assert_eq!(got, messages, "buffer capacity {capacity}");
        assert_eq!(reader.offset(), bytes.len() as u64);
    }
}

// ---- book and Level-2 rows --------------------------------------------------------------------

#[test]
fn add_order_rests_on_the_book_and_emits_one_depth_row() {
    let r = run(&[add(T0, 1, b'B', 100, P10)]);
    let bid = r.books.best_bid(LOCATE).unwrap();
    assert_eq!((bid.price, bid.shares, bid.orders), (Price4(P10), 100, 1));
    assert_eq!(r.books.best_ask(LOCATE), None);
    assert_eq!(shape(rows(&r)), vec![depth(BUY_EVENT, 10.0, 100.0)]);
    let row = &rows(&r)[0];
    assert_eq!((row.exch_ts, row.local_ts), (T0 as i64, T0 as i64));
    assert_eq!(
        row.ev & LOCAL_EVENT,
        0,
        "no local clock: rows are exchange-only"
    );

    let m = &r.manifest;
    assert!(failed(m).is_empty(), "{:?}", failed(m));
    assert_eq!(m.messages_by_type.get("A"), Some(&1));
    assert_eq!(m.messages_by_type.get("R"), Some(&1));
    assert_eq!(m.book.orders_added, 1);
    assert_eq!(m.normalization.local_clock, "absent");
    assert_eq!(m.normalization.streams[0].depth_rows, 1);
    assert!(m.license_note.starts_with("Local use only."));
    assert_eq!(r.books.symbol(LOCATE).as_deref(), Some(SYMBOL));
}

#[test]
fn partial_cancel_reduces_shares_and_keeps_priority() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10),
        add(T0 + 1, 2, b'B', 70, P10),
        cancel(T0 + 2, 1, 30),
    ]);
    assert_eq!(queue(&r, Side::Buy, P10), vec![(1, 70), (2, 70)]);
    assert_eq!(r.books.best_bid(LOCATE).unwrap().shares, 140);
    assert_eq!(
        shape(rows(&r)),
        vec![
            depth(BUY_EVENT, 10.0, 100.0),
            depth(BUY_EVENT, 10.0, 170.0),
            depth(BUY_EVENT, 10.0, 140.0),
        ]
    );
    assert_eq!(r.manifest.book.orders_cancelled_to_zero, 0);
}

#[test]
fn delete_removes_the_order_and_its_empty_level() {
    let r = run(&[add(T0, 1, b'S', 100, P10), delete(T0 + 1, 1)]);
    assert_eq!(r.books.live_orders(), 0);
    assert!(r.books.levels(LOCATE, Side::Sell).is_empty());
    assert_eq!(
        shape(rows(&r)),
        vec![depth(SELL_EVENT, 10.0, 100.0), depth(SELL_EVENT, 10.0, 0.0)]
    );
    assert_eq!(r.manifest.book.orders_deleted, 1);
}

#[test]
fn replace_is_delete_plus_add_on_the_same_side_at_the_back_of_the_queue() {
    let r = run(&[
        add(T0, 1, b'S', 100, P10),
        add(T0 + 1, 2, b'S', 200, P10),
        // Same price: the new order loses its place.
        replace(T0 + 2, 1, 3, 50, P10),
        // New price: the order moves level and keeps the sell side.
        replace(T0 + 3, 3, 4, 60, P10 + 100),
    ]);
    assert_eq!(r.books.order(1), None);
    assert_eq!(r.books.order(3), None);
    let moved = r.books.order(4).unwrap();
    assert_eq!(
        (moved.side, moved.price, moved.shares),
        (Side::Sell, Price4(P10 + 100), 60)
    );
    assert_eq!(queue(&r, Side::Sell, P10), vec![(2, 200)]);
    assert_eq!(
        shape(rows(&r)),
        vec![
            depth(SELL_EVENT, 10.0, 100.0),
            depth(SELL_EVENT, 10.0, 300.0),
            depth(SELL_EVENT, 10.0, 250.0),
            depth(SELL_EVENT, 10.0, 200.0),
            depth(SELL_EVENT, 10.01, 60.0),
        ]
    );
    assert_eq!(r.manifest.book.orders_replaced, 2);
}

#[test]
fn partial_then_full_execution_prints_before_the_depth_change() {
    let r = run(&[
        add(T0, 1, b'S', 100, P10),
        executed(T0 + 1, 1, 40, 1),
        executed(T0 + 2, 1, 60, 2),
    ]);
    assert_eq!(r.books.live_orders(), 0);
    assert_eq!(
        shape(rows(&r)),
        vec![
            depth(SELL_EVENT, 10.0, 100.0),
            // A buyer took a resting sell: the aggressor is the buy side.
            trade_row(BUY_EVENT, 10.0, 40.0),
            depth(SELL_EVENT, 10.0, 60.0),
            trade_row(BUY_EVENT, 10.0, 60.0),
            depth(SELL_EVENT, 10.0, 0.0),
        ]
    );
    let b = &r.manifest.book;
    assert_eq!(
        (b.executions, b.executed_shares, b.orders_fully_executed),
        (2, 100, 1)
    );
}

#[test]
fn execution_with_price_prints_at_its_own_price_and_non_printable_is_not_a_trade_row() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10),
        executed_at(T0 + 1, 1, 30, 1, true, P10 + 100),
        executed_at(T0 + 2, 1, 20, 2, false, P10 + 100),
    ]);
    assert_eq!(r.books.order(1).unwrap().shares, 50);
    assert_eq!(
        shape(rows(&r)),
        vec![
            depth(BUY_EVENT, 10.0, 100.0),
            trade_row(SELL_EVENT, 10.01, 30.0),
            depth(BUY_EVENT, 10.0, 70.0),
            depth(BUY_EVENT, 10.0, 50.0),
        ]
    );
    assert_eq!(r.manifest.book.non_printable_executions, 1);
    assert_eq!(r.manifest.normalization.streams[0].non_printable_skipped, 1);
}

#[test]
fn empty_book_answers_every_query_with_nothing() {
    let r = run(&[]);
    assert_eq!(r.books.best_bid(LOCATE), None);
    assert_eq!(r.books.best_ask(LOCATE), None);
    assert!(r.books.levels(LOCATE, Side::Buy).is_empty());
    assert!(
        r.books
            .level_queue(LOCATE, Side::Buy, Price4(P10))
            .is_empty()
    );
    assert_eq!(r.books.observed_queue(1), None);
    assert_eq!(r.books.order(1), None);
    assert!(rows(&r).is_empty());
    assert!(failed(&r.manifest).is_empty());
    // A locate the input never mentioned.
    assert!(r.books.levels(999, Side::Sell).is_empty());

    let nothing = reconstruct_bytes("empty", &[], options());
    assert_eq!(nothing.manifest.stop.reason, "end_of_input");
    assert_eq!(nothing.manifest.frames, 0);
    assert_eq!(
        nothing.manifest.normalization.unmatched_symbols,
        vec![SYMBOL]
    );
    assert!(nothing.l2.is_empty());
}

#[test]
fn crossed_and_locked_books_are_reported_not_repaired() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10),
        add(T0 + 10, 2, b'S', 100, P10),
        // Crossing bid: the book keeps it and stays crossed.
        add(T0 + 20, 3, b'B', 50, P10 + 100),
        // A cross that exists only inside one timestamp batch is never judged.
        add(T0 + 30, 4, b'S', 10, P10 - 300),
        delete(T0 + 30, 4),
        delete(T0 + 50, 3),
        delete(T0 + 60, 2),
    ]);
    let b = &r.manifest.book;
    assert_eq!((b.locked_entries, b.crossed_entries), (2, 1));
    assert_eq!(b.crossed_ns, 30, "crossed from T0+20 to T0+50");
    assert_eq!(b.locked_ns, 10 + 10, "locked T0+10..T0+20 and T0+50..T0+60");
    let crossed = check(&r.manifest, "book.crossed");
    assert_eq!(
        (crossed.passed, crossed.severity, crossed.count),
        (false, Severity::Warn, 1)
    );
    assert!(crossed.examples[0].contains("best bid 10.0100 vs best ask 10.0000"));
    assert!(!check(&r.manifest, "book.locked").passed);
    assert_eq!(r.manifest.book.quarantined_total, 0);
}

#[test]
fn crossed_state_still_open_at_the_end_is_closed_at_the_last_timestamp() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10 + 100),
        add(T0 + 10, 2, b'S', 100, P10),
        trading_action(T0 + 40, LOCATE, SYMBOL, b'H'),
    ]);
    assert_eq!(r.manifest.book.crossed_entries, 1);
    assert_eq!(r.manifest.book.crossed_ns, 30);
    let bid = r.books.best_bid(LOCATE).unwrap().price;
    let ask = r.books.best_ask(LOCATE).unwrap().price;
    assert!(bid > ask, "the crossed levels stay as published");
}

#[test]
fn crossed_reports_name_the_trading_state_in_force_when_the_batch_ended() {
    let r = run(&[
        // Halted: a crossed book can build up without matching.
        trading_action(T0, LOCATE, SYMBOL, b'H'),
        add(T0 + 1, 1, b'B', 100, P10 + 100),
        add(T0 + 1, 2, b'S', 100, P10),
        // Released at a later timestamp: the crossed batch above was judged while halted.
        trading_action(T0 + 2, LOCATE, SYMBOL, b'T'),
        delete(T0 + 3, 1),
        // Trading: a cross now is unexpected for continuously matched displayed orders.
        add(T0 + 4, 3, b'B', 10, P10 + 200),
    ]);
    let b = &r.manifest.book;
    assert_eq!((b.crossed_entries, b.crossed_entries_while_trading), (2, 1));
    let c = check(&r.manifest, "book.crossed");
    assert!(
        c.examples[0].ends_with("(trading state H)"),
        "{:?}",
        c.examples
    );
    assert!(
        c.examples[1].ends_with("(trading state T)"),
        "{:?}",
        c.examples
    );
    assert!(
        c.detail
            .contains("1 of them while the stock's trading state was T")
    );
}

#[test]
fn unknown_references_are_quarantined_and_never_invented() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10),
        executed(T0 + 1, 99, 10, 1),
        executed_at(T0 + 2, 99, 10, 2, true, P10),
        cancel(T0 + 3, 99, 10),
        delete(T0 + 4, 99),
        replace(T0 + 5, 99, 100, 10, P10),
        // Order 1 exists, but on locate 1, not 2.
        delete_on(T0 + 6, 2, 1),
    ]);
    assert_eq!(r.books.quarantined(Quarantine::UnknownOrder).count, 5);
    assert_eq!(r.books.quarantined(Quarantine::LocateMismatch).count, 1);
    assert_eq!(
        r.books.order(100),
        None,
        "a replace of an unknown order adds nothing"
    );
    assert_eq!(r.books.order(1).unwrap().shares, 100);
    assert_eq!(shape(rows(&r)), vec![depth(BUY_EVENT, 10.0, 100.0)]);
    let m = &r.manifest;
    assert_eq!(m.book.quarantined_total, 6);
    assert_eq!(m.book.quarantined.get("unknown_order"), Some(&5));
    let c = check(m, "book.unknown_order");
    assert_eq!((c.passed, c.severity), (false, Severity::Error));
    assert!(c.examples[0].contains("order 99"));
    assert_eq!(m.worst(), Some(Severity::Error));
}

#[test]
fn quantities_and_references_breaking_invariants_are_quarantined() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10),
        add(T0, 2, b'B', 5, P10 - 100),
        cancel(T0 + 1, 1, 101),
        executed(T0 + 2, 1, 101, 1),
        cancel(T0 + 3, 1, 0),
        add(T0 + 4, 3, b'B', 0, P10),
        add(T0 + 5, 4, b'B', 10, 0),
        add(T0 + 6, 5, b'B', 10, 2_000_000_001),
        add(T0 + 7, 1, b'S', 10, P10),
        replace(T0 + 8, 2, 1, 10, P10),
        add_for(T0 + 9, LOCATE, "OTHER", 6, b'B', 10, P10),
    ]);
    let q = |reason| r.books.quarantined(reason).count;
    assert_eq!(q(Quarantine::ExceedsRemaining), 2);
    assert_eq!(q(Quarantine::ZeroShares), 2);
    assert_eq!(q(Quarantine::InvalidPrice), 2);
    assert_eq!(q(Quarantine::DuplicateOrderRef), 2);
    assert_eq!(q(Quarantine::StockMismatch), 1);
    assert_eq!(r.books.order(1).unwrap().shares, 100);
    assert_eq!(r.books.order(2).unwrap().shares, 5);
    assert_eq!(r.books.live_orders(), 2);
}

#[test]
fn invalid_and_unknown_frames_are_skipped_counted_and_framing_continues() {
    let mut short_add = add(T0, 1, b'B', 100, P10);
    short_add.truncate(30);
    let mut bad_side = add(T0, 2, b'B', 100, P10);
    bad_side[19] = b'Z';
    let r = run(&[
        short_add,
        bad_side,
        Vec::new(),
        b"z1234".to_vec(),
        noii(T0),
        add(T0 + 1, 3, b'S', 10, P10),
    ]);
    let m = &r.manifest;
    assert_eq!(m.invalid_by_type.get("A"), Some(&2));
    assert_eq!(m.invalid_by_type.get("empty"), Some(&1));
    assert_eq!(m.unknown_by_type.get("z"), Some(&1));
    assert_eq!(m.messages_by_type.get("I"), Some(&1));
    assert_eq!(m.uninterpreted_types, vec!["I"]);
    assert_eq!(check(m, "itch.invalid_message").count, 3);
    assert_eq!(check(m, "itch.unknown_type").count, 1);
    assert_eq!(r.books.order(3).unwrap().shares, 10);
    assert_eq!(r.books.order(1), None);
    assert_eq!(m.frames, 8);
}

#[test]
fn truncated_input_stops_decoding_and_is_reported() {
    let mut bytes = file(&[
        system(T0, b'O'),
        add(T0 + 1, 1, b'B', 100, P10),
        add(T0 + 2, 2, b'B', 100, P10),
    ]);
    bytes.truncate(bytes.len() - 10);
    let r = reconstruct_bytes("cut", &bytes, options());
    let m = &r.manifest;
    assert_eq!(m.stop.reason, "truncated");
    assert!(
        m.stop
            .detail
            .as_ref()
            .unwrap()
            .contains("38 bytes needed, 28 present")
    );
    assert_eq!(m.frames, 2);
    assert_eq!(r.books.live_orders(), 1);
    assert!(!check(m, "itch.framing").passed);

    // A single stray byte where a length prefix should be.
    let mut bytes = file(&[system(T0, b'O')]);
    bytes.push(0);
    let m = reconstruct_bytes("cut", &bytes, options()).manifest;
    assert_eq!(m.stop.reason, "truncated");
    assert_eq!(m.decoded_bytes, 14);
}

#[test]
fn out_of_order_timestamp_is_reported_and_held_back_in_level2_rows() {
    let r = run(&[
        add(T0 + 2_000, 1, b'B', 100, P10),
        add(T0 + 1_000, 2, b'B', 50, P10),
        add(T0 + 3_000, 3, b'B', 50, P10),
    ]);
    // The book follows file order.
    assert_eq!(queue(&r, Side::Buy, P10), vec![(1, 100), (2, 50), (3, 50)]);
    let ts: Vec<i64> = rows(&r).iter().map(|e| e.exch_ts).collect();
    let t = T0 as i64;
    assert_eq!(ts, vec![t + 2_000, t + 2_000, t + 3_000]);
    validate_event_order(rows(&r)).unwrap();
    let m = &r.manifest;
    assert_eq!(m.timestamps.regressions, 1);
    assert_eq!(check(m, "time.regression").count, 1);
    assert_eq!(check(m, "l2.timestamp_held_back").count, 1);
    assert_eq!(m.normalization.streams[0].timestamps_held_back, 1);
}

#[test]
fn l2_refuses_overflowing_epoch_timestamps_and_inexact_quantities() {
    let mut messages = preamble();
    messages.push(add(T0, 1, b'B', 100, P10));
    let bytes = file(&messages);
    let mut opts = options();
    opts.midnight_epoch_ns = i64::MAX - T0 as i64;
    let boundary = reconstruct_bytes("boundary", &bytes, opts.clone());
    assert_eq!(rows(&boundary)[0].exch_ts, i64::MAX);
    assert!(check(&boundary.manifest, "l2.invalid_value").passed);
    opts.midnight_epoch_ns += 1;
    let overflow = reconstruct_bytes("overflow", &bytes, opts);
    assert!(rows(&overflow).is_empty());
    assert_eq!(check(&overflow.manifest, "l2.invalid_value").count, 1);

    let oversized = run(&[cross(T0, (1u64 << 53) + 1, P10, 1, b'O')]);
    assert!(rows(&oversized).is_empty());
    assert_eq!(check(&oversized.manifest, "l2.invalid_value").count, 1);
    let exact = run(&[cross(T0, (1u64 << 53) - 1, P10, 1, b'O')]);
    assert_eq!(rows(&exact)[0].qty, ((1u64 << 53) - 1) as f64);
    assert!(check(&exact.manifest, "l2.invalid_value").passed);
}

#[test]
fn cross_trade_is_a_print_not_a_book_change() {
    let r = run(&[
        add(T0, 1, b'B', 100, P10 + 200),
        add(T0, 2, b'S', 100, P10),
        // Opening cross at 10.01: the crossed orders execute non-printably...
        executed_at(T0 + 10, 1, 60, 77, false, P10 + 100),
        executed_at(T0 + 10, 2, 60, 77, false, P10 + 100),
        // ...and the cross itself is one bulk print.
        cross(T0 + 10, 60, P10 + 100, 77, b'O'),
        cross(T0 + 20, 0, P10 + 100, 78, b'C'),
        trade(T0 + 30, 25, P10 + 50, 79),
    ]);
    assert_eq!(r.books.order(1).unwrap().shares, 40);
    assert_eq!(r.books.order(2).unwrap().shares, 40);
    assert_eq!(
        shape(rows(&r)),
        vec![
            depth(BUY_EVENT, 10.02, 100.0),
            depth(SELL_EVENT, 10.0, 100.0),
            depth(BUY_EVENT, 10.02, 40.0),
            depth(SELL_EVENT, 10.0, 40.0),
            trade_row(0, 10.01, 60.0),
            trade_row(0, 10.005, 25.0),
        ]
    );
    let b = &r.manifest.book;
    assert_eq!(
        (b.cross_trades, b.cross_shares, b.zero_share_crosses),
        (2, 60, 1)
    );
    assert_eq!((b.non_cross_trades, b.non_cross_shares), (1, 25));
    assert_eq!(
        r.manifest.normalization.streams[0].zero_share_crosses_skipped,
        1
    );
}

#[test]
fn broken_trades_are_counted_without_touching_the_book() {
    let r = run(&[
        add(T0, 1, b'S', 100, P10),
        executed(T0 + 1, 1, 10, 7),
        broken(T0 + 2, 7),
        broken(T0 + 3, 8),
    ]);
    assert_eq!(r.books.order(1).unwrap().shares, 90);
    assert_eq!(r.manifest.book.broken_trades, 2);
    assert_eq!(check(&r.manifest, "trades.broken").count, 2);
    assert_eq!(check(&r.manifest, "trades.broken_unknown_match").count, 1);
    assert_eq!(rows(&r).len(), 3, "no row for a break");
}

#[test]
fn observed_queue_ahead_follows_itch_arrival_order() {
    let mut r = Reconstructor::new(options());
    for m in preamble() {
        r.push(&m);
    }
    for m in [
        add(T0, 1, b'B', 100, P10),
        add(T0 + 1, 2, b'B', 200, P10),
        add(T0 + 2, 3, b'B', 300, P10),
        add(T0 + 3, 9, b'B', 999, P10 - 100),
    ] {
        r.push(&m);
    }
    let q = r.books().observed_queue(3).unwrap();
    let ahead: Vec<_> = q.ahead.iter().map(|o| (o.order_ref, o.shares)).collect();
    assert_eq!(ahead, vec![(1, 100), (2, 200)]);
    assert_eq!(
        (q.shares_ahead, q.level_shares, q.level_orders),
        (300, 600, 3)
    );
    assert_eq!(r.books().observed_queue(1).unwrap().shares_ahead, 0);

    // Executions and partial cancels keep priority; a replace goes to the back.
    r.push(&executed(T0 + 4, 1, 50, 1));
    r.push(&cancel(T0 + 5, 3, 100));
    r.push(&replace(T0 + 6, 2, 4, 200, P10));
    let q = r.books().observed_queue(3).unwrap();
    let ahead: Vec<_> = q.ahead.iter().map(|o| (o.order_ref, o.shares)).collect();
    assert_eq!(ahead, vec![(1, 50)]);
    assert_eq!((q.order.shares, q.shares_ahead), (200, 50));
    let q = r.books().observed_queue(4).unwrap();
    let ahead: Vec<_> = q.ahead.iter().map(|o| o.order_ref).collect();
    assert_eq!(ahead, vec![1, 3]);
    assert_eq!(r.books().observed_queue(2), None);
    r.books().check_invariants().unwrap();
}

#[test]
fn apply_reports_what_each_message_did() {
    let mut books = OrderBooks::new();
    let mut apply = |m: Vec<u8>| books.apply(&decode(&m).unwrap());
    assert_eq!(apply(directory(T0, LOCATE, SYMBOL)), Applied::NoEffect);
    assert_eq!(apply(add(T0, 1, b'B', 10, P10)), Applied::Book);
    assert_eq!(apply(trade(T0, 5, P10, 1)), Applied::Print);
    assert_eq!(apply(cross(T0, 5, P10, 2, b'O')), Applied::Print);
    assert_eq!(apply(broken(T0, 1)), Applied::NoEffect);
    assert_eq!(
        apply(executed(T0, 2, 1, 3)),
        Applied::Quarantined(Quarantine::UnknownOrder)
    );
    assert_eq!(apply(executed(T0, 1, 10, 4)), Applied::Book);
    assert_eq!(books.live_orders(), 0);
}

#[test]
fn message_limit_stops_early() {
    let messages = vec![
        system(T0, b'O'),
        add(T0 + 1, 1, b'B', 1, P10),
        add(T0 + 2, 2, b'B', 1, P10),
        add(T0 + 3, 3, b'B', 1, P10),
    ];
    let r = reconstruct_bytes(
        "limited",
        &file(&messages),
        ItchOptions {
            max_messages: Some(3),
            ..ItchOptions::default()
        },
    );
    assert_eq!(r.manifest.frames, 3);
    assert_eq!(r.manifest.stop.reason, "message_limit");
    assert_eq!(r.books.live_orders(), 2);
}

#[test]
fn input_not_starting_with_start_of_messages_is_flagged() {
    let r = reconstruct_bytes("mid-day", &file(&[add(T0, 1, b'B', 1, P10)]), options());
    let c = check(&r.manifest, "itch.start_of_messages");
    assert!(!c.passed);
    assert_eq!(c.examples, vec!["first message has type A"]);
}

#[test]
fn file_manifest_hashes_the_whole_stored_file_plain_and_gzip() {
    let dir = TempDir::new("itch");
    let mut messages = preamble();
    for i in 0..50 {
        messages.push(add(T0 + i, i + 1, b'B', 10, P10));
    }
    let bytes = file(&messages);
    let plain = dir.path().join("sample.itch");
    std::fs::write(&plain, &bytes).unwrap();
    let gz = dir.path().join("sample.itch.gz");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&bytes).unwrap();
    let gz_bytes = encoder.finish().unwrap();
    std::fs::write(&gz, &gz_bytes).unwrap();

    let opts = ItchOptions {
        max_messages: Some(10),
        ..options()
    };
    for (path, stored, compression) in [(&plain, &bytes, "none"), (&gz, &gz_bytes, "gzip")] {
        let first = reconstruct_file(path, opts.clone()).unwrap().manifest;
        let source = &first.source;
        assert_eq!(source.compression, compression);
        assert_eq!(source.bytes, stored.len() as u64);
        assert_eq!(source.sha256, hex::encode(Sha256::digest(stored)));
        assert_eq!(first.frames, 10);
        assert_eq!(first.stop.reason, "message_limit");
        let again = reconstruct_file(path, opts.clone()).unwrap().manifest;
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&again).unwrap(),
            "the manifest is deterministic"
        );
    }
    let all = reconstruct_file(&gz, options()).unwrap();
    assert_eq!(all.manifest.stop.reason, "end_of_input");
    assert_eq!(all.manifest.decoded_bytes, bytes.len() as u64);
    assert_eq!(all.books.live_orders(), 50);
}

#[test]
fn decode_only_scan_counts_the_same_frames_as_a_reconstruction() {
    let dir = TempDir::new("itch-scan");
    let mut bad = add(T0, 9, b'B', 1, P10);
    bad.truncate(20);
    let mut messages = preamble();
    messages.extend([add(T0, 1, b'B', 10, P10), bad, noii(T0 + 1), b"z1".to_vec()]);
    let path = dir.path().join("scan.itch");
    std::fs::write(&path, file(&messages)).unwrap();
    let mut scan = DecodeScan::new(None);
    let (source, stop, decoded) = run_file(&path, &mut scan).unwrap();
    let full = reconstruct_file(&path, ItchOptions::default())
        .unwrap()
        .manifest;
    assert_eq!(stop.record(scan.frames), full.stop);
    assert_eq!((scan.frames, scan.invalid), (full.frames, 1));
    assert_eq!(decoded, full.decoded_bytes);
    assert_eq!(source, full.source);
    let mut expected = full.messages_by_type.clone();
    expected.extend(full.unknown_by_type.clone());
    assert_eq!(scan.by_type(), expected);
}

#[test]
fn truncated_gzip_stream_is_reported_as_truncated() {
    let dir = TempDir::new("itch-gz");
    let mut messages = preamble();
    for i in 0..2_000 {
        messages.push(add(T0 + i, i + 1, b'B', 10, P10 + (i as u32 % 50)));
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&file(&messages)).unwrap();
    let gz_bytes = encoder.finish().unwrap();
    let path = dir.path().join("cut.itch.gz");
    std::fs::write(&path, &gz_bytes[..gz_bytes.len() / 2]).unwrap();
    let r = reconstruct_file(&path, options()).unwrap();
    assert_eq!(r.manifest.stop.reason, "truncated");
    assert!(r.manifest.frames > 0 && r.manifest.frames < messages.len() as u64);
    assert!(!check(&r.manifest, "itch.framing").passed);
    r.books.check_invariants().unwrap();
}

// ---- property: aggregated L2 equals the live L3 orders ----------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Add {
        buy: bool,
        tick: u32,
        shares: u32,
    },
    Cancel {
        pick: usize,
        amount: u32,
    },
    Execute {
        pick: usize,
        amount: u32,
    },
    ExecuteAt {
        pick: usize,
        amount: u32,
        printable: bool,
    },
    Delete {
        pick: usize,
    },
    Replace {
        pick: usize,
        tick: u32,
        shares: u32,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (any::<bool>(), 0u32..12, 1u32..1_000)
            .prop_map(|(buy, tick, shares)| Op::Add { buy, tick, shares }),
        2 => (any::<usize>(), any::<u32>()).prop_map(|(pick, amount)| Op::Cancel { pick, amount }),
        2 => (any::<usize>(), any::<u32>()).prop_map(|(pick, amount)| Op::Execute { pick, amount }),
        1 => (any::<usize>(), any::<u32>(), any::<bool>())
            .prop_map(|(pick, amount, printable)| Op::ExecuteAt { pick, amount, printable }),
        1 => any::<usize>().prop_map(|pick| Op::Delete { pick }),
        1 => (any::<usize>(), 0u32..12, 1u32..1_000)
            .prop_map(|(pick, tick, shares)| Op::Replace { pick, tick, shares }),
    ]
}

/// A live order of the test's own model.
#[derive(Clone, Copy)]
struct Live {
    order_ref: u64,
    buy: bool,
    price: u32,
    shares: u32,
}

/// Sum of the model's live orders per price, best first, as `(price, shares, orders)`.
fn model_levels(live: &[Live], buy: bool) -> Vec<(u32, u64, u32)> {
    let mut levels = std::collections::BTreeMap::<u32, (u64, u32)>::new();
    for o in live.iter().filter(|o| o.buy == buy) {
        let l = levels.entry(o.price).or_default();
        l.0 += u64::from(o.shares);
        l.1 += 1;
    }
    let out = levels.into_iter().map(|(p, (s, n))| (p, s, n));
    if buy {
        out.rev().collect()
    } else {
        out.collect()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn random_valid_sequences_keep_l2_equal_to_the_live_l3_orders(
        ops in prop::collection::vec(op(), 1..250)
    ) {
        let instrument = InstrumentMeta { tick_size: 0.0001, lot_size: 1.0, contract_size: 1.0 };
        let price = |tick: u32| P10 + tick * 100;
        let mut rec = Reconstructor::new(options());
        for m in preamble() {
            rec.push(&m);
        }
        let mut live: Vec<Live> = Vec::new();
        let mut next_ref = 1u64;
        let mut l2 = L2Book::new();
        let mut replayed = 0usize;
        for (i, op) in ops.iter().enumerate() {
            let ts = T0 + i as u64;
            let n = live.len();
            let msg = match *op {
                Op::Add { buy, tick, shares } => {
                    live.push(Live { order_ref: next_ref, buy, price: price(tick), shares });
                    next_ref += 1;
                    add(ts, next_ref - 1, if buy { b'B' } else { b'S' }, shares, price(tick))
                }
                _ if n == 0 => continue,
                Op::Cancel { pick, amount } | Op::Execute { pick, amount }
                | Op::ExecuteAt { pick, amount, .. } => {
                    let o = &mut live[pick % n];
                    let take = 1 + amount % o.shares;
                    let msg = match *op {
                        Op::Cancel { .. } => cancel(ts, o.order_ref, take),
                        Op::Execute { .. } => executed(ts, o.order_ref, take, i as u64),
                        Op::ExecuteAt { printable, .. } => {
                            executed_at(ts, o.order_ref, take, i as u64, printable, o.price + 1)
                        }
                        _ => unreachable!(),
                    };
                    o.shares -= take;
                    if o.shares == 0 {
                        live.remove(pick % n);
                    }
                    msg
                }
                Op::Delete { pick } => delete(ts, live.remove(pick % n).order_ref),
                Op::Replace { pick, tick, shares } => {
                    let o = &mut live[pick % n];
                    let old = o.order_ref;
                    *o = Live { order_ref: next_ref, buy: o.buy, price: price(tick), shares };
                    next_ref += 1;
                    replace(ts, old, next_ref - 1, shares, price(tick))
                }
            };
            rec.push(&msg);
            let books = rec.books();
            prop_assert_eq!(books.quarantined_total(), 0);
            if let Err(e) = books.check_invariants() {
                return Err(TestCaseError::fail(e));
            }
            prop_assert_eq!(books.live_orders(), live.len());

            // L3 aggregation equals the model.
            for (side, buy) in [(Side::Buy, true), (Side::Sell, false)] {
                let l3: Vec<_> = books
                    .levels(LOCATE, side)
                    .iter()
                    .map(|l| (l.price.0, l.shares, l.orders))
                    .collect();
                prop_assert_eq!(&l3, &model_levels(&live, buy));
            }

            // The emitted rows, replayed into the existing L2 book, equal the L3 aggregation.
            let events = &rec.l2()[0].events;
            for ev in &events[replayed..] {
                l2.apply(ev, &instrument);
            }
            replayed = events.len();
            let expect = |buy: bool| -> Vec<(i64, f64)> {
                model_levels(&live, buy)
                    .iter()
                    .map(|&(p, s, _)| (i64::from(p), s as f64))
                    .collect()
            };
            prop_assert_eq!(l2.top_bids(usize::MAX), expect(true));
            prop_assert_eq!(l2.top_asks(usize::MAX), expect(false));
        }
        validate_event_order(&rec.l2()[0].events).unwrap();
    }
}
