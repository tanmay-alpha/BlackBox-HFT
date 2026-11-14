use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Workspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub is_public_read: bool,
    pub quota_experiments_per_day: i32,
    pub quota_sweeps_per_day: i32,
    pub quota_active_jobs: i32,
    /// Vercel Blob upload attempts this workspace may spend per month (SEC-8/SEC-11).
    pub quota_uploads_per_month: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(skip_serializing)]
    pub key_sha256: Vec<u8>,
    pub display_prefix: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DataSource {
    pub id: String,
    pub vendor: String,
    pub licence: String,
    pub licence_url: Option<String>,
    pub redistribution_allowed: bool,
    pub hostable: bool,
    pub notes: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Instrument {
    pub id: Uuid,
    pub exchange: String,
    pub symbol: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Dataset {
    pub id: String,
    pub owner_workspace_id: Option<Uuid>,
    pub title: String,
    pub kind: String,
    pub level: String,
    pub instrument_id: Uuid,
    pub data_source_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DatasetVersion {
    pub id: Uuid,
    pub dataset_id: String,
    pub version_no: i32,
    pub events_sha256: Vec<u8>,
    pub normalization_version: i32,
    pub contract_version: i32,
    pub tick_size: f64,
    pub lot_size: f64,
    pub contract_size: f64,
    pub origin_ns: i64,
    pub start_rel_ns: i64,
    pub end_rel_ns: i64,
    pub timestamp_unit: String,
    pub row_count: i64,
    pub events_bytes: i64,
    pub storage: String,
    pub storage_key: Option<String>,
    pub manifest: Value,
    pub created_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct QualityReport {
    pub dataset_version_id: Uuid,
    pub checker_version: i32,
    pub passed: bool,
    pub error_count: i32,
    pub warning_count: i32,
    pub book_valid_fraction: f64,
    pub report: Value,
    pub report_sha256: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExperimentConfig {
    pub id: Uuid,
    pub fingerprint: Vec<u8>,
    pub short_id: String,
    pub dataset_version_id: Uuid,
    pub dataset_id: String,
    pub contract_version: i32,
    pub simulation_revision: i32,
    pub engine_name: String,
    pub engine_version: String,
    pub config: Value,
    pub strategy_kind: Option<String>,
    pub interval_ns: Option<i64>,
    pub est_ticks: i64,
    pub est_tape_events_upper: i64,
    pub est_output_bytes_upper: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Run {
    pub id: Uuid,
    pub experiment_config_id: Uuid,
    pub platform: String,
    pub is_canonical: bool,
    pub blackbox_version: String,
    pub build_profile: String,
    pub worker_id: String,
    pub result_sha256: Vec<u8>,
    pub summary_sha256: Vec<u8>,
    /// Postgres digest of the stored summary fields (`bb.run_summary_digest`), written at insert.
    pub summary_row_sha256: Vec<u8>,
    pub ticks: i64,
    pub order_count: i64,
    pub fill_count: i64,
    pub tape_len: i64,
    pub equity_len: i64,
    pub wall_time_ms: i64,
    pub peak_rss_bytes: Option<i64>,
    pub metrics: Value,
    pub engine_check: Value,
    pub net_pnl: Option<f64>,
    pub fees: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub orders_submitted: Option<i64>,
    pub engine_check_agrees: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub quarantined_at: Option<DateTime<Utc>>,
    pub quarantine_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RunArtifact {
    pub id: Uuid,
    pub run_id: Uuid,
    pub kind: String,
    pub format_version: i32,
    pub storage_backend: String,
    pub storage_key: String,
    pub sha256: Vec<u8>,
    pub size_bytes: i64,
    pub uncompressed_bytes: i64,
    pub page_index: Value,
    pub page_index_sha256: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BlobGc {
    pub storage_backend: String,
    pub storage_key: String,
    pub reason: String,
    pub not_before: DateTime<Utc>,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StorageUsage {
    pub month: NaiveDate,
    pub storage_backend: String,
    pub uploads: i32,
    pub reads: i32,
    pub bytes_uploaded: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Sweep {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub spec: Value,
    pub spec_fingerprint: Vec<u8>,
    pub cell_count: i16,
    pub run_count: i16,
    /// `spec.design.type`: `grid`, `random` or `seeds`.
    pub design: Option<String>,
    pub label: String,
    pub created_by_key_id: Option<Uuid>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub experiment_config_id: Uuid,
    pub platform: String,
    pub state: String,
    pub priority: i16,
    pub submitted_by_workspace_id: Option<Uuid>,
    pub submitted_by_key_id: Option<Uuid>,
    pub run_after: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub deadline_ms: i32,
    pub claim_count: i16,
    pub lease_losses: i16,
    pub max_attempts: i16,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub ticks_done: i64,
    pub ticks_total: i64,
    pub tape_events: i64,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub error_category: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub expired_reason: Option<String>,
    pub run_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub queued_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct JobAttempt {
    pub job_id: Uuid,
    pub attempt_no: i16,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub outcome: Option<String>,
    pub error_code: Option<String>,
    pub peak_rss_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Experiment {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub experiment_config_id: Uuid,
    pub dataset_id: String,
    pub strategy_kind: String,
    pub platform: String,
    pub label: String,
    pub sweep_id: Option<Uuid>,
    /// Cell index in the sweep spec's cell order.
    pub sweep_cell: Option<i16>,
    /// `full`, `in_sample` or `out_of_sample`.
    pub sweep_segment: Option<String>,
    pub job_id: Option<Uuid>,
    pub created_by_key_id: Option<Uuid>,
    pub pinned: bool,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Comparison {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub run_a_id: Uuid,
    pub run_b_id: Uuid,
    pub comparator_version: i32,
    pub eligibility: String,
    pub reasons: Value,
    pub summary: Value,
    pub summary_sha256: Vec<u8>,
    pub label: String,
    pub created_by_key_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
    /// Read paths only (`Database::comparison`, `Database::comparisons`): whether the row still
    /// matches its `summary_row_sha256` (FINAL_AUDIT §4 item 5). `None` where not selected.
    #[sqlx(default)]
    #[serde(default)]
    pub summary_intact: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ResearchReport {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    pub run_id: Option<Uuid>,
    pub comparison_id: Option<Uuid>,
    pub sweep_id: Option<Uuid>,
    pub title: String,
    pub generator_version: i32,
    pub format: String,
    pub body: String,
    pub body_sha256: Vec<u8>,
    pub created_by_key_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WorkerHeartbeat {
    pub worker_id: String,
    pub hostname: String,
    pub pid: i32,
    pub blackbox_version: String,
    pub platform: String,
    pub mode: String,
    pub slots: i16,
    pub state: String,
    pub current_job_id: Option<Uuid>,
    pub heartbeat_interval_ms: i32,
    pub started_at: DateTime<Utc>,
    pub last_heartbeat_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SystemEvent {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub level: String,
    pub kind: String,
    pub worker_id: Option<String>,
    pub job_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub message: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditEvent {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub workspace_id: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
    pub action: String,
    pub outcome: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub request_id: Option<String>,
    pub client_ip: Option<String>,
    pub detail: Value,
}
