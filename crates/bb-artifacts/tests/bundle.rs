mod common;
use bb_artifacts::*;
use bb_core::result::{EquitySample, FillRecord, OrderRecord};
use flate2::read::MultiGzDecoder;
use std::io::{Read, Seek, SeekFrom, Write};

#[test]
fn roundtrip_paging_ranges_and_pipeline_hash() {
    let directory = tempfile::tempdir().unwrap();
    let run = common::run(10_003);
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .write_run(directory.path(), &run)
        .unwrap();
    assert_eq!(bundle.trust.result_sha256, run.manifest.result_sha256);
    assert_eq!(bundle.trust.counts.tape, 10_003);
    let reader = BundleReader::new(&bundle.trust, BundleLimits::default()).unwrap();
    let trailer = reader.verify_file(bundle.path()).unwrap();
    assert_eq!(trailer.manifest, run.manifest);
    assert_eq!(trailer.metrics, run.metrics);
    assert_eq!(trailer.engine_check, run.engine_check);
    let pages: Vec<_> = bundle
        .trust
        .page_index
        .iter()
        .enumerate()
        .filter(|(_, p)| p.section == Section::Tape)
        .collect();
    assert_eq!(
        pages.iter().map(|(_, p)| p.rows).collect::<Vec<_>>(),
        [5_000, 5_000, 3]
    );
    assert!(pages[0].1.ts_min.contains_key("local"));
    assert!(pages[0].1.ts_min.contains_key("exchange"));
    let bytes = reader
        .rows(bundle.path(), Section::Tape, 4_999..5_002)
        .unwrap();
    let expected: Vec<_> = run.tape[4_999..5_002]
        .iter()
        .flat_map(|row| {
            let mut b = serde_json::to_vec(row).unwrap();
            b.push(b'\n');
            b
        })
        .collect();
    assert_eq!(bytes, expected);
    assert!(
        reader
            .rows(bundle.path(), Section::Tape, 0..0)
            .unwrap()
            .is_empty()
    );
    for (section, expected) in [
        (Section::Orders, serde_json::to_value(&run.orders).unwrap()),
        (Section::Fills, serde_json::to_value(&run.fills).unwrap()),
        (Section::Equity, serde_json::to_value(&run.equity).unwrap()),
    ] {
        let count = bundle.trust.counts.rows(section);
        let bytes = reader.rows(bundle.path(), section, 0..count).unwrap();
        let rows: Vec<serde_json::Value> = bytes
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(|b| serde_json::from_slice(b).unwrap())
            .collect();
        assert_eq!(serde_json::Value::Array(rows), expected);
    }
    let mut stream = String::new();
    MultiGzDecoder::new(std::fs::File::open(bundle.path()).unwrap())
        .read_to_string(&mut stream)
        .unwrap();
    assert_eq!(
        stream.lines().count() as u64,
        2 + bundle.trust.counts.orders
            + bundle.trust.counts.fills
            + bundle.trust.counts.equity
            + bundle.trust.counts.tape
    );
    assert!(
        stream
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
    );
}

#[test]
fn every_member_bitflip_and_manifest_index_tampering_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .write_run(directory.path(), &common::run(5_001))
        .unwrap();
    let reader = BundleReader::new(&bundle.trust, BundleLimits::default()).unwrap();
    let members: Vec<_> = std::iter::once(bundle.trust.header.clone())
        .chain(bundle.trust.page_index.iter().map(PageIndex::member))
        .chain(std::iter::once(bundle.trust.trailer.clone()))
        .collect();
    for member in members {
        // Header, DEFLATE payload and CRC/footer corruption are all authenticated.
        for relative in [0, member.len / 2, member.len - 1] {
            let changed = directory.path().join("changed.bbz");
            std::fs::copy(bundle.path(), &changed).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&changed)
                .unwrap();
            file.seek(SeekFrom::Start(member.offset + relative))
                .unwrap();
            let mut byte = [0];
            file.read_exact(&mut byte).unwrap();
            byte[0] ^= 1;
            file.seek(SeekFrom::Start(member.offset + relative))
                .unwrap();
            file.write_all(&byte).unwrap();
            drop(file);
            assert!(matches!(
                reader.verify_file(&changed),
                Err(ArtifactError::Integrity(_))
            ));
            if member.offset == bundle.trust.header.offset
                || member.offset == bundle.trust.trailer.offset
            {
                assert!(matches!(
                    reader.page(&changed, 0),
                    Err(ArtifactError::Integrity(_))
                ));
            } else {
                let page = bundle
                    .trust
                    .page_index
                    .iter()
                    .position(|p| p.offset == member.offset)
                    .unwrap();
                assert!(matches!(
                    reader.page(&changed, page),
                    Err(ArtifactError::Integrity(_))
                ));
            }
        }
    }
    let mut changed = bundle.trust.clone();
    changed.page_index[0].sha256 = "0".repeat(64);
    assert!(matches!(
        BundleReader::new(&changed, BundleLimits::default()),
        Err(ArtifactError::Integrity(_))
    ));
    let mut changed = bundle.trust.clone();
    changed.manifest_sha256 = "0".repeat(64);
    assert!(matches!(
        BundleReader::new(&changed, BundleLimits::default())
            .unwrap()
            .page(bundle.path(), 0),
        Err(ArtifactError::Integrity(_))
    ));
    let mut changed = bundle.trust.clone();
    changed.summary_sha256 = "0".repeat(64);
    assert!(matches!(
        BundleReader::new(&changed, BundleLimits::default())
            .unwrap()
            .page(bundle.path(), 0),
        Err(ArtifactError::Integrity(_))
    ));
    let changed = directory.path().join("truncated.bbz");
    std::fs::copy(bundle.path(), &changed).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&changed)
        .unwrap()
        .set_len(bundle.trust.size_bytes - 1)
        .unwrap();
    assert!(matches!(
        reader.page(&changed, 0),
        Err(ArtifactError::Integrity(_))
    ));
    assert!(matches!(
        reader.page(&directory.path().join("absent"), 0),
        Err(ArtifactError::NotFound)
    ));
}

#[test]
fn byte_budgets_are_enforced_and_temporary_failures_clean_up() {
    let directory = tempfile::tempdir().unwrap();
    let run = common::run(20);
    let limits = BundleLimits {
        max_output_bytes: 30,
        ..BundleLimits::default()
    };
    assert!(matches!(
        BundleWriter::new(limits)
            .unwrap()
            .write_run(directory.path(), &run),
        Err(ArtifactError::LimitExceeded(_))
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    let limits = BundleLimits {
        max_page_bytes: 50,
        ..BundleLimits::default()
    };
    assert!(matches!(
        BundleWriter::new(limits)
            .unwrap()
            .write_run(directory.path(), &run),
        Err(ArtifactError::LimitExceeded(_))
    ));
    let limits = BundleLimits {
        max_page_bytes: 1024,
        ..BundleLimits::default()
    };
    let bundle = BundleWriter::new(limits)
        .unwrap()
        .write_run(directory.path(), &run)
        .unwrap();
    assert!(
        bundle
            .trust
            .page_index
            .iter()
            .filter(|p| p.section == Section::Tape)
            .count()
            > 1
    );
}

#[test]
fn empty_sections_and_stale_science_hash() {
    let directory = tempfile::tempdir().unwrap();
    let mut run = common::run(0);
    run.orders.clear();
    run.fills.clear();
    run.equity.clear();
    run.manifest.result_sha256 = bb_core::hash::sha256_json(&(&run.tape, &run.fills, &run.equity));
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .write(
            directory.path(),
            &run.manifest,
            &run.metrics,
            &run.engine_check,
            run.ticks,
            std::iter::empty::<OrderRecord>(),
            std::iter::empty::<FillRecord>(),
            std::iter::empty::<EquitySample>(),
            run.tape.iter(),
        )
        .unwrap();
    assert!(bundle.trust.page_index.is_empty());
    BundleReader::new(&bundle.trust, BundleLimits::default())
        .unwrap()
        .verify_file(bundle.path())
        .unwrap();
    run.manifest.result_sha256 = "0".repeat(64);
    assert!(matches!(
        BundleWriter::new(BundleLimits::default())
            .unwrap()
            .write_run(directory.path(), &run),
        Err(ArtifactError::Integrity(_))
    ));
}

#[test]
fn page_and_whole_run_reads_skip_the_authenticated_analytics_report() {
    let directory = tempfile::tempdir().unwrap();
    let run = common::run(5_001);
    let analytics = serde_json::json!({
        "analytics_version": "test",
        "downsampled": [{"path": "/x", "points": [[1, 2.5], [2, 3.5]]}],
    });
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .with_analytics(TrailerAnalytics::from_json(&analytics.to_string()).unwrap())
        .write_run(directory.path(), &run)
        .unwrap();
    let reader = BundleReader::new(&bundle.trust, BundleLimits::default()).unwrap();
    let full = reader.trailer(bundle.path()).unwrap();
    assert_eq!(full.analytics, Some(analytics));
    let same_as_full = |core: &BundleTrailerCore| {
        assert_eq!(
            (
                &core.manifest,
                &core.metrics,
                &core.engine_check,
                &core.counts
            ),
            (
                &full.manifest,
                &full.metrics,
                &full.engine_check,
                &full.counts
            )
        );
        assert_eq!(
            (&core.result_sha256, &core.summary_sha256, &core.page_index),
            (&full.result_sha256, &full.summary_sha256, &full.page_index)
        );
    };
    same_as_full(&reader.verify_file(bundle.path()).unwrap());
    same_as_full(&reader.trailer_core(bundle.path()).unwrap());
    let mut tape_rows = 0;
    let all: Vec<usize> = (0..bundle.trust.page_index.len()).collect();
    let core = reader
        .trailer_and_pages(bundle.path(), &all, |page_no, bytes| {
            if bundle.trust.page_index[page_no].section == Section::Tape {
                tape_rows += bytes.iter().filter(|b| **b == b'\n').count();
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(tape_rows, 5_001);
    same_as_full(&core);
    assert!(!reader.page(bundle.path(), 0).unwrap().is_empty());
}

#[test]
fn trailer_report_text_is_borrowed_only_after_authentication_and_old_bundles_have_none() {
    let directory = tempfile::tempdir().unwrap();
    let run = common::run(1);
    let analytics: serde_json::Value = serde_json::from_str(PINNED_ANALYTICS).unwrap();
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .with_analytics(TrailerAnalytics::from_json(PINNED_ANALYTICS).unwrap())
        .write_run(directory.path(), &run)
        .unwrap();
    let reader = BundleReader::new(&bundle.trust, BundleLimits::default()).unwrap();
    assert_eq!(
        reader.trailer_analytics_json(bundle.path()).unwrap(),
        Some(analytics.to_string())
    );
    let old = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .write_run(directory.path(), &run)
        .unwrap();
    assert_eq!(
        BundleReader::new(&old.trust, BundleLimits::default())
            .unwrap()
            .trailer_analytics_json(old.path())
            .unwrap(),
        None
    );

    // Same-size tampering of the compressed trailer must fail before report extraction.
    let mut corrupt = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(bundle.path())
        .unwrap();
    corrupt
        .seek(SeekFrom::Start(bundle.trust.trailer.offset + 10))
        .unwrap();
    let mut byte = [0];
    corrupt.read_exact(&mut byte).unwrap();
    corrupt.seek(SeekFrom::Current(-1)).unwrap();
    byte[0] ^= 1;
    corrupt.write_all(&byte).unwrap();
    corrupt.flush().unwrap();
    assert!(matches!(
        reader.trailer_analytics_json(bundle.path()),
        Err(ArtifactError::Integrity(_))
    ));
}

/// Unsorted keys, nested objects, escapes, extreme and negative-zero floats, `u64`/`i64` bounds.
const PINNED_ANALYTICS: &str = r#"{"zeta":{"b":[0.1,1e-7,-0.0,1.7976931348623157e308,5e-324,123456789.123456789],"a":null},"analytics_version":"execution-v3","Mixed":[{"y":"\u00e9\"\\\n\t","x":18446744073709551615,"w":-9223372036854775808},true,false,[]],"empty":{},"n":2.5e-3}"#;

/// The bundle bytes are unchanged by writing the analytics report as stored text: the pins are the
/// bundle SHA-256 and size written by the previous writer (report embedded as a
/// `serde_json::Value`), and the trailer reads back the same report. The compressed bytes depend
/// on flate2's backend, which feature unification picks: `miniz_oxide` when this crate builds
/// alone, `zlib-rs` whenever `bb-data` (its `zip` dependency) is in the build, as in `blackbox`.
/// Each backend has its own pin; a build matches exactly one.
#[test]
fn trailer_analytics_text_writes_the_same_bundle_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let run = common::run(5_001);
    let pinned = |bundle: &VerifiedBundle, pins: [(&str, u64); 2]| {
        let got = (bundle.trust.sha256.as_str(), bundle.trust.size_bytes);
        assert!(
            pins.contains(&got),
            "{got:?} is neither previous-writer pin {pins:?}"
        );
    };
    let bundle = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .with_analytics(TrailerAnalytics::from_json(PINNED_ANALYTICS).unwrap())
        .write_run(directory.path(), &run)
        .unwrap();
    pinned(
        &bundle,
        [
            (
                "ef946c69d66ad2937afd1dff1e3006176aa646d5020e8066759da0ff7a168b63",
                43_746,
            ),
            (
                "992eefd2af5429c7a13d8e1895e0a56e9f1874de0dce4b5818ead2a4c5660f64",
                45_082,
            ),
        ],
    );
    let full = BundleReader::new(&bundle.trust, BundleLimits::default())
        .unwrap()
        .trailer(bundle.path())
        .unwrap();
    assert_eq!(
        full.analytics,
        Some(serde_json::from_str::<serde_json::Value>(PINNED_ANALYTICS).unwrap())
    );
    let none = BundleWriter::new(BundleLimits::default())
        .unwrap()
        .write_run(directory.path(), &run)
        .unwrap();
    pinned(
        &none,
        [
            (
                "ac727da75b91eb2fd87d55d1c476d44e5ac901f12291bbba5c80e87c2dc31262",
                43_587,
            ),
            (
                "3da6804003b1bb5aa214213d8ef3d560be4408ed68ab50db01a61fca60d6c054",
                44_920,
            ),
        ],
    );
}
