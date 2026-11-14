//! Audit AT-5: the peak resident memory of the largest job the hosted profile admits stays under
//! 300 MB. This file holds a single test so that the process peak is this job's peak.
//!
//! The job: `maker_baseline` (the most tape-heavy bundled strategy) on the 120 s demo dataset at
//! the minimum 1 ms interval, stored through the run store. Its prediction sits just under the
//! hosted tape-event limit, so it is the largest admitted maker job on that dataset.
//!
//! Linux reads `VmHWM` from `/proc/self/status` (peak resident set); Windows reads the peak
//! working set. Other platforms skip the check.

use bb_app::{
    datasets::DatasetCtx,
    pipeline, presets,
    store::{FsRunStore, RunStore},
};
use bb_core::{limits::ExecutionLimits, time::NS_PER_S};
use bb_data::synthetic::SyntheticConfig;
use bb_sim::NoControl;

const LIMIT_BYTES: u64 = 300_000_000;

#[cfg(target_os = "linux")]
fn peak_resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: u64 = line
        .trim_start_matches("VmHWM:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(windows)]
fn peak_resident_bytes() -> Option<u64> {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..ProcessMemoryCounters::default()
    };
    // SAFETY: the pseudo-handle of the current process is always valid, and `counters` is a
    // correctly sized, writable PROCESS_MEMORY_COUNTERS whose size is passed in `cb`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then_some(counters.peak_working_set_size as u64)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn peak_resident_bytes() -> Option<u64> {
    None
}

#[test]
fn the_largest_admitted_hosted_job_stays_under_300_mb() {
    if peak_resident_bytes().is_none() {
        eprintln!("skipped: no peak-memory reading on this platform");
        return;
    }
    let root = std::env::temp_dir().join(format!("bb-hosted-peak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let manifest = {
        let built = bb_data::synthetic::generate(&SyntheticConfig {
            id: "hosted-peak".into(),
            duration_ns: 120 * NS_PER_S,
            ..SyntheticConfig::default()
        })
        .expect("generate");
        bb_data::store::save(&root.join("datasets"), &built).expect("save")
    };
    let ctx = DatasetCtx::load(&root.join("datasets"), "hosted-peak").expect("load");
    let store = FsRunStore::open(root.join("runs")).expect("store");

    let hosted = ExecutionLimits::hosted();
    let mut cfg = presets::find(&manifest, "maker_baseline")
        .expect("preset")
        .config;
    cfg.strategy.interval_ns = 1_000_000;
    let estimate = pipeline::admit(&manifest, &cfg, &hosted).expect("admitted by hosted");
    let summary =
        pipeline::find_or_execute_with(&ctx, &cfg, &store, "hosted peak", &hosted, &NoControl)
            .expect("the job completes within its budget");
    let stored = store.get(&summary.manifest.run_id).expect("read back");
    assert!(stored.is_some());
    drop(stored);

    let peak = peak_resident_bytes().expect("peak memory");
    let _ = std::fs::remove_dir_all(&root);
    println!(
        "hosted peak: {:.1} MiB for {} ticks, {} tape events (predicted {estimate:?})",
        peak as f64 / (1024.0 * 1024.0),
        summary.ticks,
        summary.tape_len
    );
    assert!(
        peak < LIMIT_BYTES,
        "peak resident memory {peak} bytes exceeds {LIMIT_BYTES}"
    );
}
