//! Controlled experiments assembled from the instrument-sized touch-maker presets.

use std::collections::BTreeSet;

use bb_core::{
    config::{FeeConfig, QueueModelConfig, RunConfig},
    dataset::DatasetManifest,
};
use bb_data::synthetic::{StressRegime, SyntheticConfig};
use serde_json::Value;

use crate::{
    api::{Demo, DemoRun},
    presets, study,
};

/// Id of the demo contrasting execution assumptions and synthetic stress regimes (Phase 5).
pub const EXECUTION_VALIDATION: &str = "execution_validation";
/// Its normal-liquidity session: the default synthetic dataset.
pub const NORMAL_DATASET: &str = "synthetic-demo";
/// Its liquidity-depletion session.
pub const DEPLETED_DATASET: &str = "synthetic-stress-depletion";
/// Its wide-spread session.
pub const WIDE_DATASET: &str = "synthetic-stress-wide-spreads";
/// Every dataset `execution_validation` runs on.
pub const EXECUTION_VALIDATION_DATASETS: [&str; 3] =
    [NORMAL_DATASET, DEPLETED_DATASET, WIDE_DATASET];

/// The stress datasets `demo seed` registers next to the default synthetic dataset: the same
/// generator settings (seed, instrument, session length, flow) with one named, synthetic regime
/// intervention each. Their ids sort after `synthetic-demo`, which stays the default catalogue
/// dataset.
pub fn stress_datasets() -> [SyntheticConfig; 2] {
    let regime = |id: &str, title: &str, regime| SyntheticConfig {
        id: id.into(),
        title: title.into(),
        regime: Some(regime),
        ..SyntheticConfig::default()
    };
    [
        regime(
            DEPLETED_DATASET,
            "Synthetic stress: liquidity depletion",
            StressRegime::LiquidityDepletion,
        ),
        regime(
            WIDE_DATASET,
            "Synthetic stress: widening spreads",
            StressRegime::WideningSpreads,
        ),
    ]
}

/// Run pairs of `execution_validation` stored as curated comparisons, as (A, B) indices into its
/// runs. Same-dataset pairs align order by order; the two cross-dataset pairs change only the
/// market regime and are stored metrics-only (not like-for-like). The depleted risk-adverse run
/// is paired with the depleted baseline, never with the normal one (two changes at once).
const EXECUTION_VALIDATION_PAIRS: [(usize, usize); 6] =
    [(0, 1), (0, 2), (0, 3), (4, 5), (0, 4), (0, 6)];

/// Dotted configuration paths that differ from the first run in at least one other run, by the
/// same rule the comparison uses.
fn varied(runs: &[DemoRun]) -> BTreeSet<String> {
    runs[1..]
        .iter()
        .flat_map(|r| bb_forensics::config_diffs(&runs[0].config, &r.config))
        .map(|d| d.path)
        .collect()
}

/// A copyable POSIX-shell recipe. It writes the complete config before invoking the CLI;
/// no preset lookup or unrecorded parameter overrides are needed.
pub(crate) fn reproduce(config: &RunConfig, filename: &str) -> String {
    format!(
        "cat > {filename} <<'BLACKBOX_CONFIG'\n{}\nBLACKBOX_CONFIG\nblackbox run --config {filename} --dataset {}",
        serde_json::to_string_pretty(config).expect("RunConfig serializes"),
        config.dataset_id
    )
}

fn experiment(
    datasets: Vec<String>,
    id: &str,
    title: &str,
    question: &str,
    how_to_read: &str,
    runs: Vec<DemoRun>,
) -> Demo {
    let values: Vec<Value> = runs
        .iter()
        .map(|r| serde_json::to_value(&r.config).expect("RunConfig serializes"))
        .collect();
    let varied = varied(&runs);
    let mut controlled = datasets;
    for (pointer, label) in [
        (
            "/strategy",
            "Strategy kind, parameters and evaluation interval",
        ),
        ("/seed", "Seed"),
        ("/risk", "Risk limits"),
        ("/record", "Recorder settings"),
        ("/execution/exchange_model", "Exchange fill model"),
        ("/execution/queue_model/type", "Queue model"),
        ("/execution/queue_model/n", "Queue exponent"),
        ("/execution/order_latency/type", "Order latency model"),
        (
            "/execution/order_latency/entry_ns",
            "Order entry latency in nanoseconds",
        ),
        (
            "/execution/order_latency/response_ns",
            "Order response latency in nanoseconds",
        ),
        (
            "/execution/feed_latency_offset_ns",
            "Feed latency offset in nanoseconds",
        ),
        (
            "/execution/fees/maker",
            "Maker fee as a fraction of traded value",
        ),
        (
            "/execution/fees/taker",
            "Taker fee as a fraction of traded value",
        ),
    ] {
        if let Some(first) = values[0].pointer(pointer)
            && values[1..]
                .iter()
                .all(|v| v.pointer(pointer) == Some(first))
        {
            controlled.push(format!("{label}: {first}"));
        }
    }
    let reproduce = runs
        .iter()
        .enumerate()
        .map(|(i, r)| reproduce(&r.config, &format!("blackbox-demo-{id}-{}.json", i + 1)))
        .collect();
    Demo {
        id: id.into(),
        title: title.into(),
        question: question.into(),
        varied: varied.into_iter().collect(),
        controlled,
        how_to_read: how_to_read.into(),
        runs,
        reproduce,
    }
}

fn dataset_line(manifest: &DatasetManifest) -> String {
    format!(
        "Dataset id: {}; events SHA-256: {}",
        manifest.id, manifest.events_sha256
    )
}

/// The three canonical demos, sized exactly like the existing presets.
pub fn demos(manifest: &DatasetManifest) -> Vec<Demo> {
    let catalogue = presets::presets(manifest);
    let config = |id: &str| {
        catalogue
            .iter()
            .find(|p| p.id == id)
            .expect("built-in preset exists")
            .config
            .clone()
    };
    let run = |label: &str, config: RunConfig| DemoRun {
        label: label.into(),
        config,
    };
    let baseline = config("maker_baseline");
    let mut log = baseline.clone();
    log.execution.queue_model = QueueModelConfig::LogProb;
    let mut zero = baseline.clone();
    zero.execution.fees = FeeConfig {
        maker: 0.0,
        taker: 0.0,
    };
    vec![
        experiment(
            vec![dataset_line(manifest)],
            "cost_of_latency",
            "Cost of latency",
            "How do simulated decisions and fills change between 1 ms and 50 ms order latency each way?",
            "Compare the aligned decisions and first divergence, then inspect simulated fills, inventory and net P&L. Later differences may be knock-on effects; replay does not predict live performance.",
            vec![
                run("1 ms entry and response latency", baseline.clone()),
                run("50 ms entry and response latency", config("maker_slow")),
            ],
        ),
        experiment(
            vec![dataset_line(manifest)],
            "queue_position",
            "Queue position uncertainty",
            "How sensitive are simulated touch-maker outcomes to the L2 queue-position model?",
            "Compare the modeled queue trajectories and aligned fill outcomes. L2 data cannot establish actual queue position; differences measure sensitivity to assumptions, not which model is true.",
            vec![
                run("Risk-adverse queue", config("maker_conservative_queue")),
                run("Log-probability queue", log),
                run("PowerProb3 queue, n = 3", baseline.clone()),
            ],
        ),
        experiment(
            vec![dataset_line(manifest)],
            "gross_vs_net",
            "Gross versus net",
            "How does the fee schedule change accounting for the same simulated touch-maker fills?",
            "Check fill identity before comparing gross P&L, fees and net P&L. Only when fills match can the net difference be read as pure fee accounting; this is not a forecast of profitability.",
            vec![
                run("2 bp maker / 4 bp taker fees", baseline),
                run("Zero maker and taker fees", zero),
                run("1 bp maker rebate / 4 bp taker fee", config("maker_rebate")),
            ],
        ),
    ]
}

/// The `execution_validation` demo: one execution assumption or the synthetic market regime
/// changed at a time against the `maker_baseline` preset on normal liquidity, with the contrasts
/// of the Phase 5 study (`study.rs`, docs/research/EXECUTION_MODEL_VALIDATION.md). `manifest`
/// answers the current manifest of each of [`EXECUTION_VALIDATION_DATASETS`]; without all of
/// them the demo is unavailable (`None`).
pub fn execution_validation<'a>(
    manifest: impl Fn(&str) -> Option<&'a DatasetManifest>,
) -> Option<Demo> {
    let normal = manifest(NORMAL_DATASET)?;
    let depleted = manifest(DEPLETED_DATASET)?;
    let wide = manifest(WIDE_DATASET)?;
    let baseline_on = |m: &DatasetManifest| {
        presets::find(m, "maker_baseline")
            .expect("built-in preset exists")
            .config
    };
    let run = |label: String, config: RunConfig| DemoRun { label, config };
    let baseline = baseline_on(normal);
    let mut heavy = baseline.clone();
    heavy.execution.order_latency = study::latency("heavy_tail");
    let mut cancel = baseline.clone();
    cancel.execution.order_latency = study::latency("cancel_delay");
    let mut feed = baseline.clone();
    feed.execution.feed_latency_offset_ns = 20_000_000;
    let depleted_baseline = baseline_on(depleted);
    let mut depleted_queue = depleted_baseline.clone();
    depleted_queue.execution.queue_model = QueueModelConfig::RiskAdverse;
    let generator = SyntheticConfig::default();
    let mut datasets = vec![format!(
        "Synthetic generator settings other than the regime: seed {}, {} s session, same instrument and flow parameters",
        generator.seed,
        generator.duration_ns / 1_000_000_000
    )];
    datasets.extend(
        [normal, depleted, wide]
            .into_iter()
            .map(|m| format!("Events SHA-256 of {}: {}", m.id, m.events_sha256)),
    );
    Some(experiment(
        datasets,
        EXECUTION_VALIDATION,
        "Execution model validation",
        "How do simulated touch-maker fills, inventory and P&L shift when one execution assumption (latency shape, cancel delay, feed delay, queue model) or the synthetic market regime changes?",
        "Same-dataset pairs share the event hash, so their orders align: read the first divergence and lifecycle classes before the metric deltas. Regime pairs (depleted liquidity, widening spreads) replay different synthetic events: they are not like-for-like, compare metrics only. The regimes are synthetic assumptions, not historical crashes. Heavy-tail latency is one seed of a stochastic model; the seed ensemble is in the Phase 5 study.",
        vec![
            run("Normal-liquidity baseline".into(), baseline.clone()),
            run(
                format!(
                    "Heavy-tail order latency (1 ms median, seed {})",
                    heavy.seed
                ),
                heavy,
            ),
            run(
                "Cancel-entry latency 20 ms (entry/response 1 ms)".into(),
                cancel,
            ),
            run("Feed latency offset +20 ms".into(), feed),
            run("Depleted-liquidity regime".into(), depleted_baseline),
            run(
                "Depleted-liquidity regime, risk-adverse queue".into(),
                depleted_queue,
            ),
            run("Widening-spreads regime".into(), baseline_on(wide)),
        ],
    ))
}

/// The run pairs `demo seed` stores as curated comparisons, as (A, B) indices into `demo.runs`:
/// the first run against each other run, except for [`EXECUTION_VALIDATION`].
pub fn curated_pairs(demo: &Demo) -> Vec<(usize, usize)> {
    if demo.id == EXECUTION_VALIDATION {
        EXECUTION_VALIDATION_PAIRS.to_vec()
    } else {
        (1..demo.runs.len()).map(|i| (0, i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use bb_core::limits::ExecutionLimits;

    use super::*;

    #[test]
    fn execution_validation_fits_hosted_limits_and_changes_one_assumption_per_pair() {
        let normal = bb_data::synthetic::generate(&SyntheticConfig::default())
            .unwrap()
            .manifest;
        assert_eq!(normal.id, NORMAL_DATASET);
        let stress: Vec<DatasetManifest> = stress_datasets()
            .iter()
            .map(|c| bb_data::synthetic::generate(c).unwrap().manifest)
            .collect();
        let manifests: Vec<&DatasetManifest> = std::iter::once(&normal).chain(&stress).collect();
        let demo = execution_validation(|id| manifests.iter().copied().find(|m| m.id == id))
            .expect("every dataset is known");
        assert!(demo.runs.len() <= 10);
        assert!(*NORMAL_DATASET < *DEPLETED_DATASET && *NORMAL_DATASET < *WIDE_DATASET);
        let hosted = ExecutionLimits::hosted();
        for run in &demo.runs {
            let m = manifests
                .iter()
                .find(|m| m.id == run.config.dataset_id)
                .unwrap();
            assert!(run.config.validate().is_empty(), "{}", run.label);
            assert_eq!(m.instrument, normal.instrument);
            let estimate = run.config.estimate_work(m);
            hosted
                .admit(&estimate)
                .unwrap_or_else(|v| panic!("{}: {v}", run.label));
            eprintln!(
                "{}: {} rows, {} ticks, <= {} tape events",
                run.label, estimate.dataset_rows, estimate.ticks, estimate.tape_events_upper
            );
            assert!(format!("{}: {}", demo.id, run.label).chars().count() <= 100);
        }
        for (a, b) in curated_pairs(&demo) {
            let (a, b) = (&demo.runs[a], &demo.runs[b]);
            // Titles stay whole (seed bounds them to 100 characters).
            assert!(
                format!("{}: {} vs {}", demo.id, a.label, b.label)
                    .chars()
                    .count()
                    <= 100
            );
            let diffs: Vec<String> = bb_forensics::config_diffs(&a.config, &b.config)
                .into_iter()
                .map(|d| d.path)
                .collect();
            if a.config.dataset_id == b.config.dataset_id {
                assert!(
                    diffs.iter().all(|p| p.starts_with("execution.")),
                    "{diffs:?}"
                );
            } else {
                assert_eq!(diffs, ["dataset_id"], "{} vs {}", a.label, b.label);
            }
        }
    }

    #[test]
    fn canonical_demos_keep_first_against_each_pairs() {
        let manifest = bb_data::synthetic::generate(&SyntheticConfig::default())
            .unwrap()
            .manifest;
        let pairs: Vec<Vec<(usize, usize)>> = demos(&manifest).iter().map(curated_pairs).collect();
        assert_eq!(
            pairs,
            [vec![(0, 1)], vec![(0, 1), (0, 2)], vec![(0, 1), (0, 2)]]
        );
    }
}
