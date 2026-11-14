//! Experiments: submit (= enqueue a job, single-flight on the fingerprint), list, detail,
//! rename, delete (API_CONTRACT §8.3).

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use bb_db::{Cursor, ExperimentQuery, ExperimentView};
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    model::{Experiment, Job, LabelPatch, SubmitExperiment, Submitted},
    respond::{
        ApiJson, Cache, Page, TruncatedBy, bounded, byte_cap, check_label, decode_cursor,
        encode_cursor, if_match, json, json_value, limit_param, uuid_param,
    },
    runs::verified_summary,
};
use crate::admission::{self, SubmitOptions};

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(create_experiment, list_experiments))
        .routes(routes!(get_experiment, patch_experiment, delete_experiment))
}

/// Query parameters as repeated pairs (`status` and `state` may repeat).
pub struct Params(pub Vec<(String, String)>);

impl Params {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }
    /// Canonical filter string for cursors (everything except `cursor`/`limit`).
    pub fn filters(&self) -> String {
        let mut pairs: Vec<String> = self
            .0
            .iter()
            .filter(|(k, _)| k != "cursor" && k != "limit")
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        pairs.sort();
        pairs.join("&")
    }
    pub fn limit(&self, default: i64, max: i64) -> Result<i64, ApiError> {
        let raw = self
            .get("limit")
            .map(|v| {
                v.parse::<i64>()
                    .map_err(|_| ApiError::invalid_field("limit", "limit must be an integer"))
            })
            .transpose()?;
        limit_param(raw, default, max)
    }
    pub fn flag(&self, name: &str) -> Result<bool, ApiError> {
        match self.get(name) {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            Some(_) => Err(ApiError::invalid_field(
                name,
                format!("{name} must be true or false"),
            )),
        }
    }
}

const EXPERIMENT_STATES: &[&str] = &[
    "created",
    "queued",
    "running",
    "cancelling",
    "completed",
    "failed",
    "cancelled",
    "expired",
    "detached",
];

/// Builds the experiment filter shared by `/experiments` and `/runs`.
pub fn experiment_query(p: &Params, caller: &Caller) -> Result<ExperimentQuery, ApiError> {
    let states = p.all("status");
    if let Some(bad) = states
        .iter()
        .find(|s| !EXPERIMENT_STATES.contains(&s.as_str()))
    {
        return Err(ApiError::invalid_field(
            "status",
            format!("unknown status `{bad}`"),
        ));
    }
    let id = |name: &str| -> Result<Option<Uuid>, ApiError> {
        p.get(name)
            .map(|v| {
                v.parse()
                    .map_err(|_| ApiError::invalid_field(name, format!("{name} must be a UUID")))
            })
            .transpose()
    };
    Ok(ExperimentQuery {
        dataset_id: p.get("dataset_id").map(str::to_string),
        strategy_kind: p.get("strategy_kind").map(str::to_string),
        states,
        sweep_id: id("sweep_id")?,
        label_prefix: p.get("label_prefix").map(str::to_string),
        created_after: super::respond::at_param("created_after_ns", p.get("created_after_ns"))?,
        created_before: super::respond::at_param("created_before_ns", p.get("created_before_ns"))?,
        created_by_key: if p.flag("mine")? {
            Some(caller.require(bb_db::Scope::Read)?.id)
        } else {
            None
        },
        run_identity: p.get("run_id").map(str::to_ascii_lowercase),
    })
}

/// Experiment resources for views, with their jobs and verified runs (two batched queries).
pub async fn experiment_resources(
    state: &Shared,
    caller: &Caller,
    views: &[ExperimentView],
    include_config: bool,
) -> Result<Vec<Experiment>, ApiError> {
    let job_ids: Vec<Uuid> = views.iter().filter_map(|v| v.experiment.job_id).collect();
    let jobs: HashMap<Uuid, bb_db::JobView> = if job_ids.is_empty() {
        HashMap::new()
    } else {
        state
            .db
            .job_views_by_ids(&job_ids)
            .await?
            .into_iter()
            .map(|j| (j.job.id, j))
            .collect()
    };
    let run_ids: Vec<Uuid> = views.iter().filter_map(|v| v.live_run_id).collect();
    let runs: HashMap<Uuid, bb_db::RunView> = if run_ids.is_empty() {
        HashMap::new()
    } else {
        state
            .db
            .run_views_by_ids(caller.workspace.id, &run_ids)
            .await?
            .into_iter()
            .map(|r| (r.run.id, r))
            .collect()
    };
    let hb = state.hub.heartbeat_interval.as_millis() as u64;
    let mut out = Vec::with_capacity(views.len());
    for view in views {
        let job = view
            .experiment
            .job_id
            .and_then(|id| jobs.get(&id))
            .map(|j| {
                Job::from_view(
                    j,
                    state.hub.job(j.job.id).and_then(|c| c.progress()),
                    None,
                    hb,
                )
            });
        let run = match view.live_run_id.and_then(|id| runs.get(&id)) {
            Some(r) => Some(verified_summary(state, r, false).await?),
            None => None,
        };
        out.push(Experiment::from_view(view, job, run, include_config)?);
    }
    Ok(out)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceParam {
    /// Workspace slug: your own (default) or `demo` (writes to demo need `admin`).
    pub workspace: Option<String>,
}

/// Submit an experiment: validates, admits against the work budget and quotas, and enqueues a
/// job, or returns the existing job or run for the same fingerprint (idempotent).
#[utoipa::path(post, path = "/experiments", tag = "experiments", security(("bearer" = [])),
    params(WorkspaceParam), request_body = SubmitExperiment,
    responses((status = 202, body = Submitted), (status = 200, body = Submitted),
              (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)))]
pub async fn create_experiment(
    State(state): State<Shared>,
    caller: Caller,
    Query(ws): Query<WorkspaceParam>,
    ApiJson(body): ApiJson<SubmitExperiment>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(ws.workspace.as_deref())?.clone();
    let label = check_label("label", body.label.as_deref().unwrap_or(""), true)?;
    let label = if label.is_empty() {
        body.config.strategy.kind.label().to_string()
    } else {
        label
    };
    let admitted = admission::admit(
        &state.db,
        workspace.id,
        state.cfg.profile,
        &state.cfg.limits,
        body.config,
    )
    .await?;
    let submission = match admission::submit(
        &state.db,
        &state.hub,
        &state.metrics,
        &admitted,
        SubmitOptions {
            workspace: workspace.id,
            key: caller.key_id(),
            label: &label,
            platform: &state.cfg.result_platform,
            priority: if workspace.kind == "demo" { 10 } else { 100 },
            deadline_ms: state.cfg.deadline_ms,
            sweep: None,
            bypass_quotas: workspace.kind == "demo",
            storage_backend: state.storage.backend,
        },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            return Err(admission::quota_details(&state.db, &workspace, caller.key_id(), e).await);
        }
    };
    let view = state
        .db
        .experiment_view(workspace.id, submission.experiment.id)
        .await?;
    let caller_for_view = Caller {
        workspace: workspace.clone(),
        ..caller.clone()
    };
    let mut resources =
        experiment_resources(&state, &caller_for_view, std::slice::from_ref(&view), false).await?;
    let experiment = resources
        .pop()
        .ok_or_else(|| ApiError::internal("experiment vanished"))?;
    let mut job = match &submission.job {
        Some(j) => Some(state.db.job_view(workspace.id, j.id).await?),
        None => None,
    }
    .map(|j| {
        Job::from_view(
            &j,
            state.hub.job(j.job.id).and_then(|c| c.progress()),
            None,
            state.hub.heartbeat_interval.as_millis() as u64,
        )
    });
    if let Some(job) = &mut job
        && job.state == "queued"
    {
        job.queue_position = state
            .db
            .queue_position(job.id.parse().unwrap_or_default())
            .await?;
    }
    let run = experiment.run.as_ref().map(|_| ());
    let status = if submission.job.is_some() && submission.created && run.is_none() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    let run_summary = match &submission.run {
        Some(_) => {
            experiment_resources(&state, &caller_for_view, std::slice::from_ref(&view), false)
                .await?
                .pop()
                .and_then(|e| e.run)
        }
        None => None,
    };
    let location = format!("/api/v1/experiments/{}", experiment.id);
    let mut response = json(
        status,
        &Submitted {
            created: submission.created,
            experiment,
            job,
            run: run_summary,
        },
        Cache::NoStore,
    )?;
    if let Ok(v) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, v);
    }
    Ok(response)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(dead_code)]
pub struct ExperimentListQuery {
    pub workspace: Option<String>,
    pub dataset_id: Option<String>,
    pub strategy_kind: Option<String>,
    /// Repeatable: `created` … `expired`, `completed`, `detached`.
    pub status: Option<Vec<String>>,
    pub sweep_id: Option<String>,
    pub label_prefix: Option<String>,
    pub created_after_ns: Option<String>,
    pub created_before_ns: Option<String>,
    /// Only experiments created by this key.
    pub mine: Option<bool>,
    /// 16-hex run id or 64-hex fingerprint.
    pub run_id: Option<String>,
    pub cursor: Option<String>,
    /// 1..=100, default 50.
    pub limit: Option<i64>,
}

/// List experiments of a workspace, newest first.
#[utoipa::path(get, path = "/experiments", tag = "experiments", security((), ("bearer" = [])),
    params(ExperimentListQuery),
    responses((status = 200, body = Page<Experiment>), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)))]
pub async fn list_experiments(
    State(state): State<Shared>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let p = Params(pairs);
    let workspace = caller.read_workspace(p.get("workspace"))?.clone();
    let q = experiment_query(&p, &caller)?;
    let limit = p.limit(50, 100)?;
    let filters = p.filters();
    let cursor: Option<Cursor> = p
        .get("cursor")
        .map(|c| decode_cursor(c, &filters))
        .transpose()?;
    let views = state
        .db
        .experiment_views(workspace.id, &q, cursor.as_ref(), limit + 1)
        .await?;
    let more = views.len() as i64 > limit;
    let views = &views[..views.len().min(limit as usize)];
    let caller_view = Caller {
        workspace: workspace.clone(),
        ..caller.clone()
    };
    let resources = experiment_resources(&state, &caller_view, views, false).await?;
    let (items, cut) = bounded(&resources, byte_cap(&state, &caller))?;
    let last = views.get(items.len().saturating_sub(1));
    let next = if (cut || more) && !items.is_empty() {
        last.map(|v| {
            encode_cursor(
                &Cursor {
                    created_at: v.experiment.created_at,
                    id: v.experiment.id,
                },
                &filters,
            )
        })
    } else {
        None
    };
    json_value(
        StatusCode::OK,
        &json!({
            "items": items,
            "next_cursor": next,
            "truncated_by": if cut { Some(TruncatedBy::Bytes) } else if more { Some(TruncatedBy::Limit) } else { None },
        }),
        Cache::NoStore,
    )
}

/// One experiment with its configuration, job and run.
#[utoipa::path(get, path = "/experiments/{id}", tag = "experiments", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = Experiment), (status = 404, body = ErrorEnvelope)))]
pub async fn get_experiment(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = uuid_param("experiment", &id)?;
    let view = state
        .db
        .experiment_view(caller.workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("experiment"))?;
    let mut items =
        experiment_resources(&state, &caller, std::slice::from_ref(&view), true).await?;
    let experiment = items
        .pop()
        .ok_or_else(|| ApiError::not_found("experiment"))?;
    let response = json(StatusCode::OK, &experiment, Cache::Revalidate)?;
    Ok(super::respond::conditional(&headers, response))
}

/// Rename (optimistic concurrency: `If-Match: "v<version>"`).
#[utoipa::path(patch, path = "/experiments/{id}", tag = "experiments", security(("bearer" = [])),
    params(("id" = String, Path), ("If-Match" = String, Header)), request_body = LabelPatch,
    responses((status = 200, body = Experiment), (status = 409, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)))]
pub async fn patch_experiment(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LabelPatch>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("experiment", &id)?;
    let version = if_match(&headers, true)?.unwrap_or_default();
    let label = check_label("label", &body.label, true)?;
    // Visible-but-foreign experiments are 404 like unknown ids (SEC-2).
    let current = state
        .db
        .experiment_view(workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("experiment"))?;
    if current.experiment.workspace_id != workspace.id {
        return Err(ApiError::not_found("experiment"));
    }
    state
        .db
        .rename_experiment(workspace.id, id, version, &label)
        .await
        .map_err(|e| match e {
            bb_db::Error::Conflict => ApiError::conflict(
                "version_mismatch",
                "the experiment changed since you read it",
            )
            .with_details(json!({ "reason": "version_mismatch", "current_version": current.experiment.version })),
            other => other.into(),
        })?;
    state.hub.bump();
    let view = state.db.experiment_view(workspace.id, id).await?;
    let mut items =
        experiment_resources(&state, &caller, std::slice::from_ref(&view), true).await?;
    let experiment = items
        .pop()
        .ok_or_else(|| ApiError::not_found("experiment"))?;
    json(StatusCode::OK, &experiment, Cache::NoStore)
}

/// Detach the experiment; its job is cancelled when no other experiment wants it.
#[utoipa::path(delete, path = "/experiments/{id}", tag = "experiments", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 204), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)))]
pub async fn delete_experiment(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("experiment", &id)?;
    let version = if_match(&headers, false)?;
    let job_id = state
        .db
        .experiment(workspace.id, id)
        .await
        .ok()
        .filter(|e| e.workspace_id == workspace.id)
        .ok_or_else(|| ApiError::not_found("experiment"))?
        .job_id;
    state
        .db
        .delete_experiment(workspace.id, id, version)
        .await
        .map_err(|e| match e {
            bb_db::Error::NotFound => ApiError::not_found("experiment"),
            bb_db::Error::Conflict => ApiError::conflict(
                "version_mismatch",
                "the experiment changed since you read it",
            ),
            other => other.into(),
        })?;
    if let Some(job) = job_id
        && let Ok(row) = state.db.job(job).await
        && row.state == "cancelling"
    {
        state.hub.cancel(job);
    }
    state.hub.bump();
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}
