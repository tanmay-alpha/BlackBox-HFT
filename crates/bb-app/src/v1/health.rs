//! Health, readiness, metrics, status, identity and the OpenAPI document (API_CONTRACT §8.1).
//! `/healthz`, `/readyz` and `/api/v1/status` answer from memory (TARGET_ARCHITECTURE §14,
//! SEC-10); only `GET /readyz?probe=1` (admin) runs a database round trip.

use std::{sync::atomic::Ordering, time::Duration};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use bb_core::CONTRACT_VERSION;
use bb_db::Scope;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    OPENAPI_JSON, Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    model::{
        Health, KeyRef, LimitsSummary, QueueSummary, QuotaUse, Quotas, Ready, Status, Whoami,
        WorkerSummary, WorkspaceRef,
    },
    respond::{Cache, json},
};
use crate::{DISCLAIMER, wire::at_ns};

pub fn root_routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(metrics))
}

/// A limit as a safe JSON number; "unlimited" (above 2^53 − 1) is `null`.
fn finite(v: u64) -> Option<u64> {
    (v <= crate::wire::MAX_SAFE_INTEGER).then_some(v)
}

pub fn v1_routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(whoami))
        .routes(routes!(openapi))
}

/// Liveness: the process answers. Never touches the database.
#[utoipa::path(get, path = "/healthz", tag = "health",
    responses((status = 200, body = Health)))]
pub async fn healthz() -> Result<Response, ApiError> {
    json(
        StatusCode::OK,
        &Health {
            status: "ok".into(),
        },
        Cache::NoStore,
    )
}

#[derive(Deserialize, IntoParams)]
pub struct ReadyQuery {
    /// `1` runs a live database probe (admin only).
    pub probe: Option<u8>,
}

/// Readiness from the state of the last database operation (the start-up migration proves
/// reachability). `probe=1` runs a `SELECT`-level check and needs `admin`.
#[utoipa::path(get, path = "/readyz", tag = "health", params(ReadyQuery),
    responses((status = 200, body = Ready), (status = 503, body = ErrorEnvelope),
              (status = 403, body = ErrorEnvelope)))]
pub async fn readyz(
    State(state): State<Shared>,
    caller: Option<axum::Extension<Caller>>,
    Query(query): Query<ReadyQuery>,
) -> Result<Response, ApiError> {
    if state.hub.is_draining() {
        return Err(ApiError::unavailable("draining", 5));
    }
    if query.probe == Some(1) {
        let caller = caller.ok_or_else(|| ApiError::unauthorized(false))?;
        caller.require(Scope::Admin)?;
        let probe = tokio::time::timeout(Duration::from_secs(2), state.db.queue_stats(None)).await;
        match probe {
            Ok(Ok(_)) => state.db_ready.store(true, Ordering::SeqCst),
            _ => {
                state.db_ready.store(false, Ordering::SeqCst);
                return Err(ApiError::unavailable("database", 5));
            }
        }
    }
    if !state.db_ready.load(Ordering::SeqCst) {
        return Err(ApiError::unavailable("database", 5));
    }
    json(
        StatusCode::OK,
        &Ready {
            status: "ready".into(),
            db: "ok".into(),
            migrations: bb_db::MIGRATOR
                .iter()
                .map(|m| m.version)
                .max()
                .map(|v| format!("{v:04}"))
                .unwrap_or_default(),
            blob: if state.storage.backend == "vercel_blob" {
                "configured".into()
            } else {
                "filesystem".into()
            },
        },
        Cache::NoStore,
    )
}

/// Prometheus text format (`admin`).
#[utoipa::path(get, path = "/metrics", tag = "health", security(("bearer" = [])),
    responses((status = 200, content_type = "text/plain", body = String),
              (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope)))]
pub async fn metrics(State(state): State<Shared>, caller: Caller) -> Result<Response, ApiError> {
    caller.require(Scope::Admin)?;
    let mut response = state.metrics.render().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The UI status bar: worker state, heartbeat age, queue depth, limits (memory only).
#[utoipa::path(get, path = "/status", tag = "health",
    responses((status = 200, body = Status)))]
pub async fn status(State(state): State<Shared>) -> Result<Response, ApiError> {
    let w = state.hub.status();
    let cfg = &state.cfg;
    json(
        StatusCode::OK,
        &Status {
            status: if state.hub.is_draining() {
                "draining"
            } else {
                "ok"
            }
            .into(),
            api_version: "v1".into(),
            contract_version: CONTRACT_VERSION,
            blackbox_version: env!("CARGO_PKG_VERSION").into(),
            git_sha: cfg.git_sha.clone(),
            engine: crate::pipeline::engine_info(),
            server_time_ns: at_ns(chrono::Utc::now()),
            worker: WorkerSummary {
                mode: state.hub.mode.into(),
                state: w.state.into(),
                slots: state.hub.slots,
                busy: w.busy,
                last_heartbeat_age_ms: w
                    .last_heartbeat
                    .map(|t| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)),
                heartbeat_interval_ms: u64::try_from(cfg.heartbeat_interval.as_millis())
                    .unwrap_or(15_000),
            },
            queue: QueueSummary {
                depth: w.queue_depth,
                running: w.running,
                as_of_ns: w.queue_as_of.map(at_ns),
            },
            limits: LimitsSummary {
                profile: cfg.profile.as_str().into(),
                max_ticks: finite(cfg.limits.max_ticks),
                max_tape_events: finite(cfg.limits.max_tape_events),
                max_output_bytes: finite(cfg.limits.max_output_bytes),
                max_sweep_cells: cfg.max_sweep_cells,
                deadline_ms: cfg.deadline_ms,
            },
            result_platform: cfg.result_platform.clone(),
            disclaimer: DISCLAIMER.into(),
        },
        Cache::NoStore,
    )
}

/// Who the caller is, and quota usage for keyed callers.
#[utoipa::path(get, path = "/whoami", tag = "health", security((), ("bearer" = [])),
    responses((status = 200, body = Whoami), (status = 401, body = ErrorEnvelope)))]
pub async fn whoami(State(state): State<Shared>, caller: Caller) -> Result<Response, ApiError> {
    let ws = &caller.workspace;
    let body = match &caller.key {
        None => Whoami {
            mode: "demo".into(),
            workspace: WorkspaceRef {
                slug: ws.slug.clone(),
                name: ws.name.clone(),
                kind: ws.kind.clone(),
            },
            key: None,
            quotas: None,
        },
        Some(key) => {
            let used = state.db.quota_usage(ws.id, Some(key.id)).await?;
            Whoami {
                mode: if ws.kind == "demo" {
                    "demo"
                } else {
                    "research"
                }
                .into(),
                workspace: WorkspaceRef {
                    slug: ws.slug.clone(),
                    name: ws.name.clone(),
                    kind: ws.kind.clone(),
                },
                key: Some(KeyRef {
                    id: key.id.to_string(),
                    label: key.label.clone(),
                    display_prefix: key.display_prefix.clone(),
                    scopes: key.scopes.clone(),
                    expires_at_ns: key.expires_at.map(at_ns),
                }),
                quotas: Some(Quotas {
                    experiments_per_day: QuotaUse {
                        limit: i64::from(ws.quota_experiments_per_day),
                        used: used.experiments_today,
                    },
                    sweeps_per_day: QuotaUse {
                        limit: i64::from(ws.quota_sweeps_per_day),
                        used: used.sweeps_today,
                    },
                    active_jobs: QuotaUse {
                        limit: i64::from(ws.quota_active_jobs),
                        used: used.active_jobs,
                    },
                }),
            }
        }
    };
    json(StatusCode::OK, &body, Cache::NoStore)
}

/// The OpenAPI 3.1 document of this API.
#[utoipa::path(get, path = "/openapi.json", tag = "health",
    responses((status = 200, content_type = "application/json", body = Object)))]
pub async fn openapi(headers: HeaderMap) -> Response {
    let mut response = OPENAPI_JSON.clone().into_response();
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    super::respond::apply_cache(
        h,
        Cache::Revalidate,
        Some(&format!("\"{}\"", env!("CARGO_PKG_VERSION"))),
    );
    super::respond::conditional(&headers, response)
}
