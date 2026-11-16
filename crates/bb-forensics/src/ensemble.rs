//! Seed ensembles (AT-12): one configuration replayed under several seeds
//! ([`SweepDesign::Seeds`]), summarised per metric as mean, sample standard deviation, minimum
//! and maximum, with every seed's run id.
//!
//! The seed drives only the stochastic order-latency models (`RunConfig::seed`). Under a latency
//! model that uses no randomness every seed replays the same simulation, so the ensemble
//! collapses to one result; the report says so explicitly instead of presenting a zero spread
//! as a finding.

use bb_core::{
    result::Metrics,
    sweep::{ObjectiveMetric, Segment, SweepDesign, SweepSpec},
};
use serde::{Deserialize, Serialize};

use crate::sweep::Caveat;

/// Version of the ensemble rules. Bump when the same inputs would give a different report.
pub const ENSEMBLE_ANALYSIS_VERSION: u32 = 1;

/// What one run of an ensemble member produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnsembleRunOutcome {
    /// Cell index in [`SweepSpec::cells`] order (the seed's position).
    pub cell: u32,
    pub segment: Segment,
    pub run_id: Option<String>,
    /// `None` when the run did not complete.
    pub metrics: Option<Metrics>,
    /// Why the run failed, if it did.
    pub error: Option<String>,
}

/// One seed of a segment and the run it produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnsembleMember {
    pub cell: u32,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub seed: u64,
    /// The run's 16-hex id; `None` until it completed.
    pub run_id: Option<String>,
    pub error: Option<String>,
}

/// Spread of one metric over the completed seeds of a segment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MetricSpread {
    /// [`ObjectiveMetric::key`], e.g. `net_pnl`.
    pub metric: String,
    /// [`ObjectiveMetric::unit`].
    pub unit: String,
    /// Seeds with a defined value.
    pub n: u32,
    pub mean: f64,
    /// Sample standard deviation (n − 1); `None` below two values.
    pub std_dev: Option<f64>,
    pub min: f64,
    pub max: f64,
}

/// The ensemble on one part of the data: the whole dataset, in sample or out of sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnsembleSegment {
    pub segment: Segment,
    /// Seeds whose run completed.
    pub completed: u32,
    /// Every seed, in the spec's order.
    pub members: Vec<EnsembleMember>,
    /// Every [`ObjectiveMetric`] defined for at least one completed run.
    pub metrics: Vec<MetricSpread>,
    /// At least two runs completed and all of them have exactly the same metrics.
    pub identical: bool,
}

/// The report of one seed ensemble.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnsembleReport {
    pub analysis_version: u32,
    /// [`SweepSpec::fingerprint`] of the analysed spec.
    pub fingerprint: String,
    pub seed_count: u32,
    /// Whether the configuration's order-latency model uses the seed at all. `false` (constant
    /// latency, or jitter of zero) means every seed replays the same simulation.
    pub seed_affects_result: bool,
    /// Every seed produced the same result: by construction when `seed_affects_result` is
    /// `false`, or observed (every segment `identical`).
    pub collapsed: bool,
    /// The explicit statement of a collapse; `None` when the seeds differ.
    pub collapse_reason: Option<String>,
    pub segments: Vec<EnsembleSegment>,
    pub caveats: Vec<Caveat>,
}

/// Percentile bootstrap CI for a mean across independent stochastic run seeds only.
/// Callers must not pass deterministic seed duplicates or pooled market regimes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BootstrapMean {
    pub n: usize,
    pub mean: f64,
    pub lower_95: f64,
    pub upper_95: f64,
    pub replicates: usize,
    pub seed: u64,
}

pub fn bootstrap_mean(values: &[f64], replicates: usize, seed: u64) -> Option<BootstrapMean> {
    if values.len() < 2 || replicates < 40 || values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mut rng = bb_core::rng::Rng::for_stream(seed, "ensemble.bootstrap.mean");
    let mut means = Vec::with_capacity(replicates);
    for _ in 0..replicates {
        let mut sum = 0.0;
        for _ in values {
            sum += values[rng.below(values.len() as u64) as usize];
        }
        means.push(sum / values.len() as f64);
    }
    means.sort_by(f64::total_cmp);
    Some(BootstrapMean {
        n: values.len(),
        mean: values.iter().sum::<f64>() / values.len() as f64,
        lower_95: means[(replicates as f64 * 0.025).ceil() as usize - 1],
        upper_95: means[(replicates as f64 * 0.975).ceil() as usize - 1],
        replicates,
        seed,
    })
}

fn spread(metric: ObjectiveMetric, values: &[f64]) -> Option<MetricSpread> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // Identical values have that value as mean and exactly zero spread (summing them first
    // would leave rounding residue, which would read as dispersion).
    let mean = if min == max {
        min
    } else {
        values.iter().sum::<f64>() / n as f64
    };
    let std_dev = (n >= 2).then(|| {
        let ss: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();
        (ss / (n - 1) as f64).sqrt()
    });
    Some(MetricSpread {
        metric: metric.key().into(),
        unit: metric.unit().into(),
        n: n as u32,
        mean,
        std_dev,
        min,
        max,
    })
}

/// The ensemble report of `outcomes` (at most one per seed and segment of `spec`, which must be a
/// [`SweepDesign::Seeds`] design). Seeds without an outcome count as not run. Fails on malformed
/// input: another design, an unknown or repeated (cell, segment), or a segment the spec's
/// partition does not have.
pub fn analyze_ensemble(
    spec: &SweepSpec,
    outcomes: &[EnsembleRunOutcome],
) -> Result<EnsembleReport, String> {
    if !matches!(spec.design, SweepDesign::Seeds { .. }) {
        return Err("an ensemble report needs a seeds design".into());
    }
    let seeds = spec.ensemble_seeds().unwrap_or_default();
    let segments: Vec<Segment> = match spec.partition {
        None => vec![Segment::Full],
        Some(_) => vec![Segment::InSample, Segment::OutOfSample],
    };
    let mut slots: Vec<Vec<Option<&EnsembleRunOutcome>>> =
        vec![vec![None; seeds.len()]; segments.len()];
    for o in outcomes {
        let s = segments
            .iter()
            .position(|s| *s == o.segment)
            .ok_or_else(|| {
                format!(
                    "cell {}: segment {:?} is not part of the spec",
                    o.cell, o.segment
                )
            })?;
        let slot = slots[s].get_mut(o.cell as usize).ok_or_else(|| {
            format!(
                "outcome for cell {} of a {}-seed ensemble",
                o.cell,
                seeds.len()
            )
        })?;
        if slot.replace(o).is_some() {
            return Err(format!(
                "two outcomes for cell {} ({:?})",
                o.cell, o.segment
            ));
        }
    }

    let seed_affects_result = spec.base.execution.order_latency.uses_seed();
    let mut out_segments = Vec::with_capacity(segments.len());
    for (segment, slot) in segments.iter().zip(&slots) {
        let members = seeds
            .iter()
            .enumerate()
            .map(|(cell, seed)| EnsembleMember {
                cell: cell as u32,
                seed: *seed,
                run_id: slot[cell].and_then(|o| o.run_id.clone()),
                error: slot[cell].and_then(|o| o.error.clone()),
            })
            .collect();
        let done: Vec<&Metrics> = slot
            .iter()
            .flatten()
            .filter_map(|o| o.metrics.as_ref())
            .collect();
        let metrics = ObjectiveMetric::ALL
            .iter()
            .filter_map(|m| {
                let values: Vec<f64> = done.iter().filter_map(|r| m.value(r)).collect();
                spread(*m, &values)
            })
            .collect();
        out_segments.push(EnsembleSegment {
            segment: *segment,
            completed: done.len() as u32,
            members,
            metrics,
            identical: done.len() >= 2 && done.iter().all(|m| *m == done[0]),
        });
    }

    let n = seeds.len();
    let observed = out_segments.iter().all(|s| s.identical);
    let collapse_reason = if !seed_affects_result {
        Some(format!(
            "The order-latency model of this configuration uses no randomness (constant latency, \
             or jitter of zero), and the seed drives nothing else: all {n} seeds replay the same \
             simulation and collapse to one result. The spread below is zero by construction, \
             not evidence of robustness. Use a jitter, log_normal or heavy_tail latency model to \
             measure seed dispersion."
        ))
    } else if observed {
        Some(format!(
            "All {n} seeds produced identical metrics: on this data the seeded latency draws \
             changed no order outcome, so the ensemble collapses to one result."
        ))
    } else {
        None
    };

    let mut caveats = Vec::new();
    let mut push = |code: &str, text: String| {
        caveats.push(Caveat {
            code: code.into(),
            text,
        })
    };
    let incomplete: usize = out_segments.iter().map(|s| n - s.completed as usize).sum();
    if incomplete > 0 {
        push(
            "incomplete_seeds",
            format!(
                "{incomplete} of {} runs did not complete; the statistics cover the completed \
                 seeds only.",
                n * out_segments.len()
            ),
        );
    }
    push(
        "seed_dispersion_only",
        "The spread measures only the seeded latency draws over one replay of one historical \
         period under modeled queue and latency assumptions. It is not a confidence interval for \
         live performance and says nothing about other market regimes."
            .into(),
    );
    if spec.partition.is_some() {
        push(
            "segments_not_pooled",
            "In-sample and out-of-sample runs are summarised separately: their windows differ in \
             length and market conditions, so pooling them would mix two experiments."
                .into(),
        );
    }
    Ok(EnsembleReport {
        analysis_version: ENSEMBLE_ANALYSIS_VERSION,
        fingerprint: spec.fingerprint(),
        seed_count: n as u32,
        seed_affects_result,
        collapsed: !seed_affects_result || observed,
        collapse_reason,
        segments: out_segments,
        caveats,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn bootstrap_fixtures() {
        assert!(super::bootstrap_mean(&[1.0], 1000, 31).is_none());
        assert!(super::bootstrap_mean(&[1.0, f64::NAN], 1000, 31).is_none());
        let equal = super::bootstrap_mean(&[3.0; 5], 1000, 31).unwrap();
        assert_eq!(
            (equal.mean, equal.lower_95, equal.upper_95),
            (3.0, 3.0, 3.0)
        );
        let pair = super::bootstrap_mean(&[0.0, 2.0], 1000, 31).unwrap();
        assert_eq!((pair.mean, pair.lower_95, pair.upper_95), (1.0, 0.0, 2.0));
        assert_eq!(Some(pair), super::bootstrap_mean(&[0.0, 2.0], 1000, 31));
    }

    use bb_core::{
        config::OrderLatencyConfig,
        sweep::{Partition, TimeWindow},
    };

    use super::*;
    use crate::{compute_metrics, testkit};

    fn spec(order_latency: OrderLatencyConfig, partition: Option<Partition>) -> SweepSpec {
        let mut base = testkit::config();
        base.execution.order_latency = order_latency;
        SweepSpec {
            base,
            design: SweepDesign::Seeds {
                seeds: vec![],
                count: Some(3),
                base_seed: Some(10),
            },
            partition,
            objective: Default::default(),
            cscv_blocks: None,
        }
    }

    fn metrics(net_pnl: f64) -> Metrics {
        let mut m = compute_metrics(&[], &[], &[], &[], &testkit::INSTRUMENT);
        m.net_pnl = net_pnl;
        m
    }

    fn outcome(cell: u32, segment: Segment, net_pnl: Option<f64>) -> EnsembleRunOutcome {
        EnsembleRunOutcome {
            cell,
            segment,
            run_id: net_pnl.map(|_| format!("{cell:016x}")),
            metrics: net_pnl.map(metrics),
            error: None,
        }
    }

    const JITTER: OrderLatencyConfig = OrderLatencyConfig::Jitter {
        entry_ns: 1,
        response_ns: 1,
        jitter_ns: 5,
    };

    #[test]
    fn spreads_every_metric_and_lists_every_seed() {
        let s = spec(JITTER, None);
        let r = analyze_ensemble(
            &s,
            &[
                outcome(0, Segment::Full, Some(1.0)),
                outcome(1, Segment::Full, Some(2.0)),
                outcome(2, Segment::Full, Some(6.0)),
            ],
        )
        .unwrap();
        assert_eq!(r.seed_count, 3);
        assert!(r.seed_affects_result && !r.collapsed && r.collapse_reason.is_none());
        let seg = &r.segments[0];
        assert_eq!(seg.completed, 3);
        assert_eq!(
            seg.members.iter().map(|m| m.seed).collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        assert_eq!(seg.members[2].run_id.as_deref(), Some("0000000000000002"));
        let pnl = seg.metrics.iter().find(|m| m.metric == "net_pnl").unwrap();
        assert_eq!((pnl.n, pnl.mean, pnl.min, pnl.max), (3, 3.0, 1.0, 6.0));
        assert!((pnl.std_dev.unwrap() - 7f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn constant_latency_collapses_explicitly() {
        let s = spec(
            OrderLatencyConfig::Constant {
                entry_ns: 1,
                response_ns: 1,
            },
            None,
        );
        let r = analyze_ensemble(
            &s,
            &[
                outcome(0, Segment::Full, Some(2.0)),
                outcome(1, Segment::Full, Some(2.0)),
                outcome(2, Segment::Full, None),
            ],
        )
        .unwrap();
        assert!(!r.seed_affects_result && r.collapsed);
        assert!(
            r.collapse_reason
                .unwrap()
                .contains("collapse to one result")
        );
        assert!(r.segments[0].identical);
        assert!(r.caveats.iter().any(|c| c.code == "incomplete_seeds"));
    }

    #[test]
    fn observed_identical_results_collapse_and_partitions_report_both_segments() {
        let w = |a, b| TimeWindow { start: a, end: b };
        let s = spec(
            JITTER,
            Some(Partition {
                in_sample: w(0, 10),
                out_of_sample: w(10, 20),
            }),
        );
        let mut all = Vec::new();
        for cell in 0..3 {
            all.push(outcome(cell, Segment::InSample, Some(1.0)));
            all.push(outcome(cell, Segment::OutOfSample, Some(-1.0)));
        }
        let r = analyze_ensemble(&s, &all).unwrap();
        assert_eq!(r.segments.len(), 2);
        assert!(r.collapsed && r.collapse_reason.unwrap().contains("identical"));
        assert!(analyze_ensemble(&s, &[outcome(0, Segment::Full, Some(1.0))]).is_err());
        assert!(analyze_ensemble(&s, &[outcome(5, Segment::InSample, Some(1.0))]).is_err());
    }
}
