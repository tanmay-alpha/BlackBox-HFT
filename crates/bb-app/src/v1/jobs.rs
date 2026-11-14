//! Jobs: list, detail with live progress, cancel (API_CONTRACT §8.5). Job status is `no-store`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use bb_db::{Cursor, JobView, Scope};
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    experiments::Params,
    model::{Empty, Job},
    respond::{
        Cache, Page, TruncatedBy, bounded, byte_cap, decode_cursor, encode_cursor, json,
        json_value, uuid_param,
    },
};

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(list_jobs))
        .routes(routes!(get_job))
        .routes(routes!(cancel_job))
}

const STATES: &[&str] = &[
    "created",
    "queued",
    "running",
    "cancelling",
    "completed",
    "failed",
    "cancelled",
    "expired",
];

pub fn job_resource(state: &Shared, view: &JobView, queue_position: Option<i64>) -> Job {
    Job::from_view(
        view,
        state.hub.job(view.job.id).and_then(|c| c.progress()),
        queue_position,
        state.hub.heartbeat_interval.as_millis() as u64,
    )
}

#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(dead_code)]
pub struct JobListQuery {
    pub workspace: Option<String>,
    /// Repeatable.
    pub state: Option<Vec<String>>,
    pub mine: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// Jobs linked to the workspace's experiments, newest first. Poll with the live states every
/// 2 s (UI §10.4).
#[utoipa::path(get, path = "/jobs", tag = "jobs", security((), ("bearer" = [])), params(JobListQuery),
    responses((status = 200, body = Page<Job>), (status = 400, body = ErrorEnvelope)))]
pub async fn list_jobs(
    State(state): State<Shared>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let p = Params(pairs);
    let workspace = caller.read_workspace(p.get("workspace"))?.clone();
    let states = p.all("state");
    if let Some(bad) = states.iter().find(|s| !STATES.contains(&s.as_str())) {
        return Err(ApiError::invalid_field(
            "state",
            format!("unknown state `{bad}`"),
        ));
    }
    let mine = if p.flag("mine")? {
        Some(caller.require(Scope::Read)?.id)
    } else {
        None
    };
    let limit = p.limit(50, 100)?;
    let filters = p.filters();
    let cursor: Option<Cursor> = p
        .get("cursor")
        .map(|c| decode_cursor(c, &filters))
        .transpose()?;
    let views = state
        .db
        .job_views(
            workspace.id,
            &states,
            mine,
            cursor.as_ref(),
            super::runs::peek(limit),
        )
        .await?;
    let more = super::runs::has_more(views.len(), limit);
    let views = &views[..views.len().min(limit as usize)];
    let jobs: Vec<Job> = views
        .iter()
        .map(|v| job_resource(&state, v, None))
        .collect();
    let (items, cut) = bounded(&jobs, byte_cap(&state, &caller))?;
    let next = if (cut || more) && !items.is_empty() {
        views.get(items.len() - 1).map(|v| {
            encode_cursor(
                &Cursor {
                    created_at: v.job.created_at,
                    id: v.job.id,
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
            "items": items, "next_cursor": next,
            "truncated_by": if cut { Some(TruncatedBy::Bytes) } else if more { Some(TruncatedBy::Limit) } else { None },
        }),
        Cache::NoStore,
    )
}

/// One job, with live progress when this process's worker holds its lease.
#[utoipa::path(get, path = "/jobs/{id}", tag = "jobs", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = Job), (status = 404, body = ErrorEnvelope)))]
pub async fn get_job(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = uuid_param("job", &id)?;
    let view = state
        .db
        .job_view(caller.workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("job"))?;
    let position = if view.job.state == "queued" {
        state.db.queue_position(id).await?
    } else {
        None
    };
    let mut job = job_resource(&state, &view, position);
    if view.job.state == "queued" {
        job.queue_depth = Some(state.hub.status().queue_depth);
    }
    json(StatusCode::OK, &job, Cache::NoStore)
}

/// Cancel: `queued`/`created` → `cancelled` now; `running` → `cancelling` (stops at the next
/// checkpoint); terminal → 409. A job another workspace still wants only detaches yours.
#[utoipa::path(post, path = "/jobs/{id}/cancel", tag = "jobs", security(("bearer" = [])),
    params(("id" = String, Path)), request_body = Empty,
    responses((status = 202, body = Job), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)))]
pub async fn cancel_job(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("job", &id)?;
    let result = state
        .db
        .request_cancel(workspace.id, id)
        .await
        .map_err(|e| match e {
            bb_db::Error::NotFound => ApiError::not_found("job"),
            bb_db::Error::Conflict => {
                ApiError::conflict("job_terminal", "the job already finished")
            }
            other => other.into(),
        })?;
    if result.job.state == "cancelling" {
        state.hub.cancel(id);
    }
    state.hub.bump();
    let view = state.db.job_view(workspace.id, id).await?;
    let mut job = job_resource(&state, &view, None);
    job.detached = Some(result.detached);
    json(StatusCode::ACCEPTED, &job, Cache::NoStore)
}
