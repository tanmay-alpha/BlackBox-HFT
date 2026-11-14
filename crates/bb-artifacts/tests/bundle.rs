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
