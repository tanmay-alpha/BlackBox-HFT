use crate::repositories::{hash, limit};
use crate::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Created,
    Queued,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Expired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Api,
    Worker,
    Reaper,
}
impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
    pub fn transition(self, to: Self, actor: Actor) -> Result<Self> {
        use JobState::*;
        let legal = match (self, to) {
            (Created, Queued)
            | (Created, Cancelled)
            | (Queued, Cancelled)
            | (Running, Cancelling) => actor == Actor::Api,
            (Created, Expired) | (Queued, Expired) => actor == Actor::Reaper,
            (Queued, Running) | (Running, Completed) | (Running, Expired) => actor == Actor::Worker,
            (Running, Failed) | (Running, Queued) | (Cancelling, Cancelled) => {
                matches!(actor, Actor::Worker | Actor::Reaper)
            }
            _ => false,
        };
        if legal {
            Ok(to)
        } else {
            Err(Error::IllegalTransition)
        }
    }
}
impl TryFrom<&str> for JobState {
    type Error = Error;
    fn try_from(s: &str) -> Result<Self> {
        match s {
            "created" => Ok(Self::Created),
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "cancelling" => Ok(Self::Cancelling),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            _ => Err(Error::Invalid("unknown job state".into())),
        }
    }
}
/// Attempt number fences workers even when an identical worker id is reused after a restart.
#[derive(Debug, Clone)]
pub struct Lease {
    pub job_id: Uuid,
    pub worker_id: String,
    pub attempt_no: i16,
}
impl Job {
    pub fn lease(&self) -> Result<Lease> {
        if self.state != "running" && self.state != "cancelling" {
            return Err(Error::LeaseLost);
        }
        Ok(Lease {
            job_id: self.id,
            worker_id: self.lease_owner.clone().ok_or(Error::LeaseLost)?,
            attempt_no: self.claim_count,
        })
    }
}
#[derive(Debug, Clone)]
pub struct Submit {
    pub config_id: Uuid,
    pub workspace_id: Uuid,
    pub api_key_id: Option<Uuid>,
    pub platform: String,
    pub label: String,
    pub priority: i16,
    pub deadline_ms: i32,
    pub max_attempts: i16,
    pub prerequisites_ready: bool,
    pub sweep: Option<(Uuid, i16, i16)>,
    /// Privileged seeding is an explicit caller decision, never inferred from an absent key.
    pub bypass_quotas: bool,
    pub storage_backend: String,
}
impl Submit {
    pub fn new(config_id: Uuid, workspace_id: Uuid, platform: impl Into<String>) -> Self {
        Self {
            config_id,
            workspace_id,
            platform: platform.into(),
            api_key_id: None,
            label: String::new(),
            priority: 100,
            deadline_ms: 600_000,
            max_attempts: 3,
            sweep: None,
            bypass_quotas: false,
            prerequisites_ready: true,
            storage_backend: "fs".into(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Submission {
    pub created: bool,
    pub experiment: Experiment,
    pub job: Option<Job>,
    pub run: Option<Run>,
}
#[derive(Debug, Clone)]
pub struct CancelResult {
    pub job: Job,
    pub detached: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRun {
    pub id: Uuid,
    pub blackbox_version: String,
    pub build_profile: String,
    pub result_sha256: Vec<u8>,
    pub summary_sha256: Vec<u8>,
    pub ticks: i64,
    pub order_count: i64,
    pub fill_count: i64,
    pub tape_len: i64,
    pub equity_len: i64,
    pub wall_time_ms: i64,
    pub peak_rss_bytes: Option<i64>,
    pub metrics: Value,
    pub engine_check: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArtifact {
    pub format_version: i32,
    pub storage_backend: String,
    pub storage_key: String,
    pub sha256: Vec<u8>,
    pub size_bytes: i64,
    pub uncompressed_bytes: i64,
    /// Entire opaque serialized BundleTrust object, including header/trailer trust ranges.
    pub page_index: Value,
    /// SHA256 of the full stored trust JSON, not BundleTrust's internal pages-only digest.
    pub page_index_sha256: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct Completion {
    pub run: Run,
    pub reused: bool,
}
#[derive(Debug, Clone, Default)]
pub struct ExperimentFilter {
    pub dataset_id: Option<String>,
    pub strategy_kind: Option<String>,
    pub states: Vec<String>,
    pub sweep_id: Option<Uuid>,
    pub label_prefix: Option<String>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub created_by_key: Option<Uuid>,
}

async fn held(tx: &mut Transaction<'_, Postgres>, lease: &Lease, cancelling: bool) -> Result<Job> {
    sqlx::query_as("SELECT * FROM bb.jobs WHERE id=$1 AND lease_owner=$2 AND claim_count=$3 AND lease_expires_at>clock_timestamp() AND (state='running' OR ($4 AND state='cancelling')) FOR UPDATE")
        .bind(lease.job_id).bind(&lease.worker_id).bind(lease.attempt_no).bind(cancelling).fetch_optional(&mut **tx).await?.ok_or(Error::LeaseLost)
}
async fn close_attempt(
    tx: &mut Transaction<'_, Postgres>,
    job: &Job,
    outcome: &str,
    code: Option<&str>,
    rss: Option<i64>,
) -> Result<()> {
    let n=sqlx::query("UPDATE bb.job_attempts SET ended_at=now(),outcome=$3,error_code=$4,peak_rss_bytes=$5 WHERE job_id=$1 AND attempt_no=$2 AND ended_at IS NULL")
        .bind(job.id).bind(job.claim_count).bind(outcome).bind(code).bind(rss).execute(&mut **tx).await?.rows_affected();
    if n != 1 {
        return Err(Error::LeaseLost);
    }
    Ok(())
}

impl Database {
    pub async fn submit(&self, s: &Submit) -> Result<Submission> {
        let mut tx = self.pool.begin().await?;
        // Lock config first to serialize same-identity attachments across workspaces.
        let c: ExperimentConfig =
            sqlx::query_as("SELECT * FROM bb.experiment_configs WHERE id=$1 FOR NO KEY UPDATE")
                .bind(s.config_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound)?;
        let w: Workspace = sqlx::query_as("SELECT * FROM bb.workspaces WHERE id=$1 FOR UPDATE")
            .bind(s.workspace_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let visible: bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.datasets d JOIN bb.dataset_versions v ON v.dataset_id=d.id WHERE v.id=$1 AND v.retired_at IS NULL AND (d.owner_workspace_id IS NULL OR d.owner_workspace_id=$2))")
            .bind(c.dataset_version_id).bind(s.workspace_id).fetch_one(&mut *tx).await?;
        if !visible {
            return Err(Error::NotFound);
        }
        if let Some(key) = s.api_key_id {
            let valid: Option<Uuid>=sqlx::query_scalar("SELECT id FROM bb.api_keys WHERE id=$1 AND workspace_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now()) FOR UPDATE")
                .bind(key).bind(w.id).fetch_optional(&mut *tx).await?;
            if valid.is_none() {
                return Err(Error::NotFound);
            }
        }
        let sweep_id = s.sweep.map(|v| v.0);
        if let Some(id) = sweep_id {
            let owns: bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.sweeps WHERE id=$1 AND workspace_id=$2 AND cancel_requested_at IS NULL)").bind(id).bind(w.id).fetch_one(&mut *tx).await?;
            if !owns {
                return Err(Error::NotFound);
            }
        }
        let exists: bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.experiments WHERE workspace_id=$1 AND experiment_config_id=$2 AND platform=$3 AND sweep_id IS NOT DISTINCT FROM $4)")
            .bind(w.id).bind(c.id).bind(&s.platform).bind(sweep_id).fetch_one(&mut *tx).await?;
        if !s.bypass_quotas && !exists {
            let daily: i64=sqlx::query_scalar("SELECT count(*) FROM bb.experiments WHERE workspace_id=$1 AND ($2::uuid IS NULL OR created_by_key_id=$2) AND created_at>=date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'")
                .bind(w.id).bind(s.api_key_id).fetch_one(&mut *tx).await?;
            if daily >= i64::from(w.quota_experiments_per_day) {
                return Err(Error::Quota("experiments_per_day"));
            }
        }
        let mut run: Option<Run> = None;
        let mut job: Option<Job> = None;
        // Each read after ON CONFLICT is a distinct READ COMMITTED snapshot, not a CTE.
        for _ in 0..3 {
            run=sqlx::query_as("SELECT * FROM bb.runs WHERE experiment_config_id=$1 AND platform=$2 AND quarantined_at IS NULL")
                .bind(c.id).bind(&s.platform).fetch_optional(&mut *tx).await?;
            if run.is_some() {
                break;
            }
            if !s.bypass_quotas && s.storage_backend == "vercel_blob" {
                let uploads:i64=sqlx::query_scalar("SELECT coalesce(sum(uploads),0)::bigint FROM bb.storage_usage_monthly WHERE storage_backend='vercel_blob' AND month=date_trunc('month',now())::date").fetch_one(&mut *tx).await?;
                let bytes:i64=sqlx::query_scalar("SELECT coalesce(sum(size_bytes),0)::bigint FROM bb.run_artifacts WHERE storage_backend='vercel_blob'").fetch_one(&mut *tx).await?;
                if uploads >= 1800 || bytes.saturating_add(c.est_output_bytes_upper) > 900_000_000 {
                    return Err(Error::Quota("storage"));
                }
            }
            job=sqlx::query_as("INSERT INTO bb.jobs (experiment_config_id,platform,ticks_total,queued_at,priority,submitted_by_workspace_id,submitted_by_key_id,deadline_ms,max_attempts,state) VALUES ($1,$2,$3,CASE WHEN $9 THEN now() END,$4,$5,$6,$7,$8,CASE WHEN $9 THEN 'queued' ELSE 'created' END) ON CONFLICT (experiment_config_id,platform) WHERE state IN ('created','queued','running','cancelling') DO NOTHING RETURNING *")
                .bind(c.id).bind(&s.platform).bind(c.est_ticks).bind(s.priority).bind(w.id).bind(s.api_key_id).bind(s.deadline_ms).bind(s.max_attempts).bind(s.prerequisites_ready).fetch_optional(&mut *tx).await?;
            if job.is_none() {
                job=sqlx::query_as("SELECT * FROM bb.jobs WHERE experiment_config_id=$1 AND platform=$2 AND state IN ('created','queued','running','cancelling') FOR UPDATE")
                    .bind(c.id).bind(&s.platform).fetch_optional(&mut *tx).await?;
            }
            if job.is_some() {
                break;
            }
        }
        if job.is_none() && run.is_none() {
            return Err(Error::Conflict);
        }
        let job_id = job.as_ref().map(|j| j.id);
        if !s.bypass_quotas
            && let Some(id) = job_id
        {
            let already_counted:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.experiments WHERE workspace_id=$1 AND job_id=$2 AND cancel_requested_at IS NULL)")
                    .bind(w.id).bind(id).fetch_one(&mut *tx).await?;
            if !already_counted {
                let active:i64=sqlx::query_scalar("SELECT count(DISTINCT e.job_id) FROM bb.experiments e JOIN bb.jobs j ON j.id=e.job_id WHERE e.workspace_id=$1 AND e.cancel_requested_at IS NULL AND j.state IN ('created','queued','running','cancelling')")
                        .bind(w.id).fetch_one(&mut *tx).await?;
                if active >= i64::from(w.quota_active_jobs) {
                    return Err(Error::Quota("active_jobs"));
                }
            }
        }
        let (x, y) = s
            .sweep
            .map(|(_, x, y)| (Some(x), Some(y)))
            .unwrap_or((None, None));
        let experiment:Experiment=sqlx::query_as("INSERT INTO bb.experiments (workspace_id,experiment_config_id,dataset_id,strategy_kind,platform,label,job_id,created_by_key_id,sweep_id,sweep_x,sweep_y) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT ON CONSTRAINT experiments_idempotency DO UPDATE SET job_id=excluded.job_id,cancel_requested_at=NULL RETURNING *")
            .bind(w.id).bind(c.id).bind(&c.dataset_id).bind(&c.strategy_kind).bind(&s.platform).bind(&s.label).bind(job_id).bind(s.api_key_id).bind(sweep_id).bind(x).bind(y).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Submission {
            created: !exists,
            experiment,
            job,
            run,
        })
    }
    pub async fn queue_created(&self, id: Uuid) -> Result<Job> {
        sqlx::query_as("UPDATE bb.jobs SET state='queued',queued_at=now() WHERE id=$1 AND state='created' RETURNING *").bind(id).fetch_optional(&self.pool).await?.ok_or(Error::Conflict)
    }
    pub async fn claim(
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
        Ok(sqlx::query_as("WITH claimed AS (UPDATE bb.jobs j SET state='running',claim_count=j.claim_count+1,lease_owner=$1,lease_expires_at=now()+make_interval(secs=>$2::double precision),heartbeat_at=now(),started_at=coalesce(j.started_at,now()) WHERE j.id=(SELECT id FROM bb.jobs WHERE state='queued' AND platform=$3 AND run_after<=now() AND expires_at>now() ORDER BY priority DESC,queued_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING j.*), att AS (INSERT INTO bb.job_attempts (job_id,attempt_no,worker_id) SELECT id,claim_count,lease_owner FROM claimed) SELECT * FROM claimed")
            .bind(worker).bind(f64::from(lease_seconds)).bind(platform).fetch_optional(&self.pool).await?)
    }
    pub async fn heartbeat(
        &self,
        lease: &Lease,
        lease_seconds: i32,
        ticks: i64,
        tape: i64,
    ) -> Result<Job> {
        if lease_seconds <= 0 {
            return Err(Error::Invalid("lease duration must be positive".into()));
        }
        sqlx::query_as("UPDATE bb.jobs SET lease_expires_at=clock_timestamp()+make_interval(secs=>$4::double precision),heartbeat_at=clock_timestamp(),ticks_done=$5,tape_events=$6 WHERE id=$1 AND lease_owner=$2 AND claim_count=$3 AND state IN ('running','cancelling') AND lease_expires_at>clock_timestamp() RETURNING *")
            .bind(lease.job_id).bind(&lease.worker_id).bind(lease.attempt_no).bind(f64::from(lease_seconds)).bind(ticks).bind(tape).fetch_optional(&self.pool).await?.ok_or(Error::LeaseLost)
    }
    pub async fn complete(&self, lease: &Lease, r: &NewRun, a: &NewArtifact) -> Result<Completion> {
        for h in [
            &r.result_sha256,
            &r.summary_sha256,
            &a.sha256,
            &a.page_index_sha256,
        ] {
            hash(h)?;
        }
        let mut tx = self.pool.begin().await?;
        let job = held(&mut tx, lease, false).await?;
        // Match storage key to identity and run UUID, rather than merely trusting its syntax.
        let fingerprint: Vec<u8> =
            sqlx::query_scalar("SELECT fingerprint FROM bb.experiment_configs WHERE id=$1")
                .bind(job.experiment_config_id)
                .fetch_one(&mut *tx)
                .await?;
        let expected = format!(
            "runs/{}/{}/{}.bbz",
            hex::encode(fingerprint),
            bb_core::platform::storage_segment(&job.platform),
            r.id
        );
        if a.storage_key != expected {
            return Err(Error::Invalid(
                "artifact key does not match run identity".into(),
            ));
        }
        let intent:Option<String>=sqlx::query_scalar("SELECT storage_key FROM bb.blob_gc WHERE storage_backend=$1 AND storage_key=$2 AND reason='upload_intent' FOR UPDATE")
            .bind(&a.storage_backend).bind(&a.storage_key).fetch_optional(&mut *tx).await?;
        if intent.is_none() {
            return Err(Error::Invalid(
                "upload intent is required before completion".into(),
            ));
        }
        let inserted:Option<Run>=sqlx::query_as("INSERT INTO bb.runs (id,experiment_config_id,platform,blackbox_version,build_profile,worker_id,result_sha256,summary_sha256,ticks,order_count,fill_count,tape_len,equity_len,wall_time_ms,peak_rss_bytes,metrics,engine_check,is_canonical) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18) ON CONFLICT (experiment_config_id,platform) WHERE quarantined_at IS NULL DO NOTHING RETURNING *")
            .bind(r.id).bind(job.experiment_config_id).bind(&job.platform).bind(&r.blackbox_version).bind(&r.build_profile).bind(&lease.worker_id).bind(&r.result_sha256).bind(&r.summary_sha256).bind(r.ticks).bind(r.order_count).bind(r.fill_count).bind(r.tape_len).bind(r.equity_len).bind(r.wall_time_ms).bind(r.peak_rss_bytes).bind(&r.metrics).bind(&r.engine_check).bind(bb_core::platform::is_canonical(&job.platform)).fetch_optional(&mut *tx).await?;
        let reused = inserted.is_none();
        let run:Run=match inserted {Some(row)=>row,None=>sqlx::query_as("SELECT * FROM bb.runs WHERE experiment_config_id=$1 AND platform=$2 AND quarantined_at IS NULL")
            .bind(job.experiment_config_id).bind(&job.platform).fetch_one(&mut *tx).await?};
        if run.result_sha256 != r.result_sha256 {
            sqlx::query("INSERT INTO bb.system_events (level,kind,worker_id,job_id,run_id,message,detail) VALUES ('error','nondeterminism_detected',$1,$2,$3,'result hash differs for the same identity',jsonb_build_object('expected',encode($4::bytea,'hex'),'actual',encode($5::bytea,'hex')))")
                .bind(&lease.worker_id).bind(job.id).bind(run.id).bind(&run.result_sha256).bind(&r.result_sha256).execute(&mut *tx).await?;
            let failed=sqlx::query("UPDATE bb.jobs SET state='failed',error_category='internal',error_code='nondeterminism_detected',error_message='result hash differs for the same identity',lease_owner=NULL,lease_expires_at=NULL,finished_at=now() WHERE id=$1 AND lease_expires_at>clock_timestamp()")
                .bind(job.id).execute(&mut *tx).await?.rows_affected();
            if failed != 1 {
                return Err(Error::LeaseLost);
            }
            close_attempt(
                &mut tx,
                &job,
                "failed",
                Some("nondeterminism_detected"),
                r.peak_rss_bytes,
            )
            .await?;
            tx.commit().await?;
            return Err(Error::Nondeterminism);
        }
        if !reused {
            sqlx::query("INSERT INTO bb.run_artifacts (run_id,kind,format_version,storage_backend,storage_key,sha256,size_bytes,uncompressed_bytes,page_index,page_index_sha256) VALUES ($1,'bundle',$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(run.id).bind(a.format_version).bind(&a.storage_backend).bind(&a.storage_key).bind(&a.sha256).bind(a.size_bytes).bind(a.uncompressed_bytes).bind(&a.page_index).bind(&a.page_index_sha256).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM bb.blob_gc WHERE storage_backend=$1 AND storage_key=$2 AND reason='upload_intent'").bind(&a.storage_backend).bind(&a.storage_key).execute(&mut *tx).await?;
        }
        // All successful uploads count, even if a duplicate object's intent is left for GC.
        sqlx::query("INSERT INTO bb.storage_usage_monthly (month,storage_backend,uploads,bytes_uploaded) VALUES (date_trunc('month',now())::date,$1,1,$2) ON CONFLICT (month,storage_backend) DO UPDATE SET uploads=bb.storage_usage_monthly.uploads+1,bytes_uploaded=bb.storage_usage_monthly.bytes_uploaded+excluded.bytes_uploaded")
            .bind(&a.storage_backend).bind(a.size_bytes).execute(&mut *tx).await?;
        let n=sqlx::query("UPDATE bb.jobs SET state='completed',run_id=$4,lease_owner=NULL,lease_expires_at=NULL,finished_at=now(),ticks_done=ticks_total WHERE id=$1 AND lease_owner=$2 AND claim_count=$3 AND state='running' AND lease_expires_at>clock_timestamp()")
            .bind(job.id).bind(&lease.worker_id).bind(lease.attempt_no).bind(run.id).execute(&mut *tx).await?.rows_affected();
        if n != 1 {
            return Err(Error::LeaseLost);
        }
        close_attempt(&mut tx, &job, "completed", None, r.peak_rss_bytes).await?;
        tx.commit().await?;
        Ok(Completion { run, reused })
    }
    /// Claim-time deduplication: no upload or simulation is necessary for an existing result.
    pub async fn complete_existing(&self, lease: &Lease, run_id: Uuid) -> Result<Run> {
        let mut tx = self.pool.begin().await?;
        let job = held(&mut tx, lease, false).await?;
        let run:Run=sqlx::query_as("SELECT r.* FROM bb.runs r WHERE r.id=$1 AND r.experiment_config_id=$2 AND r.platform=$3 AND r.quarantined_at IS NULL AND EXISTS (SELECT 1 FROM bb.run_artifacts a WHERE a.run_id=r.id)")
            .bind(run_id).bind(job.experiment_config_id).bind(&job.platform).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        let n=sqlx::query("UPDATE bb.jobs SET state='completed',run_id=$2,lease_owner=NULL,lease_expires_at=NULL,finished_at=now(),ticks_done=ticks_total WHERE id=$1 AND lease_expires_at>clock_timestamp()").bind(job.id).bind(run.id).execute(&mut *tx).await?.rows_affected();
        if n != 1 {
            return Err(Error::LeaseLost);
        }
        close_attempt(&mut tx, &job, "completed", None, None).await?;
        tx.commit().await?;
        Ok(run)
    }
    pub async fn fail(
        &self,
        lease: &Lease,
        category: &str,
        code: &str,
        message: &str,
    ) -> Result<Job> {
        let mut tx = self.pool.begin().await?;
        let job = held(&mut tx, lease, false).await?;
        let updated=sqlx::query_as("UPDATE bb.jobs SET state='failed',error_category=$2,error_code=$3,error_message=$4,lease_owner=NULL,lease_expires_at=NULL,finished_at=now() WHERE id=$1 AND lease_expires_at>clock_timestamp() RETURNING *")
            .bind(job.id).bind(category).bind(code).bind(message).fetch_optional(&mut *tx).await?.ok_or(Error::LeaseLost)?;
        close_attempt(&mut tx, &job, "failed", Some(code), None).await?;
        tx.commit().await?;
        Ok(updated)
    }
    pub async fn release(&self, lease: &Lease) -> Result<Job> {
        let mut tx = self.pool.begin().await?;
        let job = held(&mut tx, lease, true).await?;
        let cancelling = job.state == "cancelling";
        let updated=sqlx::query_as("UPDATE bb.jobs SET state=CASE WHEN state='cancelling' THEN 'cancelled' ELSE 'queued' END,lease_owner=NULL,lease_expires_at=NULL,queued_at=now(),run_after=now(),ticks_done=0,tape_events=0,finished_at=CASE WHEN state='cancelling' THEN now() END WHERE id=$1 AND lease_expires_at>clock_timestamp() RETURNING *")
            .bind(job.id).fetch_optional(&mut *tx).await?.ok_or(Error::LeaseLost)?;
        close_attempt(
            &mut tx,
            &job,
            if cancelling { "cancelled" } else { "released" },
            None,
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(updated)
    }
    pub async fn acknowledge_cancel(&self, lease: &Lease) -> Result<Job> {
        let mut tx = self.pool.begin().await?;
        let job = held(&mut tx, lease, true).await?;
        if job.state != "cancelling" {
            return Err(Error::Conflict);
        }
        let updated=sqlx::query_as("UPDATE bb.jobs SET state='cancelled',lease_owner=NULL,lease_expires_at=NULL,finished_at=now() WHERE id=$1 AND lease_expires_at>clock_timestamp() RETURNING *").bind(job.id).fetch_optional(&mut *tx).await?.ok_or(Error::LeaseLost)?;
        close_attempt(&mut tx, &job, "cancelled", None, None).await?;
        tx.commit().await?;
        Ok(updated)
    }
    pub async fn expire_attempt(&self, lease: &Lease, reason: &str) -> Result<Job> {
        let mut tx = self.pool.begin().await?;
        let job = held(&mut tx, lease, false).await?;
        let updated=sqlx::query_as("UPDATE bb.jobs SET state='expired',expired_reason=$2,lease_owner=NULL,lease_expires_at=NULL,finished_at=now() WHERE id=$1 AND lease_expires_at>clock_timestamp() RETURNING *").bind(job.id).bind(reason).fetch_optional(&mut *tx).await?.ok_or(Error::LeaseLost)?;
        close_attempt(&mut tx, &job, "expired", None, None).await?;
        tx.commit().await?;
        Ok(updated)
    }
    pub async fn request_cancel(&self, workspace: Uuid, id: Uuid) -> Result<CancelResult> {
        let mut tx = self.pool.begin().await?;
        let job:Job=sqlx::query_as("SELECT j.* FROM bb.jobs j WHERE j.id=$1 AND EXISTS (SELECT 1 FROM bb.experiments e WHERE e.job_id=j.id AND e.workspace_id=$2) FOR UPDATE")
            .bind(id).bind(workspace).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        if matches!(job.state.as_str(), "completed" | "failed" | "expired") {
            return Err(Error::Conflict);
        }
        sqlx::query("UPDATE bb.experiments SET cancel_requested_at=coalesce(cancel_requested_at,now()) WHERE job_id=$1 AND workspace_id=$2").bind(id).bind(workspace).execute(&mut *tx).await?;
        let interested:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.experiments WHERE job_id=$1 AND cancel_requested_at IS NULL)").bind(id).fetch_one(&mut *tx).await?;
        let updated = if !interested
            && matches!(job.state.as_str(), "created" | "queued" | "running")
        {
            sqlx::query_as("UPDATE bb.jobs SET state=CASE WHEN state='running' THEN 'cancelling' ELSE 'cancelled' END,cancel_requested_at=now(),finished_at=CASE WHEN state IN ('created','queued') THEN now() END WHERE id=$1 RETURNING *")
                .bind(id).fetch_one(&mut *tx).await?
        } else {
            job
        };
        tx.commit().await?;
        Ok(CancelResult {
            job: updated,
            detached: interested,
        })
    }
    pub async fn reap(&self) -> Result<Vec<Job>> {
        let mut tx = self.pool.begin().await?;
        let reaped:Vec<Job>=sqlx::query_as("WITH candidates AS (SELECT id FROM bb.jobs WHERE state IN ('running','cancelling') AND lease_expires_at<=now() ORDER BY lease_expires_at FOR UPDATE SKIP LOCKED LIMIT 100) UPDATE bb.jobs j SET state=CASE WHEN j.state='cancelling' THEN 'cancelled' WHEN j.lease_losses+1>=j.max_attempts THEN 'failed' ELSE 'queued' END,lease_losses=j.lease_losses+1,lease_owner=NULL,lease_expires_at=NULL,ticks_done=0,tape_events=0,queued_at=now(),run_after=now()+interval '30 seconds'*(j.lease_losses+1),error_category=CASE WHEN j.state='running' AND j.lease_losses+1>=j.max_attempts THEN 'lease_expired' END,error_code=CASE WHEN j.state='running' AND j.lease_losses+1>=j.max_attempts THEN 'lease_expired' END,error_message=CASE WHEN j.state='running' AND j.lease_losses+1>=j.max_attempts THEN 'worker lease expired too many times' END,finished_at=CASE WHEN j.state='cancelling' OR j.lease_losses+1>=j.max_attempts THEN now() END FROM candidates c WHERE j.id=c.id RETURNING j.*").fetch_all(&mut *tx).await?;
        for job in &reaped {
            close_attempt(
                &mut tx,
                job,
                if job.state == "cancelled" {
                    "cancelled"
                } else {
                    "lease_expired"
                },
                Some("lease_expired"),
                None,
            )
            .await?;
            sqlx::query("INSERT INTO bb.system_events (level,kind,job_id,message) VALUES ('warn','lease_reclaimed',$1,'worker lease expired')").bind(job.id).execute(&mut *tx).await?;
        }
        let mut all = reaped;
        let expired:Vec<Job>=sqlx::query_as("WITH candidates AS (SELECT id FROM bb.jobs WHERE state IN ('created','queued') AND expires_at<=now() FOR UPDATE SKIP LOCKED LIMIT 100) UPDATE bb.jobs j SET state='expired',finished_at=now(),expired_reason='not started within queue TTL' FROM candidates c WHERE j.id=c.id RETURNING j.*").fetch_all(&mut *tx).await?;
        all.extend(expired);
        tx.commit().await?;
        Ok(all)
    }
    pub async fn next_wake(&self, platform: &str) -> Result<Option<DateTime<Utc>>> {
        Ok(sqlx::query_scalar("SELECT least((SELECT min(run_after) FROM bb.jobs WHERE state='queued' AND platform=$1),(SELECT min(lease_expires_at) FROM bb.jobs WHERE state IN ('running','cancelling')),(SELECT min(expires_at) FROM bb.jobs WHERE state IN ('created','queued')))").bind(platform).fetch_one(&self.pool).await?)
    }
    pub async fn queue_position(&self, id: Uuid) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar("SELECT 1+(SELECT count(*) FROM bb.jobs q WHERE q.state='queued' AND q.platform=j.platform AND (q.priority>j.priority OR (q.priority=j.priority AND (q.queued_at,q.id)<(j.queued_at,j.id)))) FROM bb.jobs j WHERE j.id=$1 AND j.state='queued'").bind(id).fetch_optional(&self.pool).await?)
    }
    pub async fn job(&self, id: Uuid) -> Result<Job> {
        sqlx::query_as("SELECT * FROM bb.jobs WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn visible_job(&self, workspace: Uuid, id: Uuid) -> Result<Job> {
        sqlx::query_as("SELECT j.* FROM bb.jobs j WHERE j.id=$1 AND EXISTS (SELECT 1 FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.job_id=j.id AND (e.workspace_id=$2 OR w.is_public_read))").bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn attempts(&self, id: Uuid) -> Result<Vec<JobAttempt>> {
        Ok(
            sqlx::query_as("SELECT * FROM bb.job_attempts WHERE job_id=$1 ORDER BY attempt_no")
                .bind(id)
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn jobs(
        &self,
        workspace: Uuid,
        states: &[String],
        mine: Option<Uuid>,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<Job>> {
        Ok(sqlx::query_as("SELECT j.* FROM bb.jobs j WHERE EXISTS (SELECT 1 FROM bb.experiments e WHERE e.job_id=j.id AND e.workspace_id=$1 AND ($3::uuid IS NULL OR e.created_by_key_id=$3)) AND (cardinality($2::text[])=0 OR j.state=ANY($2)) AND ($4::timestamptz IS NULL OR (j.created_at,j.id)<($4,$5)) ORDER BY j.created_at DESC,j.id DESC LIMIT $6")
            .bind(workspace).bind(states).bind(mine).bind(cursor.map(|c|c.created_at)).bind(cursor.map(|c|c.id)).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn experiment(&self, workspace: Uuid, id: Uuid) -> Result<Experiment> {
        sqlx::query_as("SELECT e.* FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.id=$1 AND (e.workspace_id=$2 OR w.is_public_read)").bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn experiments(
        &self,
        workspace: Uuid,
        f: &ExperimentFilter,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<Experiment>> {
        Ok(sqlx::query_as("SELECT e.* FROM bb.experiments e LEFT JOIN bb.jobs j ON j.id=e.job_id WHERE e.workspace_id=$1 AND ($2::text IS NULL OR e.dataset_id=$2) AND ($3::text IS NULL OR e.strategy_kind=$3) AND (cardinality($4::text[])=0 OR CASE WHEN e.cancel_requested_at IS NOT NULL THEN 'cancelled' WHEN EXISTS (SELECT 1 FROM bb.runs r WHERE r.experiment_config_id=e.experiment_config_id AND r.platform=e.platform AND r.quarantined_at IS NULL) THEN 'completed' ELSE j.state END=ANY($4)) AND ($5::uuid IS NULL OR e.sweep_id=$5) AND ($6::text IS NULL OR starts_with(e.label,$6)) AND ($7::timestamptz IS NULL OR e.created_at>=$7) AND ($8::timestamptz IS NULL OR e.created_at<$8) AND ($9::uuid IS NULL OR e.created_by_key_id=$9) AND ($10::timestamptz IS NULL OR (e.created_at,e.id)<($10,$11)) ORDER BY e.created_at DESC,e.id DESC LIMIT $12")
            .bind(workspace).bind(&f.dataset_id).bind(&f.strategy_kind).bind(&f.states).bind(f.sweep_id).bind(&f.label_prefix).bind(f.created_after).bind(f.created_before).bind(f.created_by_key).bind(cursor.map(|c|c.created_at)).bind(cursor.map(|c|c.id)).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn rename_experiment(
        &self,
        workspace: Uuid,
        id: Uuid,
        version: i64,
        label: &str,
    ) -> Result<Experiment> {
        sqlx::query_as("UPDATE bb.experiments SET label=$4 WHERE id=$1 AND workspace_id=$2 AND version=$3 RETURNING *").bind(id).bind(workspace).bind(version).bind(label).fetch_optional(&self.pool).await?.ok_or(Error::Conflict)
    }
    pub async fn pin_experiment(
        &self,
        workspace: Uuid,
        id: Uuid,
        version: i64,
        pinned: bool,
    ) -> Result<Experiment> {
        sqlx::query_as("UPDATE bb.experiments SET pinned=$4 WHERE id=$1 AND workspace_id=$2 AND version=$3 RETURNING *").bind(id).bind(workspace).bind(version).bind(pinned).fetch_optional(&self.pool).await?.ok_or(Error::Conflict)
    }
    pub async fn delete_experiment(
        &self,
        workspace: Uuid,
        id: Uuid,
        version: Option<i64>,
    ) -> Result<()> {
        // Discover without locking, then follow the same job -> experiment lock order as cancel.
        let row: Experiment =
            sqlx::query_as("SELECT * FROM bb.experiments WHERE id=$1 AND workspace_id=$2")
                .bind(id)
                .bind(workspace)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(Error::NotFound)?;
        let mut tx = self.pool.begin().await?;
        if let Some(job) = row.job_id {
            sqlx::query("SELECT id FROM bb.jobs WHERE id=$1 FOR UPDATE")
                .bind(job)
                .execute(&mut *tx)
                .await?;
        }
        let deleted:Option<Experiment>=sqlx::query_as("DELETE FROM bb.experiments WHERE id=$1 AND workspace_id=$2 AND ($3::bigint IS NULL OR version=$3) AND job_id IS NOT DISTINCT FROM $4 RETURNING *")
            .bind(id).bind(workspace).bind(version).bind(row.job_id).fetch_optional(&mut *tx).await?;
        if deleted.is_none() {
            return Err(Error::Conflict);
        }
        if let Some(job) = row.job_id {
            sqlx::query("UPDATE bb.jobs SET state=CASE WHEN state='running' THEN 'cancelling' ELSE 'cancelled' END,cancel_requested_at=now(),finished_at=CASE WHEN state IN ('created','queued') THEN now() END WHERE id=$1 AND state IN ('created','queued','running') AND NOT EXISTS (SELECT 1 FROM bb.experiments WHERE job_id=$1 AND cancel_requested_at IS NULL)").bind(job).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
