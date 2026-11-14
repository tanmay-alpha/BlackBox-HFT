mod common;
use bb_artifacts::{BundleLimits, BundleWriter};
use bb_core::result::{EquitySample, FillRecord, OrderRecord};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use sha2::{Digest, Sha256};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
};

// Portable tracked live heap peak: unlike RSS high-water marks this works on Windows and
// Linux and can measure each write independently. This integration binary has one test only.
struct MeasuringAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn allocated(size: usize) {
    let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(now, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for MeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            if new_size >= layout.size() {
                allocated(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: MeasuringAllocator = MeasuringAllocator;

struct Tape(u64);
impl Serialize for Tape {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0 as usize))?;
        for seq in 0..self.0 {
            sequence.serialize_element(&common::event(seq))?;
        }
        sequence.end()
    }
}
struct Hash(Sha256);
impl Write for Hash {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn measured_write(rows: u64) -> usize {
    let directory = tempfile::tempdir().unwrap();
    let mut run = common::run(0);
    let mut hash = Hash(Sha256::new());
    serde_json::to_writer(
        &mut hash,
        &(Tape(rows), &[] as &[FillRecord], &[] as &[EquitySample]),
    )
    .unwrap();
    run.manifest.result_sha256 = hex::encode(hash.0.finalize());
    let limits = BundleLimits {
        max_page_bytes: 256 << 10,
        max_metadata_bytes: 4 << 20,
        ..BundleLimits::default()
    };
    let writer = BundleWriter::new(limits).unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let bundle = writer
        .write(
            directory.path(),
            &run.manifest,
            &run.metrics,
            &run.engine_check,
            0,
            std::iter::empty::<OrderRecord>(),
            std::iter::empty::<FillRecord>(),
            std::iter::empty::<EquitySample>(),
            (0..rows).map(common::event),
        )
        .unwrap();
    let extra = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    assert_eq!(bundle.trust.counts.tape, rows);
    println!("streamed {rows} generated rows; measured peak incremental heap = {extra} bytes");
    extra
}
#[test]
fn large_generated_run_has_bounded_measured_peak_heap() {
    let small = measured_write(25_000);
    let large = measured_write(250_000);
    assert!(
        large < 16 << 20,
        "peak allocation exceeded fixed 16 MiB budget: {large}"
    );
    assert!(
        large <= small + (4 << 20),
        "tenfold row growth retained section data: small={small}, large={large}"
    );
}
