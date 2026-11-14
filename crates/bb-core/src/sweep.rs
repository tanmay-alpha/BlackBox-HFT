//! Parameter sweeps as a reproducible experiment registry.
//!
//! A [`SweepSpec`] declares, before anything runs, every configuration that will be tried (a grid
//! of up to two [`GridAxis`] or a seeded random design), the single objective that will pick the
//! "best" one, and optionally an in-sample / out-of-sample split of the dataset in time. The same
//! spec always yields the same cells ([`SweepSpec::cells`]) and the same fingerprint
//! ([`SweepSpec::fingerprint`]), so a sweep can be cited, re-run and audited.
//!
//! The spec carries no statistics. Selection-bias analysis of the finished cells (the number of
//! configurations tried, out-of-sample rank of the in-sample winner, rank correlation, CSCV
//! probability of backtest overfitting) lives in `bb-forensics` (`analyze_sweep`); its
//! definitions and sources are in `docs/research/VALIDATION.md`, "Sweeps and overfitting".
//!
//! # Time windows
//!
//! A [`TimeWindow`] is a half-open interval `[start, end)` of exchange-clock nanoseconds relative
//! to the dataset origin. A [`RunConfig`] has no window field: a windowed run executes on a
//! derived dataset (`bb_data::slice`), whose id [`TimeWindow::slice_dataset_id`] derives from the
//! parent dataset's id and hash. [`SweepSpec::admit`] returns the exact configurations to queue,
//! with those derived dataset ids filled in.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    RelNs,
    compare::{GridAxis, MAX_GRID_CELLS},
    config::{QueueModelConfig, RunConfig},
    dataset::DatasetManifest,
    hash::sha256_json,
    limits::{BudgetViolation, ExecutionLimits, WorkEstimate},
    result::{EquitySample, Metrics},
    rng::Rng,
};

/// Version of the [`SweepSpec`] format and of the cell-generation rules. Part of the fingerprint:
/// bump it whenever the same spec would produce different cells.
pub const SWEEP_SPEC_VERSION: u32 = 1;

/// Version of the dataset slicing rules (`bb_data::slice`). Part of every derived dataset id.
pub const DATASET_SLICE_VERSION: u32 = 1;

/// Hard upper bound on the configurations of one sweep, whatever the deployment.
pub const MAX_SWEEP_CELLS: usize = MAX_GRID_CELLS;

/// Configurations per sweep on the hosted service (`docs/engineering/API_CONTRACT.md` §5).
pub const HOSTED_MAX_SWEEP_CELLS: u32 = 25;

/// Smallest and largest number of CSCV time blocks. Even values only. 16 is the example of
/// Bailey, Borwein, López de Prado & Zhu (2017); C(16, 8) = 12,870 train/test combinations.
pub const MIN_CSCV_BLOCKS: u32 = 4;
pub const MAX_CSCV_BLOCKS: u32 = 16;

/// Half-open exchange-clock interval `[start, end)`, nanoseconds relative to the dataset origin.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct TimeWindow {
    pub start: RelNs,
    pub end: RelNs,
}

impl TimeWindow {
    /// `end - start`, saturating; zero or negative for an empty window.
    pub fn len_ns(&self) -> i64 {
        self.end.saturating_sub(self.start)
    }

    pub fn contains(&self, t: RelNs) -> bool {
        self.start <= t && t < self.end
    }

    /// `n` contiguous blocks covering the window exactly: block `i` is
    /// `[start + len·i/n, start + len·(i+1)/n)` in integer arithmetic. Empty for `n == 0`.
    pub fn blocks(&self, n: u32) -> Vec<TimeWindow> {
        let len = i128::from(self.len_ns().max(0));
        let at = |i: u32| self.start + (len * i128::from(i) / i128::from(n.max(1))) as i64;
        (0..n)
            .map(|i| TimeWindow {
                start: at(i),
                end: at(i + 1),
            })
            .collect()
    }

    /// Id of the dataset that holds this window of `parent_id` (whose event file hashes to
    /// `parent_events_sha256`). A pure function of the inputs and [`DATASET_SLICE_VERSION`], at
    /// most 64 characters of `[a-z0-9_-]`.
    pub fn slice_dataset_id(&self, parent_id: &str, parent_events_sha256: &str) -> String {
        let h = sha256_json(&(
            "blackbox-dataset-slice",
            DATASET_SLICE_VERSION,
            parent_id,
            parent_events_sha256,
            self,
        ));
        let prefix: String = parent_id.chars().take(40).collect();
        format!("{prefix}-w-{}", &h[..16])
    }
}

/// Which quantity of a run the sweep optimises. Annualised Sharpe-type ratios are deliberately
/// absent (`docs/research/MARKET_MICROSTRUCTURE.md` §5).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveMetric {
    /// [`Metrics::net_pnl`], quote currency.
    NetPnl,
    /// [`Metrics::realized_pnl`], quote currency.
    RealizedPnl,
    /// [`Metrics::fees`], quote currency (minimise it).
    Fees,
    /// [`Metrics::spread_capture`], quote currency.
    SpreadCapture,
    /// Net P&L per unit of turnover in basis points; unavailable without turnover.
    PnlPerTurnoverBps,
    /// [`Metrics::max_drawdown`], quote currency (minimise it).
    MaxDrawdown,
    FillCount,
    /// [`Metrics::fill_ratio_orders`]; unavailable without submitted orders.
    FillRatioOrders,
    MeanAbsPosition,
}

impl ObjectiveMetric {
    pub fn key(self) -> &'static str {
        match self {
            ObjectiveMetric::NetPnl => "net_pnl",
            ObjectiveMetric::RealizedPnl => "realized_pnl",
            ObjectiveMetric::Fees => "fees",
            ObjectiveMetric::SpreadCapture => "spread_capture",
            ObjectiveMetric::PnlPerTurnoverBps => "pnl_per_turnover_bps",
            ObjectiveMetric::MaxDrawdown => "max_drawdown",
            ObjectiveMetric::FillCount => "fill_count",
            ObjectiveMetric::FillRatioOrders => "fill_ratio_orders",
            ObjectiveMetric::MeanAbsPosition => "mean_abs_position",
        }
    }

    /// `"quote"`, `"bps"`, `"count"`, `"ratio"` or `"qty"`.
    pub fn unit(self) -> &'static str {
        match self {
            ObjectiveMetric::NetPnl
            | ObjectiveMetric::RealizedPnl
            | ObjectiveMetric::Fees
            | ObjectiveMetric::SpreadCapture
            | ObjectiveMetric::MaxDrawdown => "quote",
            ObjectiveMetric::PnlPerTurnoverBps => "bps",
            ObjectiveMetric::FillCount => "count",
            ObjectiveMetric::FillRatioOrders => "ratio",
            ObjectiveMetric::MeanAbsPosition => "qty",
        }
    }

    /// The metric's value for a finished run; `None` when it is undefined (no turnover, no
    /// submitted orders) or not finite.
    pub fn value(self, m: &Metrics) -> Option<f64> {
        let v = match self {
            ObjectiveMetric::NetPnl => Some(m.net_pnl),
            ObjectiveMetric::RealizedPnl => Some(m.realized_pnl),
            ObjectiveMetric::Fees => Some(m.fees),
            ObjectiveMetric::SpreadCapture => Some(m.spread_capture),
            ObjectiveMetric::PnlPerTurnoverBps => {
                (m.turnover > 0.0).then(|| m.net_pnl / m.turnover * 1e4)
            }
            ObjectiveMetric::MaxDrawdown => Some(m.max_drawdown),
            ObjectiveMetric::FillCount => Some(m.fill_count as f64),
            ObjectiveMetric::FillRatioOrders => m.fill_ratio_orders,
            ObjectiveMetric::MeanAbsPosition => Some(m.mean_abs_position),
        };
        v.filter(|v| v.is_finite())
    }

    /// The running total of a time-additive metric at an equity sample, or `None` for a metric
    /// that is not a sum over time (a ratio, a maximum, a mean). Only time-additive metrics can be
    /// split into per-block contributions for CSCV.
    pub fn cumulative(self, s: &EquitySample) -> Option<f64> {
        match self {
            ObjectiveMetric::NetPnl => Some(s.equity),
            ObjectiveMetric::RealizedPnl => Some(s.realized_pnl),
            ObjectiveMetric::Fees => Some(s.fees),
            _ => None,
        }
    }

    pub fn is_time_additive(self) -> bool {
        matches!(
            self,
            ObjectiveMetric::NetPnl | ObjectiveMetric::RealizedPnl | ObjectiveMetric::Fees
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Maximize,
    Minimize,
}

/// The one quantity, declared up front, that ranks the cells.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub metric: ObjectiveMetric,
    pub direction: Direction,
}

impl Objective {
    /// `v` mapped so that larger is always better.
    pub fn oriented(&self, v: f64) -> f64 {
        match self.direction {
            Direction::Maximize => v,
            Direction::Minimize => -v,
        }
    }
}

/// Integer parameter values (nanoseconds).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntDomain {
    /// `min, min + step, …` up to and including `max` when it lies on the grid.
    Range {
        min: i64,
        max: i64,
        step: i64,
    },
    Set {
        values: Vec<i64>,
    },
}

/// Fee values (fractions of traded value).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FloatDomain {
    /// Continuous uniform on `[min, max)`.
    Uniform {
        min: f64,
        max: f64,
    },
    Set {
        values: Vec<f64>,
    },
}

/// A parameter of a random design and the values it may take. Same parameters and semantics as
/// [`GridAxis`]; a drawn value is applied through a one-value `GridAxis`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RandomParam {
    OrderLatencyNs { domain: IntDomain },
    EntryLatencyNs { domain: IntDomain },
    FeedLatencyOffsetNs { domain: IntDomain },
    QueueModel { values: Vec<QueueModelConfig> },
    MakerFee { domain: FloatDomain },
    TakerFee { domain: FloatDomain },
}

impl RandomParam {
    /// The [`GridAxis::name`] of the parameter.
    pub fn name(&self) -> &'static str {
        match self {
            RandomParam::OrderLatencyNs { .. } => "order_latency_ns",
            RandomParam::EntryLatencyNs { .. } => "entry_latency_ns",
            RandomParam::FeedLatencyOffsetNs { .. } => "feed_latency_offset_ns",
            RandomParam::QueueModel { .. } => "queue_model",
            RandomParam::MakerFee { .. } => "maker_fee",
            RandomParam::TakerFee { .. } => "taker_fee",
        }
    }

    /// Number of values of a discrete domain; `None` for a continuous one.
    fn discrete_len(&self) -> Option<u64> {
        let int_len = |d: &IntDomain| match d {
            IntDomain::Range { min, max, step } if *step > 0 && min <= max => Some(
                ((i128::from(*max) - i128::from(*min)) / i128::from(*step) + 1)
                    .min(i128::from(u64::MAX)) as u64,
            ),
            IntDomain::Range { .. } => Some(0),
            IntDomain::Set { values } => Some(values.len() as u64),
        };
        match self {
            RandomParam::OrderLatencyNs { domain }
            | RandomParam::EntryLatencyNs { domain }
            | RandomParam::FeedLatencyOffsetNs { domain } => int_len(domain),
            RandomParam::QueueModel { values } => Some(values.len() as u64),
            RandomParam::MakerFee { domain } | RandomParam::TakerFee { domain } => match domain {
                FloatDomain::Uniform { .. } => None,
                FloatDomain::Set { values } => Some(values.len() as u64),
            },
        }
    }

    /// A one-value axis holding the `i`-th discrete value, or `u` mapped into a continuous range
    /// (`u` in `[0, 1)`; `i` is ignored then, and `u` for discrete domains).
    fn axis(&self, i: u64, u: f64) -> GridAxis {
        let int = |d: &IntDomain| match d {
            IntDomain::Range { min, step, .. } => {
                (i128::from(*min) + i128::from(*step) * i128::from(i)) as i64
            }
            IntDomain::Set { values } => values[i as usize],
        };
        let float = |d: &FloatDomain| match d {
            FloatDomain::Uniform { min, max } => min + (max - min) * u,
            FloatDomain::Set { values } => values[i as usize],
        };
        match self {
            RandomParam::OrderLatencyNs { domain } => GridAxis::OrderLatencyNs {
                values: vec![int(domain)],
            },
            RandomParam::EntryLatencyNs { domain } => GridAxis::EntryLatencyNs {
                values: vec![int(domain)],
            },
            RandomParam::FeedLatencyOffsetNs { domain } => GridAxis::FeedLatencyOffsetNs {
                values: vec![int(domain)],
            },
            RandomParam::QueueModel { values } => GridAxis::QueueModel {
                values: vec![values[i as usize]],
            },
            RandomParam::MakerFee { domain } => GridAxis::MakerFee {
                values: vec![float(domain)],
            },
            RandomParam::TakerFee { domain } => GridAxis::TakerFee {
                values: vec![float(domain)],
            },
        }
    }

    /// Structural problems of the domain.
    fn problems(&self) -> Vec<String> {
        let name = self.name();
        let mut e = Vec::new();
        let distinct = |dupe: bool, e: &mut Vec<String>| {
            if dupe {
                e.push(format!("{name}: values must be distinct"));
            }
        };
        match self {
            RandomParam::OrderLatencyNs { domain }
            | RandomParam::EntryLatencyNs { domain }
            | RandomParam::FeedLatencyOffsetNs { domain } => match domain {
                IntDomain::Range { min, max, step } => {
                    if *step <= 0 {
                        e.push(format!("{name}: step must be positive"));
                    }
                    if min > max {
                        e.push(format!("{name}: min must not exceed max"));
                    }
                }
                IntDomain::Set { values } => {
                    if values.is_empty() {
                        e.push(format!("{name}: the set needs at least one value"));
                    }
                    let unique: BTreeSet<i64> = values.iter().copied().collect();
                    distinct(unique.len() != values.len(), &mut e);
                }
            },
            RandomParam::QueueModel { values } => {
                if values.is_empty() {
                    e.push(format!("{name}: the set needs at least one value"));
                }
                // Sorted-set comparison keeps this n·log n for any submitted list length.
                let unique: BTreeSet<String> = values.iter().map(|q| format!("{q:?}")).collect();
                let dupe = unique.len() != values.len();
                distinct(dupe, &mut e);
            }
            RandomParam::MakerFee { domain } | RandomParam::TakerFee { domain } => match domain {
                FloatDomain::Uniform { min, max } => {
                    if !(min.is_finite() && max.is_finite() && min < max) {
                        e.push(format!(
                            "{name}: uniform range needs finite min < max (use a set for one value)"
                        ));
                    }
                }
                FloatDomain::Set { values } => {
                    if values.is_empty() {
                        e.push(format!("{name}: the set needs at least one value"));
                    }
                    // `v + 0.0` maps -0.0 to 0.0, so the two zeros count as one value.
                    let unique: BTreeSet<u64> =
                        values.iter().map(|v| (v + 0.0).to_bits()).collect();
                    let dupe = unique.len() != values.len();
                    distinct(dupe, &mut e);
                }
            },
        }
        e
    }
}

/// How the configurations of a sweep are chosen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SweepDesign {
    /// Every combination of one or two axes. Cells are ordered row-major, the first axis
    /// varying fastest: cell `yi * x_len + xi` (as in `GridResult`).
    Grid { axes: Vec<GridAxis> },
    /// `samples` configurations drawn from the declared parameter domains with `seed`.
    ///
    /// Combinations of the discrete parameters are drawn uniformly **without replacement**. When
    /// `samples` exceeds the number of discrete combinations (possible only with a continuous
    /// parameter), every combination is used once before any is used again. Continuous
    /// parameters are drawn uniformly, each from its own seeded stream, so adding a parameter
    /// never changes the values another one receives.
    Random {
        samples: u32,
        seed: u64,
        params: Vec<RandomParam>,
    },
}

/// Contiguous, non-overlapping in-sample and out-of-sample windows. Out-of-sample lies entirely
/// after in-sample (a gap between them is allowed), so nothing chosen in sample can depend on
/// data from the out-of-sample period.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Partition {
    pub in_sample: TimeWindow,
    pub out_of_sample: TimeWindow,
}

/// A declared, bounded parameter sweep.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SweepSpec {
    pub base: RunConfig,
    pub design: SweepDesign,
    /// Without a partition every cell runs once on the whole dataset and no out-of-sample value
    /// exists.
    #[serde(default)]
    pub partition: Option<Partition>,
    pub objective: Objective,
    /// Number of equal time blocks of the selection window (in-sample, or the whole dataset) for
    /// the CSCV probability of backtest overfitting. `None` skips it.
    #[serde(default)]
    pub cscv_blocks: Option<u32>,
}

/// One parameter value of a cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ParamValue {
    /// [`GridAxis::name`], e.g. `order_latency_ns`.
    pub param: String,
    /// [`GridAxis::value_label`], e.g. `1.5 ms`.
    pub label: String,
    pub value: serde_json::Value,
}

/// Which part of the data a run covers.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    /// The whole dataset (no partition).
    Full,
    InSample,
    OutOfSample,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepRun {
    pub segment: Segment,
    /// `None` for [`Segment::Full`].
    pub window: Option<TimeWindow>,
}

/// One configuration of a sweep.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepCell {
    /// Position in [`SweepSpec::cells`], from 0.
    pub index: u32,
    /// Grid coordinates, one per axis; empty for a random design.
    pub coords: Vec<u32>,
    pub params: Vec<ParamValue>,
    /// The configuration on the spec's (parent) dataset.
    pub config: RunConfig,
    /// One `full` run, or an `in_sample` and an `out_of_sample` run.
    pub runs: Vec<SweepRun>,
}

/// One simulation to queue, with its dataset id resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlannedRun {
    pub cell: u32,
    pub segment: Segment,
    pub window: Option<TimeWindow>,
    /// The cell's configuration; for a windowed run `dataset_id` is the derived slice dataset
    /// ([`TimeWindow::slice_dataset_id`]).
    pub config: RunConfig,
    pub estimate: WorkEstimate,
}

/// Summed predicted work of every run of a sweep.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepWork {
    pub runs: u64,
    pub ticks: u64,
    pub equity_samples: u64,
    pub tape_events_upper: u64,
    pub output_bytes_upper: u64,
}

/// Admission bounds of a sweep: a cell cap, the per-run limits every run must pass, and a total
/// over all runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepLimits {
    pub max_cells: u32,
    pub per_run: ExecutionLimits,
    /// Bounds on [`SweepWork`] (its `runs` field is not bounded separately).
    pub total: SweepWork,
}

impl SweepLimits {
    /// `max_cells` configurations, each run within `per_run`, and in total no more work than
    /// `max_cells` runs each at the per-run limit. A partitioned sweep runs every cell twice on
    /// shorter windows; the total keeps it within the same envelope.
    pub fn from_execution(per_run: ExecutionLimits, max_cells: u32) -> Self {
        let n = u64::from(max_cells);
        Self {
            max_cells,
            per_run,
            total: SweepWork {
                runs: u64::MAX,
                ticks: per_run.max_ticks.saturating_mul(n),
                equity_samples: per_run.max_equity_samples.saturating_mul(n),
                tape_events_upper: per_run.max_tape_events.saturating_mul(n),
                output_bytes_upper: per_run.max_output_bytes.saturating_mul(n),
            },
        }
    }

    /// 25 cells within [`ExecutionLimits::hosted`].
    pub fn hosted() -> Self {
        Self::from_execution(ExecutionLimits::hosted(), HOSTED_MAX_SWEEP_CELLS)
    }

    /// [`MAX_SWEEP_CELLS`] cells within [`ExecutionLimits::local`].
    pub fn local() -> Self {
        Self::from_execution(ExecutionLimits::local(), MAX_SWEEP_CELLS as u32)
    }
}

/// Why a sweep was not admitted.
#[derive(Clone, Debug, PartialEq)]
pub enum SweepRefusal {
    /// The spec is malformed or a cell's configuration is invalid (HTTP 422 `invalid_config`).
    Invalid(Vec<String>),
    /// More configurations than the deployment allows.
    TooManyCells { cells: u64, limit: u32 },
    /// One run exceeds the per-run limits.
    RunOverBudget {
        cell: u32,
        segment: Segment,
        violation: BudgetViolation,
    },
    /// The sum over all runs exceeds the sweep total.
    TotalOverBudget {
        what: &'static str,
        limit: u64,
        predicted: u64,
    },
}

impl SweepRefusal {
    /// `(what, limit, predicted)` for the budget refusals (HTTP 422 `budget_exceeded` details);
    /// `None` for [`SweepRefusal::Invalid`].
    pub fn budget_details(&self) -> Option<(String, u64, u64)> {
        match self {
            SweepRefusal::Invalid(_) => None,
            SweepRefusal::TooManyCells { cells, limit } => {
                Some(("sweep_cells".into(), u64::from(*limit), *cells))
            }
            SweepRefusal::RunOverBudget { violation, .. } => Some((
                violation.what.to_string(),
                violation.limit,
                violation.predicted,
            )),
            SweepRefusal::TotalOverBudget {
                what,
                limit,
                predicted,
            } => Some((format!("sweep_total_{what}"), *limit, *predicted)),
        }
    }
}

impl std::fmt::Display for SweepRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SweepRefusal::Invalid(problems) => write!(f, "{}", problems.join("; ")),
            SweepRefusal::TooManyCells { cells, limit } => write!(
                f,
                "the sweep declares {cells} configurations; this server allows {limit}"
            ),
            SweepRefusal::RunOverBudget {
                cell,
                segment,
                violation,
            } => write!(
                f,
                "cell {cell} ({}): {violation}",
                match segment {
                    Segment::Full => "full dataset",
                    Segment::InSample => "in-sample",
                    Segment::OutOfSample => "out-of-sample",
                }
            ),
            SweepRefusal::TotalOverBudget {
                what,
                limit,
                predicted,
            } => write!(
                f,
                "the sweep's runs together predict {predicted} {what}, above the sweep limit of \
                 {limit}; use fewer cells, shorter windows or a longer strategy interval"
            ),
        }
    }
}

/// An admitted sweep: its cells and the exact runs to queue, in cell order (in-sample before
/// out-of-sample).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SweepPlan {
    pub fingerprint: String,
    pub cells: Vec<SweepCell>,
    pub runs: Vec<PlannedRun>,
    pub total: SweepWork,
}

impl SweepSpec {
    /// SHA-256 (hex) over the spec version and the canonical JSON of the spec. Equal specs have
    /// equal fingerprints; any change to the base configuration, design, seed, partition,
    /// objective or CSCV blocks changes it.
    pub fn fingerprint(&self) -> String {
        sha256_json(&("blackbox-sweep-spec", SWEEP_SPEC_VERSION, self))
    }

    /// Number of configurations the spec declares, computed without generating them.
    pub fn cell_count(&self) -> u64 {
        match &self.design {
            SweepDesign::Grid { axes } => axes
                .iter()
                .fold(1u64, |n, a| n.saturating_mul(a.len() as u64)),
            SweepDesign::Random { samples, .. } => u64::from(*samples),
        }
    }

    /// The window the best cell is selected on: the in-sample window, or the whole dataset
    /// `[start, end + 1)` of `manifest` without a partition.
    pub fn selection_window(&self, manifest: &DatasetManifest) -> TimeWindow {
        match self.partition {
            Some(p) => p.in_sample,
            None => TimeWindow {
                start: manifest.start,
                end: manifest.end.saturating_add(1),
            },
        }
    }

    /// Problems that make the spec meaningless regardless of its cells. Cheap: never generates a
    /// cell.
    fn structural_problems(&self) -> Vec<String> {
        let n = self.cell_count();
        // First and alone: everything below is linear or quadratic in the declared values.
        if n > MAX_SWEEP_CELLS as u64 {
            return vec![format!(
                "the sweep declares {n} configurations; the limit is {MAX_SWEEP_CELLS}"
            )];
        }
        let mut e = Vec::new();
        match &self.design {
            SweepDesign::Grid { axes } => {
                if axes.is_empty() || axes.len() > 2 {
                    e.push("a grid sweeps one or two axes".into());
                }
                if axes.iter().any(GridAxis::is_empty) {
                    // Also guards the check below: with an empty axis the cell count is zero
                    // and the other axis' length is unbounded.
                    e.push("every grid axis needs at least one value".into());
                } else {
                    for a in axes {
                        let unique: BTreeSet<String> = (0..a.len())
                            .map(|i| format!("{:?}", single(a, i)))
                            .collect();
                        if unique.len() != a.len() {
                            let labels: Vec<String> =
                                (0..a.len()).map(|i| a.value_label(i)).collect();
                            e.push(format!(
                                "grid axis {} repeats a value ({}): a repeated configuration is \
                                 a wasted trial that still counts towards selection bias",
                                a.name(),
                                labels.join(", ")
                            ));
                        }
                    }
                }
                let names: Vec<&str> = axes.iter().map(GridAxis::name).collect();
                e.extend(param_set_problems(&names));
            }
            SweepDesign::Random {
                samples,
                seed,
                params,
            } => {
                if *samples == 0 {
                    e.push("a random design needs at least one sample".into());
                }
                if params.is_empty() {
                    e.push("a random design needs at least one parameter".into());
                }
                if *seed > crate::time::MAX_SAFE_INTEGER as u64 {
                    e.push(format!(
                        "random design seed must be at most {} (2^53 - 1)",
                        crate::time::MAX_SAFE_INTEGER
                    ));
                }
                for p in params {
                    e.extend(p.problems());
                }
                let names: Vec<&str> = params.iter().map(RandomParam::name).collect();
                e.extend(param_set_problems(&names));
                if e.is_empty() && params.iter().all(|p| p.discrete_len().is_some()) {
                    let combos = params
                        .iter()
                        .fold(1u64, |k, p| k.saturating_mul(p.discrete_len().unwrap_or(1)));
                    if u64::from(*samples) > combos {
                        e.push(format!(
                            "{samples} samples requested but the discrete parameters have only \
                             {combos} combinations; sampling is without replacement"
                        ));
                    }
                }
            }
        }
        if let Some(p) = self.partition {
            for (name, w) in [
                ("in_sample", p.in_sample),
                ("out_of_sample", p.out_of_sample),
            ] {
                if w.start >= w.end {
                    e.push(format!("partition.{name} must have start < end"));
                }
            }
            if p.out_of_sample.start < p.in_sample.end {
                e.push(
                    "partition.out_of_sample must start at or after partition.in_sample ends: \
                     out-of-sample data may not precede or overlap the data the selection is \
                     made on"
                        .into(),
                );
            }
        }
        if let Some(s) = self.cscv_blocks {
            if !(MIN_CSCV_BLOCKS..=MAX_CSCV_BLOCKS).contains(&s) || s % 2 != 0 {
                e.push(format!(
                    "cscv_blocks must be even and between {MIN_CSCV_BLOCKS} and {MAX_CSCV_BLOCKS}"
                ));
            }
            if !self.objective.metric.is_time_additive() {
                e.push(format!(
                    "cscv_blocks needs a time-additive objective (net_pnl, realized_pnl or fees); \
                     {} cannot be split into per-block contributions",
                    self.objective.metric.key()
                ));
            }
            if let Some(p) = self.partition {
                e.extend(self.block_length_problem(p.in_sample, s));
            }
        }
        e
    }

    /// Each CSCV block must hold at least one equity sample of the base configuration.
    fn block_length_problem(&self, selection: TimeWindow, blocks: u32) -> Option<String> {
        let step = self
            .base
            .strategy
            .interval_ns
            .saturating_mul(i64::from(self.base.record.sample_every_ticks.max(1)));
        let block = selection.len_ns() / i64::from(blocks.max(1));
        (block < step).then(|| {
            format!(
                "cscv_blocks: {blocks} blocks of the selection window are {block} ns each, \
                 shorter than one equity sample ({step} ns)"
            )
        })
    }

    /// Human-readable problems; empty means every cell may run. Structural checks run first and
    /// return early, so an oversized sweep is rejected without generating a cell.
    pub fn validate(&self) -> Vec<String> {
        match self.cells() {
            Ok(cells) => {
                let mut e = self.base.validate();
                for c in &cells {
                    for problem in c.config.validate() {
                        let msg = format!("cell {}: {problem}", c.index);
                        if !e.contains(&msg) {
                            e.push(msg);
                        }
                    }
                }
                e
            }
            Err(e) => e,
        }
    }

    /// The configurations of the sweep, in a fixed order. Same spec, same cells, on every
    /// platform: random designs use the project's own generator (`crate::rng`) and only exact
    /// IEEE-754 arithmetic. Returns the structural problems instead when there are any.
    pub fn cells(&self) -> Result<Vec<SweepCell>, Vec<String>> {
        let problems = self.structural_problems();
        if !problems.is_empty() {
            return Err(problems);
        }
        let runs = match self.partition {
            None => vec![SweepRun {
                segment: Segment::Full,
                window: None,
            }],
            Some(p) => vec![
                SweepRun {
                    segment: Segment::InSample,
                    window: Some(p.in_sample),
                },
                SweepRun {
                    segment: Segment::OutOfSample,
                    window: Some(p.out_of_sample),
                },
            ],
        };
        let assignments: Vec<(Vec<u32>, Vec<GridAxis>)> = match &self.design {
            SweepDesign::Grid { axes } => grid_assignments(axes),
            SweepDesign::Random {
                samples,
                seed,
                params,
            } => random_assignments(*samples as usize, *seed, params),
        };
        Ok(assignments
            .into_iter()
            .enumerate()
            .map(|(index, (coords, axes))| {
                let mut config = self.base.clone();
                let mut params = Vec::with_capacity(axes.len());
                for axis in &axes {
                    config = axis.apply(&config, 0);
                    params.push(ParamValue {
                        param: axis.name().to_string(),
                        label: axis.value_label(0),
                        value: axis_value(axis),
                    });
                }
                SweepCell {
                    index: index as u32,
                    coords,
                    params,
                    config,
                    runs: runs.clone(),
                }
            })
            .collect())
    }

    /// Full admission against the parent dataset and a deployment's limits: validation, window
    /// bounds, the cell cap, every run's predicted work against the per-run limits, and the sum
    /// against the sweep total. Returns the exact runs to queue.
    ///
    /// A windowed run is estimated on the parent manifest with `start`/`end` clipped to the
    /// window and the parent's full row count (an upper bound: the slice holds fewer rows plus a
    /// book snapshot of at most the parent's level count).
    pub fn admit(
        &self,
        parent: &DatasetManifest,
        limits: &SweepLimits,
    ) -> Result<SweepPlan, SweepRefusal> {
        let structural = self.structural_problems();
        if !structural.is_empty() {
            return Err(SweepRefusal::Invalid(structural));
        }
        let cells_declared = self.cell_count();
        if cells_declared > u64::from(limits.max_cells) {
            return Err(SweepRefusal::TooManyCells {
                cells: cells_declared,
                limit: limits.max_cells,
            });
        }
        let mut problems = self.validate();
        if parent.id != self.base.dataset_id {
            problems.push(format!(
                "base.dataset_id `{}` does not name the dataset `{}` the sweep is admitted on",
                self.base.dataset_id, parent.id
            ));
        }
        let span = TimeWindow {
            start: parent.start,
            end: parent.end.saturating_add(1),
        };
        if let Some(p) = self.partition {
            for (name, w) in [
                ("in_sample", p.in_sample),
                ("out_of_sample", p.out_of_sample),
            ] {
                if w.start < span.start || w.end > span.end {
                    problems.push(format!(
                        "partition.{name} [{}, {}) lies outside the dataset's exchange-time span \
                         [{}, {})",
                        w.start, w.end, span.start, span.end
                    ));
                }
            }
        } else if let Some(s) = self.cscv_blocks {
            problems.extend(self.block_length_problem(span, s));
        }
        if parent.events_sha256.is_empty() && self.partition.is_some() {
            problems.push(format!(
                "dataset `{}` has no recorded events hash, so its windows cannot be named",
                parent.id
            ));
        }
        if !problems.is_empty() {
            return Err(SweepRefusal::Invalid(problems));
        }

        let cells = self.cells().map_err(SweepRefusal::Invalid)?;
        let windowed = |w: TimeWindow| DatasetManifest {
            start: w.start,
            end: w.end,
            ..parent.clone()
        };
        let manifests: Vec<(Option<TimeWindow>, DatasetManifest)> = match self.partition {
            None => vec![(None, parent.clone())],
            Some(p) => vec![
                (Some(p.in_sample), windowed(p.in_sample)),
                (Some(p.out_of_sample), windowed(p.out_of_sample)),
            ],
        };
        let mut runs = Vec::with_capacity(cells.len() * manifests.len());
        let mut total = SweepWork::default();
        for cell in &cells {
            for (run, (window, manifest)) in cell.runs.iter().zip(&manifests) {
                let estimate = cell.config.estimate_work(manifest);
                limits.per_run.admit(&estimate).map_err(|violation| {
                    SweepRefusal::RunOverBudget {
                        cell: cell.index,
                        segment: run.segment,
                        violation,
                    }
                })?;
                total.runs += 1;
                total.ticks = total.ticks.saturating_add(estimate.ticks);
                total.equity_samples = total.equity_samples.saturating_add(estimate.equity_samples);
                total.tape_events_upper = total
                    .tape_events_upper
                    .saturating_add(estimate.tape_events_upper);
                total.output_bytes_upper = total
                    .output_bytes_upper
                    .saturating_add(estimate.output_bytes_upper);
                let mut config = cell.config.clone();
                if let Some(w) = window {
                    config.dataset_id = w.slice_dataset_id(&parent.id, &parent.events_sha256);
                }
                runs.push(PlannedRun {
                    cell: cell.index,
                    segment: run.segment,
                    window: *window,
                    config,
                    estimate,
                });
            }
        }
        for (what, predicted, limit) in [
            ("ticks", total.ticks, limits.total.ticks),
            (
                "equity_samples",
                total.equity_samples,
                limits.total.equity_samples,
            ),
            (
                "tape_events",
                total.tape_events_upper,
                limits.total.tape_events_upper,
            ),
            (
                "output_bytes",
                total.output_bytes_upper,
                limits.total.output_bytes_upper,
            ),
        ] {
            if predicted > limit {
                return Err(SweepRefusal::TotalOverBudget {
                    what,
                    limit,
                    predicted,
                });
            }
        }
        Ok(SweepPlan {
            fingerprint: self.fingerprint(),
            cells,
            runs,
            total,
        })
    }
}

/// Parameters swept together must be distinct and must not set the same field twice.
fn param_set_problems(names: &[&str]) -> Vec<String> {
    let mut e = Vec::new();
    for (i, a) in names.iter().enumerate() {
        if names[..i].contains(a) {
            e.push(format!("parameter {a} is swept twice"));
        }
    }
    if names.contains(&"order_latency_ns") && names.contains(&"entry_latency_ns") {
        e.push("order_latency_ns and entry_latency_ns both set the entry latency".into());
    }
    e
}

/// The single value of a one-value axis as JSON.
fn axis_value(axis: &GridAxis) -> serde_json::Value {
    let v = match axis {
        GridAxis::OrderLatencyNs { values }
        | GridAxis::EntryLatencyNs { values }
        | GridAxis::FeedLatencyOffsetNs { values } => serde_json::to_value(values[0]),
        GridAxis::QueueModel { values } => serde_json::to_value(values[0]),
        GridAxis::MakerFee { values } | GridAxis::TakerFee { values } => {
            serde_json::to_value(values[0])
        }
    };
    v.expect("parameter values serialize")
}

/// The `i`-th value of `axis` as a one-value axis.
fn single(axis: &GridAxis, i: usize) -> GridAxis {
    match axis {
        GridAxis::OrderLatencyNs { values } => GridAxis::OrderLatencyNs {
            values: vec![values[i]],
        },
        GridAxis::EntryLatencyNs { values } => GridAxis::EntryLatencyNs {
            values: vec![values[i]],
        },
        GridAxis::FeedLatencyOffsetNs { values } => GridAxis::FeedLatencyOffsetNs {
            values: vec![values[i]],
        },
        GridAxis::QueueModel { values } => GridAxis::QueueModel {
            values: vec![values[i]],
        },
        GridAxis::MakerFee { values } => GridAxis::MakerFee {
            values: vec![values[i]],
        },
        GridAxis::TakerFee { values } => GridAxis::TakerFee {
            values: vec![values[i]],
        },
    }
}

fn grid_assignments(axes: &[GridAxis]) -> Vec<(Vec<u32>, Vec<GridAxis>)> {
    match axes {
        [x] => (0..x.len())
            .map(|xi| (vec![xi as u32], vec![single(x, xi)]))
            .collect(),
        [x, y] => (0..y.len())
            .flat_map(|yi| {
                (0..x.len()).map(move |xi| {
                    (
                        vec![xi as u32, yi as u32],
                        vec![single(x, xi), single(y, yi)],
                    )
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn random_assignments(
    samples: usize,
    seed: u64,
    params: &[RandomParam],
) -> Vec<(Vec<u32>, Vec<GridAxis>)> {
    let lens: Vec<Option<u64>> = params.iter().map(RandomParam::discrete_len).collect();
    let discrete: Vec<u64> = lens.iter().flatten().copied().collect();
    let combos_total = discrete.iter().fold(1u64, |k, n| k.saturating_mul(*n));

    // Discrete combinations, uniformly without replacement within each pass over the space.
    // Redrawing a combination already taken in this pass is exact rejection sampling; with at
    // most MAX_SWEEP_CELLS samples it terminates quickly even when the space is that small.
    let mut rng = Rng::for_stream(seed, "sweep.random.discrete");
    let mut combos: Vec<Vec<u64>> = Vec::with_capacity(samples);
    while combos.len() < samples {
        let pass = combos_total.min((samples - combos.len()) as u64) as usize;
        let mut seen = BTreeSet::new();
        while seen.len() < pass {
            let c: Vec<u64> = discrete.iter().map(|n| rng.below(*n)).collect();
            if seen.insert(c.clone()) {
                combos.push(c);
            }
        }
    }

    // Continuous parameters: one stream per parameter name.
    let mut uniforms: Vec<Option<Rng>> = params
        .iter()
        .zip(&lens)
        .map(|(p, len)| {
            len.is_none()
                .then(|| Rng::for_stream(seed, &format!("sweep.random.{}", p.name())))
        })
        .collect();

    combos
        .into_iter()
        .map(|combo| {
            let mut next_discrete = combo.into_iter();
            let axes = params
                .iter()
                .zip(uniforms.iter_mut())
                .map(|(p, u)| match u {
                    Some(r) => p.axis(0, r.next_f64()),
                    None => p.axis(next_discrete.next().unwrap_or(0), 0.0),
                })
                .collect();
            (Vec::new(), axes)
        })
        .collect()
}

#[cfg(test)]
#[path = "sweep_tests.rs"]
mod tests;
