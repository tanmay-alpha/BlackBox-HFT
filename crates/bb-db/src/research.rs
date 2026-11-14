use crate::repositories::{hash, limit};
use crate::*;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewSweep {
    pub workspace_id: Uuid,
    pub spec: Value,
    pub spec_fingerprint: Vec<u8>,
    pub x_len: i16,
    pub y_len: i16,
    pub label: String,
    pub created_by_key_id: Option<Uuid>,
}
#[derive(Debug, Clone)]
pub struct NewComparison {
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
}
#[derive(Debug, Clone)]
pub struct NewReport {
    pub workspace_id: Uuid,
    pub kind: String,
    pub run_id: Option<Uuid>,
    pub comparison_id: Option<Uuid>,
    pub sweep_id: Option<Uuid>,
    pub title: String,
    pub generator_version: i32,
    pub body: String,
    pub body_sha256: Vec<u8>,
    pub created_by_key_id: Option<Uuid>,
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QueueDepth {
    pub platform: String,
    pub state: String,
    pub count: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct NewAuditEvent<'a> {
    pub workspace: Option<Uuid>,
    pub key: Option<Uuid>,
    pub action: &'a str,
    pub outcome: &'a str,
    pub target_type: Option<&'a str>,
    pub target_id: Option<&'a str>,
    pub request_id: Option<&'a str>,
    pub client_ip: Option<&'a str>,
    pub detail: &'a Value,
}

impl Database {
    pub async fn create_sweep(&self, s: &NewSweep, bypass_quotas: bool) -> Result<Sweep> {
        hash(&s.spec_fingerprint)?;
        let mut tx = self.pool.begin().await?;
        let w: Workspace = sqlx::query_as("SELECT * FROM bb.workspaces WHERE id=$1 FOR UPDATE")
            .bind(s.workspace_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let existing: Option<Sweep> =
            sqlx::query_as("SELECT * FROM bb.sweeps WHERE workspace_id=$1 AND spec_fingerprint=$2")
                .bind(w.id)
                .bind(&s.spec_fingerprint)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(row) = existing {
            return Ok(row);
        }
        if !bypass_quotas {
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM bb.sweeps WHERE workspace_id=$1 AND ($2::uuid IS NULL OR created_by_key_id=$2) AND created_at>=date_trunc('day',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'")
                .bind(w.id).bind(s.created_by_key_id).fetch_one(&mut *tx).await?;
            if count >= i64::from(w.quota_sweeps_per_day) {
                return Err(Error::Quota("sweeps_per_day"));
            }
        }
        let row=sqlx::query_as("INSERT INTO bb.sweeps (workspace_id,spec,spec_fingerprint,x_len,y_len,label,created_by_key_id) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING *")
            .bind(w.id).bind(&s.spec).bind(&s.spec_fingerprint).bind(s.x_len).bind(s.y_len).bind(&s.label).bind(s.created_by_key_id).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }
    pub async fn sweep(&self, workspace: Uuid, id: Uuid) -> Result<Sweep> {
        sqlx::query_as("SELECT s.* FROM bb.sweeps s JOIN bb.workspaces w ON w.id=s.workspace_id WHERE s.id=$1 AND (s.workspace_id=$2 OR w.is_public_read)").bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn sweeps(
        &self,
        workspace: Uuid,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<Sweep>> {
        Ok(sqlx::query_as("SELECT * FROM bb.sweeps WHERE workspace_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)<($2,$3)) ORDER BY created_at DESC,id DESC LIMIT $4")
            .bind(workspace).bind(cursor.map(|c|c.created_at)).bind(cursor.map(|c|c.id)).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn sweep_cells(&self, workspace: Uuid, id: Uuid) -> Result<Vec<Experiment>> {
        self.sweep(workspace, id).await?;
        Ok(sqlx::query_as(
            "SELECT * FROM bb.experiments WHERE sweep_id=$1 ORDER BY sweep_y,sweep_x",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn rename_sweep(
        &self,
        workspace: Uuid,
        id: Uuid,
        version: i64,
        label: &str,
    ) -> Result<Sweep> {
        sqlx::query_as("UPDATE bb.sweeps SET label=$4 WHERE id=$1 AND workspace_id=$2 AND version=$3 RETURNING *").bind(id).bind(workspace).bind(version).bind(label).fetch_optional(&self.pool).await?.ok_or(Error::Conflict)
    }
    pub async fn cancel_sweep(&self, workspace: Uuid, id: Uuid) -> Result<Sweep> {
        let mut tx = self.pool.begin().await?;
        // Lock workspace to fence new cell submissions while all interested jobs are inspected.
        sqlx::query("SELECT id FROM bb.workspaces WHERE id=$1 FOR UPDATE")
            .bind(workspace)
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT j.id FROM bb.jobs j WHERE EXISTS (SELECT 1 FROM bb.experiments e WHERE e.job_id=j.id AND e.sweep_id=$1 AND e.workspace_id=$2) ORDER BY j.id FOR UPDATE")
            .bind(id).bind(workspace).execute(&mut *tx).await?;
        let row:Sweep=sqlx::query_as("UPDATE bb.sweeps SET cancel_requested_at=coalesce(cancel_requested_at,now()) WHERE id=$1 AND workspace_id=$2 RETURNING *").bind(id).bind(workspace).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
        sqlx::query("UPDATE bb.experiments SET cancel_requested_at=coalesce(cancel_requested_at,now()) WHERE sweep_id=$1 AND workspace_id=$2").bind(id).bind(workspace).execute(&mut *tx).await?;
        sqlx::query("UPDATE bb.jobs j SET state=CASE WHEN j.state='running' THEN 'cancelling' ELSE 'cancelled' END,cancel_requested_at=now(),finished_at=CASE WHEN j.state IN ('created','queued') THEN now() END WHERE j.state IN ('created','queued','running') AND EXISTS (SELECT 1 FROM bb.experiments e WHERE e.job_id=j.id AND e.sweep_id=$1 AND e.workspace_id=$2) AND NOT EXISTS (SELECT 1 FROM bb.experiments e WHERE e.job_id=j.id AND e.cancel_requested_at IS NULL)").bind(id).bind(workspace).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }
    pub async fn delete_sweep(&self, workspace: Uuid, id: Uuid) -> Result<()> {
        self.cancel_sweep(workspace, id).await?;
        sqlx::query("DELETE FROM bb.sweeps WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(workspace)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn create_comparison(&self, c: &NewComparison) -> Result<Comparison> {
        hash(&c.summary_sha256)?;
        let visible:i64=sqlx::query_scalar("SELECT count(*) FROM bb.runs r WHERE r.id=ANY($1::uuid[]) AND r.quarantined_at IS NULL AND EXISTS (SELECT 1 FROM bb.experiments e JOIN bb.workspaces w ON w.id=e.workspace_id WHERE e.experiment_config_id=r.experiment_config_id AND e.platform=r.platform AND (e.workspace_id=$2 OR w.is_public_read))")
            .bind(vec![c.run_a_id,c.run_b_id]).bind(c.workspace_id).fetch_one(&self.pool).await?;
        if visible != 2 {
            return Err(Error::NotFound);
        }
        sqlx::query("INSERT INTO bb.comparisons (workspace_id,run_a_id,run_b_id,comparator_version,eligibility,reasons,summary,summary_sha256,label,created_by_key_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (workspace_id,run_a_id,run_b_id,comparator_version) DO NOTHING")
            .bind(c.workspace_id).bind(c.run_a_id).bind(c.run_b_id).bind(c.comparator_version).bind(&c.eligibility).bind(&c.reasons).bind(&c.summary).bind(&c.summary_sha256).bind(&c.label).bind(c.created_by_key_id).execute(&self.pool).await?;
        Ok(sqlx::query_as("SELECT * FROM bb.comparisons WHERE workspace_id=$1 AND run_a_id=$2 AND run_b_id=$3 AND comparator_version=$4").bind(c.workspace_id).bind(c.run_a_id).bind(c.run_b_id).bind(c.comparator_version).fetch_one(&self.pool).await?)
    }
    /// The stored comparison of `run_a` with `run_b` in `workspace` by `comparator_version`.
    pub async fn find_comparison(
        &self,
        workspace: Uuid,
        run_a: Uuid,
        run_b: Uuid,
        comparator_version: i32,
    ) -> Result<Option<Comparison>> {
        Ok(sqlx::query_as("SELECT * FROM bb.comparisons WHERE workspace_id=$1 AND run_a_id=$2 AND run_b_id=$3 AND comparator_version=$4").bind(workspace).bind(run_a).bind(run_b).bind(comparator_version).fetch_optional(&self.pool).await?)
    }
    pub async fn comparison(&self, workspace: Uuid, id: Uuid) -> Result<Comparison> {
        sqlx::query_as("SELECT c.* FROM bb.comparisons c JOIN bb.workspaces w ON w.id=c.workspace_id WHERE c.id=$1 AND (c.workspace_id=$2 OR w.is_public_read)").bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn comparisons(
        &self,
        workspace: Uuid,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<Comparison>> {
        Ok(sqlx::query_as("SELECT * FROM bb.comparisons WHERE workspace_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)<($2,$3)) ORDER BY created_at DESC,id DESC LIMIT $4").bind(workspace).bind(cursor.map(|c|c.created_at)).bind(cursor.map(|c|c.id)).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn rename_comparison(
        &self,
        workspace: Uuid,
        id: Uuid,
        version: i64,
        label: &str,
    ) -> Result<Comparison> {
        sqlx::query_as("UPDATE bb.comparisons SET label=$4 WHERE id=$1 AND workspace_id=$2 AND version=$3 RETURNING *").bind(id).bind(workspace).bind(version).bind(label).fetch_optional(&self.pool).await?.ok_or(Error::Conflict)
    }
    pub async fn delete_comparison(&self, workspace: Uuid, id: Uuid) -> Result<()> {
        if sqlx::query("DELETE FROM bb.comparisons WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(workspace)
            .execute(&self.pool)
            .await?
            .rows_affected()
            == 0
        {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    pub async fn save_report(&self, r: &NewReport) -> Result<ResearchReport> {
        hash(&r.body_sha256)?;
        match (r.kind.as_str(), r.run_id, r.comparison_id, r.sweep_id) {
            ("run", Some(id), None, None) => {
                let visible:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.runs r JOIN bb.experiments e ON e.experiment_config_id=r.experiment_config_id AND e.platform=r.platform JOIN bb.workspaces w ON w.id=e.workspace_id WHERE r.id=$1 AND r.quarantined_at IS NULL AND (e.workspace_id=$2 OR w.is_public_read))").bind(id).bind(r.workspace_id).fetch_one(&self.pool).await?;
                if !visible {
                    return Err(Error::NotFound);
                }
            }
            ("comparison", None, Some(id), None) => {
                self.comparison(r.workspace_id, id).await?;
            }
            ("sweep", None, None, Some(id)) => {
                self.sweep(r.workspace_id, id).await?;
            }
            _ => return Err(Error::Invalid("report subject must match kind".into())),
        }
        Ok(sqlx::query_as("INSERT INTO bb.research_reports (workspace_id,kind,run_id,comparison_id,sweep_id,title,generator_version,body,body_sha256,created_by_key_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING *")
            .bind(r.workspace_id).bind(&r.kind).bind(r.run_id).bind(r.comparison_id).bind(r.sweep_id).bind(&r.title).bind(r.generator_version).bind(&r.body).bind(&r.body_sha256).bind(r.created_by_key_id).fetch_one(&self.pool).await?)
    }
    pub async fn report(&self, workspace: Uuid, id: Uuid) -> Result<ResearchReport> {
        sqlx::query_as("SELECT r.* FROM bb.research_reports r JOIN bb.workspaces w ON w.id=r.workspace_id WHERE r.id=$1 AND (r.workspace_id=$2 OR w.is_public_read)").bind(id).bind(workspace).fetch_optional(&self.pool).await?.ok_or(Error::NotFound)
    }
    pub async fn reports(
        &self,
        workspace: Uuid,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<ResearchReport>> {
        Ok(sqlx::query_as("SELECT * FROM bb.research_reports WHERE workspace_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)<($2,$3)) ORDER BY created_at DESC,id DESC LIMIT $4").bind(workspace).bind(cursor.map(|c|c.created_at)).bind(cursor.map(|c|c.id)).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn delete_report(&self, workspace: Uuid, id: Uuid) -> Result<()> {
        if sqlx::query("DELETE FROM bb.research_reports WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(workspace)
            .execute(&self.pool)
            .await?
            .rows_affected()
            == 0
        {
            return Err(Error::NotFound);
        }
        Ok(())
    }
    pub async fn runs(
        &self,
        workspace: Uuid,
        platform: &str,
        cursor: Option<&Cursor>,
        count: i64,
    ) -> Result<Vec<Run>> {
        Ok(sqlx::query_as("SELECT r.* FROM bb.runs r WHERE r.platform=$2 AND r.quarantined_at IS NULL AND EXISTS (SELECT 1 FROM bb.experiments e WHERE e.workspace_id=$1 AND e.experiment_config_id=r.experiment_config_id AND e.platform=r.platform) AND ($3::timestamptz IS NULL OR (r.created_at,r.id)<($3,$4)) ORDER BY r.created_at DESC,r.id DESC LIMIT $5").bind(workspace).bind(platform).bind(cursor.map(|c|c.created_at)).bind(cursor.map(|c|c.id)).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn audit(&self, event: NewAuditEvent<'_>) -> Result<()> {
        let NewAuditEvent {
            workspace,
            key,
            action,
            outcome,
            target_type,
            target_id,
            request_id,
            client_ip,
            detail,
        } = event;
        sqlx::query("INSERT INTO bb.audit_events (workspace_id,api_key_id,action,outcome,target_type,target_id,request_id,client_ip,detail) VALUES ($1,$2,$3,$4,$5,$6,$7,$8::text::inet,$9)").bind(workspace).bind(key).bind(action).bind(outcome).bind(target_type).bind(target_id).bind(request_id).bind(client_ip).bind(detail).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn audit_events(
        &self,
        workspace: Uuid,
        before: Option<i64>,
        count: i64,
    ) -> Result<Vec<AuditEvent>> {
        Ok(sqlx::query_as("SELECT id,at,workspace_id,api_key_id,action,outcome,target_type,target_id,request_id,host(client_ip) AS client_ip,detail FROM bb.audit_events WHERE workspace_id=$1 AND ($2::bigint IS NULL OR id<$2) ORDER BY id DESC LIMIT $3").bind(workspace).bind(before).bind(limit(count)?).fetch_all(&self.pool).await?)
    }
    pub async fn queue_depth(&self) -> Result<Vec<QueueDepth>> {
        Ok(sqlx::query_as("SELECT platform,state,count(*) AS count FROM bb.jobs WHERE state IN ('created','queued','running','cancelling') GROUP BY platform,state ORDER BY platform,state").fetch_all(&self.pool).await?)
    }
    pub async fn oldest_queued_at(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(
            sqlx::query_scalar("SELECT min(queued_at) FROM bb.jobs WHERE state='queued'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
}
