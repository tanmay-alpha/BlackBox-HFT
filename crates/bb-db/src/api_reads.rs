//! Read models of `/api/v1` (API_CONTRACT §8–§9): joined views, quota usage, queue statistics and
//! analytics rows, plus the fair claim (SEC-8), the Blob read budget (SEC-11) and the egress
//! counters (SEC-12). Additive to the repositories; reads are workspace-scoped where the contract
//! requires it.
use crate::repositories::{hash, limit};
use crate::*;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Submissions counted against the daily and active-job quotas (API_CONTRACT §5, `whoami`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct QuotaUsage {
    pub experiments_today: i64,
    pub sweeps_today: i64,
    pub active_jobs: i64,
}

/// Queue counters for `/api/v1/status` and `/system/queue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct QueueStats {
    pub queued: i64,
    pub running: i64,
    pub oldest_queued_at: Option<DateTime<Utc>>,
    pub lease_reclaims_24h: i64,
}

/// A run joined with its identity, dataset version and the experiment that makes it visible to
/// the caller (own workspace first, then the public demo workspace).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunView {
    #[sqlx(flatten)]
    pub run: Run,
    pub short_id: String,
    pub fingerprint: Vec<u8>,
    pub dataset_id: String,
    pub dataset_version_no: i32,
    pub events_sha256: Vec<u8>,
    pub origin_ns: i64,
    pub strategy_kind: Option<String>,
    pub interval_ns: Option<i64>,
    pub contract_version: i32,
    pub simulation_revision: i32,
    pub engine_name: String,
    pub engine_version: String,
    pub config: Value,
    pub view_experiment_id: Option<Uuid>,
    pub view_label: Option<String>,
}

/// Orderings of `GET /runs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunSort {
    #[default]
    CreatedDesc,
    NetPnlDesc,
    NetPnlAsc,
    MaxDrawdownAsc,
}

/// Keyset position for [`RunSort`]: `value` is the sort column (absent for `CreatedDesc`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunKey {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub value: Option<f64>,
}
impl RunView {
    pub fn key(&self, sort: RunSort) -> RunKey {
        RunKey {
            created_at: self.run.created_at,
            id: self.run.id,
            value: match sort {
                RunSort::CreatedDesc => None,
                RunSort::NetPnlDesc | RunSort::NetPnlAsc => Some(self.run.net_pnl.unwrap_or(0.0)),
                RunSort::MaxDrawdownAsc => Some(self.run.max_drawdown.unwrap_or(0.0)),
            },
        }
    }
}

/// Filters shared by `GET /experiments` and `GET /runs` (API_CONTRACT §8.3/§8.4).
#[derive(Debug, Clone, Default)]
pub struct ExperimentQuery {
    pub dataset_id: Option<String>,
    pub strategy_kind: Option<String>,
    /// Derived experiment status (`created` … `expired`, `completed`, `detached`).
    pub states: Vec<String>,
    pub sweep_id: Option<Uuid>,
    pub label_prefix: Option<String>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub created_by_key: Option<Uuid>,
    /// 16-hex short id or 64-hex fingerprint of the experiment identity.
    pub run_identity: Option<String>,
}

/// An experiment joined with its config identity, live run and creating key.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExperimentView {
    #[sqlx(flatten)]
    pub experiment: Experiment,
    pub workspace_slug: String,
    pub fingerprint: Vec<u8>,
    pub short_id: String,
    pub dataset_version_no: i32,
    pub interval_ns: Option<i64>,
    pub est_ticks: i64,
    pub est_tape_events_upper: i64,
    pub est_output_bytes_upper: i64,
    pub config: Value,
    pub key_prefix: Option<String>,
    pub key_label: Option<String>,
    pub live_run_id: Option<Uuid>,
    pub job_state: Option<String>,
    /// `completed` when a live run exists, `cancelled` once this workspace asked to cancel,
    /// otherwise the job state, or `detached` without a job.
    pub status: String,
}

/// A job joined with its experiment identity and run.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JobView {
    #[sqlx(flatten)]
    pub job: Job,
    pub fingerprint: Vec<u8>,
    pub short_id: String,
    pub run_short_id: Option<String>,
}

/// One run of a sweep cell (keyed by cell index and segment) with the state of its job and the
/// metrics of its live run.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SweepCellView {
    pub experiment_id: Uuid,
    pub cell: i16,
    /// `full`, `in_sample` or `out_of_sample`.
    pub segment: String,
    pub job_id: Option<Uuid>,
    pub job_state: Option<String>,
    pub error_message: Option<String>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    /// Row id of the live run (for opening its bundle).
    pub run_uuid: Option<Uuid>,
    pub run_short_id: Option<String>,
    /// The run's bb-core `Metrics`.
    pub metrics: Option<Value>,
    pub net_pnl: Option<f64>,
    pub fees: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub fill_count: Option<i64>,
}

/// Stored analytics report: the exact JSON bytes and their digest.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AnalyticsRow {
    pub analytics_version: String,
    pub report_json: String,
    pub report_sha256: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

/// A visible run identity found by fingerprint (demo catalogue).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VisibleIdentity {
    pub fingerprint: Vec<u8>,
    pub short_id: String,
    pub experiment_id: Uuid,
}

const RUN_VIEW: &str = "SELECT r.*,c.short_id,c.fingerprint,c.dataset_id,v.version_no AS dataset_version_no,v.events_sha256,v.origin_ns,c.strategy_kind,c.interval_ns,c.contract_version,c.simulation_revision,c.engine_name,c.engine_version,c.config,e.id AS view_experiment_id,e.label AS view_label FROM bb.runs r JOIN bb.experiment_configs c ON c.id=r.experiment_config_id JOIN bb.dataset_versions v ON v.id=c.dataset_version_id LEFT JOIN LATERAL (SELECT e.id,e.label FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.experiment_config_id=r.experiment_config_id AND e.platform=r.platform AND (e.workspace_id=$1 OR w.is_public_read) ORDER BY (e.workspace_id=$1) DESC,(e.sweep_id IS NULL) DESC,e.created_at,e.id LIMIT 1) e ON true";
// Filters for run listings: $1 workspace, $2 platform, $3 dataset, $4 strategy, $5 sweep,
// $6 label prefix, $7 created after, $8 created before, $9 key, $10 identity.
const RUN_LIST_FILTER: &str = " WHERE r.platform=$2 AND r.quarantined_at IS NULL AND EXISTS (SELECT 1 FROM bb.experiments f WHERE f.workspace_id=$1 AND f.experiment_config_id=r.experiment_config_id AND f.platform=r.platform AND ($5::uuid IS NULL OR f.sweep_id=$5) AND ($6::text IS NULL OR starts_with(f.label,$6)) AND ($9::uuid IS NULL OR f.created_by_key_id=$9)) AND ($3::text IS NULL OR c.dataset_id=$3) AND ($4::text IS NULL OR c.strategy_kind=$4) AND ($7::timestamptz IS NULL OR r.created_at>=$7) AND ($8::timestamptz IS NULL OR r.created_at<$8) AND ($10::text IS NULL OR c.short_id=$10 OR encode(c.fingerprint,'hex')=$10)";
const EXPERIMENT_VIEW: &str = "SELECT e.*,w.slug AS workspace_slug,c.fingerprint,c.short_id,v.version_no AS dataset_version_no,c.interval_ns,c.est_ticks,c.est_tape_events_upper,c.est_output_bytes_upper,c.config,k.display_prefix AS key_prefix,k.label AS key_label,r.id AS live_run_id,j.state AS job_state,CASE WHEN r.id IS NOT NULL THEN 'completed' WHEN e.cancel_requested_at IS NOT NULL THEN 'cancelled' WHEN j.id IS NULL THEN 'detached' ELSE j.state END AS status FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id JOIN bb.experiment_configs c ON c.id=e.experiment_config_id JOIN bb.dataset_versions v ON v.id=c.dataset_version_id LEFT JOIN bb.api_keys k ON k.id=e.created_by_key_id LEFT JOIN bb.jobs j ON j.id=e.job_id LEFT JOIN bb.runs r ON r.experiment_config_id=e.experiment_config_id AND r.platform=e.platform AND r.quarantined_at IS NULL";
const JOB_VIEW: &str = "SELECT j.*,c.fingerprint,c.short_id,rc.short_id AS run_short_id FROM bb.jobs j JOIN bb.experiment_configs c ON c.id=j.experiment_config_id LEFT JOIN bb.runs r ON r.id=j.run_id LEFT JOIN bb.experiment_configs rc ON rc.id=r.experiment_config_id";

fn sort_sql(sort: RunSort) -> (&'static str, &'static str) {
    // (keyset predicate over $11 created_at, $12 id, $13 value; ORDER BY)
    match sort {
        RunSort::CreatedDesc => (
            " AND ($11::timestamptz IS NULL OR (r.created_at,r.id)<($11,$12))",
            " ORDER BY r.created_at DESC,r.id DESC LIMIT $14",
        ),
        RunSort::NetPnlDesc => (
            " AND ($13::float8 IS NULL OR (coalesce(r.net_pnl,0),r.id)<($13,$12))",
            " ORDER BY coalesce(r.net_pnl,0) DESC,r.id DESC LIMIT $14",
        ),
        RunSort::NetPnlAsc => (
            " AND ($13::float8 IS NULL OR (coalesce(r.net_pnl,0),r.id)>($13,$12))",
            " ORDER BY coalesce(r.net_pnl,0),r.id LIMIT $14",
        ),
        RunSort::MaxDrawdownAsc => (
            " AND ($13::float8 IS NULL OR (coalesce(r.max_drawdown,0),r.id)>($13,$12))",
            " ORDER BY coalesce(r.max_drawdown,0),r.id LIMIT $14",
        ),
    }
}

fn egress_class(class: &str) -> Result<()> {
    if !matches!(class, "anonymous" | "keyed") {
        return Err(Error::Invalid(
            "egress class must be anonymous or keyed".into(),
        ));
    }
    Ok(())
}

impl Database {
    pub async fn dataset_version_by_id(&self, id: Uuid) -> Result<DatasetVersion> {
        sqlx::query_as("SELECT * FROM bb.dataset_versions WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn workspace_by_id(&self, id: Uuid) -> Result<Workspace> {
        sqlx::query_as("SELECT * FROM bb.workspaces WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn api_key(&self, id: Uuid) -> Result<ApiKey> {
        sqlx::query_as("SELECT * FROM bb.api_keys WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn all_api_keys(&self) -> Result<Vec<ApiKey>> {
        Ok(
            sqlx::query_as("SELECT * FROM bb.api_keys ORDER BY created_at DESC,id DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    /// [`Database::claim`] with fair ordering (SEC-8): priority first, then the submitting
    /// workspace with the fewest running/cancelling jobs, then the workspace whose latest claim
    /// is oldest (never claimed first), then queue order.
    pub async fn claim_fair(
        &self,
        worker: &str,
        platform: &str,
        lease_seconds: i32,
    ) -> Result<Option<Job>> {
        if worker.is_empty() || lease_seconds <= 0 {
            return Err(Error::Invalid(
                "worker and positive lease duration required".into(),
            ));
        }
        Ok(sqlx::query_as("WITH claimed AS (UPDATE bb.jobs j SET state='running',claim_count=j.claim_count+1,lease_owner=$1,lease_expires_at=now()+make_interval(secs=>$2::double precision),heartbeat_at=now(),started_at=coalesce(j.started_at,now()) WHERE j.id=(SELECT q.id FROM bb.jobs q WHERE q.state='queued' AND q.platform=$3 AND q.run_after<=now() AND q.expires_at>now() ORDER BY q.priority DESC,(SELECT count(*) FROM bb.jobs l WHERE l.submitted_by_workspace_id IS NOT DISTINCT FROM q.submitted_by_workspace_id AND l.state IN ('running','cancelling')),(SELECT max(t.claimed_at) FROM bb.job_attempts t JOIN bb.jobs b ON b.id=t.job_id WHERE b.submitted_by_workspace_id IS NOT DISTINCT FROM q.submitted_by_workspace_id) ASC NULLS FIRST,q.queued_at,q.id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING j.*), att AS (INSERT INTO bb.job_attempts (job_id,attempt_no,worker_id) SELECT id,claim_count,lease_owner FROM claimed) SELECT * FROM claimed")
            .bind(worker).bind(f64::from(lease_seconds)).bind(platform).fetch_optional(&self.pool).await?)
    }

    // ----- budgets -----

    /// Reserves one Blob read op and `bytes` before a GET/HEAD attempt (SEC-11). `false` (nothing
    /// charged) once the month's reads or bytes would exceed their caps, or today's reads would
    /// exceed the daily pacing share `(monthly_reads_cap - reads before today) / days left`.
    pub async fn charge_storage_read(
        &self,
        backend: &str,
        bytes: i64,
        monthly_reads_cap: i32,
        monthly_bytes_cap: i64,
    ) -> Result<bool> {
        self.charge_storage_read_on(
            backend,
            bytes,
            monthly_reads_cap,
            monthly_bytes_cap,
            Utc::now().date_naive(),
        )
        .await
    }
    /// [`Database::charge_storage_read`] for an explicit UTC day (tests).
    #[doc(hidden)]
    pub async fn charge_storage_read_on(
        &self,
        backend: &str,
        bytes: i64,
        monthly_reads_cap: i32,
        monthly_bytes_cap: i64,
        today: NaiveDate,
    ) -> Result<bool> {
        if bytes < 0 || monthly_reads_cap < 0 || monthly_bytes_cap < 0 {
            return Err(Error::Invalid("storage budget values must be >= 0".into()));
        }
        let month = today.with_day(1).expect("day 1 exists");
        let next = month
            .checked_add_months(chrono::Months::new(1))
            .ok_or_else(|| Error::Invalid("date out of range".into()))?;
        let days_left = (next - today).num_days();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO bb.storage_usage_monthly (month,storage_backend) VALUES ($1,$2) ON CONFLICT DO NOTHING")
            .bind(month).bind(backend).execute(&mut *tx).await?;
        let (month_reads, month_bytes): (i32, i64) = sqlx::query_as("SELECT reads,read_bytes FROM bb.storage_usage_monthly WHERE month=$1 AND storage_backend=$2 FOR UPDATE")
            .bind(month).bind(backend).fetch_one(&mut *tx).await?;
        let today_reads: i32 = sqlx::query_scalar("SELECT coalesce((SELECT reads FROM bb.storage_reads_daily WHERE day=$1 AND storage_backend=$2),0)")
            .bind(today).bind(backend).fetch_one(&mut *tx).await?;
        let before_today = i64::from(month_reads) - i64::from(today_reads);
        let daily_cap = ((i64::from(monthly_reads_cap) - before_today) / days_left).max(1);
        if i64::from(month_reads) + 1 > i64::from(monthly_reads_cap)
            || month_bytes.saturating_add(bytes) > monthly_bytes_cap
            || i64::from(today_reads) + 1 > daily_cap
        {
            return Ok(false);
        }
        sqlx::query("UPDATE bb.storage_usage_monthly SET reads=reads+1,read_bytes=read_bytes+$3 WHERE month=$1 AND storage_backend=$2")
            .bind(month).bind(backend).bind(bytes).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO bb.storage_reads_daily (day,storage_backend,reads,read_bytes) VALUES ($1,$2,1,$3) ON CONFLICT (day,storage_backend) DO UPDATE SET reads=bb.storage_reads_daily.reads+1,read_bytes=bb.storage_reads_daily.read_bytes+excluded.read_bytes")
            .bind(today).bind(backend).bind(bytes).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
    /// Adds response bytes to today's (UTC) egress counter of `class` (`anonymous` or `keyed`)
    /// and returns the new total (SEC-12).
    pub async fn add_egress(&self, class: &str, bytes: i64) -> Result<i64> {
        egress_class(class)?;
        if bytes < 0 {
            return Err(Error::Invalid("egress bytes must be >= 0".into()));
        }
        Ok(sqlx::query_scalar("INSERT INTO bb.egress_usage_daily (day,class,bytes) VALUES ((now() AT TIME ZONE 'UTC')::date,$1,$2) ON CONFLICT (day,class) DO UPDATE SET bytes=bb.egress_usage_daily.bytes+excluded.bytes RETURNING bytes")
            .bind(class).bind(bytes).fetch_one(&self.pool).await?)
    }
    /// Today's (UTC) egress bytes of `class`.
    pub async fn egress_today(&self, class: &str) -> Result<i64> {
        egress_class(class)?;
        Ok(sqlx::query_scalar("SELECT coalesce((SELECT bytes FROM bb.egress_usage_daily WHERE day=(now() AT TIME ZONE 'UTC')::date AND class=$1),0)")
            .bind(class).fetch_one(&self.pool).await?)
    }
    pub async fn quota_usage(&self, workspace: Uuid, key: Option<Uuid>) -> Result<QuotaUsage> {
        Ok(sqlx::query_as("SELECT (SELECT count(*) FROM bb.experiments WHERE workspace_id=$1 AND ($2::uuid IS NULL OR created_by_key_id=$2) AND created_at>=date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS experiments_today,(SELECT count(*) FROM bb.sweeps WHERE workspace_id=$1 AND ($2::uuid IS NULL OR created_by_key_id=$2) AND created_at>=date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS sweeps_today,(SELECT count(DISTINCT e.job_id) FROM bb.experiments e JOIN bb.jobs j ON j.id=e.job_id WHERE e.workspace_id=$1 AND e.cancel_requested_at IS NULL AND j.state IN ('created','queued','running','cancelling')) AS active_jobs")
            .bind(workspace).bind(key).fetch_one(&self.pool).await?)
    }
    /// `platform = None` counts every platform.
    pub async fn queue_stats(&self, platform: Option<&str>) -> Result<QueueStats> {
        Ok(sqlx::query_as("SELECT (SELECT count(*) FROM bb.jobs WHERE state='queued' AND ($1::text IS NULL OR platform=$1)) AS queued,(SELECT count(*) FROM bb.jobs WHERE state IN ('running','cancelling') AND ($1::text IS NULL OR platform=$1)) AS running,(SELECT min(queued_at) FROM bb.jobs WHERE state='queued' AND ($1::text IS NULL OR platform=$1)) AS oldest_queued_at,(SELECT count(*) FROM bb.system_events WHERE kind='lease_reclaimed' AND at>now()-interval '24 hours') AS lease_reclaims_24h")
            .bind(platform).fetch_one(&self.pool).await?)
    }
    pub async fn data_source(&self, id: &str) -> Result<DataSource> {
        sqlx::query_as("SELECT * FROM bb.data_sources WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn instrument(&self, id: Uuid) -> Result<Instrument> {
        sqlx::query_as("SELECT * FROM bb.instruments WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }

    // ----- runs -----

    pub async fn run_views(
        &self,
        workspace: Uuid,
        platform: &str,
        f: &ExperimentQuery,
        sort: RunSort,
        after: Option<&RunKey>,
        count: i64,
    ) -> Result<Vec<RunView>> {
        let (keyset, order) = sort_sql(sort);
        let sql = format!("{RUN_VIEW}{RUN_LIST_FILTER}{keyset}{order}");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(platform)
            .bind(&f.dataset_id)
            .bind(&f.strategy_kind)
            .bind(f.sweep_id)
            .bind(&f.label_prefix)
            .bind(f.created_after)
            .bind(f.created_before)
            .bind(f.created_by_key)
            .bind(&f.run_identity)
            .bind(after.map(|k| k.created_at))
            .bind(after.map(|k| k.id))
            .bind(after.and_then(|k| k.value))
            .bind(limit(count)?)
            .fetch_all(&self.pool)
            .await?)
    }
    /// The live run of an identity (16-hex short id or 64-hex fingerprint) on `platform`,
    /// visible in `workspace` or the public demo workspace.
    pub async fn run_view(
        &self,
        workspace: Uuid,
        identity: &str,
        platform: &str,
    ) -> Result<RunView> {
        let sql = format!(
            "{RUN_VIEW} WHERE (c.short_id=$2 OR encode(c.fingerprint,'hex')=$2) AND r.platform=$3 AND r.quarantined_at IS NULL AND e.id IS NOT NULL"
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(identity)
            .bind(platform)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    /// A run by row id (comparisons, reports), visible in `workspace` or demo; live only.
    pub async fn run_view_by_id(&self, workspace: Uuid, id: Uuid) -> Result<RunView> {
        let sql =
            format!("{RUN_VIEW} WHERE r.id=$2 AND r.quarantined_at IS NULL AND e.id IS NOT NULL");
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn run_views_by_ids(&self, workspace: Uuid, ids: &[Uuid]) -> Result<Vec<RunView>> {
        let sql = format!(
            "{RUN_VIEW} WHERE r.id=ANY($2::uuid[]) AND r.quarantined_at IS NULL AND e.id IS NOT NULL"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(ids)
            .fetch_all(&self.pool)
            .await?)
    }
    /// The most recently quarantined run of a visible identity, when no live run exists: reads
    /// of a run that failed verification keep answering `integrity_failed`.
    pub async fn quarantined_run(
        &self,
        workspace: Uuid,
        identity: &str,
        platform: &str,
    ) -> Result<Option<Run>> {
        Ok(sqlx::query_as("SELECT r.* FROM bb.runs r JOIN bb.experiment_configs c ON c.id=r.experiment_config_id WHERE (c.short_id=$2 OR encode(c.fingerprint,'hex')=$2) AND r.platform=$3 AND r.quarantined_at IS NOT NULL AND NOT EXISTS (SELECT 1 FROM bb.runs l WHERE l.experiment_config_id=r.experiment_config_id AND l.platform=r.platform AND l.quarantined_at IS NULL) AND EXISTS (SELECT 1 FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.experiment_config_id=c.id AND e.platform=r.platform AND (e.workspace_id=$1 OR w.is_public_read)) ORDER BY r.quarantined_at DESC LIMIT 1")
            .bind(workspace).bind(identity).bind(platform).fetch_optional(&self.pool).await?)
    }
    /// Visible live runs among the given identities (fingerprints), e.g. the demo catalogue.
    pub async fn visible_identities(
        &self,
        workspace: Uuid,
        fingerprints: &[Vec<u8>],
        platform: &str,
    ) -> Result<Vec<VisibleIdentity>> {
        Ok(sqlx::query_as("SELECT DISTINCT ON (c.id) c.fingerprint,c.short_id,e.id AS experiment_id FROM bb.experiment_configs c JOIN bb.runs r ON r.experiment_config_id=c.id AND r.platform=$3 AND r.quarantined_at IS NULL JOIN bb.experiments e ON e.experiment_config_id=c.id AND e.platform=r.platform JOIN bb.workspaces w ON w.id=e.workspace_id WHERE c.fingerprint=ANY($2::bytea[]) AND (e.workspace_id=$1 OR w.is_public_read) ORDER BY c.id,(e.workspace_id=$1) DESC,e.created_at,e.id")
            .bind(workspace).bind(fingerprints).bind(platform).fetch_all(&self.pool).await?)
    }

    // ----- experiments -----

    pub async fn experiment_views(
        &self,
        workspace: Uuid,
        q: &ExperimentQuery,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<ExperimentView>> {
        let sql = format!(
            "SELECT * FROM ({EXPERIMENT_VIEW} WHERE e.workspace_id=$1 AND ($2::text IS NULL OR e.dataset_id=$2) AND ($3::text IS NULL OR e.strategy_kind=$3) AND ($4::uuid IS NULL OR e.sweep_id=$4) AND ($5::text IS NULL OR starts_with(e.label,$5)) AND ($6::timestamptz IS NULL OR e.created_at>=$6) AND ($7::timestamptz IS NULL OR e.created_at<$7) AND ($8::uuid IS NULL OR e.created_by_key_id=$8) AND ($9::text IS NULL OR c.short_id=$9 OR encode(c.fingerprint,'hex')=$9) AND ($10::timestamptz IS NULL OR (e.created_at,e.id)<($10,$11))) x WHERE cardinality($12::text[])=0 OR x.status=ANY($12) ORDER BY x.created_at DESC,x.id DESC LIMIT $13"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(&q.dataset_id)
            .bind(&q.strategy_kind)
            .bind(q.sweep_id)
            .bind(&q.label_prefix)
            .bind(q.created_after)
            .bind(q.created_before)
            .bind(q.created_by_key)
            .bind(&q.run_identity)
            .bind(cursor.map(|c| c.created_at))
            .bind(cursor.map(|c| c.id))
            .bind(&q.states)
            .bind(limit(count)?)
            .fetch_all(&self.pool)
            .await?)
    }
    /// One experiment visible in `workspace` or the public demo workspace.
    pub async fn experiment_view(&self, workspace: Uuid, id: Uuid) -> Result<ExperimentView> {
        let sql =
            format!("{EXPERIMENT_VIEW} WHERE e.id=$2 AND (e.workspace_id=$1 OR w.is_public_read)");
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }

    // ----- jobs -----

    pub async fn job_views(
        &self,
        workspace: Uuid,
        states: &[String],
        mine: Option<Uuid>,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<JobView>> {
        let sql = format!(
            "{JOB_VIEW} WHERE EXISTS (SELECT 1 FROM bb.experiments e WHERE e.job_id=j.id AND e.workspace_id=$1 AND ($3::uuid IS NULL OR e.created_by_key_id=$3)) AND (cardinality($2::text[])=0 OR j.state=ANY($2)) AND ($4::timestamptz IS NULL OR (j.created_at,j.id)<($4,$5)) ORDER BY j.created_at DESC,j.id DESC LIMIT $6"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(states)
            .bind(mine)
            .bind(cursor.map(|c| c.created_at))
            .bind(cursor.map(|c| c.id))
            .bind(limit(count)?)
            .fetch_all(&self.pool)
            .await?)
    }
    /// One job linked to an experiment of `workspace` or the public demo workspace.
    pub async fn job_view(&self, workspace: Uuid, id: Uuid) -> Result<JobView> {
        let sql = format!(
            "{JOB_VIEW} WHERE j.id=$2 AND EXISTS (SELECT 1 FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.job_id=j.id AND (e.workspace_id=$1 OR w.is_public_read))"
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(workspace)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    /// Jobs by id without visibility checks: callers pass ids taken from visible experiments.
    pub async fn job_views_by_ids(&self, ids: &[Uuid]) -> Result<Vec<JobView>> {
        let sql = format!("{JOB_VIEW} WHERE j.id=ANY($1::uuid[])");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(ids)
            .fetch_all(&self.pool)
            .await?)
    }

    // ----- sweeps -----

    pub async fn sweep_cell_views(&self, sweep: Uuid) -> Result<Vec<SweepCellView>> {
        Ok(sqlx::query_as("SELECT e.id AS experiment_id,e.sweep_cell AS cell,e.sweep_segment AS segment,e.job_id,j.state AS job_state,j.error_message,e.cancel_requested_at,r.id AS run_uuid,CASE WHEN r.id IS NOT NULL THEN c.short_id END AS run_short_id,r.metrics,r.net_pnl,r.fees,r.max_drawdown,r.fill_count FROM bb.experiments e JOIN bb.experiment_configs c ON c.id=e.experiment_config_id LEFT JOIN bb.jobs j ON j.id=e.job_id LEFT JOIN bb.runs r ON r.experiment_config_id=e.experiment_config_id AND r.platform=e.platform AND r.quarantined_at IS NULL WHERE e.sweep_id=$1 ORDER BY e.sweep_cell,e.sweep_segment")
            .bind(sweep).fetch_all(&self.pool).await?)
    }

    // ----- analytics -----

    /// Stores a run's analytics once; a second call with other bytes is a conflict.
    pub async fn put_run_analytics(
        &self,
        run: Uuid,
        version: &str,
        report_json: &str,
        sha256: &[u8],
    ) -> Result<AnalyticsRow> {
        hash(sha256)?;
        sqlx::query("INSERT INTO bb.run_analytics (run_id,analytics_version,report_json,report_sha256) VALUES ($1,$2,$3,$4) ON CONFLICT (run_id) DO NOTHING")
            .bind(run).bind(version).bind(report_json).bind(sha256).execute(&self.pool).await?;
        let row = self.run_analytics(run).await?.ok_or(Error::NotFound)?;
        if row.report_sha256 != sha256 || row.analytics_version != version {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn run_analytics(&self, run: Uuid) -> Result<Option<AnalyticsRow>> {
        Ok(sqlx::query_as("SELECT analytics_version,report_json,report_sha256,created_at FROM bb.run_analytics WHERE run_id=$1")
            .bind(run).fetch_optional(&self.pool).await?)
    }
    pub async fn put_dataset_analytics(
        &self,
        version_id: Uuid,
        version: &str,
        report_json: &str,
        sha256: &[u8],
    ) -> Result<AnalyticsRow> {
        hash(sha256)?;
        sqlx::query("INSERT INTO bb.dataset_analytics (dataset_version_id,analytics_version,report_json,report_sha256) VALUES ($1,$2,$3,$4) ON CONFLICT (dataset_version_id,analytics_version) DO NOTHING")
            .bind(version_id).bind(version).bind(report_json).bind(sha256).execute(&self.pool).await?;
        let row = self
            .dataset_analytics(version_id, version)
            .await?
            .ok_or(Error::NotFound)?;
        if row.report_sha256 != sha256 {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn dataset_analytics(
        &self,
        version_id: Uuid,
        version: &str,
    ) -> Result<Option<AnalyticsRow>> {
        Ok(sqlx::query_as("SELECT analytics_version,report_json,report_sha256,created_at FROM bb.dataset_analytics WHERE dataset_version_id=$1 AND analytics_version=$2")
            .bind(version_id).bind(version).fetch_optional(&self.pool).await?)
    }
}
