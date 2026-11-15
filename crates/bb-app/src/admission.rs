//! The one admission path (TARGET_ARCHITECTURE §8.1): every experiment, sweep cell and demo
//! seed passes [`admit`] and enters the same `bb.jobs` queue through [`submit`].

use bb_core::{
    CONTRACT_VERSION, ENGINE_NAME, ENGINE_VERSION,
    config::{RunConfig, StrategyKind},
    dataset::DatasetManifest,
    limits::{ExecutionLimits, WorkEstimate},
};
use bb_db::{Database, DatasetVersion, ExperimentConfig, NewConfig, Submission, Submit};
use serde_json::json;
use uuid::Uuid;

use crate::{
    config::Profile, metrics::Metrics, pipeline, registry::manifest_of, v1::error::ApiError,
    worker::WorkerHub,
};

/// Most scripted orders one hosted configuration may carry (SEC-7).
pub const MAX_SCRIPTED_ORDERS_HOSTED: usize = 1_000;

/// A configuration that passed validation and the work budget.
pub struct Admitted {
    pub config: RunConfig,
    pub version: DatasetVersion,
    pub manifest: DatasetManifest,
    pub estimate: WorkEstimate,
    pub config_row: ExperimentConfig,
}

/// Validates `config` against the current version of its dataset and the profile's limits,
/// then registers its identity (idempotent).
pub async fn admit(
    db: &Database,
    workspace: Uuid,
    profile: Profile,
    limits: &ExecutionLimits,
    config: RunConfig,
) -> Result<Admitted, ApiError> {
    let mut problems = config.validate();
    if profile == Profile::Hosted
        && let StrategyKind::Scripted(params) = &config.strategy.kind
        && params.orders.len() > MAX_SCRIPTED_ORDERS_HOSTED
    {
        problems.insert(
            0,
            format!(
                "strategy.kind.orders has {} orders; at most {MAX_SCRIPTED_ORDERS_HOSTED} are \
                 accepted here",
                params.orders.len()
            ),
        );
    }
    if !problems.is_empty() {
        return Err(ApiError::invalid_config(problems));
    }
    // The dataset must be visible to the workspace (global or owned).
    db.dataset(workspace, &config.dataset_id)
        .await
        .map_err(|e| match e {
            bb_db::Error::NotFound => ApiError::not_found("dataset"),
            other => other.into(),
        })?;
    let version = db
        .current_version(&config.dataset_id)
        .await
        .map_err(|e| match e {
            bb_db::Error::NotFound => ApiError::not_found("dataset"),
            other => other.into(),
        })?;
    let manifest = manifest_of(&version)?;
    let estimate = config.estimate_work(&manifest);
    if let Err(violation) = limits.admit(&estimate) {
        return Err(ApiError::budget_exceeded(
            format!(
                "{violation} (the dataset spans {:.3} s; the strategy interval is {} ns)",
                manifest.end.saturating_sub(manifest.start).max(0) as f64 / 1e9,
                config.strategy.interval_ns
            ),
            json!({
                "what": violation.what,
                "limit": violation.limit,
                "estimate": violation.predicted,
                "profile": profile.as_str(),
            }),
        ));
    }
    let identity = pipeline::run_identity(&manifest, &config);
    let fingerprint = hex::decode(&identity.config_sha256)
        .map_err(|_| ApiError::internal("identity is not hex"))?;
    let count = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    let new_config = NewConfig {
        fingerprint,
        dataset_version_id: version.id,
        dataset_id: version.dataset_id.clone(),
        contract_version: i32::try_from(CONTRACT_VERSION).unwrap_or(1),
        simulation_revision: i32::try_from(bb_core::SIMULATION_REVISION).unwrap_or(1),
        engine_name: ENGINE_NAME.into(),
        engine_version: ENGINE_VERSION.into(),
        config: serde_json::to_value(&config)?,
        est_ticks: count(estimate.ticks),
        est_tape_events_upper: count(estimate.tape_events_upper),
        est_output_bytes_upper: count(estimate.output_bytes_upper),
    };
    // Concurrent first registrations of one identity can collide on the `short_id` unique
    // index (ON CONFLICT only arbitrates the fingerprint); a retry then finds the winner's row.
    let mut attempt = 0;
    let config_row = loop {
        match db.register_config(&new_config).await {
            Ok(row) => break row,
            Err(bb_db::Error::Conflict) if attempt < 3 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20 * attempt)).await;
            }
            Err(bb_db::Error::Conflict) => {
                return Err(ApiError::conflict(
                    "identity_collision",
                    "another configuration already uses this 16-hex run id",
                ));
            }
            Err(other) => return Err(other.into()),
        }
    };
    Ok(Admitted {
        config,
        version,
        manifest,
        estimate,
        config_row,
    })
}

/// Options of one submission.
pub struct SubmitOptions<'a> {
    pub workspace: Uuid,
    pub key: Option<Uuid>,
    pub label: &'a str,
    pub platform: &'a str,
    pub priority: i16,
    pub deadline_ms: i32,
    /// `(sweep, cell index, segment)` of a sweep run.
    pub sweep: Option<(Uuid, i16, &'static str)>,
    pub bypass_quotas: bool,
    pub storage_backend: &'a str,
}

/// Single-flight submission (DATABASE_DESIGN §13.3): N identical concurrent submissions create
/// one job. Wakes the in-process worker when a job is live.
pub async fn submit(
    db: &Database,
    hub: &WorkerHub,
    metrics: &Metrics,
    admitted: &Admitted,
    opts: SubmitOptions<'_>,
) -> Result<Submission, ApiError> {
    let mut s = Submit::new(admitted.config_row.id, opts.workspace, opts.platform);
    s.api_key_id = opts.key;
    s.label = opts.label.to_string();
    s.priority = opts.priority;
    s.deadline_ms = opts.deadline_ms;
    s.sweep = opts.sweep;
    s.bypass_quotas = opts.bypass_quotas;
    s.storage_backend = opts.storage_backend.to_string();
    s.request_id = Some(crate::v1::error::request_id()).filter(|id| id != "-");
    let submission = db.submit(&s).await.map_err(|e| match e {
        // Limit and usage are filled in by `quota_details`.
        bb_db::Error::Quota(scope) => ApiError::quota_exceeded(scope, 0, 0),
        bb_db::Error::NotFound => ApiError::not_found("dataset"),
        other => other.into(),
    })?;
    if submission.job.is_some() {
        Metrics::inc(&metrics.jobs_submitted_total);
    } else {
        Metrics::inc(&metrics.jobs_deduplicated_total);
    }
    hub.submitted();
    Ok(submission)
}

/// Fills `quota_exceeded` details with the real limit and usage.
pub async fn quota_details(
    db: &Database,
    workspace: &bb_db::Workspace,
    key: Option<Uuid>,
    err: ApiError,
) -> ApiError {
    if err.code != "quota_exceeded" {
        return err;
    }
    let scope = err.details["scope"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // The rejected transaction has rolled back; record the keyed denial separately. There
    // is no successful mutation to commit with it, and anonymous requests never reach here.
    if let Some(key) = key {
        let request_id = crate::v1::error::request_id();
        let detail = serde_json::json!({ "scope": scope });
        if let Err(error) = db
            .audit(bb_db::NewAuditEvent {
                workspace: Some(workspace.id),
                key: Some(key),
                action: if scope == "storage" {
                    "storage.budget_exceeded"
                } else {
                    "quota.exceeded"
                },
                outcome: "denied",
                target_type: Some("workspace"),
                target_id: Some(&workspace.slug),
                request_id: Some(&request_id),
                client_ip: None,
                detail: &detail,
            })
            .await
        {
            return error.into();
        }
    }
    let Ok(used) = db.quota_usage(workspace.id, key).await else {
        return err;
    };
    let (limit, used) = match scope.as_str() {
        "experiments_per_day" => (
            i64::from(workspace.quota_experiments_per_day),
            used.experiments_today,
        ),
        "sweeps_per_day" => (i64::from(workspace.quota_sweeps_per_day), used.sweeps_today),
        "active_jobs" => (i64::from(workspace.quota_active_jobs), used.active_jobs),
        _ => return err,
    };
    ApiError::quota_exceeded(&scope, limit, used)
}
