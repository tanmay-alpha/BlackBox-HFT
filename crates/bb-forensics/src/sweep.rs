//! The registry report of a finished parameter sweep: how many configurations were tried, which
//! one the declared objective selected in sample, how it fared out of sample, and how much of
//! the apparent edge selection alone could explain.
//!
//! Definitions and sources (all in `docs/research/VALIDATION.md`, "Sweeps and overfitting"):
//!
//! * Spearman rank correlation with average ranks for ties.
//! * Expected maximum of `N` independent standard normals,
//!   `E[max_N] ≈ (1 − γ)·Φ⁻¹(1 − 1/N) + γ·Φ⁻¹(1 − 1/(N·e))` (Bailey, Borwein, López de Prado &
//!   Zhu 2014, "Pseudo-mathematics and financial charlatanism", Notices of the AMS 61(5);
//!   Proposition 2.1, eq. 2.4 of the April 2014 preprint), as a yardstick for how far the best of
//!   `N` pure-noise trials stands out.
//! * Probability of Backtest Overfitting by combinatorially symmetric cross-validation (CSCV;
//!   Bailey, Borwein, López de Prado & Zhu 2017, "The probability of backtest overfitting",
//!   Journal of Computational Finance 20(4)).
//!
//! No annualised Sharpe ratio is ever computed (`docs/research/MARKET_MICROSTRUCTURE.md` §5).
//! White's reality check and Hansen's SPA test are not implemented; the report gives counts,
//! ranks and the CSCV estimate instead of a p-value.

use bb_core::{
    result::EquitySample,
    sweep::{Direction, MAX_CSCV_BLOCKS, Objective, ObjectiveMetric, SweepSpec, TimeWindow},
};
use serde::{Deserialize, Serialize};

/// Version of the analysis rules. Bump when the same inputs would give a different report.
pub const SWEEP_ANALYSIS_VERSION: u32 = 1;
/// Fewest cells with both an in-sample and an out-of-sample value for a rank correlation.
pub const MIN_SPEARMAN_CELLS: usize = 3;
/// Fewest cells for CSCV. With fewer, the relative rank takes so few values that the estimate
/// says little.
pub const MIN_PBO_CELLS: usize = 4;
/// From this many trials on, the selection-bias caveat quotes the expected maximum under noise.
pub const MANY_TRIALS: u32 = 10;

/// What one cell produced. Values are the spec's objective metric
/// ([`ObjectiveMetric::value`]) of the cell's runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepCellOutcome {
    pub cell: u32,
    /// On the selection data: the in-sample window, or the whole dataset without a partition.
    /// `None` when the run did not complete or the metric is undefined for it.
    pub in_sample: Option<f64>,
    /// `None` without a partition, or when the out-of-sample run did not complete.
    #[serde(default)]
    pub out_of_sample: Option<f64>,
    /// The objective's contribution in each of the spec's `cscv_blocks` blocks of the selection
    /// window, in time order ([`objective_blocks`]).
    #[serde(default)]
    pub blocks: Option<Vec<f64>>,
    /// Why a run of the cell failed, if one did.
    #[serde(default)]
    pub error: Option<String>,
}

/// A statistic, or the reason it is not reported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Availability<T> {
    Available(T),
    Unavailable { reason: String },
}

impl<T> Availability<T> {
    fn unavailable(reason: impl Into<String>) -> Self {
        Availability::Unavailable {
            reason: reason.into(),
        }
    }

    pub fn available(&self) -> Option<&T> {
        match self {
            Availability::Available(v) => Some(v),
            Availability::Unavailable { .. } => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfigurationCounts {
    /// Every configuration the spec declares. All of them count as trials for selection bias,
    /// whether they completed, failed or never ran.
    pub tried: u32,
    pub completed_in_sample: u32,
    pub completed_out_of_sample: u32,
    /// Cells with an error.
    pub failed: u32,
    /// Cells with neither an in-sample value nor an error (not run, cancelled, or the objective
    /// undefined for them).
    pub missing: u32,
}

/// The cell the objective selected in sample, and what happened to it out of sample.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SelectedCell {
    pub cell: u32,
    pub in_sample: f64,
    /// Other cells with exactly the same in-sample value; the lowest index is selected.
    pub tied_with: u32,
    pub out_of_sample: Option<f64>,
    /// 1 is best, in the objective's direction, among the `out_of_sample_of` cells with an
    /// out-of-sample value: one plus the number of cells strictly better.
    pub out_of_sample_rank: Option<u32>,
    pub out_of_sample_of: u32,
    /// In-sample minus out-of-sample in the objective's direction: positive means the selected
    /// cell did worse out of sample.
    pub degradation: Option<f64>,
    /// `degradation / |in_sample|`; `None` when the in-sample value is zero.
    pub relative_degradation: Option<f64>,
    /// Time-additive objectives only: each value divided by its window's length in seconds, so
    /// windows of different length compare.
    pub in_sample_per_second: Option<f64>,
    pub out_of_sample_per_second: Option<f64>,
    pub degradation_per_second: Option<f64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RankCorrelation {
    /// Spearman's rho between in-sample and out-of-sample values (average ranks for ties).
    pub spearman_rho: f64,
    /// Cells with both values.
    pub cells: u32,
}

/// Spread of one set of cell values (raw metric values, not oriented).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Dispersion {
    pub n: u32,
    pub min: f64,
    /// Quantiles by linear interpolation between order statistics (Hyndman & Fan type 7).
    pub p25: f64,
    pub median: f64,
    pub p75: f64,
    pub max: f64,
    pub mean: f64,
    /// Sample standard deviation (n − 1); `None` for one value.
    pub std_dev: Option<f64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DispersionReport {
    pub in_sample: Option<Dispersion>,
    pub out_of_sample: Option<Dispersion>,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SelectionBias {
    /// Number of trials: every declared configuration.
    pub trials: u32,
    /// How many in-sample standard deviations the selected value lies above the in-sample mean
    /// of the completed cells, in the objective's direction.
    pub selected_z: Option<f64>,
    /// Approximate expected maximum of `trials` independent standard normals (Bailey et al.
    /// 2014): what `selected_z` would be on average if every cell were noise. `None` below two
    /// trials.
    pub expected_max_z_under_noise: Option<f64>,
}

/// Probability of Backtest Overfitting by CSCV.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Pbo {
    /// Fraction of the train/test splits in which the in-sample best ranks at or below the
    /// out-of-sample median (logit ≤ 0).
    pub pbo: f64,
    /// `S`: equal time blocks of the selection window.
    pub blocks: u32,
    /// `C(S, S/2)` train/test splits evaluated.
    pub combinations: u32,
    /// `N`: configurations in the matrix (all declared cells).
    pub cells: u32,
    pub logits_at_or_below_zero: u32,
    pub median_logit: f64,
}

/// A plain-language warning that belongs next to the numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Caveat {
    /// Stable key: `selection_bias`, `incomplete_cells`, `window_lengths`, `few_cells`,
    /// `not_a_significance_test`, `cscv_assumptions`, `no_annualised_sharpe`.
    pub code: String,
    pub text: String,
}

/// The registry report of one sweep.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepReport {
    pub analysis_version: u32,
    /// [`SweepSpec::fingerprint`] of the analysed spec.
    pub fingerprint: String,
    pub objective: Objective,
    /// [`ObjectiveMetric::unit`] of the objective.
    pub unit: String,
    pub has_out_of_sample: bool,
    pub configurations: ConfigurationCounts,
    /// `None` when no cell has an in-sample value.
    pub selected: Option<SelectedCell>,
    pub rank_correlation: Availability<RankCorrelation>,
    pub dispersion: DispersionReport,
    pub selection_bias: SelectionBias,
    pub pbo: Availability<Pbo>,
    pub caveats: Vec<Caveat>,
}

/// Per-block contributions of a time-additive objective over `window`, from a run's equity
/// samples: block `i` of `window.blocks(blocks)` gets the change of the metric's running total
/// ([`ObjectiveMetric::cumulative`]) from the last sample before the block starts to the last
/// sample before it ends. The last block runs to the final (closing) sample, and the running
/// total before the first sample is zero, so the blocks sum to the run's final total.
pub fn objective_blocks(
    metric: ObjectiveMetric,
    equity: &[EquitySample],
    window: TimeWindow,
    blocks: u32,
) -> Result<Vec<f64>, String> {
    if !metric.is_time_additive() {
        return Err(format!(
            "{} is not a running total and has no per-block contributions",
            metric.key()
        ));
    }
    if blocks == 0 || equity.is_empty() {
        return Err("per-block values need at least one block and one equity sample".into());
    }
    // Running total of the last sample strictly before `t` (samples are in time order).
    let total_before = |t: i64| {
        let n = equity.partition_point(|s| s.ts < t);
        n.checked_sub(1)
            .and_then(|i| metric.cumulative(&equity[i]))
            .unwrap_or(0.0)
    };
    let last = metric.cumulative(&equity[equity.len() - 1]).unwrap_or(0.0);
    let bounds = window.blocks(blocks);
    Ok(bounds
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let end = if i + 1 == bounds.len() {
                last
            } else {
                total_before(b.end)
            };
            end - total_before(b.start)
        })
        .collect())
}

/// The registry report of `outcomes` (at most one per cell of `spec`). Cells without an outcome
/// count as missing. Fails on malformed input: an unknown or repeated cell, a non-finite value,
/// out-of-sample values without a partition, or a block vector of the wrong length.
pub fn analyze_sweep(
    spec: &SweepSpec,
    outcomes: &[SweepCellOutcome],
) -> Result<SweepReport, String> {
    let tried = spec.cell_count();
    let tried = u32::try_from(tried).map_err(|_| format!("{tried} cells is not a sweep"))?;
    let objective = spec.objective;
    let has_oos = spec.partition.is_some();
    let mut by_cell: Vec<Option<&SweepCellOutcome>> = vec![None; tried as usize];
    for o in outcomes {
        let slot = by_cell
            .get_mut(o.cell as usize)
            .ok_or_else(|| format!("outcome for cell {} of a {tried}-cell sweep", o.cell))?;
        if slot.replace(o).is_some() {
            return Err(format!("two outcomes for cell {}", o.cell));
        }
        let finite = |v: Option<f64>| v.is_none_or(f64::is_finite);
        if !finite(o.in_sample) || !finite(o.out_of_sample) {
            return Err(format!("cell {}: values must be finite", o.cell));
        }
        if o.out_of_sample.is_some() && !has_oos {
            return Err(format!(
                "cell {}: an out-of-sample value without a partition in the spec",
                o.cell
            ));
        }
        if let Some(b) = &o.blocks {
            if Some(b.len() as u32) != spec.cscv_blocks {
                return Err(format!(
                    "cell {}: {} block values, the spec declares {:?}",
                    o.cell,
                    b.len(),
                    spec.cscv_blocks
                ));
            }
            if !b.iter().all(|v| v.is_finite()) {
                return Err(format!("cell {}: block values must be finite", o.cell));
            }
        }
    }

    let is_values: Vec<(u32, f64)> = by_cell
        .iter()
        .flatten()
        .filter_map(|o| o.in_sample.map(|v| (o.cell, v)))
        .collect();
    let oos_values: Vec<(u32, f64)> = by_cell
        .iter()
        .flatten()
        .filter_map(|o| o.out_of_sample.map(|v| (o.cell, v)))
        .collect();
    let failed = by_cell
        .iter()
        .flatten()
        .filter(|o| o.error.is_some())
        .count() as u32;
    let missing = by_cell
        .iter()
        .filter(|o| o.is_none_or(|o| o.in_sample.is_none() && o.error.is_none()))
        .count() as u32;
    let configurations = ConfigurationCounts {
        tried,
        completed_in_sample: is_values.len() as u32,
        completed_out_of_sample: oos_values.len() as u32,
        failed,
        missing,
    };

    let selected = select(&objective, spec, &is_values, &oos_values);
    let rank_correlation = rank_correlation(has_oos, &by_cell);
    let raw = |v: &[(u32, f64)]| v.iter().map(|(_, x)| *x).collect::<Vec<f64>>();
    let dispersion = DispersionReport {
        in_sample: dispersion(&raw(&is_values)),
        out_of_sample: dispersion(&raw(&oos_values)),
    };
    let oriented_is: Vec<f64> = is_values
        .iter()
        .map(|(_, v)| objective.oriented(*v))
        .collect();
    let selected_z = selected.and_then(|s| {
        let d = dispersion_of_oriented(&oriented_is)?;
        (d.1 > 0.0).then(|| (objective.oriented(s.in_sample) - d.0) / d.1)
    });
    let selection_bias = SelectionBias {
        trials: tried,
        selected_z,
        expected_max_z_under_noise: expected_max_of_normals(tried),
    };
    let pbo = pbo(spec, &by_cell);
    let caveats = caveats(
        spec,
        &configurations,
        selected.as_ref(),
        &selection_bias,
        &pbo,
    );
    Ok(SweepReport {
        analysis_version: SWEEP_ANALYSIS_VERSION,
        fingerprint: spec.fingerprint(),
        objective,
        unit: objective.metric.unit().to_string(),
        has_out_of_sample: has_oos,
        configurations,
        selected,
        rank_correlation,
        dispersion,
        selection_bias,
        pbo,
        caveats,
    })
}

fn select(
    objective: &Objective,
    spec: &SweepSpec,
    is_values: &[(u32, f64)],
    oos_values: &[(u32, f64)],
) -> Option<SelectedCell> {
    // Strictly greater keeps the lowest index on ties (values are in cell order).
    let (cell, is) = is_values
        .iter()
        .copied()
        .fold(None, |best, (c, v)| match best {
            Some((_, b)) if objective.oriented(v) <= objective.oriented(b) => best,
            _ => Some((c, v)),
        })?;
    let tied_with = is_values
        .iter()
        .filter(|(c, v)| *c != cell && *v == is)
        .count() as u32;
    let oos = oos_values.iter().find(|(c, _)| *c == cell).map(|(_, v)| *v);
    let out_of_sample_rank = oos.map(|x| {
        1 + oos_values
            .iter()
            .filter(|(_, v)| objective.oriented(*v) > objective.oriented(x))
            .count() as u32
    });
    let degradation = oos.map(|x| objective.oriented(is) - objective.oriented(x));
    let per_second = |v: f64, w: TimeWindow| v / (w.len_ns() as f64 / 1e9);
    let windows = spec
        .partition
        .filter(|_| objective.metric.is_time_additive());
    let in_sample_per_second = windows.map(|p| per_second(is, p.in_sample));
    let out_of_sample_per_second = windows
        .zip(oos)
        .map(|(p, x)| per_second(x, p.out_of_sample));
    Some(SelectedCell {
        cell,
        in_sample: is,
        tied_with,
        out_of_sample: oos,
        out_of_sample_rank,
        out_of_sample_of: oos_values.len() as u32,
        degradation,
        relative_degradation: degradation.filter(|_| is != 0.0).map(|d| d / is.abs()),
        in_sample_per_second,
        out_of_sample_per_second,
        degradation_per_second: in_sample_per_second
            .zip(out_of_sample_per_second)
            .map(|(a, b)| objective.oriented(a) - objective.oriented(b)),
    })
}

/// Average ranks from 1 (smallest); tied values share the mean of their positions.
fn average_ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut ranks = vec![0.0; values.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        // Positions i..=j (0-based) are ranks i+1..=j+1; their mean is (i + j)/2 + 1.
        let rank = (i + j) as f64 / 2.0 + 1.0;
        for &k in &order[i..=j] {
            ranks[k] = rank;
        }
        i = j + 1;
    }
    ranks
}

/// Spearman's rho: Pearson's correlation of the average ranks. `None` when either side has no
/// variance in rank (all values tied).
pub fn spearman(x: &[f64], y: &[f64]) -> Option<f64> {
    if x.len() != y.len() || x.len() < 2 {
        return None;
    }
    let (rx, ry) = (average_ranks(x), average_ranks(y));
    let mean = (x.len() as f64 + 1.0) / 2.0;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in rx.iter().zip(&ry) {
        let (dx, dy) = (a - mean, b - mean);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    (sxx > 0.0 && syy > 0.0).then(|| sxy / (sxx * syy).sqrt())
}

fn rank_correlation(
    has_oos: bool,
    by_cell: &[Option<&SweepCellOutcome>],
) -> Availability<RankCorrelation> {
    if !has_oos {
        return Availability::unavailable(
            "no out-of-sample window was declared, so there is nothing to correlate the \
             in-sample ranking with",
        );
    }
    let (x, y): (Vec<f64>, Vec<f64>) = by_cell
        .iter()
        .flatten()
        .filter_map(|o| o.in_sample.zip(o.out_of_sample))
        .unzip();
    if x.len() < MIN_SPEARMAN_CELLS {
        return Availability::unavailable(format!(
            "{} cells have both an in-sample and an out-of-sample value; at least \
             {MIN_SPEARMAN_CELLS} are needed",
            x.len()
        ));
    }
    match spearman(&x, &y) {
        Some(rho) => Availability::Available(RankCorrelation {
            spearman_rho: rho,
            cells: x.len() as u32,
        }),
        None => Availability::unavailable(
            "every cell has the same in-sample value or the same out-of-sample value, so there \
             is no ranking to correlate",
        ),
    }
}

/// Hyndman & Fan type 7 quantile of sorted values.
fn quantile(sorted: &[f64], p: f64) -> f64 {
    let h = (sorted.len() - 1) as f64 * p;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

/// `(mean, sample standard deviation)`; `None` below two values.
fn dispersion_of_oriented(values: &[f64]) -> Option<(f64, f64)> {
    let d = dispersion(values)?;
    Some((d.mean, d.std_dev?))
}

fn dispersion(values: &[f64]) -> Option<Dispersion> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let mean = sorted.iter().sum::<f64>() / n as f64;
    let std_dev = (n >= 2).then(|| {
        let ss: f64 = sorted.iter().map(|v| (v - mean) * (v - mean)).sum();
        (ss / (n - 1) as f64).sqrt()
    });
    Some(Dispersion {
        n: n as u32,
        min: sorted[0],
        p25: quantile(&sorted, 0.25),
        median: quantile(&sorted, 0.5),
        p75: quantile(&sorted, 0.75),
        max: sorted[n - 1],
        mean,
        std_dev,
    })
}

/// Inverse of the standard normal distribution function, by Acklam's rational approximation
/// (relative error below 1.15e-9). `p` in `(0, 1)`.
pub fn inverse_normal_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const P_LOW: f64 = 0.02425;
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < P_LOW {
        tail((-2.0 * p.ln()).sqrt())
    } else if p <= 1.0 - P_LOW {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    }
}

/// `E[max of n iid N(0,1)] ≈ (1 − γ)·Φ⁻¹(1 − 1/n) + γ·Φ⁻¹(1 − 1/(n·e))` (Bailey et al. 2014,
/// Proposition 2.1, eq. 2.4). Asymptotic: 1.575 against the exact 1.539 at n = 10 (the paper
/// quotes 1.57 for n = 10).
pub fn expected_max_of_normals(n: u32) -> Option<f64> {
    (n >= 2).then(|| {
        let n = f64::from(n);
        let gamma = std::f64::consts::EULER_GAMMA;
        (1.0 - gamma) * inverse_normal_cdf(1.0 - 1.0 / n)
            + gamma * inverse_normal_cdf(1.0 - 1.0 / (n * std::f64::consts::E))
    })
}

/// CSCV (Bailey, Borwein, López de Prado & Zhu 2017, §2): `m[n][b]` is cell `n`'s oriented
/// performance in block `b` (larger is better) of `S` blocks. For every way of choosing `S/2`
/// blocks as the training set: select the cell with the largest training sum (lowest index on
/// ties), find its relative rank `ω = r / (N + 1)` among the testing sums of all cells (`r` the
/// average rank from 1 = worst), and count `λ = ln(ω / (1 − ω)) ≤ 0`. PBO is that count over the
/// number of splits: the paper's `∫_{-∞}^0 f(λ) dλ` (§3.1), which counts a selected cell exactly at
/// the median as overfit. (Its Definition 2.2 writes a strict `r̄ < N/2`; under pure noise and
/// even `N` the `λ ≤ 0` form has expectation exactly 1/2.) The paper leaves ties unspecified; the
/// tie rules above are this implementation's. `None` unless `S` is even, `2 ≤ S ≤`
/// [`MAX_CSCV_BLOCKS`], `N ≥ 2` and every row has `S` values.
pub fn cscv_pbo(m: &[Vec<f64>]) -> Option<Pbo> {
    let n = m.len();
    let s = m.first()?.len();
    let too_many = s > MAX_CSCV_BLOCKS as usize;
    if n < 2 || s < 2 || s % 2 != 0 || too_many || m.iter().any(|row| row.len() != s) {
        return None;
    }
    let mut logits = Vec::new();
    let mut at_or_below = 0u32;
    let mut train = vec![0.0; n];
    let mut test = vec![0.0; n];
    for mask in 0u32..(1u32 << s) {
        if mask.count_ones() as usize != s / 2 {
            continue;
        }
        for (k, row) in m.iter().enumerate() {
            let (mut a, mut b) = (0.0, 0.0);
            for (j, v) in row.iter().enumerate() {
                if mask & (1 << j) != 0 {
                    a += v;
                } else {
                    b += v;
                }
            }
            train[k] = a;
            test[k] = b;
        }
        let mut best = 0;
        for k in 1..n {
            if train[k] > train[best] {
                best = k;
            }
        }
        let x = test[best];
        let below = test.iter().filter(|v| **v < x).count();
        let equal = test.iter().filter(|v| **v == x).count();
        // Average rank r = below + (equal + 1)/2; λ ≤ 0 ⇔ r ≤ (N + 1)/2 ⇔ 2·below + equal ≤ N,
        // decided in integers.
        if 2 * below + equal <= n {
            at_or_below += 1;
        }
        let rank = below as f64 + (equal as f64 + 1.0) / 2.0;
        let omega = rank / (n as f64 + 1.0);
        logits.push((omega / (1.0 - omega)).ln());
    }
    let combinations = logits.len() as u32;
    logits.sort_by(f64::total_cmp);
    Some(Pbo {
        pbo: f64::from(at_or_below) / f64::from(combinations),
        blocks: s as u32,
        combinations,
        cells: n as u32,
        logits_at_or_below_zero: at_or_below,
        median_logit: quantile(&logits, 0.5),
    })
}

fn pbo(spec: &SweepSpec, by_cell: &[Option<&SweepCellOutcome>]) -> Availability<Pbo> {
    let Some(s) = spec.cscv_blocks else {
        return Availability::unavailable("the spec does not declare cscv_blocks");
    };
    if by_cell.len() < MIN_PBO_CELLS {
        return Availability::unavailable(format!(
            "CSCV needs at least {MIN_PBO_CELLS} configurations; the sweep declares {}",
            by_cell.len()
        ));
    }
    let rows: Vec<Vec<f64>> = by_cell
        .iter()
        .filter_map(|o| o.and_then(|o| o.blocks.as_ref()))
        .map(|b| b.iter().map(|v| spec.objective.oriented(*v)).collect())
        .collect();
    if rows.len() < by_cell.len() {
        return Availability::unavailable(format!(
            "{} of {} configurations have no per-block values (failed or not run); CSCV needs \
             the complete matrix, and dropping cells would hide trials",
            by_cell.len() - rows.len(),
            by_cell.len()
        ));
    }
    match cscv_pbo(&rows) {
        Some(p) => Availability::Available(p),
        None => Availability::unavailable(format!("{s} blocks cannot be split into halves")),
    }
}

fn caveats(
    spec: &SweepSpec,
    counts: &ConfigurationCounts,
    selected: Option<&SelectedCell>,
    bias: &SelectionBias,
    pbo: &Availability<Pbo>,
) -> Vec<Caveat> {
    let mut out = Vec::new();
    let mut push = |code: &str, text: String| {
        out.push(Caveat {
            code: code.into(),
            text,
        })
    };
    let n = counts.tried;
    let metric = spec.objective.metric.key();
    let verb = match spec.objective.direction {
        Direction::Maximize => "highest",
        Direction::Minimize => "lowest",
    };
    let mut bias_text = if n == 1 {
        "One configuration was declared, so nothing was selected. Its value is still one replay \
         of one historical period under modeled queue and latency assumptions."
            .to_string()
    } else {
        match (spec.partition.is_some(), selected) {
            (false, _) => format!(
                "The configuration with the {verb} {metric} of {n} tried was chosen on the same \
                 data that scores it. The best of {n} noisy estimates is biased upward even when \
                 no configuration is better than any other, so this value overstates what the \
                 configuration would do on other data. No out-of-sample window was declared, so \
                 the size of the bias is not measured."
            ),
            (
                true,
                Some(SelectedCell {
                    out_of_sample: Some(oos),
                    out_of_sample_rank: Some(rank),
                    out_of_sample_of: of,
                    ..
                }),
            ) => format!(
                "The configuration with the {verb} in-sample {metric} of {n} tried was chosen in \
                 sample; that value is biased upward by the choice. Out of sample, which played \
                 no part in the choice, it scored {oos} (rank {rank} of {of}). That is the less \
                 biased number, but it is one holdout period: a single draw, not a \
                 distribution."
            ),
            (true, _) => format!(
                "The configuration with the {verb} in-sample {metric} of {n} tried was chosen in \
                 sample; that value is biased upward by the choice. Its out-of-sample value is \
                 missing, so the bias is not measured."
            ),
        }
    };
    if n >= MANY_TRIALS
        && let Some(e) = bias.expected_max_z_under_noise
    {
        bias_text.push_str(&format!(
            " With {n} independent pure-noise trials the best would lie about {e:.2} standard \
             deviations above the mean of all trials (Bailey, Borwein, López de Prado & Zhu \
             2014){}.",
            match bias.selected_z {
                Some(z) => format!("; the selected cell lies {z:.2} above it in sample"),
                None => String::new(),
            }
        ));
    }
    push("selection_bias", bias_text);

    let incomplete = counts.failed + counts.missing;
    if incomplete > 0 {
        push(
            "incomplete_cells",
            format!(
                "{incomplete} of {n} configurations produced no in-sample value ({} failed, {} \
                 missing). They still count as trials: the selection was made among the \
                 survivors, and the parameter values that failed are part of the result.",
                counts.failed, counts.missing
            ),
        );
    }
    if let Some(p) = spec.partition
        && p.in_sample.len_ns() != p.out_of_sample.len_ns()
    {
        let secs = |w: TimeWindow| w.len_ns() as f64 / 1e9;
        let text = if spec.objective.metric.is_time_additive() {
            format!(
                "The in-sample and out-of-sample windows last {} s and {} s. {metric} is a \
                 total over time: compare the per-second values, not the totals.",
                secs(p.in_sample),
                secs(p.out_of_sample)
            )
        } else {
            format!(
                "The in-sample and out-of-sample windows last {} s and {} s, and {metric} is not \
                 normalised for window length, so the two values are not directly comparable.",
                secs(p.in_sample),
                secs(p.out_of_sample)
            )
        };
        push("window_lengths", text);
    }
    if counts.completed_in_sample < MIN_SPEARMAN_CELLS as u32 && n > 1 {
        push(
            "few_cells",
            format!(
                "Only {} configurations completed; dispersion and rank statistics over so few \
                 cells say little.",
                counts.completed_in_sample
            ),
        );
    }
    push(
        "not_a_significance_test",
        "Rank correlation, dispersion and the standardised distance describe these cells only. \
         They are not significance tests: cells with neighbouring parameters share most of \
         their behaviour and are not independent trials."
            .into(),
    );
    if let Availability::Available(p) = pbo {
        push(
            "cscv_assumptions",
            format!(
                "PBO (Bailey, Borwein, López de Prado & Zhu 2017) is the fraction of the {} \
                 ways to split the selection window's {} time blocks into equal halves in which \
                 the configuration best on one half ranks at or below the median on the other. \
                 It treats the blocks as exchangeable, ignoring dependence across block \
                 boundaries, and scores performance as the sum of {metric}. It is computed on \
                 the selection window only and is separate from the out-of-sample holdout.{}",
                p.combinations,
                p.blocks,
                if p.cells < 10 {
                    format!(
                        " With {} configurations the relative rank takes only {} values, so the \
                         estimate is coarse.",
                        p.cells, p.cells
                    )
                } else {
                    String::new()
                }
            ),
        );
    }
    push(
        "no_annualised_sharpe",
        "No annualised Sharpe ratio is reported: a replay of minutes of one market regime \
         cannot support one (docs/research/MARKET_MICROSTRUCTURE.md §5)."
            .into(),
    );
    out
}

#[cfg(test)]
#[path = "sweep_tests.rs"]
mod tests;
