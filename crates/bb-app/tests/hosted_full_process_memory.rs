//! Audit AT-5 / SEC-6, full-process fixture (`scripts/research/hosted-memory.sh`,
//! `docs/rebuild/HOSTED_MEMORY.md`): the candidate jobs the fixture measures in the hosted
//! container must be exactly the largest admitted shapes of every preset on the datasets a
//! hosted deployment carries (the three synthetic datasets `blackbox demo seed` registers; the
//! hosted API has no dataset upload).
//!
//! A largest admitted shape of a preset on a dataset: the smallest admitted strategy interval
//! (1 ms grid), equity sampled on every tick, and the largest admitted feed-latency offset (it
//! lengthens the local clock span, adding ticks and equity samples). The soft cap on recorded
//! queue updates takes two values where the prediction does not depend on it (resting-quote and
//! taker strategies): the preset's own cap, and no cap. Without a cap a maker run can reach the
//! hard tape budget early and stop; with the preset's cap it runs to the end with fewer tape
//! events. Which shape uses the most memory is measured, not predicted: the fixture runs every
//! candidate. Scripted orders keep the preset's cap, which their prediction includes.
//!
//! If this test fails, a preset, a dataset or a hosted limit changed: replace the entries of
//! `JOB_CANDIDATES` in the script with the `expected` entries this test prints.

use bb_app::{demos, presets};
use bb_core::{
    config::{MAX_INTERVAL_NS, MAX_LATENCY_NS, MIN_INTERVAL_NS, RunConfig, StrategyKind},
    dataset::DatasetManifest,
    limits::{ExecutionLimits, WorkEstimate},
};
use bb_data::synthetic::SyntheticConfig;

/// Intervals and offsets are searched on a 1 ms grid.
const STEP_NS: i64 = 1_000_000;

fn shaped(base: &RunConfig, interval_ns: i64, offset_ns: i64, max_queue_updates: u32) -> RunConfig {
    let mut config = base.clone();
    config.strategy.interval_ns = interval_ns;
    config.record.sample_every_ticks = 1;
    config.record.max_queue_updates_per_order = max_queue_updates;
    config.execution.feed_latency_offset_ns = offset_ns;
    config
}

fn admitted(
    manifest: &DatasetManifest,
    config: &RunConfig,
    hosted: &ExecutionLimits,
) -> Option<WorkEstimate> {
    let estimate = config.estimate_work(manifest);
    (config.validate().is_empty() && hosted.admit(&estimate).is_ok()).then_some(estimate)
}

/// Largest `k` in `lo..=hi` with `ok(k)`, given `ok(lo)` and `ok` monotone (true then false).
fn last_true(mut lo: i64, mut hi: i64, ok: impl Fn(i64) -> bool) -> i64 {
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// `(interval, offset, estimate)` of the largest admitted shape with queue cap `cap`, if any.
fn largest_shape(
    manifest: &DatasetManifest,
    base: &RunConfig,
    cap: u32,
    hosted: &ExecutionLimits,
) -> Option<(i64, i64, WorkEstimate)> {
    let ok = |interval: i64, offset: i64| {
        admitted(manifest, &shaped(base, interval, offset, cap), hosted)
    };
    // Estimates shrink as the interval grows: the smallest admitted interval is the first.
    let interval = (MIN_INTERVAL_NS..=MAX_INTERVAL_NS.min(10_000 * STEP_NS))
        .step_by(STEP_NS as usize)
        .find(|&i| ok(i, 0).is_some())?;
    let steps = last_true(0, MAX_LATENCY_NS / STEP_NS, |k| {
        ok(interval, k * STEP_NS).is_some()
    });
    let offset = steps * STEP_NS;
    Some((interval, offset, ok(interval, offset)?))
}

/// Queue-update caps measured for `base`: its own, and none where the prediction ignores it.
fn caps(base: &RunConfig) -> Vec<u32> {
    let own = base.record.max_queue_updates_per_order;
    match base.strategy.kind {
        StrategyKind::Scripted(_) => vec![own],
        _ if own == u32::MAX => vec![own],
        _ => vec![own, u32::MAX],
    }
}

/// The whitespace-separated entries of `JOB_CANDIDATES="..."` in the fixture script.
fn script_candidates() -> Vec<String> {
    let script = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/research/hosted-memory.sh"),
    )
    .expect("read scripts/research/hosted-memory.sh");
    let (_, rest) = script
        .split_once("\nJOB_CANDIDATES=\"")
        .expect("JOB_CANDIDATES assignment");
    let (list, _) = rest.split_once('"').expect("closing quote");
    list.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn the_fixture_measures_the_largest_admitted_shape_of_every_hosted_preset() {
    let hosted = ExecutionLimits::hosted();
    let mut configs = vec![SyntheticConfig::default()];
    configs.extend(demos::stress_datasets());
    let mut expected = Vec::new();
    for config in &configs {
        let manifest = bb_data::synthetic::generate(config)
            .expect("generate")
            .manifest;
        for preset in presets::presets(&manifest) {
            for cap in caps(&preset.config) {
                let (interval, offset, estimate) =
                    largest_shape(&manifest, &preset.config, cap, &hosted).unwrap_or_else(|| {
                        panic!("{} (cap {cap}) refused on {}", preset.id, manifest.id)
                    });
                println!(
                    "{:<30} {:<25} interval {:>9} offset {:>12} cap {:>10}  ticks {:>7} tape {:>7} equity {:>7} bytes {:>10}",
                    manifest.id,
                    preset.id,
                    interval,
                    offset,
                    cap,
                    estimate.ticks,
                    estimate.tape_events_upper,
                    estimate.equity_samples,
                    estimate.output_bytes_upper
                );
                expected.push(format!(
                    "{}/{}/{interval}/{offset}/{cap}",
                    manifest.id, preset.id
                ));
            }
        }
    }
    println!(
        "expected JOB_CANDIDATES entries:\n    {}",
        expected.join("\n    ")
    );
    assert_eq!(script_candidates(), expected);
}
