//! Aggregate report of one ITCH input for scripts/research/itch-validate.sh: the reconstruction
//! manifest (framing, decode, timestamps, book quarantine), the data-quality report
//! (`bb_data::itch::report`) and the L3 touch metrics of one symbol over an exchange-time
//! window (`bb_analytics::l3::TouchMetrics`). One pass over the input; aggregates only.
//!
//! usage: bbreport INPUT SYMBOL T0_NS T1_NS OUT.json [MIDNIGHT_EPOCH_NS]
use std::{fs, path::Path};

use bb_analytics::l3::{
    Quote, Touch, TouchEvent, TouchEventKind, TouchMetrics, TouchMetricsConfig,
};
use bb_data::itch::{
    FrameSink, ItchOptions, Reconstructor,
    book::{Change, OrderBooks, PrintKind},
    decode::{Message, Side, decode},
    report::QualityReporter,
    run_file,
};

struct Sink {
    rec: Reconstructor,
    quality: QualityReporter,
    metrics: TouchMetrics,
    symbol: String,
    target: Option<u16>,
    last_ts: u64,
}

fn touch(books: &OrderBooks, locate: u16) -> Touch {
    let q = |l: bb_data::itch::book::LevelView| Quote {
        price4: l.price.0,
        shares: l.shares,
    };
    Touch {
        bid: books.best_bid(locate).map(q),
        ask: books.best_ask(locate).map(q),
    }
}

impl FrameSink for Sink {
    fn limit_reached(&self) -> bool {
        false
    }

    fn push(&mut self, frame: &[u8]) {
        self.rec.push(frame);
        let Ok(msg) = decode(frame) else {
            self.quality.push(frame);
            return;
        };
        let Some(header) = msg.header() else {
            self.quality.push(frame);
            return;
        };
        self.last_ts = self.last_ts.max(header.timestamp_ns);
        let locate = header.stock_locate;
        if let Message::StockDirectory(d) = msg
            && d.stock.trimmed() == self.symbol
        {
            self.target = Some(locate);
        }
        let kind = match msg {
            Message::AddOrder(_) => TouchEventKind::Add,
            Message::OrderExecuted(_) | Message::OrderExecutedWithPrice(_) => {
                TouchEventKind::Execute
            }
            Message::OrderCancel(_) => TouchEventKind::PartialCancel,
            Message::OrderDelete(_) => TouchEventKind::Delete,
            Message::OrderReplace(_) => TouchEventKind::Replace,
            _ => {
                self.quality.push(frame);
                return;
            }
        };
        if self.target != Some(locate) {
            self.quality.push(frame);
            return;
        }
        let books = self.quality.books();
        let before = touch(books, locate);
        let deleted = match msg {
            Message::OrderDelete(m) => books.order(m.order_ref).map_or(0, |o| u64::from(o.shares)),
            _ => 0,
        };
        self.quality.push(frame);
        let books = self.quality.books();
        let changes = books.changes();
        if !changes.iter().any(|c| matches!(c, Change::Level { .. })) {
            return; // quarantined: nothing applied
        }
        let mut bid_left = before.bid.map_or(0, |q| q.shares);
        let mut ask_left = before.ask.map_or(0, |q| q.shares);
        let mut resting_side = None;
        let mut executed = 0;
        for c in changes {
            match *c {
                Change::Level {
                    side,
                    price,
                    shares,
                    ..
                } => match side {
                    Side::Buy if before.bid.is_some_and(|q| q.price4 == price.0) => {
                        bid_left = shares
                    }
                    Side::Sell if before.ask.is_some_and(|q| q.price4 == price.0) => {
                        ask_left = shares
                    }
                    _ => {}
                },
                Change::Print(p) => {
                    if let PrintKind::Execution { resting, .. } = p.kind {
                        resting_side = Some(resting);
                        executed += p.shares;
                    }
                }
            }
        }
        let shares = match msg {
            Message::AddOrder(m) => u64::from(m.shares),
            Message::OrderReplace(m) => u64::from(m.shares),
            Message::OrderCancel(m) => u64::from(m.shares),
            Message::OrderDelete(_) => deleted,
            _ => executed,
        };
        let ts_ns = i64::try_from(header.timestamp_ns).expect("48-bit timestamp");
        self.metrics.push(&TouchEvent {
            ts_ns,
            kind,
            resting_side,
            shares,
            before,
            after: touch(books, locate),
            bid_after_at_before_price: bid_left,
            ask_after_at_before_price: ask_left,
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: bbreport INPUT SYMBOL T0_NS T1_NS OUT.json [MIDNIGHT_EPOCH_NS]");
        std::process::exit(2);
    }
    let symbol = args[2].clone();
    let t0: i64 = args[3].parse().expect("T0_NS");
    let t1: i64 = args[4].parse().expect("T1_NS");
    let midnight_epoch_ns: i64 = args.get(6).map_or(0, |s| s.parse().expect("MIDNIGHT"));
    let options = ItchOptions {
        max_messages: None,
        symbols: vec![symbol.clone()],
        midnight_epoch_ns,
    };
    let mut sink = Sink {
        rec: Reconstructor::new(options),
        quality: QualityReporter::new(None, std::slice::from_ref(&symbol)),
        metrics: TouchMetrics::new(TouchMetricsConfig::new(t0, t1)),
        symbol,
        target: None,
        last_ts: 0,
    };
    let (source, stop, decoded) = run_file(Path::new(&args[1]), &mut sink).expect("read input");
    let Sink {
        rec,
        quality,
        metrics,
        target,
        last_ts,
        ..
    } = sink;
    let reconstruction = rec.finish(source, stop, decoded);
    let manifest = serde_json::to_value(&reconstruction.manifest).expect("manifest");
    let quality = quality.finish();
    let metrics = metrics.finish(i64::try_from(last_ts).expect("48-bit timestamp"));
    let out = serde_json::json!({
        "schema": "blackbox.itch-validate.report/1",
        "symbol_locate": target,
        "manifest": manifest,
        "quality": quality,
        "metrics": metrics,
    });
    fs::write(
        &args[5],
        serde_json::to_string_pretty(&out).expect("json") + "\n",
    )
    .expect("write");
    println!(
        "frames={} stop={} quality_checks_failed={} episodes_agree={} window_events={}",
        reconstruction.manifest.frames,
        reconstruction.manifest.stop.reason,
        quality.checks.iter().filter(|c| !c.passed).count(),
        quality.episodes_agree_with_book,
        metrics.events_in_window
    );
}
