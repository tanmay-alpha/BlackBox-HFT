//! Data-quality checks shared by the synthetic and the recorded pipeline.
//!
//! Findings come from two places. Row-level facts that only the source knows (line numbers,
//! exact decimal precision, trade ids) are collected while the pipeline produces its rows, in a
//! [`SourceScan`] and optionally a [`TradeIdFindings`]. Everything else is measured by
//! [`analyze`] in one pass over the normalized event array on the exchange clock, replaying the
//! book the same way [`crate::book`] does.
//!
//! Nothing here repairs data. Each check states what was found and what, if anything, the
//! pipeline did about it.

use bb_core::{
    dataset::{EventCounts, InstrumentMeta, QualityCheck, QualityReport, Quantiles, Severity},
    time::NS_PER_S,
};
use hftbacktest::types::{
    DEPTH_CLEAR_EVENT, DEPTH_EVENT, DEPTH_SNAPSHOT_EVENT, EXCH_EVENT, LOCAL_EVENT, TRADE_EVENT,
};

use crate::{
    DataError, Event,
    book::{Bbo, L2Book},
    has, kind,
    normalize::validate_event_order,
    side_of,
};

/// Examples kept per check.
pub const MAX_EXAMPLES: usize = 5;

/// A crossed book that lasts longer than this in total makes `depth.crossed_book` an error.
pub const CROSSED_ERROR_NS: i64 = NS_PER_S;

/// Default threshold of the `depth.event_gap` check.
pub const DEFAULT_GAP_WARN_NS: i64 = 5 * NS_PER_S;

/// Occurrences of one kind of problem: a count and the first few concrete cases.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Finding {
    pub count: u64,
    pub examples: Vec<String>,
}

impl Finding {
    /// Counts one occurrence; `example` is only evaluated while examples are still being kept.
    pub fn hit(&mut self, example: impl FnOnce() -> String) {
        self.count += 1;
        if self.examples.len() < MAX_EXAMPLES {
            self.examples.push(example());
        }
    }
}

/// Whether a source row's price and quantity are whole multiples of the tick and lot size, as
/// established by the pipeline (exact decimal arithmetic for text sources).
#[derive(Copy, Clone, Debug)]
pub struct RowPrecision {
    pub price_on_tick: bool,
    pub qty_on_lot: bool,
}

/// Row-level findings, fed one source row at a time in source order (before any timestamp shift
/// or re-ordering). `at` names the row for examples, e.g. `"trades.csv:17"`.
#[derive(Clone, Debug, Default)]
pub struct SourceScan {
    pub exch_out_of_order_trades: Finding,
    pub exch_out_of_order_book: Finding,
    pub local_before_exch: Finding,
    /// Most negative `local_ts - exch_ts` seen (0 when none was negative).
    pub worst_negative_latency_ns: i64,
    pub trades_missing_side: Finding,
    pub duplicate_depth_rows: Finding,
    pub bad_values: Finding,
    pub price_off_tick: Finding,
    pub qty_off_lot: Finding,
    last_trade_exch_ts: Option<i64>,
    last_book_exch_ts: Option<i64>,
    last_book_row: Option<(u64, i64, i64, u64, u64)>,
}

impl SourceScan {
    pub fn trade(&mut self, ev: &Event, precision: RowPrecision, at: impl Fn() -> String) {
        if let Some(prev) = self.last_trade_exch_ts
            && ev.exch_ts < prev
        {
            self.exch_out_of_order_trades
                .hit(|| format!("{}: exch_ts {} after {}", at(), ev.exch_ts, prev));
        }
        self.last_trade_exch_ts = Some(ev.exch_ts);
        if side_of(ev).is_none() {
            self.trades_missing_side
                .hit(|| format!("{}: price {} qty {}", at(), ev.px, ev.qty));
        }
        self.common(ev, precision, &at);
    }

    /// Depth and snapshot-level rows as they appear in the source.
    pub fn book(&mut self, ev: &Event, precision: RowPrecision, at: impl Fn() -> String) {
        if let Some(prev) = self.last_book_exch_ts
            && ev.exch_ts < prev
        {
            self.exch_out_of_order_book
                .hit(|| format!("{}: exch_ts {} after {}", at(), ev.exch_ts, prev));
        }
        self.last_book_exch_ts = Some(ev.exch_ts);
        let fingerprint = (
            ev.ev,
            ev.exch_ts,
            ev.local_ts,
            ev.px.to_bits(),
            ev.qty.to_bits(),
        );
        if self.last_book_row == Some(fingerprint) {
            self.duplicate_depth_rows
                .hit(|| format!("{}: price {} qty {}", at(), ev.px, ev.qty));
        }
        self.last_book_row = Some(fingerprint);
        self.common(ev, precision, &at);
    }

    fn common(&mut self, ev: &Event, precision: RowPrecision, at: &dyn Fn() -> String) {
        let latency = ev.local_ts - ev.exch_ts;
        if latency < 0 {
            self.local_before_exch
                .hit(|| format!("{}: local_ts is {} ns before exch_ts", at(), -latency));
            self.worst_negative_latency_ns = self.worst_negative_latency_ns.min(latency);
        }
        let price_ok = ev.px.is_finite() && ev.px > 0.0;
        let qty_ok = ev.qty.is_finite() && ev.qty >= 0.0;
        if !price_ok || !qty_ok {
            self.bad_values
                .hit(|| format!("{}: price {} qty {}", at(), ev.px, ev.qty));
        }
        if !precision.price_on_tick {
            self.price_off_tick
                .hit(|| format!("{}: price {}", at(), ev.px));
        }
        if !precision.qty_on_lot {
            self.qty_off_lot.hit(|| format!("{}: qty {}", at(), ev.qty));
        }
    }
}

/// Findings about trade identifiers. Only sources that carry ids produce this.
#[derive(Clone, Debug, Default)]
pub struct TradeIdFindings {
    /// Rows repeating an earlier id with the same trade content. These rows were dropped.
    pub exact_duplicates: Finding,
    /// Rows repeating an earlier id with different content. These rows were kept.
    pub conflicting: Finding,
    /// Integer ids that do not follow their predecessor by exactly one.
    pub gaps: Finding,
    pub rows_with_id: u64,
    pub rows_without_id: u64,
    /// Ids that are not plain integers; the gap check cannot be applied to them.
    pub non_integer_ids: u64,
}

pub struct ReportInput<'a> {
    /// Normalized engine events.
    pub events: &'a [Event],
    pub instrument: &'a InstrumentMeta,
    pub scan: &'a SourceScan,
    /// `None` leaves the trade-id checks out of the report.
    pub trade_ids: Option<&'a TradeIdFindings>,
    /// Snapshots found after the first one.
    pub mid_file_snapshots: &'a Finding,
    /// Number of snapshots in the source, for [`EventCounts`].
    pub snapshots: u64,
    /// Whether the first book row of the source belongs to a snapshot.
    pub starts_with_snapshot: bool,
    pub local_ts_shift_ns: i64,
    pub gap_warn_ns: i64,
    /// How tick and lot multiples were established, for the precision checks' detail text.
    pub precision_basis: &'a str,
}

pub struct Analysis {
    pub report: QualityReport,
    pub counts: EventCounts,
    /// First and last exchange timestamps (absolute nanoseconds).
    pub first_exch_ts: i64,
    pub last_exch_ts: i64,
}

/// Nearest-rank quantiles (the p-th percentile is the value at rank `ceil(p/100 * n)`).
/// Sorts `values` in place.
pub fn quantiles(values: &mut [i64]) -> Option<Quantiles> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let n = values.len();
    let at_rank = |pct: usize| values[(pct * n).div_ceil(100).max(1) - 1] as f64;
    let sum: i128 = values.iter().map(|v| *v as i128).sum();
    Some(Quantiles {
        n: n as u64,
        min: values[0] as f64,
        p50: at_rank(50),
        p90: at_rank(90),
        p99: at_rank(99),
        max: values[n - 1] as f64,
        mean: sum as f64 / n as f64,
    })
}

/// Per-batch measurements on the exchange clock. A batch is the set of rows sharing one exchange
/// timestamp; the book is only judged after a complete batch.
#[derive(Default)]
struct BatchStats {
    valid_ns: i128,
    crossed_ns: i128,
    crossed_batches: Finding,
    crossed_episodes: u64,
    prev_crossed: bool,
    last_valid: bool,
    gaps: Vec<i64>,
    long_gaps: Finding,
    spreads: Vec<i64>,
    last_bbo: Bbo,
}

impl BatchStats {
    fn close(&mut self, ts: i64, next_ts: Option<i64>, bbo: Bbo, origin: i64, gap_warn_ns: i64) {
        let held_ns = next_ts.map_or(0, |next| (next - ts).max(0));
        let crossed = bbo.is_crossed();
        self.last_valid = bbo.is_valid();
        if self.last_valid {
            self.valid_ns += held_ns as i128;
        }
        if crossed {
            self.crossed_ns += held_ns as i128;
            if !self.prev_crossed {
                self.crossed_episodes += 1;
            }
            self.crossed_batches.hit(|| {
                format!(
                    "t=+{} ns: best bid tick {} >= best ask tick {}",
                    ts - origin,
                    bbo.bid.map_or(0, |(t, _)| t),
                    bbo.ask.map_or(0, |(t, _)| t)
                )
            });
        }
        self.prev_crossed = crossed;

        if bbo != self.last_bbo {
            self.last_bbo = bbo;
            if let (Some((bid, _)), Some((ask, _))) = (bbo.bid, bbo.ask) {
                self.spreads.push(ask - bid);
            }
        }

        if let Some(next) = next_ts {
            let gap = next - ts;
            self.gaps.push(gap);
            if gap > gap_warn_ns {
                self.long_gaps
                    .hit(|| format!("t=+{} ns: no event for {} ns", ts - origin, gap));
            }
        }
    }
}

fn check(
    id: &str,
    title: &str,
    severity: Severity,
    finding: &Finding,
    detail: String,
) -> QualityCheck {
    QualityCheck {
        id: id.to_string(),
        title: title.to_string(),
        severity,
        passed: finding.count == 0,
        count: finding.count,
        detail,
        examples: finding.examples.clone(),
    }
}

/// Runs every check and gathers the manifest's event counts.
///
/// Fails only when there is no exchange-clock event at all, in which case there is no dataset to
/// describe.
pub fn analyze(input: &ReportInput<'_>) -> Result<Analysis, DataError> {
    let events = input.events;
    let instrument = input.instrument;
    let origin = events
        .iter()
        .find(|e| has(e, EXCH_EVENT))
        .map(|e| e.exch_ts)
        .ok_or_else(|| DataError::Invalid("the dataset has no exchange-clock events".into()))?;

    let mut counts = EventCounts {
        rows: events.len() as u64,
        snapshots: input.snapshots,
        ..EventCounts::default()
    };
    let mut book = L2Book::new();
    let mut stats = BatchStats::default();
    let mut latencies: Vec<i64> = Vec::new();
    let mut split = Finding::default();
    let mut outside = Finding::default();
    let mut trades_checked = 0u64;
    let mut batch_ts = origin;

    for ev in events.iter().filter(|e| has(e, EXCH_EVENT)) {
        if ev.exch_ts != batch_ts {
            stats.close(
                batch_ts,
                Some(ev.exch_ts),
                book.bbo(),
                origin,
                input.gap_warn_ns,
            );
            batch_ts = ev.exch_ts;
        }
        latencies.push(ev.local_ts - ev.exch_ts);
        if !has(ev, LOCAL_EVENT) {
            split.hit(|| {
                format!(
                    "exch t=+{} ns, local t=+{} ns",
                    ev.exch_ts - origin,
                    ev.local_ts - origin
                )
            });
        }
        match kind(ev) {
            TRADE_EVENT => {
                counts.trades += 1;
                // Rows of one timestamp keep source order, trades first, so the book here is the
                // one the trade executed against.
                let bbo = book.bbo();
                if let (true, Some((bid, _)), Some((ask, _))) = (bbo.is_valid(), bbo.bid, bbo.ask)
                {
                    trades_checked += 1;
                    let tick = instrument.tick(ev.px);
                    if tick > ask + 1 || tick < bid - 1 {
                        outside.hit(|| {
                            format!(
                                "t=+{} ns: trade at tick {tick}, book {bid}/{ask}",
                                ev.exch_ts - origin
                            )
                        });
                    }
                }
            }
            DEPTH_EVENT => {
                counts.depth_updates += 1;
                book.apply(ev, instrument);
            }
            DEPTH_SNAPSHOT_EVENT => {
                counts.snapshot_levels += 1;
                book.apply(ev, instrument);
            }
            DEPTH_CLEAR_EVENT => {
                counts.depth_clears += 1;
                book.apply(ev, instrument);
            }
            _ => {}
        }
    }
    stats.close(batch_ts, None, book.bbo(), origin, input.gap_warn_ns);
    let last_exch_ts = batch_ts;
    let span_ns = (last_exch_ts - origin).max(0);
    let book_valid_fraction = if span_ns > 0 {
        stats.valid_ns as f64 / span_ns as f64
    } else if stats.last_valid {
        1.0
    } else {
        0.0
    };

    let scan = input.scan;
    let mut checks = Vec::new();

    for (id, title, what, finding) in [
        (
            "time.exch_out_of_order.trades",
            "Trade rows out of exchange-time order",
            "trade",
            &scan.exch_out_of_order_trades,
        ),
        (
            "time.exch_out_of_order.book",
            "Book rows out of exchange-time order",
            "book",
            &scan.exch_out_of_order_book,
        ),
    ] {
        let detail = if finding.count == 0 {
            format!("Source {what} rows are in exchange-time order.")
        } else {
            format!(
                "{} source {what} rows have an exchange timestamp earlier than the row before \
                 them. They were kept and re-sorted by exchange time (stable sort).",
                finding.count
            )
        };
        checks.push(check(id, title, Severity::Warn, finding, detail));
    }

    let shift = input.local_ts_shift_ns;
    let detail = if scan.local_before_exch.count > 0 {
        format!(
            "{} source rows were received before their exchange timestamp (worst by {} ns), \
             which means the two clocks disagree. Every local timestamp was shifted by {shift} \
             ns; feed latency is therefore biased by an unknown clock offset.",
            scan.local_before_exch.count, -scan.worst_negative_latency_ns
        )
    } else if shift != 0 {
        format!(
            "No source row was received before its exchange timestamp. Every local timestamp \
             was shifted by {shift} ns so that feed latency is strictly positive."
        )
    } else {
        "No source row was received before its exchange timestamp; no shift was applied."
            .to_string()
    };
    checks.push(check(
        "time.local_before_exch",
        "Local receipt time before exchange time",
        Severity::Warn,
        &scan.local_before_exch,
        detail,
    ));

    checks.push(check(
        "time.rows_split_for_ordering",
        "Rows split to satisfy both clocks",
        Severity::Info,
        &split,
        format!(
            "{} source rows arrive locally in a different order than they occurred at the \
             exchange. Each is stored twice, once per clock, as the engine requires.",
            split.count
        ),
    ));

    if let Some(ids) = input.trade_ids {
        let mut dup = Finding {
            count: ids.exact_duplicates.count + ids.conflicting.count,
            examples: ids.conflicting.examples.clone(),
        };
        for example in &ids.exact_duplicates.examples {
            if dup.examples.len() < MAX_EXAMPLES {
                dup.examples.push(example.clone());
            }
        }
        let severity = if ids.conflicting.count > 0 {
            Severity::Error
        } else {
            Severity::Warn
        };
        checks.push(check(
            "trades.duplicate_id",
            "Duplicate trade ids",
            severity,
            &dup,
            format!(
                "{} rows repeat an earlier trade id with the same exchange time, side, price \
                 and amount and were dropped. {} rows repeat an earlier id with different \
                 content and were kept. {} of {} trade rows carry an id.",
                ids.exact_duplicates.count,
                ids.conflicting.count,
                ids.rows_with_id,
                ids.rows_with_id + ids.rows_without_id
            ),
        ));

        let detail = if ids.rows_with_id == 0 {
            "Not evaluated: the source carries no trade ids.".to_string()
        } else if ids.non_integer_ids > 0 {
            format!(
                "Evaluated on integer ids only; {} ids are not integers. {} integer ids do not \
                 follow their predecessor by one. Missing trades are never filled in.",
                ids.non_integer_ids, ids.gaps.count
            )
        } else {
            format!(
                "{} trade ids do not follow their predecessor by one, so trades may be missing \
                 or the venue's ids are not consecutive. Missing trades are never filled in.",
                ids.gaps.count
            )
        };
        checks.push(check(
            "trades.id_gap",
            "Non-consecutive trade ids",
            Severity::Warn,
            &ids.gaps,
            detail,
        ));
    }

    checks.push(check(
        "trades.missing_side",
        "Trades without an aggressor side",
        Severity::Info,
        &scan.trades_missing_side,
        format!(
            "{} of {} trades do not state which side initiated them. They are kept without a \
             side; no side is inferred.",
            scan.trades_missing_side.count, counts.trades
        ),
    ));

    checks.push(check(
        "depth.duplicate_row",
        "Repeated depth rows",
        Severity::Info,
        &scan.duplicate_depth_rows,
        format!(
            "{} book rows are identical to the row before them. They were kept; a repeated \
             absolute quantity does not change the book.",
            scan.duplicate_depth_rows.count
        ),
    ));

    let no_initial_snapshot = Finding {
        count: u64::from(!input.starts_with_snapshot),
        examples: Vec::new(),
    };
    checks.push(check(
        "depth.initial_snapshot",
        "Book starts from a snapshot",
        Severity::Warn,
        &no_initial_snapshot,
        if input.starts_with_snapshot {
            "The book data begins with a full snapshot.".to_string()
        } else {
            "The book data does not begin with a snapshot. The reconstructed book only contains \
             levels that were updated since the start and is incomplete for an unknown time."
                .to_string()
        },
    ));

    checks.push(check(
        "depth.mid_file_snapshot",
        "Snapshots after the start of the data",
        Severity::Warn,
        input.mid_file_snapshots,
        format!(
            "{} snapshots begin after earlier book rows. A vendor re-snapshots after a \
             reconnect, so book updates may have been missed before each of them (a possible \
             capture gap).",
            input.mid_file_snapshots.count
        ),
    ));

    let crossed_severity = if stats.crossed_ns > CROSSED_ERROR_NS as i128 {
        Severity::Error
    } else {
        Severity::Warn
    };
    checks.push(check(
        "depth.crossed_book",
        "Crossed or locked book",
        crossed_severity,
        &stats.crossed_batches,
        format!(
            "The reconstructed book had best bid >= best ask after {} exchange-time batches, in \
             {} separate episodes lasting {} ns in total.",
            stats.crossed_batches.count, stats.crossed_episodes, stats.crossed_ns
        ),
    ));

    checks.push(check(
        "depth.event_gap",
        "Long gaps between events",
        Severity::Warn,
        &stats.long_gaps,
        format!(
            "{} gaps between consecutive distinct exchange timestamps exceed {} ns. A long gap \
             can be a quiet market or missing data; the two cannot be told apart here.",
            stats.long_gaps.count, input.gap_warn_ns
        ),
    ));

    checks.push(check(
        "precision.price_off_tick",
        "Prices off the tick grid",
        Severity::Error,
        &scan.price_off_tick,
        format!(
            "{} source rows have a price that is not a whole multiple of the tick size {} ({}). \
             The engine rounds such prices to the nearest tick.",
            scan.price_off_tick.count, instrument.tick_size, input.precision_basis
        ),
    ));

    checks.push(check(
        "precision.qty_off_lot",
        "Quantities off the lot grid",
        Severity::Error,
        &scan.qty_off_lot,
        format!(
            "{} source rows have a quantity that is not a whole multiple of the lot size {} \
             ({}).",
            scan.qty_off_lot.count, instrument.lot_size, input.precision_basis
        ),
    ));

    checks.push(check(
        "values.non_positive_price_or_negative_qty",
        "Impossible prices or quantities",
        Severity::Error,
        &scan.bad_values,
        format!(
            "{} source rows have a price that is not positive or a quantity that is negative \
             (or a non-finite value). They were kept unchanged.",
            scan.bad_values.count
        ),
    ));

    checks.push(check(
        "trades.outside_book",
        "Trades away from the reconstructed book",
        Severity::Warn,
        &outside,
        format!(
            "{} of {} trades printed more than one tick outside the reconstructed best bid and \
             ask at their exchange timestamp ({} trades occurred while the book was one-sided \
             or crossed and were not checked). Trades and book updates may be timestamped \
             inconsistently, or book updates are missing.",
            outside.count,
            trades_checked,
            counts.trades - trades_checked
        ),
    ));

    let mut order = Finding::default();
    let order_detail = match validate_event_order(events) {
        Ok(()) => "Exchange-flagged rows are ordered by exchange time and local-flagged rows by \
                   local time."
            .to_string(),
        Err(err) => {
            order.hit(|| err.to_string());
            "The event array violates the ordering the engine requires; it must not be \
             simulated."
                .to_string()
        }
    };
    checks.push(check(
        "events.order_valid",
        "Event array ordering",
        Severity::Error,
        &order,
        order_detail,
    ));

    let report = QualityReport {
        checks,
        feed_latency_ns: quantiles(&mut latencies),
        spread_ticks: quantiles(&mut stats.spreads),
        inter_event_gap_ns: quantiles(&mut stats.gaps),
        book_valid_fraction,
    };
    Ok(Analysis {
        report,
        counts,
        first_exch_ts: origin,
        last_exch_ts,
    })
}

#[cfg(test)]
mod tests {
    use hftbacktest::types::{BUY_EVENT, SELL_EVENT};

    use super::*;
    use crate::make_event;

    const T0: i64 = 1_700_000_000_000_000_000;

    fn instrument() -> InstrumentMeta {
        InstrumentMeta {
            tick_size: 1.0,
            lot_size: 1.0,
            contract_size: 1.0,
        }
    }

    fn ev(flags: u64, rel: i64, px: f64, qty: f64) -> Event {
        make_event(
            flags | EXCH_EVENT | LOCAL_EVENT,
            T0 + rel,
            T0 + rel + 10,
            px,
            qty,
        )
    }

    fn run(events: &[Event], scan: &SourceScan, gap_warn_ns: i64) -> Analysis {
        let inst = instrument();
        analyze(&ReportInput {
            events,
            instrument: &inst,
            scan,
            trade_ids: None,
            mid_file_snapshots: &Finding::default(),
            snapshots: 0,
            local_ts_shift_ns: 0,
            gap_warn_ns,
            starts_with_snapshot: true,
            precision_basis: "test",
        })
        .unwrap()
    }

    fn find<'a>(report: &'a QualityReport, id: &str) -> &'a QualityCheck {
        report
            .checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("check {id} missing"))
    }

    #[test]
    fn nearest_rank_quantiles() {
        let mut v: Vec<i64> = (1..=100).rev().collect();
        let q = quantiles(&mut v).unwrap();
        assert_eq!((q.n, q.min, q.max), (100, 1.0, 100.0));
        assert_eq!((q.p50, q.p90, q.p99), (50.0, 90.0, 99.0));
        assert_eq!(q.mean, 50.5);

        let mut v = vec![7, 3, 5];
        let q = quantiles(&mut v).unwrap();
        // ceil(0.5 * 3) = 2nd smallest, ceil(0.9 * 3) = 3rd.
        assert_eq!((q.p50, q.p90, q.p99), (5.0, 7.0, 7.0));
        assert_eq!(quantiles(&mut []), None);
    }

    #[test]
    fn clean_data_passes_every_check_and_counts_rows() {
        let events = vec![
            ev(DEPTH_CLEAR_EVENT, 0, 0.0, 0.0),
            ev(DEPTH_SNAPSHOT_EVENT | BUY_EVENT, 0, 100.0, 5.0),
            ev(DEPTH_SNAPSHOT_EVENT | SELL_EVENT, 0, 101.0, 5.0),
            ev(TRADE_EVENT | BUY_EVENT, 100, 101.0, 2.0),
            ev(DEPTH_EVENT | SELL_EVENT, 100, 101.0, 3.0),
            ev(DEPTH_EVENT | SELL_EVENT, 300, 102.0, 4.0),
        ];
        let a = run(&events, &SourceScan::default(), 1_000);
        assert!(a.report.checks.iter().all(|c| c.passed), "{:?}", a.report);
        assert_eq!(a.report.worst(), None);
        assert!(a.report.checks.iter().all(|c| !c.id.starts_with("trades.duplicate")));
        assert_eq!(
            a.counts,
            EventCounts {
                rows: 6,
                trades: 1,
                depth_updates: 2,
                snapshot_levels: 2,
                snapshots: 0,
                depth_clears: 1,
            }
        );
        assert_eq!((a.first_exch_ts, a.last_exch_ts), (T0, T0 + 300));
        assert_eq!(a.report.book_valid_fraction, 1.0);
        let lat = a.report.feed_latency_ns.unwrap();
        assert_eq!((lat.n, lat.min, lat.max), (6, 10.0, 10.0));
        // Distinct timestamps 0, 100, 300.
        let gaps = a.report.inter_event_gap_ns.unwrap();
        assert_eq!((gaps.n, gaps.min, gaps.max), (2, 100.0, 200.0));
        // Top of book changed after the batches at 0 and 100; the far level at 300 is not a
        // top-of-book change.
        let spread = a.report.spread_ticks.unwrap();
        assert_eq!((spread.n, spread.min, spread.max), (2, 1.0, 1.0));
    }

    #[test]
    fn crossed_book_gap_and_outside_trade_are_reported() {
        let s = NS_PER_S;
        let events = vec![
            ev(DEPTH_EVENT | BUY_EVENT, 0, 100.0, 5.0),
            ev(DEPTH_EVENT | SELL_EVENT, 0, 101.0, 5.0),
            // Trade five ticks above the ask.
            ev(TRADE_EVENT | BUY_EVENT, s, 106.0, 1.0),
            // The bid jumps onto the ask: locked for 3 s across two batches.
            ev(DEPTH_EVENT | BUY_EVENT, 2 * s, 101.0, 1.0),
            ev(DEPTH_EVENT | BUY_EVENT, 3 * s, 101.0, 2.0),
            ev(DEPTH_EVENT | BUY_EVENT, 5 * s, 101.0, 0.0),
            ev(DEPTH_EVENT | SELL_EVENT, 10 * s, 101.0, 6.0),
        ];
        let a = run(&events, &SourceScan::default(), 4 * s);

        let crossed = find(&a.report, "depth.crossed_book");
        assert!(!crossed.passed);
        assert_eq!(crossed.count, 2);
        assert_eq!(crossed.severity, Severity::Error);
        assert!(crossed.detail.contains("1 separate episodes"));
        assert!(crossed.detail.contains(&format!("{} ns in total", 3 * s)));

        let gap = find(&a.report, "depth.event_gap");
        assert_eq!(gap.count, 1);
        assert_eq!(gap.examples.len(), 1);

        let outside = find(&a.report, "trades.outside_book");
        assert_eq!(outside.count, 1);
        assert_eq!(outside.severity, Severity::Warn);

        // Valid for [0, 2 s) and [5 s, 10 s] out of 10 s.
        assert!((a.report.book_valid_fraction - 0.7).abs() < 1e-12);
        assert_eq!(a.report.worst(), Some(Severity::Error));
    }

    #[test]
    fn a_brief_cross_is_a_warning() {
        let events = vec![
            ev(DEPTH_EVENT | BUY_EVENT, 0, 100.0, 5.0),
            ev(DEPTH_EVENT | SELL_EVENT, 0, 100.0, 5.0),
            ev(DEPTH_EVENT | SELL_EVENT, 1_000, 100.0, 0.0),
            ev(DEPTH_EVENT | SELL_EVENT, 1_000, 101.0, 5.0),
        ];
        let a = run(&events, &SourceScan::default(), NS_PER_S);
        let crossed = find(&a.report, "depth.crossed_book");
        assert_eq!((crossed.count, crossed.severity), (1, Severity::Warn));
    }

    #[test]
    fn source_scan_collects_row_level_findings() {
        let on_grid = RowPrecision {
            price_on_tick: true,
            qty_on_lot: true,
        };
        let mut scan = SourceScan::default();
        let raw = |flags: u64, exch: i64, local: i64, px: f64, qty: f64| {
            make_event(flags, exch, local, px, qty)
        };
        scan.trade(&raw(TRADE_EVENT | BUY_EVENT, 100, 110, 5.0, 1.0), on_grid, || "t:2".into());
        // Goes backwards, has no side, and was received before it happened.
        scan.trade(&raw(TRADE_EVENT, 90, 80, 5.0, 1.0), on_grid, || "t:3".into());
        let depth = raw(DEPTH_EVENT | SELL_EVENT, 100, 120, 6.0, 2.0);
        scan.book(&depth, on_grid, || "b:2".into());
        scan.book(&depth, on_grid, || "b:3".into());
        scan.book(
            &raw(DEPTH_EVENT | BUY_EVENT, 95, 125, -1.0, 2.0),
            RowPrecision {
                price_on_tick: false,
                qty_on_lot: false,
            },
            || "b:4".into(),
        );

        assert_eq!(scan.exch_out_of_order_trades.count, 1);
        assert_eq!(scan.exch_out_of_order_trades.examples[0], "t:3: exch_ts 90 after 100");
        assert_eq!(scan.trades_missing_side.count, 1);
        assert_eq!(scan.local_before_exch.count, 1);
        assert_eq!(scan.worst_negative_latency_ns, -10);
        assert_eq!(scan.duplicate_depth_rows.count, 1);
        assert_eq!(scan.exch_out_of_order_book.count, 1);
        assert_eq!(scan.bad_values.count, 1);
        assert_eq!(scan.price_off_tick.count, 1);
        assert_eq!(scan.qty_off_lot.count, 1);

        // The findings surface in the report with their severities.
        let events = vec![ev(DEPTH_EVENT | BUY_EVENT, 0, 100.0, 5.0)];
        let a = run(&events, &scan, NS_PER_S);
        assert_eq!(find(&a.report, "precision.price_off_tick").severity, Severity::Error);
        assert!(!find(&a.report, "precision.price_off_tick").passed);
        assert!(!find(&a.report, "time.local_before_exch").passed);
        assert!(!find(&a.report, "depth.duplicate_row").passed);
        assert_eq!(a.report.book_valid_fraction, 0.0);
    }

    #[test]
    fn examples_are_capped_and_trade_id_checks_are_optional() {
        let mut f = Finding::default();
        for i in 0..20 {
            f.hit(|| format!("case {i}"));
        }
        assert_eq!(f.count, 20);
        assert_eq!(f.examples.len(), MAX_EXAMPLES);

        fn report(events: &[Event], ids: &TradeIdFindings) -> Result<Analysis, DataError> {
            let inst = instrument();
            analyze(&ReportInput {
                events,
                instrument: &inst,
                scan: &SourceScan::default(),
                trade_ids: Some(ids),
                mid_file_snapshots: &Finding::default(),
                snapshots: 1,
                local_ts_shift_ns: 0,
                gap_warn_ns: NS_PER_S,
                starts_with_snapshot: true,
                precision_basis: "test",
            })
        }

        let mut ids = TradeIdFindings {
            rows_with_id: 10,
            ..TradeIdFindings::default()
        };
        ids.exact_duplicates.hit(|| "dup".into());
        let events = vec![ev(DEPTH_EVENT | BUY_EVENT, 0, 100.0, 5.0)];
        let a = report(&events, &ids).unwrap();
        let dup = find(&a.report, "trades.duplicate_id");
        assert_eq!((dup.count, dup.severity, dup.passed), (1, Severity::Warn, false));
        assert!(find(&a.report, "trades.id_gap").passed);

        ids.conflicting.hit(|| "conflict".into());
        let a = report(&events, &ids).unwrap();
        let dup = find(&a.report, "trades.duplicate_id");
        assert_eq!((dup.count, dup.severity), (2, Severity::Error));
        assert_eq!(dup.examples[0], "conflict");

        assert!(matches!(report(&[], &ids), Err(DataError::Invalid(_))));
    }
}
