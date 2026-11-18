//! Local-only, sequential execution-model study. No HTTP route or database writes.
//!
//! Besides the research output (`study.json`, `study.md`, `cells/*.json`), the study writes a
//! versioned, read-only public result (`public/study-result.json`, contract
//! [`PUBLIC_SCHEMA`] version [`PUBLIC_SCHEMA_VERSION`]) with its SHA-256 next to it. The public
//! result holds per-cell summaries only (no tapes, no wall-clock facts), so the same binary on the
//! same platform class writes it byte for byte identically. The web study viewer reads it.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::BufWriter,
    path::Path,
    time::Instant,
};

use anyhow::{Result, bail};
use bb_core::{
    compare::Comparison,
    config::{CubicLeg, LogNormalLeg, OrderLatencyConfig, QueueModelConfig, RunConfig},
    dataset::DatasetManifest,
    result::RunResult,
};
use bb_data::synthetic::{StressRegime, SyntheticConfig};
use bb_forensics::{CompareInput, bootstrap_mean, compare_runs};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{datasets::DatasetCtx, demos, pipeline, presets};

const BOOTSTRAP_SEED: u64 = 3105;
const BOOTSTRAP_REPLICATES: usize = 2000;

/// Name of the public study-result contract.
pub const PUBLIC_SCHEMA: &str = "blackbox.study-result";
/// Version of the public study-result contract. Bump on any incompatible change; additive fields
/// keep the version.
pub const PUBLIC_SCHEMA_VERSION: u32 = 1;
/// File name of the public study result inside `<out>/public/`; its SHA-256 is in
/// `<file>.sha256` (`sha256sum` format).
pub const PUBLIC_FILE: &str = "study-result.json";

/// Every latency level of the study, the matched-median baseline first.
const LATENCIES: [&str; 7] = [
    "constant",
    "jitter",
    "log_normal",
    "heavy_tail",
    "entry_slow",
    "response_slow",
    "cancel_delay",
];

/// The study's latency levels by name (also used by the `execution_validation` demo).
pub(crate) fn latency(name: &str) -> OrderLatencyConfig {
    let leg = |median_ns, sigma| LogNormalLeg { median_ns, sigma };
    match name {
        "jitter" => OrderLatencyConfig::Jitter {
            entry_ns: 500_000,
            response_ns: 500_000,
            jitter_ns: 1_000_000,
        },
        "log_normal" => OrderLatencyConfig::LogNormal {
            entry: leg(1_000_000, 0.7),
            response: leg(1_000_000, 0.7),
            cancel: None,
        },
        "heavy_tail" => {
            // Median x=(1+clip)/2=.55. Round the minimum to integer nanoseconds.
            let tail = CubicLeg {
                min_ns: (1_000_000.0_f64 / (1.0 + 0.5 / (10.0 * 0.55_f64.powi(3)))).round() as i64,
                jitter: 0.5,
                clip: 0.1,
                unit: 10.0,
            };
            OrderLatencyConfig::HeavyTail {
                entry: tail,
                response: tail,
                cancel: None,
            }
        }
        "entry_slow" => OrderLatencyConfig::Constant {
            entry_ns: 10_000_000,
            response_ns: 1_000_000,
        },
        "response_slow" => OrderLatencyConfig::Constant {
            entry_ns: 1_000_000,
            response_ns: 10_000_000,
        },
        "cancel_delay" => OrderLatencyConfig::LogNormal {
            entry: leg(1_000_000, 0.0),
            response: leg(1_000_000, 0.0),
            cancel: Some(leg(20_000_000, 0.0)),
        },
        _ => OrderLatencyConfig::Constant {
            entry_ns: 1_000_000,
            response_ns: 1_000_000,
        },
    }
}

fn stochastic(name: &str) -> bool {
    matches!(name, "jitter" | "log_normal" | "heavy_tail")
}

fn queues() -> Vec<(&'static str, QueueModelConfig)> {
    vec![
        ("power_prob3", QueueModelConfig::PowerProb3 { n: 3.0 }),
        ("risk_adverse", QueueModelConfig::RiskAdverse),
        ("log_prob", QueueModelConfig::LogProb),
        ("log_prob2", QueueModelConfig::LogProb2),
        ("power_prob", QueueModelConfig::PowerProb { n: 3.0 }),
        ("power_prob2", QueueModelConfig::PowerProb2 { n: 3.0 }),
    ]
}

fn measures(run: &RunResult) -> BTreeMap<String, f64> {
    let m = &run.metrics;
    let mut out = BTreeMap::from([
        ("fills".into(), m.fill_count as f64),
        ("maker_fills".into(), m.maker_fill_count as f64),
        ("taker_fills".into(), m.taker_fill_count as f64),
        (
            "unfilled_orders".into(),
            m.orders_submitted
                .saturating_sub(m.orders_filled + m.orders_partially_filled) as f64,
        ),
        ("fees".into(), m.fees),
        ("mean_abs_position".into(), m.mean_abs_position),
        (
            "max_abs_position".into(),
            m.max_long.abs().max(m.max_short.abs()),
        ),
        ("net_pnl".into(), m.net_pnl),
    ]);
    if m.fill_count > 0 {
        out.insert(
            "maker_fill_share".into(),
            m.maker_fill_count as f64 / m.fill_count as f64,
        );
        out.insert(
            "taker_fill_share".into(),
            m.taker_fill_count as f64 / m.fill_count as f64,
        );
    }
    if let Some(v) = m.fill_ratio_orders {
        out.insert("fill_rate".into(), v);
    }
    if let Some(q) = &m.time_to_first_fill_ns {
        out.insert("time_to_fill_p50_ns".into(), q.p50);
        out.insert("time_to_fill_p90_ns".into(), q.p90);
    }
    let mut queue_n = 0_u64;
    let mut queue_sum = 0.0;
    let mut queue_min = f64::INFINITY;
    let mut queue_max = f64::NEG_INFINITY;
    for value in run.orders.iter().filter_map(|o| o.queue_ahead_at_entry) {
        queue_n += 1;
        queue_sum += value;
        queue_min = queue_min.min(value);
        queue_max = queue_max.max(value);
    }
    if queue_n > 0 {
        out.insert("modeled_queue_at_entry_n".into(), queue_n as f64);
        out.insert(
            "modeled_queue_at_entry_mean".into(),
            queue_sum / queue_n as f64,
        );
        out.insert("modeled_queue_at_entry_min".into(), queue_min);
        out.insert("modeled_queue_at_entry_max".into(), queue_max);
    }
    out
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    use std::io::Write;
    let mut writer = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.flush()?;
    Ok(())
}

/// The study's name of a queue model, when it is one of its levels.
fn queue_name(queue: &QueueModelConfig) -> Option<&'static str> {
    queues()
        .into_iter()
        .find(|(_, q)| q == queue)
        .map(|(name, _)| name)
}

/// The study's name of an order-latency model, when it is one of its levels.
fn latency_name(config: &OrderLatencyConfig) -> Option<&'static str> {
    LATENCIES.into_iter().find(|name| latency(name) == *config)
}

/// The deterministic part of a dataset manifest the public result names. `regime` is `None` for
/// the default generator (no regime intervention).
fn public_dataset(manifest: &DatasetManifest, regime: Option<&str>) -> Value {
    json!({
        "id": manifest.id,
        "title": manifest.title,
        "regime": regime,
        "events_sha256": manifest.events_sha256,
        "events_bytes": manifest.events_bytes,
        "counts": manifest.counts,
        "generator": manifest.generator,
    })
}

/// A run's bounded, deterministic public summary: identity hashes, scalar measures, the P&L
/// decomposition and its comparison with a reference run. Never the tape, the fills or
/// wall-clock facts (`created_unix_ms`, `wall_time_ms`).
fn public_run(
    run: &RunResult,
    measures: &BTreeMap<String, f64>,
    comparison: Option<&Comparison>,
) -> Map<String, Value> {
    let manifest = &run.manifest;
    let m = &run.metrics;
    let pair = |p: &bb_core::compare::OrderPair| {
        json!({
            "order_id": p.order_id,
            "tick_no": p.tick_no,
            "decision_ts_ns": p.decision_ts,
            "class": p.class,
            "differences": p.differences,
        })
    };
    let comparison = comparison.map(|c| {
        let alignment = c.alignment.as_ref();
        json!({
            "reference_run_id": c.run_a,
            "aligned": c.eligibility.aligned,
            "like_for_like": c.eligibility.like_for_like,
            "config_diffs": c.config_diffs,
            "first_divergence": alignment.and_then(|a| a.first_divergence.as_ref()).map(pair),
            "first_outcome_divergence": alignment.and_then(|a| a.first_outcome_divergence.as_ref()).map(pair),
            "lifecycle_counts": alignment.map(|a| json!({
                "identical": a.identical,
                "same_fills_different_history": a.same_fills_different_history,
                "same_fills_different_fees": a.same_fills_different_fees,
                "same_outcome_different_timing": a.same_outcome_different_timing,
                "same_decision_different_outcome": a.same_decision_different_outcome,
                "different_decision": a.different_decision,
                "only_a": a.only_a,
                "only_b": a.only_b
            })),
        })
    });
    let Value::Object(summary) = json!({
        "run_id": manifest.run_id,
        "config_sha256": manifest.config_sha256,
        "result_sha256": manifest.result_sha256,
        "dataset": { "id": manifest.dataset.id, "sha256": manifest.dataset.sha256, "rows": manifest.dataset.rows },
        "seed": manifest.config.seed,
        "queue_model": queue_name(&manifest.config.execution.queue_model),
        "latency": latency_name(&manifest.config.execution.order_latency),
        "feed_offset_ns": manifest.config.execution.feed_latency_offset_ns,
        "measures": measures,
        "accounting": {
            "realized_pnl": m.realized_pnl,
            "unrealized_pnl": m.unrealized_pnl,
            "gross_pnl": m.realized_pnl + m.unrealized_pnl,
            "fees": m.fees,
            "net_pnl": m.net_pnl
        },
        "orders": {
            "submitted": m.orders_submitted,
            "filled": m.orders_filled,
            "partially_filled": m.orders_partially_filled,
            "canceled": m.orders_canceled
        },
        "comparison": comparison,
    }) else {
        unreachable!("json! object")
    };
    summary
}

/// The `execution_validation` demo runs recomputed on the demo's own synthetic datasets (the
/// default generator and [`demos::stress_datasets`], generated under `root`), with the demo's
/// curated pairs. Their config and dataset hashes are the ones `demo seed` stores, so a viewer
/// can link a stored public run after checking both hashes. Keeps only pair-A runs resident.
fn demo_anchors(root: &Path) -> Result<Value> {
    fs::create_dir_all(root)?;
    let mut contexts = Vec::new();
    let mut datasets = Vec::new();
    for config in std::iter::once(SyntheticConfig::default()).chain(demos::stress_datasets()) {
        let built = bb_data::synthetic::generate(&config)?;
        bb_data::store::save(root, &built)?;
        drop(built);
        let ctx = DatasetCtx::load(root, &config.id)?;
        let regime = config
            .regime
            .map(|r| serde_json::to_value(r).map(|v| v.as_str().unwrap_or_default().to_owned()))
            .transpose()?;
        datasets.push(public_dataset(ctx.manifest(), regime.as_deref()));
        contexts.push(ctx);
    }
    let ctx_of = |id: &str| contexts.iter().find(|c| c.manifest().id == id);
    let Some(demo) = demos::execution_validation(|id| ctx_of(id).map(DatasetCtx::manifest)) else {
        bail!("execution_validation needs each of its datasets");
    };
    let pairs = demos::curated_pairs(&demo);
    let mut resident: BTreeMap<usize, RunResult> = BTreeMap::new();
    let mut runs = Vec::new();
    for (index, demo_run) in demo.runs.iter().enumerate() {
        let ctx = ctx_of(&demo_run.config.dataset_id).expect("generated above");
        let run = pipeline::execute(ctx, &demo_run.config)?;
        let reference = pairs.iter().find(|(_, b)| *b == index).map(|(a, _)| *a);
        let comparison = reference.map(|a| {
            let base = &resident[&a];
            let base_ctx = ctx_of(&base.manifest.dataset.id).expect("generated above");
            compare_runs(
                CompareInput {
                    run: base,
                    label: "curated pair A",
                    instrument: Some(&base_ctx.manifest().instrument),
                },
                CompareInput {
                    run: &run,
                    label: "curated pair B",
                    instrument: Some(&ctx.manifest().instrument),
                },
            )
        });
        let mut entry = public_run(&run, &measures(&run), comparison.as_ref());
        entry.insert("index".into(), json!(index));
        entry.insert("label".into(), json!(demo_run.label));
        entry.insert("reference_index".into(), json!(reference));
        runs.push(Value::Object(entry));
        if pairs.iter().any(|(a, _)| *a == index) {
            resident.insert(index, run);
        }
    }
    Ok(json!({
        "demo_id": demo.id,
        "title": demo.title,
        "question": demo.question,
        "datasets": datasets,
        "runs": runs,
        "pairs": pairs,
    }))
}

/// Generates only synthetic datasets inside a fresh output directory and admits every cell
/// through the ordinary local pipeline. At most a baseline and current run are resident.
pub fn exec_validation(out: &Path, quick: bool) -> Result<Value> {
    if out.exists() {
        bail!(
            "study output already exists: {}; choose a fresh directory",
            out.display()
        );
    }
    fs::create_dir_all(out.join("datasets"))?;
    fs::create_dir_all(out.join("cells"))?;
    let started = Instant::now();
    let regimes = if quick {
        vec![
            StressRegime::NormalLiquidity,
            StressRegime::LiquidityDepletion,
        ]
    } else {
        vec![
            StressRegime::NormalLiquidity,
            StressRegime::WideningSpreads,
            StressRegime::LiquidityDepletion,
            StressRegime::VolatilityBurst,
            StressRegime::ThinBook,
            StressRegime::FeedStaleness,
        ]
    };
    let seeds: Vec<u64> = if quick {
        vec![1, 2]
    } else {
        vec![1, 7, 19, 31, 43]
    };
    let mut cells = Vec::new();
    let mut aggregates = Vec::new();
    let mut global_baseline: Option<bb_core::result::RunManifest> = None;
    let mut global_measures = BTreeMap::<String, f64>::new();
    let mut public_cells = Vec::new();
    let mut public_datasets = Vec::new();
    for regime in regimes {
        let regime_name = serde_json::to_value(regime)?.as_str().unwrap().to_owned();
        let generator = SyntheticConfig {
            id: format!("study-{regime_name}"),
            duration_ns: if quick { 2_000_000_000 } else { 30_000_000_000 },
            regime: Some(regime),
            ..SyntheticConfig::default()
        };
        let built = bb_data::synthetic::generate(&generator)?;
        bb_data::store::save(&out.join("datasets"), &built)?;
        drop(built);
        let ctx = DatasetCtx::load(&out.join("datasets"), &generator.id)?;
        public_datasets.push(public_dataset(ctx.manifest(), Some(&regime_name)));
        let base = presets::find(ctx.manifest(), "maker_baseline")
            .unwrap()
            .config;
        let baseline = pipeline::execute(&ctx, &base)?;
        if global_baseline.is_none() {
            global_baseline = Some(baseline.manifest.clone());
            global_measures = measures(&baseline);
        }
        let mut designs: Vec<(&str, QueueModelConfig, &str, i64, &str)> = vec![(
            "power_prob3",
            base.execution.queue_model,
            "constant",
            0,
            "regime_baseline",
        )];
        if regime == StressRegime::NormalLiquidity {
            for (name, queue) in queues().into_iter().skip(1).take(if quick { 1 } else { 5 }) {
                designs.push((name, queue, "constant", 0, "one_factor"));
            }
            for name in if quick {
                vec!["jitter", "cancel_delay"]
            } else {
                vec![
                    "jitter",
                    "log_normal",
                    "heavy_tail",
                    "entry_slow",
                    "response_slow",
                    "cancel_delay",
                ]
            } {
                designs.push((
                    "power_prob3",
                    base.execution.queue_model,
                    name,
                    0,
                    "one_factor",
                ));
            }
            designs.push((
                "power_prob3",
                base.execution.queue_model,
                "constant",
                20_000_000,
                "one_factor",
            ));
        }
        if matches!(
            regime,
            StressRegime::NormalLiquidity
                | StressRegime::WideningSpreads
                | StressRegime::LiquidityDepletion
        ) {
            for (name, queue) in queues().into_iter().take(2) {
                if regime != StressRegime::NormalLiquidity && name != "power_prob3" {
                    designs.push((name, queue, "constant", 0, "factorial"));
                }
                if !(regime == StressRegime::NormalLiquidity && name == "power_prob3") && !quick {
                    designs.push((name, queue, "heavy_tail", 0, "factorial"));
                }
            }
        }
        for (queue_name, queue, latency_name, feed_offset, design) in designs {
            let run_seeds = if stochastic(latency_name) {
                seeds.as_slice()
            } else {
                &seeds[..1]
            };
            let mut values: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut members = Vec::new();
            for seed in run_seeds {
                let mut config: RunConfig = base.clone();
                config.seed = *seed;
                config.execution.queue_model = queue;
                config.execution.order_latency = latency(latency_name);
                config.execution.feed_latency_offset_ns = feed_offset;
                let alternative = if config == base {
                    None
                } else {
                    Some(pipeline::execute(&ctx, &config)?)
                };
                let run = alternative.as_ref().unwrap_or(&baseline);
                let metrics = measures(run);
                for (key, value) in &metrics {
                    values.entry(key.clone()).or_default().push(*value);
                }
                let comparison = compare_runs(
                    CompareInput {
                        run: &baseline,
                        label: "within-regime baseline",
                        instrument: Some(&ctx.manifest().instrument),
                    },
                    CompareInput {
                        run,
                        label: "study cell",
                        instrument: Some(&ctx.manifest().instrument),
                    },
                );
                let execution = bb_analytics::execution::compute_execution_summary(
                    run,
                    &ctx.bbo,
                    &ctx.manifest().instrument,
                    &bb_analytics::execution::ExecutionConfig {
                        bootstrap_replicates: 0,
                        ..Default::default()
                    },
                )?;
                let global_delta: BTreeMap<&str, f64> = metrics
                    .iter()
                    .filter_map(|(key, value)| {
                        global_measures
                            .get(key)
                            .map(|base| (key.as_str(), value - base))
                    })
                    .collect();
                let id = &run.manifest.run_id;
                write_json(
                    &out.join("cells").join(format!("{id}.json")),
                    &json!({ "manifest": run.manifest, "dataset_manifest": ctx.manifest(), "metrics": run.metrics, "measures": metrics, "first_fill": execution.first_fill, "complete_fill": execution.complete_fill, "queue": execution.queue, "comparison": comparison }),
                )?;
                members.push(id.clone());
                let mut public = public_run(run, &metrics, Some(&comparison));
                for (key, value) in [
                    ("regime", json!(regime_name)),
                    ("queue_model", json!(queue_name)),
                    ("latency", json!(latency_name)),
                    ("design", json!(design)),
                    ("stochastic", json!(stochastic(latency_name))),
                    ("detail", json!(format!("cells/{id}.json"))),
                ] {
                    public.insert(key.into(), value);
                }
                public_cells.push(Value::Object(public));
                cells.push(json!({
                    "run_id": id,
                    "manifest": run.manifest,
                    "regime": regime_name,
                    "queue_model": queue_name,
                    "latency": latency_name,
                    "feed_offset_ns": feed_offset,
                    "seed": seed,
                    "design": design,
                    "stochastic": stochastic(latency_name),
                    "changed_vs_within_regime_baseline": comparison.config_diffs,
                    "changed_vs_global_baseline": {
                        "regime": if regime == StressRegime::NormalLiquidity { Value::Null } else { json!(regime_name) },
                        "execution_config_diffs": comparison.config_diffs
                    },
                    "delta_vs_global_baseline": global_delta,
                    "global_order_alignment_available": regime == StressRegime::NormalLiquidity,
                    "baseline_run_id": baseline.manifest.run_id,
                    "first_divergence": comparison.alignment.as_ref().and_then(|a| a.first_divergence.as_ref()),
                    "lifecycle_counts": comparison.alignment.as_ref().map(|a| json!({
                        "identical": a.identical,
                        "same_fills_different_history": a.same_fills_different_history,
                        "same_fills_different_fees": a.same_fills_different_fees,
                        "same_outcome_different_timing": a.same_outcome_different_timing,
                        "same_decision_different_outcome": a.same_decision_different_outcome,
                        "different_decision": a.different_decision,
                        "only_a": a.only_a,
                        "only_b": a.only_b
                    })),
                    "measures": metrics,
                    "detail": format!("cells/{id}.json")
                }));
            }
            let summary: BTreeMap<String, Value> = values.into_iter().map(|(key, vals)| {
                let value = if stochastic(latency_name) { json!({ "kind": "stochastic_seed_mean", "bootstrap": bootstrap_mean(&vals, BOOTSTRAP_REPLICATES, BOOTSTRAP_SEED) }) } else { json!({ "kind": "deterministic_single", "value": vals[0] }) };
                (key, value)
            }).collect();
            aggregates.push(json!({ "regime": regime_name, "queue_model": queue_name, "latency": latency_name, "feed_offset_ns": feed_offset, "design": design, "members": members, "metrics": summary }));
        }
    }
    let surfaces: Vec<Value> = [
        "normal_liquidity",
        "widening_spreads",
        "liquidity_depletion",
    ]
    .into_iter()
    .filter_map(|regime| {
        let rows: Vec<&Value> = aggregates
            .iter()
            .filter(|a| {
                a["regime"] == regime
                    && a["feed_offset_ns"] == 0
                    && (a["latency"] == "constant" || a["latency"] == "heavy_tail")
                    && (a["queue_model"] == "power_prob3" || a["queue_model"] == "risk_adverse")
            })
            .collect();
        (!rows.is_empty()).then(|| {
            json!({
                "stratum": { "regime": regime },
                "axes": ["queue_model", "latency"],
                "rows": rows
            })
        })
    })
    .collect();
    let design = json!({
        "generator_seed": 7,
        "duration_ns": if quick { 2_000_000_000_i64 } else { 30_000_000_000_i64 },
        "latency_seeds": seeds,
        "bootstrap_seed": BOOTSTRAP_SEED,
        "bootstrap_replicates": BOOTSTRAP_REPLICATES,
        "bootstrap_method": "percentile mean, nearest rank, resample independent stochastic run seeds",
        "matched_median_ns": 1_000_000,
        "cross_regime_alignment": "unavailable: different event hashes; comparisons use the baseline within each regime"
    });
    let caveats = json!([
        "Synthetic assumptions, not historical crash reconstructions or empirical fill probabilities.",
        "Intervals cover latency-seed Monte Carlo dispersion only, conditional on one generated market; five seeds give imprecise intervals.",
        "Cancel-delay uses sigma=0 and is deterministic despite the log_normal tag; no seed replicates.",
        "Queue position and fills are modeled; actual counterfactual execution is unobservable. No market impact, out-of-sample or annualised claims."
    ]);
    let baseline = global_baseline
        .as_ref()
        .expect("every study has a first regime");
    // Declared by the generating environment (as for `serve`); not verified against the tree.
    let source_commit = std::env::var("BB_GIT_SHA")
        .ok()
        .filter(|s| (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| s.to_ascii_lowercase());
    let public = json!({
        "schema": PUBLIC_SCHEMA,
        "schema_version": PUBLIC_SCHEMA_VERSION,
        "study_id": if quick { "exec-validation-quick" } else { "exec-validation" },
        "title": "Execution model validation",
        "data_class": "synthetic",
        "generator": {
            "command": if quick { "blackbox study exec-validation --out <dir> --quick" } else { "blackbox study exec-validation --out <dir>" },
            "blackbox_version": baseline.blackbox_version,
            "simulation_revision": baseline.simulation_revision,
            "run_contract_version": baseline.contract_version,
            "engine": baseline.engine,
            "platform": baseline.platform,
            "source_commit": source_commit
        },
        "design": design,
        "baseline": {
            "preset": "maker_baseline",
            "regime": "normal_liquidity",
            "queue_model": "power_prob3",
            "latency": "constant",
            "feed_offset_ns": 0,
            "run_id": baseline.run_id,
            "config_sha256": baseline.config_sha256,
            "dataset": { "id": baseline.dataset.id, "sha256": baseline.dataset.sha256 },
            "config": baseline.config
        },
        // Arrays in the study's level order (baseline first), not maps sorted by name.
        "queue_models": queues()
            .into_iter()
            .map(|(name, config)| json!({ "name": name, "config": config }))
            .collect::<Vec<_>>(),
        "latency_models": LATENCIES
            .into_iter()
            .map(|name| json!({ "name": name, "config": latency(name), "stochastic": stochastic(name) }))
            .collect::<Vec<_>>(),
        "uncertainty": {
            "source": "seed_variation",
            "method": "percentile bootstrap of the mean over independent stochastic latency seeds, nearest rank",
            "level": 0.95,
            "replicates": BOOTSTRAP_REPLICATES,
            "seed": BOOTSTRAP_SEED,
            "applies_to_latencies": LATENCIES.into_iter().filter(|name| stochastic(name)).collect::<Vec<_>>(),
            "excludes": "market sampling uncertainty, model misspecification and calibration error"
        },
        "datasets": public_datasets,
        "cells": public_cells,
        "aggregates": aggregates,
        "sensitivity_surfaces": surfaces,
        "demo_anchors": demo_anchors(&out.join("demo-datasets"))?,
        "caveats": caveats
    });
    let public_dir = out.join("public");
    fs::create_dir_all(&public_dir)?;
    // Compact, without any line break: a checkout that rewrites line endings cannot change the
    // hashed bytes.
    let public_bytes = serde_json::to_vec(&public)?;
    let public_sha256 = hex::encode(Sha256::digest(&public_bytes));
    fs::write(public_dir.join(PUBLIC_FILE), &public_bytes)?;
    fs::write(
        public_dir.join(format!("{PUBLIC_FILE}.sha256")),
        format!("{public_sha256}  {PUBLIC_FILE}\n"),
    )?;
    let report = json!({
        "schema_version": 1,
        "study": "exec-validation",
        "quick": quick,
        "local_only": true,
        "wall_time_ms": started.elapsed().as_millis() as u64,
        "baseline_manifest": global_baseline,
        "design": design,
        "cells": cells,
        "aggregates": aggregates,
        "sensitivity_surfaces": surfaces,
        "caveats": caveats,
        "public_result": {
            "path": format!("public/{PUBLIC_FILE}"),
            "schema": PUBLIC_SCHEMA,
            "schema_version": PUBLIC_SCHEMA_VERSION,
            "sha256": public_sha256
        }
    });
    write_json(&out.join("study.json"), &report)?;
    let mut markdown = String::from(
        "# Execution model validation\n\nSynthetic simulated execution, not real fills. CIs resample only stochastic latency seeds; deterministic cells are single values. Across regimes, order alignment is invalid; first-divergence details compare within-regime baselines. See study.json and cells/*.json for complete manifests, lifecycle changes, queue estimates, fill-time quantiles and Kaplan–Meier curves.\n\n## Sensitivity tables (queue × latency, stratified by regime)\n\n| Regime | Queue | Latency | Feed offset ns | Runs | Fill rate | Net P&L |\n|---|---|---|---:|---:|---|---|\n",
    );
    for a in report["aggregates"].as_array().unwrap() {
        let format_metric = |key: &str| -> String {
            let m = &a["metrics"][key];
            if let Some(v) = m["value"].as_f64() {
                format!("{v:.6}")
            } else if let Some(v) = m["bootstrap"]["mean"].as_f64() {
                format!(
                    "{v:.6} [{:.6}, {:.6}]",
                    m["bootstrap"]["lower_95"].as_f64().unwrap(),
                    m["bootstrap"]["upper_95"].as_f64().unwrap()
                )
            } else {
                "unavailable".into()
            }
        };
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            a["regime"].as_str().unwrap(),
            a["queue_model"].as_str().unwrap(),
            a["latency"].as_str().unwrap(),
            a["feed_offset_ns"],
            a["members"].as_array().unwrap().len(),
            format_metric("fill_rate"),
            format_metric("net_pnl")
        ));
    }
    fs::write(out.join("study.md"), markdown)?;
    println!(
        "exec-validation: {} cells; {} ms; {}",
        report["cells"].as_array().unwrap().len(),
        report["wall_time_ms"],
        out.display()
    );
    println!(
        "public result: {} (sha256 {})",
        out.join("public").join(PUBLIC_FILE).display(),
        report["public_result"]["sha256"]
            .as_str()
            .unwrap_or_default()
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    #[test]
    fn matched_shape_medians_and_deterministic_cancel() {
        use bb_core::config::OrderLatencyConfig;
        match super::latency("heavy_tail") {
            OrderLatencyConfig::HeavyTail { entry, .. } => {
                let median = entry.min_ns as f64
                    * (1.0 + entry.jitter / (entry.unit * ((1.0 + entry.clip) / 2.0).powi(3)));
                assert!((median - 1_000_000.0).abs() < 1.0);
            }
            _ => panic!("expected heavy tail"),
        }
        match super::latency("jitter") {
            OrderLatencyConfig::Jitter {
                entry_ns,
                jitter_ns,
                ..
            } => assert_eq!(entry_ns + jitter_ns / 2, 1_000_000),
            _ => panic!("expected jitter"),
        }
        assert!(!super::stochastic("cancel_delay"));
        match super::latency("cancel_delay") {
            OrderLatencyConfig::LogNormal {
                entry,
                response,
                cancel: Some(cancel),
            } => {
                assert_eq!((entry.sigma, response.sigma, cancel.sigma), (0.0, 0.0, 0.0));
                assert!(cancel.median_ns > entry.median_ns);
            }
            _ => panic!("expected separate deterministic cancel leg"),
        }
    }

    #[test]
    fn quick_study_smoke() {
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path().join("study");
        let report = super::exec_validation(&out, true).unwrap();
        let cells = report["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 8);
        assert!(out.join("study.json").is_file() && out.join("study.md").is_file());
        assert!(
            cells
                .iter()
                .all(|c| c["manifest"]["config_sha256"].as_str().unwrap().len() == 64)
        );
        assert_eq!(
            cells
                .iter()
                .filter(|c| c["latency"] == "cancel_delay")
                .count(),
            1
        );
        assert_eq!(cells.iter().filter(|c| c["latency"] == "jitter").count(), 2);
        for aggregate in report["aggregates"].as_array().unwrap() {
            let measure = &aggregate["metrics"]["net_pnl"];
            if aggregate["latency"] == "jitter" {
                assert_eq!(measure["bootstrap"]["n"], 2);
            } else {
                assert!(measure.get("value").is_some());
                assert!(measure.get("bootstrap").is_none());
            }
        }
        assert!(super::exec_validation(&out, true).is_err());
    }

    /// The public result is byte-identical across repeated runs, carries its own SHA-256, holds no
    /// wall-clock facts, and its demo anchors are the stored `execution_validation` demo runs.
    #[test]
    fn public_result_is_deterministic_hashed_and_anchored() {
        use sha2::{Digest, Sha256};
        let temp = tempfile::tempdir().unwrap();
        let read = |dir: &str| {
            let out = temp.path().join(dir);
            let report = super::exec_validation(&out, true).unwrap();
            let public = out.join("public");
            let bytes = std::fs::read(public.join(super::PUBLIC_FILE)).unwrap();
            let sums =
                std::fs::read_to_string(public.join(format!("{}.sha256", super::PUBLIC_FILE)))
                    .unwrap();
            (report, bytes, sums)
        };
        let (report, first, sums) = read("a");
        let (_, second, sums_again) = read("b");
        assert!(first == second, "public result differs between runs");
        assert_eq!(sums, sums_again);
        let digest = hex::encode(Sha256::digest(&first));
        assert_eq!(sums, format!("{digest}  {}\n", super::PUBLIC_FILE));
        assert_eq!(report["public_result"]["sha256"], digest);
        let text = std::str::from_utf8(&first).unwrap();
        assert!(
            !text.contains(['\n', '\r']),
            "line endings would make the hash checkout-dependent"
        );
        for clock in ["wall_time_ms", "created_unix_ms", "\"profile\""] {
            assert!(!text.contains(clock), "{clock} in the public result");
        }
        let public: serde_json::Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(public["schema"], super::PUBLIC_SCHEMA);
        assert_eq!(public["schema_version"], super::PUBLIC_SCHEMA_VERSION);
        assert_eq!(public["study_id"], "exec-validation-quick");
        let cells = public["cells"].as_array().unwrap();
        assert_eq!(cells.len(), report["cells"].as_array().unwrap().len());
        for cell in cells {
            assert_eq!(cell["config_sha256"].as_str().unwrap().len(), 64);
            assert_eq!(cell["dataset"]["sha256"].as_str().unwrap().len(), 64);
            assert!(
                cell["config_sha256"]
                    .as_str()
                    .unwrap()
                    .starts_with(cell["run_id"].as_str().unwrap())
            );
            let a = &cell["accounting"];
            let net = a["gross_pnl"].as_f64().unwrap() - a["fees"].as_f64().unwrap();
            assert!((net - a["net_pnl"].as_f64().unwrap()).abs() < 1e-9);
        }
        // Run ids are config-hash prefixes; these are the `execution_validation` runs `demo seed`
        // stores (docs/research/EXECUTION_MODEL_VALIDATION.md), on the demo's own datasets.
        let anchors = &public["demo_anchors"];
        let ids: Vec<&str> = anchors["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["run_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "253bbc23b342f373",
                "08200b05ca35723f",
                "5a063572329e1c80",
                "bcf5c37532e25266",
                "88e0971baa6491ab",
                "69ca873c448e2dd1",
                "4d0de6620234a7fe"
            ]
        );
        let runs = anchors["runs"].as_array().unwrap();
        assert!(runs[0]["comparison"].is_null());
        assert_eq!(
            runs[1]["comparison"]["reference_run_id"],
            "253bbc23b342f373"
        );
        assert_eq!(runs[1]["comparison"]["aligned"], true);
        assert_eq!(runs[4]["comparison"]["aligned"], false);
        assert_eq!(
            runs[5]["comparison"]["reference_run_id"],
            "88e0971baa6491ab"
        );
        assert_eq!(runs[1]["latency"], "heavy_tail");
        assert_eq!(runs[5]["queue_model"], "risk_adverse");
    }
}
