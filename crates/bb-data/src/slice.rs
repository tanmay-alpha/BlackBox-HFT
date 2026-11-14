//! Time-window slices of a dataset, as self-contained derived datasets.
//!
//! A sweep's in-sample and out-of-sample runs execute on slices of one parent dataset
//! (`bb_core::sweep`). A slice is a pure function of the parent's verified events and the window,
//! so the same parent and window always give the same events, manifest and (once saved) the same
//! event-file hash.
//!
//! # Rules (slicing version [`DATASET_SLICE_VERSION`])
//!
//! For a window `[s, e)` of exchange-clock nanoseconds relative to the parent origin:
//!
//! * **Book at the start.** The parent's exchange-clock book after every exchange row with
//!   `exch_ts < s`, and its local-clock book after every local row with `local_ts <= s`, are
//!   emitted first as a clear row plus one snapshot row per level, stamped `exch_ts = s`,
//!   `local_ts = s + 1`. When the two books are equal one snapshot carries both clock flags;
//!   otherwise (updates in flight at `s`) the exchange snapshot is flagged `EXCH_EVENT` only and
//!   the local one `LOCAL_EVENT` only. When both books are empty nothing is emitted.
//! * **Exchange clock.** Exactly the parent's exchange rows with `s <= exch_ts < e`.
//! * **Local clock.** The parent's local rows with `local_ts > s` and `exch_ts < e`. This includes
//!   rows that happened at the exchange before `s` but reach the strategy after it ("carried in",
//!   kept on the local clock only, with their true `exch_ts`), so the strategy's view after `s`
//!   is the parent's view, not one that sees in-flight updates early. Local deliveries of rows
//!   with `exch_ts < e` that arrive at or after `e` are kept.
//! * Rows keep the parent's array order, origin, instrument and values; the snapshot prices are
//!   `tick × tick_size` of the parent's reconstructed levels.
//!
//! The book is reconstructed with [`L2Book`], which applies rows literally. After a crossed
//! parent feed the pinned engine's own depth can differ from it (see [`L2Book`]); the slice
//! reproduces the literal book.

use bb_core::{
    dataset::{DatasetManifest, InputFile, QualityCheck, Severity, SourceInfo, TimestampInfo},
    sweep::{DATASET_SLICE_VERSION, TimeWindow},
};
use hftbacktest::types::{
    BUY_EVENT, DEPTH_CLEAR_EVENT, DEPTH_EVENT, DEPTH_SNAPSHOT_EVENT, EXCH_EVENT, LOCAL_EVENT,
    SELL_EVENT,
};
use serde::{Deserialize, Serialize};

use crate::{
    BuiltDataset, DataError, Event, LoadedDataset,
    book::L2Book,
    check_span, has, kind, make_event,
    normalize::validate_event_order,
    quality::{self, DEFAULT_GAP_WARN_NS, Finding, ReportInput, SourceScan},
};

/// Quality checks computed from the source files during ingestion. A slice has no source files,
/// so these are copied from the parent (whole-span counts), labelled as such.
const SOURCE_LEVEL_CHECKS: [&str; 11] = [
    "time.exch_out_of_order.trades",
    "time.exch_out_of_order.book",
    "time.local_before_exch",
    "trades.duplicate_id",
    "trades.id_gap",
    "trades.missing_side",
    "depth.duplicate_row",
    "depth.mid_file_snapshot",
    "precision.price_off_tick",
    "precision.qty_off_lot",
    "values.non_positive_price_or_negative_qty",
];

/// How a slice was derived. Stored as `{"slice": …}` in the derived manifest's `generator`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SliceProvenance {
    pub slicing_version: u32,
    pub parent_id: String,
    /// The parent's `events_sha256`: the slice is a function of exactly those bytes.
    pub parent_events_sha256: String,
    pub window: TimeWindow,
    /// Levels in the exchange-clock and local-clock snapshots at the window start (equal and
    /// emitted once when the two books agree).
    pub exchange_snapshot_levels: u64,
    pub local_snapshot_levels: u64,
    /// Rows with `exch_ts` before the window that reach the strategy after its start.
    pub carried_in_local_rows: u64,
    /// The parent manifest's own `generator`, so a chain of derivations stays reproducible.
    pub parent_generator: Option<serde_json::Value>,
}

impl SliceProvenance {
    /// The provenance of a derived dataset; `None` for a dataset that is not a slice.
    pub fn of(manifest: &DatasetManifest) -> Option<SliceProvenance> {
        let slice = manifest.generator.as_ref()?.get("slice")?;
        serde_json::from_value(slice.clone()).ok()
    }
}

/// Every level of a book: `(tick, qty)` bids best first, then asks best first.
type Levels = (Vec<(i64, f64)>, Vec<(i64, f64)>);

fn levels(book: &L2Book) -> Levels {
    (
        book.top_bids(book.bid_levels()),
        book.top_asks(book.ask_levels()),
    )
}

/// Clear row plus one snapshot row per level, all at `(exch_ts, local_ts)` with `clock` flags.
fn push_snapshot(
    out: &mut Vec<Event>,
    book: &Levels,
    clock: u64,
    exch_ts: i64,
    local_ts: i64,
    tick_size: f64,
) {
    out.push(make_event(
        DEPTH_CLEAR_EVENT | clock,
        exch_ts,
        local_ts,
        0.0,
        0.0,
    ));
    for (side, side_levels) in [(BUY_EVENT, &book.0), (SELL_EVENT, &book.1)] {
        for (tick, qty) in side_levels {
            out.push(make_event(
                DEPTH_SNAPSHOT_EVENT | side | clock,
                exch_ts,
                local_ts,
                *tick as f64 * tick_size,
                *qty,
            ));
        }
    }
}

/// The derived dataset holding `window` of `parent` (rules in the module documentation). Its id
/// is [`TimeWindow::slice_dataset_id`]; `events_file`, `events_sha256` and `events_bytes` are
/// filled by [`crate::store::save`] like any built dataset.
pub fn slice(parent: &LoadedDataset, window: TimeWindow) -> Result<BuiltDataset, DataError> {
    let m = &parent.manifest;
    if m.events_sha256.is_empty() {
        return Err(DataError::Invalid(format!(
            "dataset `{}` has no recorded events hash; only a saved dataset can be sliced",
            m.id
        )));
    }
    if window.start >= window.end {
        return Err(DataError::Invalid(format!(
            "slice window [{}, {}) is empty",
            window.start, window.end
        )));
    }
    if window.start < m.start || window.end > m.end.saturating_add(1) {
        return Err(DataError::Invalid(format!(
            "slice window [{}, {}) lies outside the exchange-time span [{}, {}] of `{}`",
            window.start, window.end, m.start, m.end, m.id
        )));
    }
    let origin = parent.origin_ns;
    let (s, e) = match (
        origin.checked_add(window.start),
        origin.checked_add(window.end),
    ) {
        (Some(s), Some(e)) => (s, e),
        _ => return Err(DataError::Invalid("slice window overflows".into())),
    };
    let instrument = &m.instrument;

    let mut exch_book = L2Book::new();
    let mut local_book = L2Book::new();
    let mut body = Vec::new();
    let mut carried_in = 0u64;
    for ev in parent.events.iter() {
        let on_exch = has(ev, EXCH_EVENT);
        let on_local = has(ev, LOCAL_EVENT);
        if on_exch && ev.exch_ts < s {
            exch_book.apply(ev, instrument);
        }
        if on_local && ev.local_ts <= s {
            local_book.apply(ev, instrument);
        }
        let keep_exch = on_exch && s <= ev.exch_ts && ev.exch_ts < e;
        let keep_local = on_local && ev.local_ts > s && ev.exch_ts < e;
        if !(keep_exch || keep_local) {
            continue;
        }
        let mut row = ev.clone();
        row.ev &= !(EXCH_EVENT | LOCAL_EVENT);
        if keep_exch {
            row.ev |= EXCH_EVENT;
        }
        if keep_local {
            row.ev |= LOCAL_EVENT;
            if ev.exch_ts < s {
                carried_in += 1;
            }
        }
        body.push(row);
    }

    let exch_levels = levels(&exch_book);
    let local_levels = levels(&local_book);
    let count = |l: &Levels| (l.0.len() + l.1.len()) as u64;
    let mut events = Vec::with_capacity(body.len() + 2 * (count(&exch_levels) as usize + 1));
    let snapshot_local_ts = s.saturating_add(1);
    let tick = instrument.tick_size;
    if exch_levels == local_levels {
        if count(&exch_levels) > 0 {
            let both = EXCH_EVENT | LOCAL_EVENT;
            push_snapshot(&mut events, &exch_levels, both, s, snapshot_local_ts, tick);
        }
    } else {
        if count(&exch_levels) > 0 {
            push_snapshot(
                &mut events,
                &exch_levels,
                EXCH_EVENT,
                s,
                snapshot_local_ts,
                tick,
            );
        }
        if count(&local_levels) > 0 {
            push_snapshot(
                &mut events,
                &local_levels,
                LOCAL_EVENT,
                s,
                snapshot_local_ts,
                tick,
            );
        }
    }
    events.extend(body);
    if !events.iter().any(|ev| has(ev, EXCH_EVENT)) {
        return Err(DataError::Invalid(format!(
            "slice window [{}, {}) of `{}` holds no exchange-clock event and no book",
            window.start, window.end, m.id
        )));
    }
    validate_event_order(&events)?;
    check_span(&events, origin)?;

    // Snapshot batches and whether the book data opens with one, from the exchange rows.
    let mut snapshot_batches = 0u64;
    let mut last_snapshot_ts = None;
    let mut starts_with_snapshot = None;
    for ev in events.iter().filter(|ev| has(ev, EXCH_EVENT)) {
        let k = kind(ev);
        if starts_with_snapshot.is_none() && matches!(k, DEPTH_EVENT | DEPTH_SNAPSHOT_EVENT) {
            starts_with_snapshot = Some(k == DEPTH_SNAPSHOT_EVENT);
        }
        if starts_with_snapshot.is_none() && k == DEPTH_CLEAR_EVENT {
            starts_with_snapshot = Some(true);
        }
        if k == DEPTH_SNAPSHOT_EVENT && last_snapshot_ts != Some(ev.exch_ts) {
            snapshot_batches += 1;
            last_snapshot_ts = Some(ev.exch_ts);
        }
    }
    let analysis = quality::analyze(&ReportInput {
        events: &events,
        instrument,
        scan: &SourceScan::default(),
        trade_ids: None,
        mid_file_snapshots: &Finding::default(),
        snapshots: snapshot_batches,
        starts_with_snapshot: starts_with_snapshot.unwrap_or(false),
        local_ts_shift_ns: m.timestamps.local_ts_shift_ns,
        gap_warn_ns: DEFAULT_GAP_WARN_NS,
        precision_basis: "values are copied unchanged from the parent dataset",
    })?;

    let mut report = analysis.report;
    let inherited = |c: &QualityCheck| QualityCheck {
        detail: format!(
            "Not recomputable for a slice; copied from the parent dataset `{}` (whole span): {}",
            m.id, c.detail
        ),
        ..c.clone()
    };
    let parent_check = |id: &str| m.quality.checks.iter().find(|c| c.id == id);
    let mut checks: Vec<QualityCheck> = Vec::with_capacity(report.checks.len() + 4);
    for c in &report.checks {
        if SOURCE_LEVEL_CHECKS.contains(&c.id.as_str()) {
            if let Some(p) = parent_check(&c.id) {
                checks.push(inherited(p));
            }
        } else {
            checks.push(c.clone());
        }
    }
    for p in &m.quality.checks {
        if SOURCE_LEVEL_CHECKS.contains(&p.id.as_str()) && !checks.iter().any(|c| c.id == p.id) {
            checks.push(inherited(p));
        }
    }
    let exch_n = count(&exch_levels);
    let local_n = count(&local_levels);
    checks.push(QualityCheck {
        id: "slice.boundary".into(),
        title: "Book state at the slice start".into(),
        severity: Severity::Info,
        passed: true,
        count: carried_in,
        detail: format!(
            "Slice of `{}` over exchange time [{}, {}) ns. The parent's book at the start was \
             emitted as a snapshot: {exch_n} levels on the exchange clock, {local_n} levels on \
             the local clock{}. {carried_in} rows that happened at the exchange before the start \
             but reach the strategy after it are kept on the local clock only.",
            m.id,
            window.start,
            window.end,
            if exch_levels == local_levels {
                " (the same book, one snapshot)"
            } else {
                " (updates were in flight, so two snapshots)"
            }
        ),
        examples: Vec::new(),
    });
    report.checks = checks;

    let provenance = SliceProvenance {
        slicing_version: DATASET_SLICE_VERSION,
        parent_id: m.id.clone(),
        parent_events_sha256: m.events_sha256.clone(),
        window,
        exchange_snapshot_levels: exch_n,
        local_snapshot_levels: local_n,
        carried_in_local_rows: carried_in,
        parent_generator: m.generator.clone(),
    };
    let seconds = |ns: i64| ns as f64 / 1e9;
    let manifest = DatasetManifest {
        contract_version: m.contract_version,
        id: window.slice_dataset_id(&m.id, &m.events_sha256),
        title: format!(
            "{} [{} s, {} s)",
            m.title,
            seconds(window.start),
            seconds(window.end)
        ),
        kind: m.kind,
        level: m.level,
        source: SourceInfo {
            inputs: vec![InputFile {
                name: format!("{}/{}", m.id, m.events_file),
                role: "slice_parent".into(),
                bytes: m.events_bytes,
                sha256: m.events_sha256.clone(),
                rows: m.counts.rows,
            }],
            ..m.source.clone()
        },
        instrument: *instrument,
        // The parent's origin, so relative timestamps (and sweep windows) mean the same instant
        // in the parent and in every slice of it.
        origin_ns: m.origin_ns.clone(),
        start: analysis.first_exch_ts - origin,
        end: analysis.last_exch_ts - origin,
        timestamps: TimestampInfo {
            note: format!(
                "{} Slice of `{}` (slicing version {DATASET_SLICE_VERSION}); timestamps are the \
                 parent's.",
                m.timestamps.note, m.id
            ),
            ..m.timestamps.clone()
        },
        counts: analysis.counts,
        events_file: String::new(),
        events_sha256: String::new(),
        events_bytes: 0,
        generator: Some(serde_json::json!({ "slice": provenance })),
        quality: report,
    };
    Ok(BuiltDataset { manifest, events })
}

#[cfg(test)]
#[path = "slice_tests.rs"]
mod tests;
