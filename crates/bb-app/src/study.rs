//! Local-only, sequential execution-model study. No HTTP route or database writes.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::BufWriter,
    path::Path,
    time::Instant,
};

use anyhow::{Result, bail};
use bb_core::{
    config::{CubicLeg, LogNormalLeg, OrderLatencyConfig, QueueModelConfig, RunConfig},
    result::RunResult,
};
use bb_data::synthetic::{StressRegime, SyntheticConfig};
use bb_forensics::{CompareInput, bootstrap_mean, compare_runs};
use serde_json::{Value, json};

use crate::{datasets::DatasetCtx, pipeline, presets};

const BOOTSTRAP_SEED: u64 = 3105;
const BOOTSTRAP_REPLICATES: usize = 2000;

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
                let execution = bb_analytics::execution::compute_execution_streaming(
                    run,
                    &ctx.bbo,
                    &ctx.manifest().instrument,
                    &bb_analytics::execution::ExecutionConfig {
                        bootstrap_replicates: 0,
                        ..Default::default()
                    },
                    |_| {},
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
    let report = json!({
        "schema_version": 1,
        "study": "exec-validation",
        "quick": quick,
        "local_only": true,
        "wall_time_ms": started.elapsed().as_millis() as u64,
        "baseline_manifest": global_baseline,
        "design": {
            "generator_seed": 7,
            "duration_ns": if quick { 2_000_000_000_i64 } else { 30_000_000_000_i64 },
            "latency_seeds": seeds,
            "bootstrap_seed": BOOTSTRAP_SEED,
            "bootstrap_replicates": BOOTSTRAP_REPLICATES,
            "bootstrap_method": "percentile mean, nearest rank, resample independent stochastic run seeds",
            "matched_median_ns": 1_000_000,
            "cross_regime_alignment": "unavailable: different event hashes; comparisons use the baseline within each regime"
        },
        "cells": cells,
        "aggregates": aggregates,
        "sensitivity_surfaces": surfaces,
        "caveats": [
            "Synthetic assumptions, not historical crash reconstructions or empirical fill probabilities.",
            "Intervals cover latency-seed Monte Carlo dispersion only, conditional on one generated market; five seeds give imprecise intervals.",
            "Cancel-delay uses sigma=0 and is deterministic despite the log_normal tag; no seed replicates.",
            "Queue position and fills are modeled; actual counterfactual execution is unobservable. No market impact, out-of-sample or annualised claims."
        ]
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
}
