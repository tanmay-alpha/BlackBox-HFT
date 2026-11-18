//! A row-version fingerprint of everything the anonymous demo snapshot reads (`bb-app`
//! `v1/snapshot.rs`), so a server can tell in one cheap round trip whether another process
//! (`blackbox demo seed`, a separate worker, an operator's SQL) changed those rows since its build.
//!
//! Per table: a digest of the sorted `(primary key, xmin)` pairs. Transaction IDs are allocated,
//! not committed, in order: their sum can stay equal after two updates. Keeping the association
//! with each key detects that case, as well as inserts, deletes and updates behind disabled
//! triggers (subject to digest collisions and transaction-ID wraparound, not a durable audit log).
//! JSON arrays make composite/text keys unambiguous. Scans and per-table sorts are bounded by the
//! demo/public catalogue, not the private experiment history; see PHASE6_RELIABILITY.md for cost
//! and the audited dependency list.

use uuid::Uuid;

use crate::{Database, Result};

const DEMO_DATA_VERSION: &str = r"
WITH
 s AS (SELECT * FROM bb.sweeps WHERE workspace_id=$1),
 e AS (SELECT t.*,t.xmin::text AS row_version FROM bb.experiments t
       WHERE workspace_id=$1 OR workspace_id IN (SELECT id FROM bb.workspaces WHERE is_public_read)
          OR sweep_id IN (SELECT id FROM s)),
 j AS (SELECT t.*,t.xmin::text AS row_version FROM bb.jobs t WHERE id IN (SELECT job_id FROM e)),
 c AS (SELECT t.*,t.xmin::text AS row_version FROM bb.experiment_configs t
       WHERE id IN (SELECT experiment_config_id FROM e)
          OR id IN (SELECT experiment_config_id FROM j)
          OR id IN (SELECT experiment_config_id FROM bb.runs WHERE id IN (SELECT run_id FROM j))),
 d AS (SELECT t.*,t.xmin::text AS row_version FROM bb.datasets t
       WHERE owner_workspace_id IS NULL OR owner_workspace_id=$1),
 v AS (SELECT t.*,t.xmin::text AS row_version FROM bb.dataset_versions t
       WHERE dataset_id IN (SELECT id FROM d) OR id IN (SELECT dataset_version_id FROM c))
SELECT concat_ws(' ',
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(id,row_version) ORDER BY id)::text,'[]')) FROM e),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(id,row_version) ORDER BY id)::text,'[]')) FROM j),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(id,row_version) ORDER BY id)::text,'[]')) FROM c),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.runs t WHERE experiment_config_id IN (SELECT id FROM c) OR id IN (SELECT run_id FROM j)),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.run_artifacts t WHERE run_id IN
   (SELECT id FROM bb.runs WHERE experiment_config_id IN (SELECT id FROM c)
                          OR id IN (SELECT run_id FROM j))),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.api_keys t WHERE id IN (SELECT created_by_key_id FROM e)),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.workspaces t),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.comparisons t WHERE workspace_id=$1),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.research_reports t WHERE workspace_id=$1),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.sweeps t WHERE workspace_id=$1),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(id,row_version) ORDER BY id)::text,'[]')) FROM d),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(id,row_version) ORDER BY id)::text,'[]')) FROM v),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.dataset_version_id,t.checker_version,t.xmin::text)
                            ORDER BY t.dataset_version_id,t.checker_version)::text,'[]'))
  FROM bb.dataset_quality_reports t WHERE dataset_version_id IN (SELECT id FROM v)),
 (SELECT md5(coalesce(jsonb_agg(jsonb_build_array(t.id,t.xmin::text) ORDER BY t.id)::text,'[]'))
  FROM bb.data_sources t WHERE id IN (SELECT data_source_id FROM d)))
";

impl Database {
    /// The fingerprint of the rows visible to the demo `workspace`'s anonymous lists and
    /// catalogues (one statement, one round trip). A sorted key/version digest per table avoids
    /// equal-sum cancellation; like any finite digest it is not a permanent change/audit counter.
    pub async fn demo_data_version(&self, workspace: Uuid) -> Result<String> {
        Ok(sqlx::query_scalar(DEMO_DATA_VERSION)
            .bind(workspace)
            .fetch_one(&self.pool)
            .await?)
    }
}
