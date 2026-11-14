//! Execution budgets, run control and platform policy at the pipeline level (audit AT-5, AT-7,
//! AT-9 parts that do not need the job system): over-budget configurations are refused before
//! anything runs, a run that crosses its budget or is cancelled stores nothing, a stored result
//! from another platform class is never returned as this platform's result, and seeds that JSON
//! clients would round are refused.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use bb_app::{
    AppError,
    datasets::DatasetCtx,
    pipeline, presets,
    store::{FsRunStore, RunStore},
};
use bb_core::{
    config::RunConfig,
    dataset::DatasetManifest,
    limits::ExecutionLimits,
    platform::platform_class,
    result::{RunResult, RunSummary},
    time::NS_PER_S,
};
use bb_data::synthetic::SyntheticConfig;
use bb_sim::{NoControl, Progress, RunControl};

const DATASET_ID: &str = "budget-synth";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bb-app-budget-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn synthetic(seconds: i64) -> SyntheticConfig {
    SyntheticConfig {
        id: DATASET_ID.to_string(),
        duration_ns: seconds * NS_PER_S,
        ..SyntheticConfig::default()
    }
}

fn make_dataset(dir: &TempDir, seconds: i64) -> DatasetCtx {
    let built = bb_data::synthetic::generate(&synthetic(seconds)).expect("generate");
    let root = dir.0.join("datasets");
    bb_data::store::save(&root, &built).expect("save");
    DatasetCtx::load(&root, DATASET_ID).expect("load")
}

fn preset(manifest: &DatasetManifest, id: &str) -> RunConfig {
    presets::find(manifest, id)
        .unwrap_or_else(|| panic!("preset {id} missing"))
        .config
}

fn budget_message(result: Result<impl std::fmt::Debug, AppError>) -> String {
    match result {
        Err(AppError::BudgetExceeded(message)) => message,
        other => panic!("expected budget_exceeded, got {other:?}"),
    }
}

/// The 1800 s / 1 ms case of the baseline audit (§5.5: 1.58 GB peak working set, 918 MB stored)
/// is refused by the hosted profile before anything runs, and still admitted locally.
#[test]
fn the_audit_case_is_refused_by_the_hosted_profile_at_admission() {
    let built = bb_data::synthetic::generate(&synthetic(1_800)).expect("generate");
    let manifest = built.manifest;
    let mut cfg = preset(&manifest, "maker_baseline");
    cfg.strategy.interval_ns = 1_000_000;

    let estimate = cfg.estimate_work(&manifest);
    assert!(
        estimate.ticks >= 1_800_000,
        "predicted ticks {}",
        estimate.ticks
    );
    let message = budget_message(pipeline::admit(&manifest, &cfg, &ExecutionLimits::hosted()));
    // The dataset (954k rows) is refused first; with that bound lifted, the ticks are.
    assert!(message.contains("dataset_rows"), "{message}");
    let rows_lifted = ExecutionLimits {
        max_dataset_rows: u64::MAX,
        ..ExecutionLimits::hosted()
    };
    let message = budget_message(pipeline::admit(&manifest, &cfg, &rows_lifted));
    assert!(message.contains("ticks"), "{message}");
    assert!(pipeline::admit(&manifest, &cfg, &ExecutionLimits::local()).is_ok());

    // The local profile keeps today's single bound on strategy evaluations.
    let mut long = manifest.clone();
    long.end = 1_200 * 3_600 * NS_PER_S;
    cfg.strategy.interval_ns = 1_000_000;
    let message = budget_message(pipeline::admit(&long, &cfg, &ExecutionLimits::local()));
    assert!(message.contains("ticks"), "{message}");
}

/// Every preset at its default interval on the hosted demo dataset is admitted, and the
/// prediction bounds what the run actually does.
#[test]
fn predictions_bound_the_actual_runs_and_presets_fit_the_hosted_profile() {
    let dir = TempDir::new("predict");
    let ctx = make_dataset(&dir, 120);
    let manifest = ctx.manifest().clone();
    for p in presets::presets(&manifest) {
        for interval_ns in [p.config.strategy.interval_ns, 10_000_000] {
            let mut cfg = p.config.clone();
            cfg.strategy.interval_ns = interval_ns;
            let estimate = cfg.estimate_work(&manifest);
            let run = pipeline::execute_with(&ctx, &cfg, &ExecutionLimits::hosted(), &NoControl)
                .unwrap_or_else(|e| panic!("{} at {interval_ns} ns: {e}", p.id));
            let bytes = serde_json::to_vec(&run).unwrap().len() as u64;
            let what = format!("{} at {interval_ns} ns: {estimate:?}", p.id);
            assert!(estimate.ticks >= run.ticks, "{what}, ran {}", run.ticks);
            assert!(estimate.ticks <= run.ticks + 2, "{what}, ran {}", run.ticks);
            assert!(estimate.equity_samples >= run.equity.len() as u64, "{what}");
            assert!(
                estimate.tape_events_upper >= run.tape.len() as u64,
                "{what}, tape {}",
                run.tape.len()
            );
            assert!(
                estimate.output_bytes_upper >= bytes,
                "{what}, {bytes} bytes"
            );
        }
    }
}

struct CancelAfter {
    reports: AtomicU64,
    after: u64,
    cancelled: AtomicBool,
}

impl RunControl for CancelAfter {
    fn on_progress(&self, _p: &Progress) {
        self.reports.fetch_add(1, Ordering::Relaxed);
    }

    fn should_cancel(&self) -> bool {
        let stop = self.reports.load(Ordering::Relaxed) >= self.after;
        if stop {
            self.cancelled.store(true, Ordering::Relaxed);
        }
        stop
    }
}

fn stored(store: &FsRunStore) -> Vec<RunSummary> {
    store.list().expect("list")
}

/// A run whose prediction passes but which crosses a budget while running is aborted, and a
/// cancelled run stops; neither leaves anything in the store. The prediction is defeated here by
/// understating the dataset's size in its manifest.
#[test]
fn runtime_breaches_and_cancellation_store_nothing() {
    let dir = TempDir::new("runtime");
    let mut ctx = make_dataset(&dir, 30);
    let store = FsRunStore::open(dir.0.join("runs")).expect("store");
    let cfg = preset(ctx.manifest(), "maker_baseline");
    let full: RunResult = pipeline::execute(&ctx, &cfg).expect("local run");
    let full_bytes = serde_json::to_vec(&full).unwrap().len() as u64;

    ctx.dataset.manifest.counts.rows = 0;
    let estimate = cfg.estimate_work(ctx.manifest());
    assert!(estimate.tape_events_upper < full.tape.len() as u64);
    assert!(estimate.output_bytes_upper < full_bytes);

    // Tape-event budget, enforced inside the simulation.
    let tape_limit = ExecutionLimits {
        max_tape_events: estimate.tape_events_upper,
        ..ExecutionLimits::local()
    };
    let message = budget_message(pipeline::find_or_execute_with(
        &ctx,
        &cfg,
        &store,
        "over tape budget",
        &tape_limit,
        &NoControl,
    ));
    assert!(message.contains("tape_events"), "{message}");

    // Output-byte budget, enforced on the finished result before it is stored.
    let byte_limit = ExecutionLimits {
        max_output_bytes: estimate.output_bytes_upper,
        ..ExecutionLimits::local()
    };
    let message = budget_message(pipeline::find_or_execute_with(
        &ctx,
        &cfg,
        &store,
        "over byte budget",
        &byte_limit,
        &NoControl,
    ));
    assert!(message.contains("output_bytes"), "{message}");

    // Cancellation after two progress reports.
    let control = CancelAfter {
        reports: AtomicU64::new(0),
        after: 2,
        cancelled: AtomicBool::new(false),
    };
    let limits = ExecutionLimits {
        progress_every_ticks: 10,
        ..ExecutionLimits::local()
    };
    match pipeline::find_or_execute_with(&ctx, &cfg, &store, "cancelled", &limits, &control) {
        Err(AppError::Cancelled(_)) => {}
        other => panic!("expected cancelled, got {other:?}"),
    }
    assert!(control.cancelled.load(Ordering::Relaxed));
    assert_eq!(control.reports.load(Ordering::Relaxed), 2);

    assert!(stored(&store).is_empty(), "a refused run was stored");
    let leftovers: Vec<_> = std::fs::read_dir(store.root()).unwrap().collect();
    assert!(leftovers.is_empty(), "the store directory is not empty");

    // The same configuration with no budget pressure is stored normally.
    let summary = pipeline::find_or_execute(&ctx, &cfg, &store, "ok").expect("stored");
    assert_eq!(summary.manifest.result_sha256, full.manifest.result_sha256);
}

/// Audit AT-7: every stored result records its platform class, and a result from another class
/// is never returned as this platform's result.
#[test]
fn a_result_from_another_platform_class_is_not_reused() {
    let dir = TempDir::new("platform");
    let ctx = make_dataset(&dir, 30);
    let store = FsRunStore::open(dir.0.join("runs")).expect("store");
    let cfg = preset(ctx.manifest(), "maker_baseline");

    let summary = pipeline::find_or_execute(&ctx, &cfg, &store, "here").expect("first run");
    assert_eq!(summary.manifest.platform, platform_class());
    let again = pipeline::find_or_execute(&ctx, &cfg, &store, "again").expect("reused");
    assert_eq!(
        again.manifest.created_unix_ms,
        summary.manifest.created_unix_ms
    );

    // Rewrite the stored run as if another platform class had computed it.
    let other = if platform_class() == "linux-x86_64" {
        "windows-x86_64-gnu"
    } else {
        "linux-x86_64"
    };
    let run_dir = store.root().join(&summary.manifest.run_id);
    for file in ["summary.json", "run.json"] {
        let path = run_dir.join(file);
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["manifest"]["platform"] = serde_json::json!(other);
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    }
    let fresh = FsRunStore::open(store.root()).expect("reopen");
    match pipeline::find_or_execute(&ctx, &cfg, &fresh, "elsewhere") {
        Err(AppError::PlatformMismatch(message)) => {
            assert!(message.contains(other), "{message}");
            assert!(message.contains(&platform_class()), "{message}");
        }
        other => panic!("expected platform_mismatch, got {other:?}"),
    }
    // The foreign result is left as it was.
    let kept = fresh.summary(&summary.manifest.run_id).unwrap().unwrap();
    assert_eq!(kept.manifest.platform, other);
}

/// Audit AT-9: a seed above 2^53 - 1 is refused (a browser would round it, so the displayed seed
/// would not reproduce the run); 2^53 - 1 itself is accepted.
#[test]
fn seeds_beyond_the_json_safe_range_are_refused() {
    let dir = TempDir::new("seed");
    let ctx = make_dataset(&dir, 30);
    let mut cfg = preset(ctx.manifest(), "maker_baseline");
    cfg.seed = 9_007_199_254_740_993;
    match pipeline::execute(&ctx, &cfg) {
        Err(err @ AppError::InvalidConfig(_)) => {
            assert_eq!(err.status().as_u16(), 422);
            assert!(err.to_string().contains("seed"), "{err}");
        }
        other => panic!("expected invalid_config, got {:?}", other.map(|_| ())),
    }
    cfg.seed = 9_007_199_254_740_991;
    assert!(pipeline::check_config(&cfg).is_ok());
}
