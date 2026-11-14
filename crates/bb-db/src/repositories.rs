use crate::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Run,
    Admin,
}
impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Run => "run",
            Self::Admin => "admin",
        }
    }
}
impl ApiKey {
    pub fn permits(&self, required: Scope) -> bool {
        self.scopes.iter().any(|s| match s.as_str() {
            "admin" => true,
            "run" => required <= Scope::Run,
            "read" => required == Scope::Read,
            _ => false,
        })
    }
}
/// Plaintext is returned once, is not serializable and has no Debug implementation.
pub struct MintedKey {
    pub plaintext: String,
    pub key: ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}
impl Cursor {
    pub fn encode(&self, filter: &str) -> Result<String> {
        let data = serde_json::to_vec(&(self, hex::encode(Sha256::digest(filter.as_bytes()))))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(URL_SAFE_NO_PAD.encode(data))
    }
    pub fn decode(encoded: &str, filter: &str) -> Result<Self> {
        let data = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::Invalid("invalid cursor".into()))?;
        let (cursor, hash): (Self, String) =
            serde_json::from_slice(&data).map_err(|_| Error::Invalid("invalid cursor".into()))?;
        if hash != hex::encode(Sha256::digest(filter.as_bytes())) {
            return Err(Error::Invalid("cursor filters changed".into()));
        }
        Ok(cursor)
    }
}
pub(crate) fn limit(n: i64) -> Result<i64> {
    if !(1..=100).contains(&n) {
        return Err(Error::Invalid("limit must be 1..=100".into()));
    }
    Ok(n)
}
pub(crate) fn hash(bytes: &[u8]) -> Result<()> {
    if bytes.len() != 32 {
        return Err(Error::Invalid("SHA256 must contain 32 bytes".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewDatasetVersion {
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
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewConfig {
    pub fingerprint: Vec<u8>,
    pub dataset_version_id: Uuid,
    pub dataset_id: String,
    pub contract_version: i32,
    pub simulation_revision: i32,
    pub engine_name: String,
    pub engine_version: String,
    pub config: Value,
    pub est_ticks: i64,
    pub est_tape_events_upper: i64,
    pub est_output_bytes_upper: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct NewDataSource<'a> {
    pub id: &'a str,
    pub vendor: &'a str,
    pub licence: &'a str,
    pub licence_url: Option<&'a str>,
    pub redistribution: bool,
    pub hostable: bool,
    pub notes: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct NewDataset<'a> {
    pub id: &'a str,
    pub owner: Option<Uuid>,
    pub title: &'a str,
    pub kind: &'a str,
    pub instrument: Uuid,
    pub source: &'a str,
    pub hosted: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct NewSystemEvent<'a> {
    pub level: &'a str,
    pub kind: &'a str,
    pub worker: Option<&'a str>,
    pub job: Option<Uuid>,
    pub run: Option<Uuid>,
    pub message: &'a str,
    pub detail: &'a Value,
}

impl Database {
    pub async fn create_workspace(&self, slug: &str, name: &str) -> Result<Workspace> {
        Ok(sqlx::query_as(
            "INSERT INTO bb.workspaces (slug,name,kind) VALUES ($1,$2,'research') RETURNING *",
        )
        .bind(slug)
        .bind(name)
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn workspace(&self, slug: &str) -> Result<Workspace> {
        sqlx::query_as("SELECT * FROM bb.workspaces WHERE slug=$1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        Ok(sqlx::query_as("SELECT * FROM bb.workspaces ORDER BY slug")
            .fetch_all(&self.pool)
            .await?)
    }
    pub async fn update_workspace(
        &self,
        id: Uuid,
        version: i64,
        name: &str,
        daily: i32,
        sweeps: i32,
        active: i32,
    ) -> Result<Workspace> {
        sqlx::query_as("UPDATE bb.workspaces SET name=$3,quota_experiments_per_day=$4,quota_sweeps_per_day=$5,quota_active_jobs=$6 WHERE id=$1 AND version=$2 RETURNING *")
            .bind(id).bind(version).bind(name).bind(daily).bind(sweeps).bind(active).fetch_optional(&self.pool).await?.ok_or(Error::Conflict)
    }
    pub async fn mint_key(
        &self,
        workspace: Uuid,
        label: &str,
        scopes: &[Scope],
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<MintedKey> {
        if scopes.is_empty() {
            return Err(Error::Invalid("at least one scope is required".into()));
        }
        let mut secret = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut secret)
            .map_err(|_| Error::Invalid("operating system entropy unavailable".into()))?;
        let plaintext = format!("bbk_{}", URL_SAFE_NO_PAD.encode(secret));
        let digest = Sha256::digest(plaintext.as_bytes()).to_vec();
        let scopes: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        let key = sqlx::query_as("INSERT INTO bb.api_keys (workspace_id,key_sha256,display_prefix,label,scopes,expires_at) VALUES ($1,$2,$3,$4,$5,$6) RETURNING *")
            .bind(workspace).bind(digest).bind(&plaintext[..8]).bind(label).bind(scopes).bind(expires_at).fetch_one(&self.pool).await?;
        Ok(MintedKey { plaintext, key })
    }
    pub async fn authenticate(&self, plaintext: &str) -> Result<Option<ApiKey>> {
        if plaintext.len() != 47
            || !plaintext.starts_with("bbk_")
            || !plaintext[4..]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Ok(None);
        }
        let digest = Sha256::digest(plaintext.as_bytes()).to_vec();
        let key: Option<ApiKey> = sqlx::query_as("SELECT * FROM bb.api_keys WHERE key_sha256=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now())")
            .bind(digest).fetch_optional(&self.pool).await?;
        if let Some(key) = &key {
            sqlx::query("UPDATE bb.api_keys SET last_used_at=now() WHERE id=$1 AND (last_used_at IS NULL OR last_used_at<now()-interval '1 hour')")
                .bind(key.id).execute(&self.pool).await?;
        }
        Ok(key)
    }
    pub async fn revoke_key(&self, workspace: Uuid, id: Uuid) -> Result<ApiKey> {
        sqlx::query_as("UPDATE bb.api_keys SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1 AND workspace_id=$2 RETURNING *")
            .bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn api_keys(&self, workspace: Uuid) -> Result<Vec<ApiKey>> {
        Ok(sqlx::query_as(
            "SELECT * FROM bb.api_keys WHERE workspace_id=$1 ORDER BY created_at DESC,id DESC",
        )
        .bind(workspace)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn register_source(&self, source: NewDataSource<'_>) -> Result<DataSource> {
        let NewDataSource {
            id,
            vendor,
            licence,
            licence_url,
            redistribution,
            hostable,
            notes,
        } = source;
        sqlx::query("INSERT INTO bb.data_sources (id,vendor,licence,licence_url,redistribution_allowed,hostable,notes) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (id) DO NOTHING")
            .bind(id).bind(vendor).bind(licence).bind(licence_url).bind(redistribution).bind(hostable).bind(notes).execute(&self.pool).await?;
        let row: DataSource = sqlx::query_as("SELECT * FROM bb.data_sources WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        if row.vendor != vendor
            || row.licence != licence
            || row.licence_url.as_deref() != licence_url
            || row.redistribution_allowed != redistribution
            || row.hostable != hostable
            || row.notes != notes
        {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn register_instrument(
        &self,
        exchange: &str,
        symbol: &str,
        kind: &str,
    ) -> Result<Instrument> {
        sqlx::query("INSERT INTO bb.instruments (exchange,symbol,kind) VALUES ($1,$2,$3) ON CONFLICT (exchange,symbol) DO NOTHING")
            .bind(exchange).bind(symbol).bind(kind).execute(&self.pool).await?;
        let row: Instrument =
            sqlx::query_as("SELECT * FROM bb.instruments WHERE exchange=$1 AND symbol=$2")
                .bind(exchange)
                .bind(symbol)
                .fetch_one(&self.pool)
                .await?;
        if row.kind != kind {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn register_dataset(&self, dataset: NewDataset<'_>) -> Result<Dataset> {
        let NewDataset {
            id,
            owner,
            title,
            kind,
            instrument,
            source,
            hosted,
        } = dataset;
        if hosted {
            let hostable: Option<bool> =
                sqlx::query_scalar("SELECT hostable FROM bb.data_sources WHERE id=$1")
                    .bind(source)
                    .fetch_optional(&self.pool)
                    .await?;
            if hostable != Some(true) {
                return Err(Error::Invalid("source is not hostable".into()));
            }
        }
        sqlx::query("INSERT INTO bb.datasets (id,owner_workspace_id,title,kind,level,instrument_id,data_source_id) VALUES ($1,$2,$3,$4,'l2_mbp',$5,$6) ON CONFLICT (id) DO NOTHING")
            .bind(id).bind(owner).bind(title).bind(kind).bind(instrument).bind(source).execute(&self.pool).await?;
        let row: Dataset = sqlx::query_as("SELECT * FROM bb.datasets WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        if row.owner_workspace_id != owner
            || row.instrument_id != instrument
            || row.data_source_id != source
            || row.kind != kind
        {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn dataset(&self, workspace: Uuid, id: &str) -> Result<Dataset> {
        sqlx::query_as("SELECT * FROM bb.datasets WHERE id=$1 AND (owner_workspace_id IS NULL OR owner_workspace_id=$2)")
            .bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn datasets(
        &self,
        workspace: Uuid,
        after: Option<&str>,
        count: i64,
    ) -> Result<Vec<Dataset>> {
        Ok(sqlx::query_as("SELECT * FROM bb.datasets WHERE (owner_workspace_id IS NULL OR owner_workspace_id=$1) AND ($2::text IS NULL OR id>$2) ORDER BY id LIMIT $3")
            .bind(workspace).bind(after).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn register_version(&self, v: &NewDatasetVersion) -> Result<DatasetVersion> {
        hash(&v.events_sha256)?;
        sqlx::query("INSERT INTO bb.dataset_versions (dataset_id,version_no,events_sha256,normalization_version,contract_version,tick_size,lot_size,contract_size,origin_ns,start_rel_ns,end_rel_ns,timestamp_unit,row_count,events_bytes,storage,storage_key,manifest) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) ON CONFLICT (dataset_id,events_sha256) DO NOTHING")
            .bind(&v.dataset_id).bind(v.version_no).bind(&v.events_sha256).bind(v.normalization_version).bind(v.contract_version).bind(v.tick_size).bind(v.lot_size).bind(v.contract_size).bind(v.origin_ns).bind(v.start_rel_ns).bind(v.end_rel_ns).bind(&v.timestamp_unit).bind(v.row_count).bind(v.events_bytes).bind(&v.storage).bind(&v.storage_key).bind(&v.manifest).execute(&self.pool).await?;
        let row: DatasetVersion = sqlx::query_as(
            "SELECT * FROM bb.dataset_versions WHERE dataset_id=$1 AND events_sha256=$2",
        )
        .bind(&v.dataset_id)
        .bind(&v.events_sha256)
        .fetch_one(&self.pool)
        .await?;
        if row.version_no != v.version_no
            || row.normalization_version != v.normalization_version
            || row.contract_version != v.contract_version
            || row.tick_size != v.tick_size
            || row.lot_size != v.lot_size
            || row.contract_size != v.contract_size
            || row.origin_ns != v.origin_ns
            || row.start_rel_ns != v.start_rel_ns
            || row.end_rel_ns != v.end_rel_ns
            || row.timestamp_unit != v.timestamp_unit
            || row.row_count != v.row_count
            || row.events_bytes != v.events_bytes
            || row.storage != v.storage
            || row.storage_key != v.storage_key
            || row.manifest != v.manifest
        {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn current_version(&self, dataset: &str) -> Result<DatasetVersion> {
        sqlx::query_as("SELECT * FROM bb.dataset_versions WHERE dataset_id=$1 AND retired_at IS NULL ORDER BY version_no DESC LIMIT 1").bind(dataset).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn dataset_version(&self, dataset: &str, version: i32) -> Result<DatasetVersion> {
        sqlx::query_as("SELECT * FROM bb.dataset_versions WHERE dataset_id=$1 AND version_no=$2")
            .bind(dataset)
            .bind(version)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn dataset_versions(
        &self,
        dataset: &str,
        before: Option<i32>,
        count: i64,
    ) -> Result<Vec<DatasetVersion>> {
        Ok(sqlx::query_as("SELECT * FROM bb.dataset_versions WHERE dataset_id=$1 AND ($2::integer IS NULL OR version_no<$2) ORDER BY version_no DESC LIMIT $3").bind(dataset).bind(before).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn retire_version(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE bb.dataset_versions SET retired_at=coalesce(retired_at,now()) WHERE id=$1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn put_quality_report(&self, r: &QualityReport) -> Result<QualityReport> {
        hash(&r.report_sha256)?;
        sqlx::query("INSERT INTO bb.dataset_quality_reports (dataset_version_id,checker_version,passed,error_count,warning_count,book_valid_fraction,report,report_sha256) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING")
            .bind(r.dataset_version_id).bind(r.checker_version).bind(r.passed).bind(r.error_count).bind(r.warning_count).bind(r.book_valid_fraction).bind(&r.report).bind(&r.report_sha256).execute(&self.pool).await?;
        let row = self
            .quality_report(r.dataset_version_id, Some(r.checker_version))
            .await?;
        if row.report_sha256 != r.report_sha256
            || row.report != r.report
            || row.passed != r.passed
            || row.error_count != r.error_count
            || row.warning_count != r.warning_count
            || row.book_valid_fraction != r.book_valid_fraction
        {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn quality_report(
        &self,
        version: Uuid,
        checker: Option<i32>,
    ) -> Result<QualityReport> {
        sqlx::query_as("SELECT * FROM bb.dataset_quality_reports WHERE dataset_version_id=$1 AND ($2::integer IS NULL OR checker_version=$2) ORDER BY checker_version DESC LIMIT 1")
            .bind(version).bind(checker).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn register_config(&self, c: &NewConfig) -> Result<ExperimentConfig> {
        hash(&c.fingerprint)?;
        sqlx::query("INSERT INTO bb.experiment_configs (fingerprint,dataset_version_id,dataset_id,contract_version,simulation_revision,engine_name,engine_version,config,est_ticks,est_tape_events_upper,est_output_bytes_upper) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (fingerprint) DO NOTHING")
            .bind(&c.fingerprint).bind(c.dataset_version_id).bind(&c.dataset_id).bind(c.contract_version).bind(c.simulation_revision).bind(&c.engine_name).bind(&c.engine_version).bind(&c.config).bind(c.est_ticks).bind(c.est_tape_events_upper).bind(c.est_output_bytes_upper).execute(&self.pool).await?;
        let row: ExperimentConfig =
            sqlx::query_as("SELECT * FROM bb.experiment_configs WHERE fingerprint=$1")
                .bind(&c.fingerprint)
                .fetch_one(&self.pool)
                .await?;
        if row.dataset_version_id != c.dataset_version_id
            || row.config != c.config
            || row.engine_name != c.engine_name
            || row.engine_version != c.engine_version
            || row.contract_version != c.contract_version
            || row.simulation_revision != c.simulation_revision
            || row.est_ticks != c.est_ticks
            || row.est_tape_events_upper != c.est_tape_events_upper
            || row.est_output_bytes_upper != c.est_output_bytes_upper
        {
            return Err(Error::Conflict);
        }
        Ok(row)
    }
    pub async fn config(&self, id: Uuid) -> Result<ExperimentConfig> {
        sqlx::query_as("SELECT * FROM bb.experiment_configs WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn run(&self, id: Uuid) -> Result<Run> {
        sqlx::query_as("SELECT * FROM bb.runs WHERE id=$1 AND quarantined_at IS NULL")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn existing_run(&self, config: Uuid, platform: &str) -> Result<Option<Run>> {
        Ok(sqlx::query_as("SELECT * FROM bb.runs WHERE experiment_config_id=$1 AND platform=$2 AND quarantined_at IS NULL").bind(config).bind(platform).fetch_optional(&self.pool).await?)
    }
    pub async fn visible_run(
        &self,
        workspace: Uuid,
        identity: &str,
        platform: &str,
    ) -> Result<Run> {
        sqlx::query_as("SELECT r.* FROM bb.runs r JOIN bb.experiment_configs c ON c.id=r.experiment_config_id WHERE (c.short_id=$1 OR encode(c.fingerprint,'hex')=$1) AND r.platform=$2 AND r.quarantined_at IS NULL AND EXISTS (SELECT 1 FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.experiment_config_id=c.id AND e.platform=r.platform AND (e.workspace_id=$3 OR w.is_public_read))")
            .bind(identity).bind(platform).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn artifact(&self, run: Uuid) -> Result<RunArtifact> {
        sqlx::query_as("SELECT * FROM bb.run_artifacts WHERE run_id=$1 AND kind='bundle'")
            .bind(run)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }
    pub async fn quarantine_run(&self, run: Uuid, reason: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE bb.runs SET quarantined_at=coalesce(quarantined_at,now()),quarantine_reason=coalesce(quarantine_reason,$2) WHERE id=$1").bind(run).bind(reason).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO bb.system_events (level,kind,run_id,message) VALUES ('error','integrity_failed',$1,$2)").bind(run).bind(reason).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn upload_intent(&self, backend: &str, key: &str) -> Result<()> {
        sqlx::query("INSERT INTO bb.blob_gc (storage_backend,storage_key,reason,not_before) VALUES ($1,$2,'upload_intent',now()+interval '1 hour') ON CONFLICT DO NOTHING")
            .bind(backend).bind(key).execute(&self.pool).await?;
        Ok(())
    }
    /// Read-only preview. Object deletion must use `claim_gc` so completion is fenced.
    pub async fn gc_due(&self, count: i64) -> Result<Vec<BlobGc>> {
        // Diagnostics only: this snapshot does not reserve objects for deletion.
        Ok(sqlx::query_as("SELECT g.* FROM bb.blob_gc g WHERE g.not_before<=now() AND NOT EXISTS (SELECT 1 FROM bb.run_artifacts a WHERE a.storage_backend=g.storage_backend AND a.storage_key=g.storage_key) ORDER BY g.not_before LIMIT $1")
            .bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn gc_ack(&self, backend: &str, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM bb.blob_gc WHERE storage_backend=$1 AND storage_key=$2")
            .bind(backend)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn gc_failed(&self, backend: &str, key: &str, message: &str) -> Result<()> {
        sqlx::query("UPDATE bb.blob_gc SET attempts=attempts+1,last_error=$3,not_before=now()+interval '5 minutes' WHERE storage_backend=$1 AND storage_key=$2").bind(backend).bind(key).bind(message).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn record_storage_read(&self, backend: &str) -> Result<()> {
        sqlx::query("INSERT INTO bb.storage_usage_monthly (month,storage_backend,reads) VALUES (date_trunc('month',now())::date,$1,1) ON CONFLICT (month,storage_backend) DO UPDATE SET reads=bb.storage_usage_monthly.reads+1")
            .bind(backend).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn storage_usage(&self) -> Result<Vec<StorageUsage>> {
        Ok(sqlx::query_as("SELECT * FROM bb.storage_usage_monthly WHERE month=date_trunc('month',now())::date ORDER BY storage_backend").fetch_all(&self.pool).await?)
    }
    pub async fn emit_event(&self, event: NewSystemEvent<'_>) -> Result<SystemEvent> {
        let NewSystemEvent {
            level,
            kind,
            worker,
            job,
            run,
            message,
            detail,
        } = event;
        Ok(sqlx::query_as("INSERT INTO bb.system_events (level,kind,worker_id,job_id,run_id,message,detail) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING *")
            .bind(level).bind(kind).bind(worker).bind(job).bind(run).bind(message).bind(detail).fetch_one(&self.pool).await?)
    }
    pub async fn events(
        &self,
        before: Option<i64>,
        kind: Option<&str>,
        level: Option<&str>,
        count: i64,
    ) -> Result<Vec<SystemEvent>> {
        Ok(sqlx::query_as("SELECT * FROM bb.system_events WHERE ($1::bigint IS NULL OR id<$1) AND ($2::text IS NULL OR kind=$2) AND ($3::text IS NULL OR level=$3) ORDER BY id DESC LIMIT $4")
            .bind(before).bind(kind).bind(level).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn worker_heartbeat(&self, w: &WorkerHeartbeat) -> Result<WorkerHeartbeat> {
        Ok(sqlx::query_as("INSERT INTO bb.worker_heartbeats (worker_id,hostname,pid,blackbox_version,platform,mode,slots,state,current_job_id,heartbeat_interval_ms,stopped_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,CASE WHEN $8='stopped' THEN now() END) ON CONFLICT (worker_id) DO UPDATE SET hostname=excluded.hostname,pid=excluded.pid,blackbox_version=excluded.blackbox_version,platform=excluded.platform,mode=excluded.mode,slots=excluded.slots,state=excluded.state,current_job_id=excluded.current_job_id,heartbeat_interval_ms=excluded.heartbeat_interval_ms,last_heartbeat_at=now(),stopped_at=excluded.stopped_at RETURNING *")
            .bind(&w.worker_id).bind(&w.hostname).bind(w.pid).bind(&w.blackbox_version).bind(&w.platform).bind(&w.mode).bind(w.slots).bind(&w.state).bind(w.current_job_id).bind(w.heartbeat_interval_ms).fetch_one(&self.pool).await?)
    }
    pub async fn workers(&self) -> Result<Vec<WorkerHeartbeat>> {
        Ok(
            sqlx::query_as("SELECT * FROM bb.worker_heartbeats ORDER BY last_heartbeat_at DESC")
                .fetch_all(&self.pool)
                .await?,
        )
    }
}
