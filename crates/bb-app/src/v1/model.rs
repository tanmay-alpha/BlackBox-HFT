//! Resource shapes of `/api/v1` that are not bb-core types (API_CONTRACT §9). Nanosecond
//! fields are `String` here already; bb-core payloads inside are converted by `wire::to_wire`.

use bb_core::{
    compare::{Comparison as CoreComparison, GridSpec},
    config::RunConfig,
    result::{EngineInfo, EngineStateCheck, Metrics},
    sweep::{Objective, ParamValue, Partition, Segment, SweepSpec},
};
use bb_db::{ExperimentView, JobView, RunView};
use bb_forensics::{EnsembleReport, SweepReport};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::ApiError;
use crate::{wire::at_ns, worker::LiveProgress};

#[derive(Serialize, ToSchema)]
pub struct Health {
    pub status: String,
}

#[derive(Serialize, ToSchema)]
pub struct Ready {
    pub status: String,
    pub db: String,
    pub migrations: String,
    pub blob: String,
}

#[derive(Serialize, ToSchema)]
pub struct WorkerSummary {
    /// `in_process` or `standalone`.
    pub mode: String,
    /// `starting`, `idle`, `busy`, `draining` or `stopped`.
    pub state: String,
    pub slots: usize,
    pub busy: usize,
    pub last_heartbeat_age_ms: Option<u64>,
    pub heartbeat_interval_ms: u64,
}

#[derive(Serialize, ToSchema)]
pub struct QueueSummary {
    pub depth: i64,
    pub running: i64,
    /// When the worker last polled the queue; `null` before its first poll.
    pub as_of_ns: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LimitsSummary {
    pub profile: String,
    /// `null` = unlimited (local profile).
    pub max_ticks: Option<u64>,
    pub max_tape_events: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub max_sweep_cells: usize,
    pub deadline_ms: i32,
}

/// `GET /api/v1/status` (memory only).
#[derive(Serialize, ToSchema)]
pub struct Status {
    /// `ok` or `draining`.
    pub status: String,
    pub api_version: String,
    pub contract_version: u32,
    pub blackbox_version: String,
    pub git_sha: Option<String>,
    pub engine: EngineInfo,
    pub server_time_ns: String,
    pub worker: WorkerSummary,
    pub queue: QueueSummary,
    pub limits: LimitsSummary,
    /// Platform class of results submitted here.
    pub result_platform: String,
    pub disclaimer: String,
}

#[derive(Serialize, ToSchema)]
pub struct WorkspaceRef {
    pub slug: String,
    pub name: String,
    /// `demo`, `research` or `system`.
    pub kind: String,
}

#[derive(Serialize, ToSchema)]
pub struct KeyRef {
    pub id: String,
    pub label: String,
    pub display_prefix: String,
    pub scopes: Vec<String>,
    pub expires_at_ns: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct QuotaUse {
    pub limit: i64,
    pub used: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Quotas {
    pub experiments_per_day: QuotaUse,
    pub sweeps_per_day: QuotaUse,
    pub active_jobs: QuotaUse,
}

/// `GET /api/v1/whoami`.
#[derive(Serialize, ToSchema)]
pub struct Whoami {
    /// `demo` (anonymous) or `research`.
    pub mode: String,
    pub workspace: WorkspaceRef,
    pub key: Option<KeyRef>,
    pub quotas: Option<Quotas>,
}

#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct Estimate {
    pub ticks: i64,
    pub tape_events_upper: i64,
    pub output_bytes_upper: i64,
}

#[derive(Serialize, ToSchema)]
pub struct JobProgress {
    pub done: u64,
    pub total: u64,
    /// Always `ticks`.
    pub unit: String,
    pub tape_events: u64,
}

#[derive(Serialize, ToSchema)]
pub struct JobErrorBody {
    /// `invalid_config`, `budget_exceeded`, `simulation_failed`, `lease_expired`,
    /// `integrity_failed`, `storage_failed` or `internal`.
    pub category: String,
    pub code: String,
    pub message: String,
}

/// API_CONTRACT §9.3. `running → queued` is a legal transition (re-queued after a drain or a
/// lost lease); `attempt` and `lease_losses` tell the two apart.
#[derive(Serialize, ToSchema)]
pub struct Job {
    pub id: String,
    pub kind: String,
    /// `created`, `queued`, `running`, `cancelling`, `completed`, `failed`, `cancelled`,
    /// `expired`.
    pub state: String,
    pub version: i64,
    pub experiment_fingerprint: String,
    pub run_identity: String,
    pub platform: String,
    pub priority: i16,
    pub attempt: i16,
    pub max_attempts: i16,
    pub lease_losses: i16,
    pub created_at_ns: String,
    pub queued_at_ns: Option<String>,
    pub started_at_ns: Option<String>,
    pub finished_at_ns: Option<String>,
    pub queue_position: Option<i64>,
    pub queue_depth: Option<i64>,
    pub progress: Option<JobProgress>,
    pub last_heartbeat_at_ns: Option<String>,
    pub heartbeat_interval_ms: u64,
    pub cancel_requested_at_ns: Option<String>,
    pub error: Option<JobErrorBody>,
    pub expired_reason: Option<String>,
    pub run_id: Option<String>,
    pub run_ids: Vec<String>,
    /// Set by `POST /jobs/{id}/cancel` when only the caller's experiment was detached.
    pub detached: Option<bool>,
}

impl Job {
    pub fn from_view(
        view: &JobView,
        live: Option<LiveProgress>,
        queue_position: Option<i64>,
        heartbeat_interval_ms: u64,
    ) -> Self {
        let j = &view.job;
        let progress = match live {
            Some(p) if p.attempt == j.claim_count && j.state == "running" => Some(JobProgress {
                done: p.done.min(p.total),
                total: p.total,
                unit: "ticks".into(),
                tape_events: p.tape_events,
            }),
            _ if j.heartbeat_at.is_some()
                && matches!(j.state.as_str(), "running" | "cancelling") =>
            {
                Some(JobProgress {
                    done: u64::try_from(j.ticks_done).unwrap_or(0),
                    total: u64::try_from(j.ticks_total).unwrap_or(0),
                    unit: "ticks".into(),
                    tape_events: u64::try_from(j.tape_events).unwrap_or(0),
                })
            }
            _ if j.state == "completed" => Some(JobProgress {
                done: u64::try_from(j.ticks_total).unwrap_or(0),
                total: u64::try_from(j.ticks_total).unwrap_or(0),
                unit: "ticks".into(),
                tape_events: u64::try_from(j.tape_events).unwrap_or(0),
            }),
            _ => None,
        };
        Self {
            id: j.id.to_string(),
            kind: j.kind.clone(),
            state: j.state.clone(),
            version: j.version,
            experiment_fingerprint: hex::encode(&view.fingerprint),
            run_identity: view.short_id.clone(),
            platform: j.platform.clone(),
            priority: j.priority,
            attempt: j.claim_count,
            max_attempts: j.max_attempts,
            lease_losses: j.lease_losses,
            created_at_ns: at_ns(j.created_at),
            queued_at_ns: j.queued_at.map(at_ns),
            started_at_ns: j.started_at.map(at_ns),
            finished_at_ns: j.finished_at.map(at_ns),
            queue_position,
            queue_depth: None,
            progress,
            last_heartbeat_at_ns: j.heartbeat_at.map(at_ns),
            heartbeat_interval_ms,
            cancel_requested_at_ns: j.cancel_requested_at.map(at_ns),
            error: match (&j.error_category, &j.error_code) {
                (Some(category), Some(code)) => Some(JobErrorBody {
                    category: category.clone(),
                    code: code.clone(),
                    message: j.error_message.clone().unwrap_or_default(),
                }),
                _ => None,
            },
            expired_reason: j.expired_reason.clone(),
            run_id: view.run_short_id.clone(),
            run_ids: view.run_short_id.iter().cloned().collect(),
            detached: None,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct DatasetRef {
    pub id: String,
    pub version: i32,
}

#[derive(Serialize, ToSchema)]
pub struct RunDatasetRef {
    pub id: String,
    pub version: i32,
    pub events_sha256: String,
    pub origin_ns: String,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedBy {
    pub display_prefix: String,
    pub label: String,
}

#[derive(Serialize, ToSchema)]
pub struct BundleRef {
    pub format_version: i32,
    pub sha256: String,
    pub size_bytes: i64,
}

#[derive(Serialize, ToSchema)]
pub struct RunCounts {
    pub ticks: i64,
    pub orders: i64,
    pub fills: i64,
    pub tape_events: i64,
    pub equity_samples: i64,
}

/// A completed run (API_CONTRACT §9.4). Metrics are verified against `summary_sha256`.
#[derive(Serialize, ToSchema)]
pub struct RunSummary {
    pub run_id: String,
    pub fingerprint: String,
    pub platform: String,
    pub canonical: bool,
    /// The caller's experiment label for this run (the demo label if only demo shows it).
    pub label: Option<String>,
    pub experiment_id: Option<String>,
    pub dataset: RunDatasetRef,
    pub strategy_kind: Option<String>,
    pub interval_ns: Option<String>,
    pub contract_version: i32,
    pub simulation_revision: i32,
    pub engine: EngineInfo,
    pub blackbox_version: String,
    pub build_profile: String,
    pub result_sha256: String,
    pub summary_sha256: String,
    pub bundle: Option<BundleRef>,
    pub counts: RunCounts,
    pub metrics: Metrics,
    pub engine_check: EngineStateCheck,
    pub wall_time_ms: i64,
    pub peak_rss_bytes: Option<i64>,
    pub created_at_ns: String,
    /// Detail only (`GET /runs/{run_id}`).
    pub config: Option<RunConfig>,
}

impl RunSummary {
    pub fn from_view(view: &RunView, include_config: bool) -> Result<Self, ApiError> {
        let r = &view.run;
        let metrics: Metrics = serde_json::from_value(r.metrics.clone())?;
        let engine_check: EngineStateCheck = serde_json::from_value(r.engine_check.clone())?;
        let config = if include_config {
            Some(serde_json::from_value(view.config.clone())?)
        } else {
            None
        };
        Ok(Self {
            run_id: view.short_id.clone(),
            fingerprint: hex::encode(&view.fingerprint),
            platform: r.platform.clone(),
            canonical: r.is_canonical,
            label: view.view_label.clone(),
            experiment_id: view.view_experiment_id.map(|id| id.to_string()),
            dataset: RunDatasetRef {
                id: view.dataset_id.clone(),
                version: view.dataset_version_no,
                events_sha256: hex::encode(&view.events_sha256),
                origin_ns: view.origin_ns.to_string(),
            },
            strategy_kind: view.strategy_kind.clone(),
            interval_ns: view.interval_ns.map(|v| v.to_string()),
            contract_version: view.contract_version,
            simulation_revision: view.simulation_revision,
            engine: EngineInfo {
                name: view.engine_name.clone(),
                version: view.engine_version.clone(),
                features: vec!["backtest".into()],
            },
            blackbox_version: r.blackbox_version.clone(),
            build_profile: r.build_profile.clone(),
            result_sha256: hex::encode(&r.result_sha256),
            summary_sha256: hex::encode(&r.summary_sha256),
            bundle: None,
            counts: RunCounts {
                ticks: r.ticks,
                orders: r.order_count,
                fills: r.fill_count,
                tape_events: r.tape_len,
                equity_samples: r.equity_len,
            },
            metrics,
            engine_check,
            wall_time_ms: r.wall_time_ms,
            peak_rss_bytes: r.peak_rss_bytes,
            created_at_ns: at_ns(r.created_at),
            config,
        })
    }
}

/// API_CONTRACT §9.4 `Experiment`.
#[derive(Serialize, ToSchema)]
pub struct Experiment {
    pub id: String,
    pub workspace: String,
    pub label: String,
    pub version: i64,
    pub pinned: bool,
    pub run_id: String,
    pub fingerprint: String,
    pub dataset: DatasetRef,
    pub strategy_kind: String,
    pub interval_ns: Option<String>,
    pub platform: String,
    pub sweep: Option<String>,
    /// `completed` when a run exists, else the job state, `cancelled` after this workspace
    /// cancelled, or `detached`.
    pub status: String,
    pub job: Option<Job>,
    pub run: Option<RunSummary>,
    pub estimate: Estimate,
    pub created_by: Option<CreatedBy>,
    pub created_at_ns: String,
    /// Detail only.
    pub config: Option<RunConfig>,
}

impl Experiment {
    pub fn from_view(
        view: &ExperimentView,
        job: Option<Job>,
        run: Option<RunSummary>,
        include_config: bool,
    ) -> Result<Self, ApiError> {
        let e = &view.experiment;
        Ok(Self {
            id: e.id.to_string(),
            workspace: view.workspace_slug.clone(),
            label: e.label.clone(),
            version: e.version,
            pinned: e.pinned,
            run_id: view.short_id.clone(),
            fingerprint: hex::encode(&view.fingerprint),
            dataset: DatasetRef {
                id: e.dataset_id.clone(),
                version: view.dataset_version_no,
            },
            strategy_kind: e.strategy_kind.clone(),
            interval_ns: view.interval_ns.map(|v| v.to_string()),
            platform: e.platform.clone(),
            sweep: e.sweep_id.map(|s| s.to_string()),
            status: view.status.clone(),
            job,
            run,
            estimate: Estimate {
                ticks: view.est_ticks,
                tape_events_upper: view.est_tape_events_upper,
                output_bytes_upper: view.est_output_bytes_upper,
            },
            created_by: match (&view.key_prefix, &view.key_label) {
                (Some(p), Some(l)) => Some(CreatedBy {
                    display_prefix: p.clone(),
                    label: l.clone(),
                }),
                _ => None,
            },
            created_at_ns: at_ns(e.created_at),
            config: if include_config {
                Some(serde_json::from_value(view.config.clone())?)
            } else {
                None
            },
        })
    }
}

/// Body of `POST /api/v1/experiments`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitExperiment {
    pub config: RunConfig,
    #[serde(default)]
    pub label: Option<String>,
}

/// Response of `POST /api/v1/experiments`.
#[derive(Serialize, ToSchema)]
pub struct Submitted {
    pub created: bool,
    pub experiment: Experiment,
    pub job: Option<Job>,
    pub run: Option<RunSummary>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelPatch {
    pub label: String,
}

/// Body of `POST /jobs/{id}/cancel` and `POST /sweeps/{id}/cancel` (empty object).
#[derive(Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Serialize, ToSchema)]
pub struct DataSourceRef {
    pub id: String,
    pub vendor: String,
    pub licence: String,
    pub hostable: bool,
}

#[derive(Serialize, ToSchema)]
pub struct InstrumentRef {
    pub exchange: String,
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    pub contract_size: f64,
}

#[derive(Serialize, ToSchema)]
pub struct QualitySummary {
    pub checker_version: i32,
    pub passed: bool,
    pub error_count: i32,
    pub warning_count: i32,
}

/// API_CONTRACT §9.4 `DatasetVersion`. `manifest` only in detail responses.
#[derive(Serialize, ToSchema)]
pub struct DatasetVersion {
    pub dataset_id: String,
    pub version: i32,
    pub events_sha256: String,
    pub normalization_version: i32,
    pub contract_version: i32,
    pub instrument: InstrumentRef,
    pub origin_ns: String,
    pub start_ns: String,
    pub end_ns: String,
    pub timestamp_unit: String,
    pub rows: i64,
    pub events_bytes: i64,
    pub source: Option<DataSourceRef>,
    pub quality: Option<QualitySummary>,
    pub retired_at_ns: Option<String>,
    pub manifest: Option<bb_core::dataset::DatasetManifest>,
}

/// `GET /datasets` item and `GET /datasets/{id}`.
#[derive(Serialize, ToSchema)]
pub struct Dataset {
    pub id: String,
    pub title: String,
    /// `synthetic` or `recorded`.
    pub kind: String,
    pub level: String,
    /// `global` or `workspace`.
    pub owner: String,
    pub current_version: Option<DatasetVersion>,
    pub created_at_ns: String,
}

/// `GET /datasets/{id}/versions/{v}/quality`.
#[derive(Serialize, ToSchema)]
pub struct Quality {
    pub checker_version: i32,
    pub passed: bool,
    pub error_count: i32,
    pub warning_count: i32,
    pub book_valid_fraction: f64,
    pub report: bb_core::dataset::QualityReport,
    pub report_sha256: String,
}

/// Body of `POST /api/v1/comparisons`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateComparison {
    pub run_a: String,
    pub run_b: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Platform class of both runs (default: this deployment's result platform).
    #[serde(default)]
    pub platform: Option<String>,
}

/// A stored comparison: bb-core `Comparison` plus resource fields. In this resource
/// `alignment.pairs` is always empty; pairs are paged by `/comparisons/{id}/pairs`.
#[derive(Serialize, ToSchema)]
pub struct Comparison {
    pub id: String,
    pub label: String,
    pub version: i64,
    pub comparator_version: i32,
    /// `aligned` or `metrics_only`.
    pub status: String,
    pub created_at_ns: String,
    #[serde(flatten)]
    pub comparison: CoreComparison,
}

/// The sweep to run: a `SweepSpec` (has `design`), or a legacy two-axis `GridSpec` (has `x` and
/// `y`), which is converted to a grid sweep on the whole dataset ranked by net P&L.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum SweepRequestSpec {
    Sweep(SweepSpec),
    Grid(GridSpec),
}

impl SweepRequestSpec {
    pub fn into_spec(self) -> SweepSpec {
        match self {
            SweepRequestSpec::Sweep(spec) => spec,
            SweepRequestSpec::Grid(grid) => grid.into(),
        }
    }
}

impl<'de> Deserialize<'de> for SweepRequestSpec {
    /// Chooses the shape by the `design` key, so a malformed spec gets the precise error of the
    /// shape it was meant to be instead of serde's "no variant matched".
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        if value.get("design").is_some() {
            serde_json::from_value(value)
                .map(SweepRequestSpec::Sweep)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(SweepRequestSpec::Grid)
                .map_err(|e| D::Error::custom(format!("{e} (a SweepSpec needs `design`)")))
        }
    }
}

/// Body of `POST /api/v1/sweeps`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSweep {
    pub spec: SweepRequestSpec,
    #[serde(default)]
    pub label: Option<String>,
}

/// One axis of a grid design.
#[derive(Serialize, ToSchema)]
pub struct SweepAxis {
    pub axis: String,
    pub labels: Vec<String>,
}

/// Counts over the sweep's runs (a partitioned cell has two).
#[derive(Serialize, ToSchema, Default)]
pub struct SweepProgress {
    /// Configurations declared by the spec.
    pub cells_total: usize,
    /// Runs planned: one per cell, or in-sample and out-of-sample per cell.
    pub runs_total: usize,
    pub completed: usize,
    pub running: usize,
    pub queued: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub expired: usize,
}

#[derive(Serialize, ToSchema)]
pub struct CellMetrics {
    pub net_pnl: Option<f64>,
    pub fill_count: Option<i64>,
    pub fees: Option<f64>,
    pub max_drawdown: Option<f64>,
}

/// One run of a cell: its job state and, once completed, its run.
#[derive(Serialize, ToSchema)]
pub struct SweepCellRun {
    pub segment: Segment,
    /// `None` until the run was submitted (an interrupted submission; resubmit the spec).
    pub experiment_id: Option<String>,
    pub job_id: Option<String>,
    /// `queued`, `running`, `completed`, `failed`, `cancelled`, `expired`, `detached` or
    /// `not_submitted`.
    pub state: String,
    pub run_id: Option<String>,
    pub error: Option<String>,
    pub metrics: Option<CellMetrics>,
}

/// One configuration of a sweep and the state of its runs.
#[derive(Serialize, ToSchema)]
pub struct SweepCell {
    /// Position in the spec's cell order, from 0.
    pub index: u32,
    /// Grid coordinates, one per axis; empty for random and seeds designs.
    pub coords: Vec<u32>,
    pub params: Vec<ParamValue>,
    pub runs: Vec<SweepCellRun>,
}

/// API_CONTRACT §8.7 `Sweep`.
#[derive(Serialize, ToSchema)]
pub struct Sweep {
    pub id: String,
    pub label: String,
    pub version: i64,
    /// `grid`, `random` or `seeds`.
    pub design: String,
    /// `SweepSpec::fingerprint`.
    pub fingerprint: String,
    /// The axes of a grid design (first axis varies fastest); empty otherwise.
    pub axes: Vec<SweepAxis>,
    pub partition: Option<Partition>,
    pub objective: Objective,
    /// `queued`, `running`, `completed`, `partial` or `cancelled`.
    pub state: String,
    /// Every run is terminal: `GET /sweeps/{id}/report` is available.
    pub report_ready: bool,
    pub progress: SweepProgress,
    /// Omitted (`null`) in list responses.
    pub cells: Option<Vec<SweepCell>>,
    pub spec: Option<SweepSpec>,
    pub created_at_ns: String,
}

#[derive(Serialize, ToSchema)]
pub struct SweepSubmitted {
    /// `false` when the same spec was already submitted in this workspace (idempotent; any
    /// run missing from an interrupted submission is submitted now).
    pub created: bool,
    pub sweep: Sweep,
}

/// A configuration as analysed: its parameters and objective values.
#[derive(Serialize, ToSchema)]
pub struct SweepReportCell {
    pub index: u32,
    pub params: Vec<ParamValue>,
    /// Objective on the selection data: the in-sample window, or the whole dataset.
    pub in_sample: Option<f64>,
    pub out_of_sample: Option<f64>,
    pub in_sample_run_id: Option<String>,
    pub out_of_sample_run_id: Option<String>,
    pub error: Option<String>,
}

/// `GET /sweeps/{id}/report`: the analysis of a finished sweep. Grid and random designs carry
/// `analysis` (selection, IS→OOS degradation, Spearman, PBO, caveats); a seeds design carries
/// `ensemble` (per-metric dispersion over seeds) instead.
#[derive(Serialize, ToSchema)]
pub struct SweepReportResponse {
    pub sweep_id: String,
    pub label: String,
    pub design: String,
    pub state: String,
    pub objective: Objective,
    pub partition: Option<Partition>,
    pub cells: Vec<SweepReportCell>,
    pub analysis: Option<SweepReport>,
    pub ensemble: Option<EnsembleReport>,
}

/// Body of `POST /api/v1/reports`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateReport {
    /// `run`, `comparison` or `sweep`.
    pub kind: String,
    pub subject_id: String,
    pub title: String,
}

/// API_CONTRACT §9.4 `Report`.
#[derive(Serialize, ToSchema)]
pub struct Report {
    pub id: String,
    pub kind: String,
    pub subject_id: Option<String>,
    pub title: String,
    pub generator_version: i32,
    pub format: String,
    pub body_sha256: String,
    pub size_bytes: usize,
    pub created_at_ns: String,
}

/// A preset sized for a dataset (as `GET /api/presets`), plus whether it is admitted here.
#[derive(Serialize, ToSchema)]
pub struct Preset {
    pub id: String,
    pub title: String,
    pub description: String,
    pub demonstrates: String,
    pub config: RunConfig,
}

#[derive(Serialize, ToSchema)]
pub struct DemoRun {
    pub label: String,
    pub config: RunConfig,
    /// The visible run for this exact configuration, if it has been simulated.
    pub run_id: Option<String>,
    pub experiment_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Demo {
    pub id: String,
    pub title: String,
    pub question: String,
    pub varied: Vec<String>,
    pub controlled: Vec<String>,
    pub how_to_read: String,
    pub runs: Vec<DemoRun>,
    pub reproduce: Vec<String>,
}
