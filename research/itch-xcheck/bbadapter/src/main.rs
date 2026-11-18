//! BlackBox side of the independent comparison: the bb-data ITCH decoder and `OrderBooks`,
//! unchanged, emitting the same record format as `itchbook_adapter.cpp`. Local use only.
//! usage: bbadapter RAW OUT MAX_MESSAGES T0_NS T1_NS SAMPLE_EVERY SYMBOL...
use bb_data::itch::book::{Change, LevelView, OrderBooks, PrintKind};
use bb_data::itch::decode::{Side, decode};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufWriter, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = fs::read(&args[1]).unwrap();
    let mut out = BufWriter::new(fs::File::create(&args[2]).unwrap());
    let max: u64 = args[3].parse().unwrap();
    let t0: u64 = args[4].parse().unwrap();
    let t1: u64 = args[5].parse().unwrap();
    let every: u64 = args[6].parse().unwrap();
    let wanted: BTreeSet<&str> = args[7..].iter().map(String::as_str).collect();
    let mut books = OrderBooks::new();
    let mut selected = BTreeSet::new();
    let mut counts = [0u64; 256];
    let (mut seq, mut pos, mut invalid) = (0u64, 0usize, 0u64);
    while pos + 2 <= bytes.len() && (max == 0 || seq < max) {
        let len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if pos + 2 + len > bytes.len() {
            break;
        }
        let frame = &bytes[pos + 2..pos + 2 + len];
        seq += 1;
        counts[frame[0] as usize] += 1;
        pos += 2 + len;
        let msg = match decode(frame) {
            Ok(m) => m,
            Err(_) => {
                invalid += 1;
                continue;
            }
        };
        if let Some(h) = msg.header() {
            books.apply(&msg);
            let loc = h.stock_locate;
            if frame[0] == b'R'
                && books
                    .symbol(loc)
                    .is_some_and(|s| wanted.contains(s.as_str()))
            {
                selected.insert(loc);
            }
            for c in books.changes() {
                if let Change::Print(p) = c {
                    let (side, cross) = match p.kind {
                        PrintKind::Execution { resting, printable } => {
                            if !printable {
                                continue;
                            }
                            (if resting == Side::Buy { b'B' } else { b'S' }, b' ')
                        }
                        PrintKind::NonCross => (b' ', b' '),
                        PrintKind::Cross { cross_type } => (b' ', cross_type),
                    };
                    writeln!(
                        out,
                        "T {} {} {} {} {} {} {} {} {}",
                        seq,
                        p.timestamp_ns,
                        p.locate,
                        frame[0],
                        side,
                        p.price.0,
                        p.shares,
                        p.match_number,
                        cross
                    )
                    .unwrap();
                }
            }
            if b"AFECXDU".contains(&frame[0])
                && selected.contains(&loc)
                && (t0..t1).contains(&h.timestamp_ns)
            {
                let bid = books.best_bid(loc);
                let ask = books.best_ask(loc);
                write!(
                    out,
                    "E {} {} {} {} {} {} {} |",
                    seq,
                    h.timestamp_ns,
                    loc,
                    bid.map_or(0, |l| l.price.0),
                    bid.map_or(0, |l| l.shares),
                    ask.map_or(0, |l| l.price.0),
                    ask.map_or(0, |l| l.shares)
                )
                .unwrap();
                for side in [Side::Buy, Side::Sell] {
                    for l in books.top_levels(loc, side, 5) {
                        write!(out, " {} {} {}", l.price.0, l.shares, l.orders).unwrap();
                    }
                    write!(out, " |").unwrap();
                }
                writeln!(out).unwrap();
            }
        }
        if seq % every == 0 {
            snapshot(&mut out, &books, &selected, seq);
        }
    }
    let all: BTreeSet<u16> = (0..=u16::MAX).collect();
    snapshot(&mut out, &books, &all, seq);
    books.check_invariants().unwrap();
    writeln!(out, "Z {} {} {}", seq, pos, books.live_orders()).unwrap();
    for (t, c) in counts.iter().enumerate() {
        if *c > 0 {
            writeln!(out, "C {t} {c}").unwrap();
        }
    }
    out.flush().unwrap();
    println!(
        "frames={seq} bytes={pos} live={} invalid={invalid} quarantined={} trailing={}",
        books.live_orders(),
        books.quarantined_total(),
        bytes.len() - pos
    );
}

fn snapshot(out: &mut impl Write, books: &OrderBooks, locates: &BTreeSet<u16>, seq: u64) {
    for &loc in locates {
        for (side, code) in [(Side::Buy, b'B'), (Side::Sell, b'S')] {
            for LevelView {
                price,
                shares,
                orders,
            } in books.levels(loc, side)
            {
                writeln!(out, "L {seq} {loc} {code} {} {shares} {orders}", price.0).unwrap();
            }
        }
    }
}
