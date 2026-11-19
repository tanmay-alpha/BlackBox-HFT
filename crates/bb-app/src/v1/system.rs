//! Operator routes (`admin` scope, API_CONTRACT §8.9). All `no-store`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use bb_db::Scope;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    respond::{Cache, Query, json, limit_param, uuid_param},
};
use crate::wire::at_ns;

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(workers))
        .routes(routes!(queue))
        .routes(routes!(events))
        .routes(routes!(usage))
        .routes(routes!(api_keys))
        .routes(routes!(revoke_key))
}

#[derive(Serialize, ToSchema)]
pub struct WorkerRow {
    pub worker_id: String,
    pub mode: String,
    pub platform: String,
    pub blackbox_version: String,
    pub state: String,
    pub slots: i16,
    pub current_job_id: Option<String>,
    pub last_heartbeat_age_ms: i64,
}

#[utoipa::path(get, path = "/system/workers", tag = "system", security(("bearer" = [])),
    responses((status = 200, body = Vec<WorkerRow>), (status = 403, body = ErrorEnvelope)))]
pub async fn workers(State(state): State<Shared>, caller: Caller) -> Result<Response, ApiError> {
    caller.require(Scope::Admin)?;
    let now = chrono::Utc::now();
    let rows: Vec<WorkerRow> = state
        .db
        .workers()
        .await?
        .into_iter()
        .map(|w| WorkerRow {
            worker_id: w.worker_id,
            mode: w.mode,
            platform: w.platform,
            blackbox_version: w.blackbox_version,
            state: w.state,
            slots: w.slots,
            current_job_id: w.current_job_id.map(|j| j.to_string()),
            last_heartbeat_age_ms: (now - w.last_heartbeat_at).num_milliseconds(),
        })
        .collect();
    json(StatusCode::OK, &rows, Cache::NoStore)
}

#[derive(Serialize, ToSchema)]
pub struct QueueView {
    pub depth_by_platform: serde_json::Map<String, serde_json::Value>,
    pub running: i64,
    pub oldest_queued_age_ms: Option<i64>,
    pub lease_reclaims_24h: i64,
}

#[utoipa::path(get, path = "/system/queue", tag = "system", security(("bearer" = [])),
    responses((status = 200, body = QueueView), (status = 403, body = ErrorEnvelope)))]
pub async fn queue(State(state): State<Shared>, caller: Caller) -> Result<Response, ApiError> {
    caller.require(Scope::Admin)?;
    let stats = state.db.queue_stats(None).await?;
    let mut depth = serde_json::Map::new();
    for row in state.db.queue_depth().await? {
        if row.state == "queued" {
            depth.insert(row.platform, json!(row.count));
        }
    }
    json(
        StatusCode::OK,
        &QueueView {
            depth_by_platform: depth,
            running: stats.running,
            oldest_queued_age_ms: stats
                .oldest_queued_at
                .map(|t| (chrono::Utc::now() - t).num_milliseconds()),
            lease_reclaims_24h: stats.lease_reclaims_24h,
        },
        Cache::NoStore,
    )
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventsQuery {
    pub kind: Option<String>,
    pub level: Option<String>,
    /// Event id to continue before.
    pub before: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct EventRow {
    pub id: i64,
    pub at_ns: String,
    /// `system` or `audit`.
    pub source: String,
    pub level: Option<String>,
    pub kind: String,
    pub message: String,
    #[schema(value_type = Object)]
    pub detail: serde_json::Value,
}

#[utoipa::path(get, path = "/system/events", tag = "system", security(("bearer" = [])), params(EventsQuery),
    responses((status = 200, body = Vec<EventRow>), (status = 403, body = ErrorEnvelope)))]
pub async fn events(
    State(state): State<Shared>,
    caller: Caller,
    Query(q): Query<EventsQuery>,
) -> Result<Response, ApiError> {
    let key = caller.require(Scope::Admin)?;
    let limit = limit_param(q.limit, 50, 100)?;
    let mut rows: Vec<EventRow> = state
        .db
        .events(q.before, q.kind.as_deref(), q.level.as_deref(), limit)
        .await?
        .into_iter()
        .map(|e| EventRow {
            id: e.id,
            at_ns: at_ns(e.at),
            source: "system".into(),
            level: Some(e.level),
            kind: e.kind,
            message: e.message,
            detail: e.detail,
        })
        .collect();
    rows.extend(
        state
            .db
            .audit_events(key.workspace_id, q.before, limit)
            .await?
            .into_iter()
            .map(|a| EventRow {
                id: a.id,
                at_ns: at_ns(a.at),
                source: "audit".into(),
                level: None,
                kind: a.action,
                message: a.outcome,
                detail: a.detail,
            }),
    );
    json(StatusCode::OK, &rows, Cache::NoStore)
}

#[utoipa::path(get, path = "/system/usage", tag = "system", security(("bearer" = [])),
    responses((status = 200, body = Object), (status = 403, body = ErrorEnvelope)))]
pub async fn usage(State(state): State<Shared>, caller: Caller) -> Result<Response, ApiError> {
    caller.require(Scope::Admin)?;
    let rows = state.db.storage_usage().await?;
    let blob = rows.iter().find(|r| r.storage_backend == "vercel_blob");
    json(
        StatusCode::OK,
        &json!({
            "blob": {
                "month": blob.map(|b| b.month.to_string()),
                "uploads": blob.map_or(0, |b| b.uploads), "uploads_limit": 2000,
                "reads": blob.map_or(0, |b| b.reads), "reads_limit": 10000,
                "bytes_uploaded": blob.map_or(0, |b| b.bytes_uploaded), "bytes_limit": 1_000_000_000i64,
            },
            "egress": {
                "anonymous_today": state.egress.today(true),
                "anonymous_budget": state.cfg.egress_anonymous_daily_bytes,
                "keyed_today": state.egress.today(false),
                "keyed_budget": state.cfg.egress_keyed_daily_bytes,
            },
        }),
        Cache::NoStore,
    )
}

#[derive(Serialize, ToSchema)]
pub struct KeyRow {
    pub id: String,
    pub workspace_id: String,
    pub label: String,
    pub display_prefix: String,
    pub scopes: Vec<String>,
    pub created_at_ns: String,
    pub expires_at_ns: Option<String>,
    pub revoked_at_ns: Option<String>,
    pub last_used_at_ns: Option<String>,
}

fn key_row(k: bb_db::ApiKey) -> KeyRow {
    KeyRow {
        id: k.id.to_string(),
        workspace_id: k.workspace_id.to_string(),
        label: k.label,
        display_prefix: k.display_prefix,
        scopes: k.scopes,
        created_at_ns: at_ns(k.created_at),
        expires_at_ns: k.expires_at.map(at_ns),
        revoked_at_ns: k.revoked_at.map(at_ns),
        last_used_at_ns: k.last_used_at.map(at_ns),
    }
}

#[utoipa::path(get, path = "/system/api-keys", tag = "system", security(("bearer" = [])),
    responses((status = 200, body = Vec<KeyRow>), (status = 403, body = ErrorEnvelope)))]
pub async fn api_keys(State(state): State<Shared>, caller: Caller) -> Result<Response, ApiError> {
    caller.require(Scope::Admin)?;
    let rows: Vec<KeyRow> = state
        .db
        .all_api_keys()
        .await?
        .into_iter()
        .map(key_row)
        .collect();
    json(StatusCode::OK, &rows, Cache::NoStore)
}

#[utoipa::path(post, path = "/system/api-keys/{id}/revoke", tag = "system", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = KeyRow), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)))]
pub async fn revoke_key(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require(Scope::Admin)?;
    let id = uuid_param("api key", &id)?;
    let key = state
        .db
        .revoke_key_any(id, &caller.audit(&caller.workspace))
        .await
        .map_err(|_| ApiError::not_found("api key"))?;
    json(StatusCode::OK, &key_row(key), Cache::NoStore)
}
