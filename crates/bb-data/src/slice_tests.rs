use super::*;
use crate::{
    book::bbo_series,
    store,
    synthetic::{self, SyntheticConfig},
    testutil::TempDir,
};

const MS: i64 = 1_000_000;

/// A saved and reloaded 3 s synthetic dataset (so it has an events hash).
fn parent(tmp: &TempDir) -> LoadedDataset {
    let cfg = SyntheticConfig {
        duration_ns: 3_000 * MS,
        ..Default::default()
    };
    let built = synthetic::generate(&cfg).expect("generate");
    let saved = store::save(tmp.path(), &built).expect("save");
    store::load(tmp.path(), &saved.id).expect("load")
}

/// The full book on one clock after every row of that clock stamped at or before `t`
/// (absolute nanoseconds).
fn book_at(events: &[Event], ds: &DatasetManifest, clock: u64, t: i64) -> Levels {
    let mut book = L2Book::new();
    for ev in events.iter().filter(|e| has(e, clock)) {
        let ts = if clock == EXCH_EVENT {
            ev.exch_ts
        } else {
            ev.local_ts
        };
        if ts > t {
            break;
        }
        book.apply(ev, &ds.instrument);
    }
    levels(&book)
}

fn window(start_ms: i64, end_ms: i64) -> TimeWindow {
    TimeWindow {
        start: start_ms * MS,
        end: end_ms * MS,
    }
}

#[test]
fn slice_of_the_whole_span_is_the_parent() {
    let tmp = TempDir::new("slice-whole");
    let p = parent(&tmp);
    let m = &p.manifest;
    let whole = TimeWindow {
        start: m.start,
        end: m.end + 1,
    };
    let s = slice(&p, whole).unwrap();
    assert_eq!(s.events, *p.events);
    assert_eq!(
        bbo_series(&s.events, &m.instrument, p.origin_ns),
        bbo_series(&p.events, &m.instrument, p.origin_ns)
    );
    assert_eq!(s.manifest.counts, m.counts);
    assert_eq!((s.manifest.start, s.manifest.end), (m.start, m.end));
    let prov = SliceProvenance::of(&s.manifest).unwrap();
    assert_eq!(prov.carried_in_local_rows, 0);
    assert_eq!(
        (prov.exchange_snapshot_levels, prov.local_snapshot_levels),
        (0, 0)
    );
}

#[test]
fn book_at_the_slice_start_and_after_equals_the_parents() {
    let tmp = TempDir::new("slice-book");
    let p = parent(&tmp);
    let m = &p.manifest;
    // Odd boundaries so the start falls between events, with feed updates in flight.
    let w = TimeWindow {
        start: 1_234_567_891,
        end: 2_345_678_901,
    };
    let s = slice(&p, w).unwrap();
    let (s0, e0) = (p.origin_ns + w.start, p.origin_ns + w.end);
    let prov = SliceProvenance::of(&s.manifest).unwrap();
    assert!(
        prov.carried_in_local_rows > 0,
        "the fixture should exercise in-flight rows"
    );
    assert!(prov.exchange_snapshot_levels > 0);

    // Exchange clock: equal at the start and at every exchange timestamp of the window.
    let mut checkpoints: Vec<i64> = vec![s0];
    checkpoints.extend(
        p.events
            .iter()
            .filter(|ev| has(ev, EXCH_EVENT) && (s0..e0).contains(&ev.exch_ts))
            .map(|ev| ev.exch_ts),
    );
    checkpoints.dedup();
    for &t in &checkpoints {
        assert_eq!(
            book_at(&s.events, m, EXCH_EVENT, t),
            book_at(&p.events, m, EXCH_EVENT, t),
            "exchange book differs at {t}"
        );
    }
    // Local clock: from the snapshot's delivery on, equal at every local timestamp until the
    // parent delivers its first row from after the window (which the slice leaves out).
    let cutoff = p
        .events
        .iter()
        .filter(|ev| has(ev, LOCAL_EVENT) && ev.exch_ts >= e0)
        .map(|ev| ev.local_ts)
        .min()
        .unwrap();
    let mut local: Vec<i64> = vec![s0 + 1];
    local.extend(
        s.events
            .iter()
            .filter(|ev| has(ev, LOCAL_EVENT) && ev.local_ts < cutoff)
            .map(|ev| ev.local_ts),
    );
    local.dedup();
    assert!(local.len() > 100, "{}", local.len());
    for &t in &local {
        assert_eq!(
            book_at(&s.events, m, LOCAL_EVENT, t),
            book_at(&p.events, m, LOCAL_EVENT, t),
            "local book differs at {t}"
        );
    }
}

#[test]
fn no_row_falls_outside_the_window() {
    let tmp = TempDir::new("slice-outside");
    let p = parent(&tmp);
    let w = window(1_000, 2_000);
    let s = slice(&p, w).unwrap();
    let (s0, e0) = (p.origin_ns + w.start, p.origin_ns + w.end);
    let mut carried = 0;
    for ev in &s.events {
        assert!(has(ev, EXCH_EVENT) || has(ev, LOCAL_EVENT));
        if has(ev, EXCH_EVENT) {
            assert!((s0..e0).contains(&ev.exch_ts), "{ev:?}");
        }
        if has(ev, LOCAL_EVENT) {
            assert!(ev.local_ts > s0 && ev.exch_ts < e0, "{ev:?}");
            if ev.exch_ts < s0 {
                // Carried in: happened before the start, delivered after it, local clock only.
                assert!(!has(ev, EXCH_EVENT));
                carried += 1;
            }
        }
    }
    let prov = SliceProvenance::of(&s.manifest).unwrap();
    assert_eq!(prov.carried_in_local_rows, carried);
    assert!(s.manifest.start >= w.start && s.manifest.end < w.end);
    validate_event_order(&s.events).unwrap();
}

#[test]
fn slices_are_deterministic_and_content_addressed() {
    let tmp = TempDir::new("slice-hash");
    let p = parent(&tmp);
    let w = window(500, 1_500);
    let a = slice(&p, w).unwrap();
    let b = slice(&p, w).unwrap();
    assert_eq!(a.events, b.events);
    assert_eq!(a.manifest, b.manifest);
    assert_eq!(
        a.manifest.id,
        w.slice_dataset_id(&p.manifest.id, &p.manifest.events_sha256)
    );

    let out1 = TempDir::new("slice-hash-1");
    let out2 = TempDir::new("slice-hash-2");
    let saved1 = store::save(out1.path(), &a).unwrap();
    let saved2 = store::save(out2.path(), &b).unwrap();
    assert_eq!(saved1.events_sha256.len(), 64);
    assert_eq!(saved1.events_sha256, saved2.events_sha256);
    assert_ne!(saved1.events_sha256, p.manifest.events_sha256);

    // The derived dataset is a normal dataset: it loads, verified, with its provenance.
    let loaded = store::load(out1.path(), &saved1.id).unwrap();
    assert_eq!(*loaded.events, a.events);
    assert_eq!(loaded.origin_ns, p.origin_ns);
    let prov = SliceProvenance::of(&loaded.manifest).unwrap();
    assert_eq!(prov.slicing_version, DATASET_SLICE_VERSION);
    assert_eq!(prov.parent_id, p.manifest.id);
    assert_eq!(prov.parent_events_sha256, p.manifest.events_sha256);
    assert_eq!(prov.window, w);
    assert_eq!(
        loaded.manifest.source.inputs[0].sha256,
        p.manifest.events_sha256
    );

    let other = slice(&p, window(500, 1_501)).unwrap();
    assert_ne!(other.manifest.id, a.manifest.id);
    assert!(SliceProvenance::of(&p.manifest).is_none());
}

#[test]
fn quality_report_separates_recomputed_and_inherited_checks() {
    let tmp = TempDir::new("slice-quality");
    let p = parent(&tmp);
    let s = slice(&p, window(1_000, 2_000)).unwrap();
    let find = |id: &str| s.manifest.quality.checks.iter().find(|c| c.id == id);
    let inherited = find("time.local_before_exch").unwrap();
    assert!(inherited.detail.starts_with("Not recomputable for a slice"));
    let initial = find("depth.initial_snapshot").unwrap();
    assert!(initial.passed, "{initial:?}");
    let boundary = find("slice.boundary").unwrap();
    assert_eq!(boundary.severity, Severity::Info);
    assert!(s.manifest.quality.feed_latency_ns.is_some());
}

#[test]
fn invalid_windows_are_refused() {
    let tmp = TempDir::new("slice-invalid");
    let p = parent(&tmp);
    let m = &p.manifest;
    for w in [
        window(1_000, 1_000),
        window(2_000, 1_000),
        TimeWindow {
            start: m.start - 1,
            end: 1_000 * MS,
        },
        TimeWindow {
            start: 0,
            end: m.end + 2,
        },
    ] {
        assert!(
            matches!(slice(&p, w), Err(DataError::Invalid(_))),
            "{w:?} accepted"
        );
    }
    let unsaved = LoadedDataset {
        manifest: DatasetManifest {
            events_sha256: String::new(),
            ..m.clone()
        },
        ..p.clone()
    };
    assert!(matches!(
        slice(&unsaved, window(0, 1_000)),
        Err(DataError::Invalid(_))
    ));
}
