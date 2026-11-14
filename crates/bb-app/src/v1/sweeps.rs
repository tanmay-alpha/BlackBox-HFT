//! Sweeps (API_CONTRACT §8.7): a sensitivity grid admitted as one unit and fanned out into
//! ordinary experiments through the same admission and the same job queue (no second
//! executor; TARGET_ARCHITECTURE §5.5).

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bb_core::compare::GridSpec;
use bb_db::{Cursor, NewSweep, SweepCellView};
use serde_json::json;
use sha2::{Digest, Sha256};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    experiments::{Params, WorkspaceParam},
    model::{
        CellMetrics, CreateSweep, Empty, LabelPatch, Sweep, SweepAxis, SweepCell, SweepProgress,
        SweepSubmitted,
    },
    respond::{
        ApiJson, Cache, Page, TruncatedBy, bounded, byte_cap, check_label, conditional,
        decode_cursor, encode_cursor, if_match, json, json_value, uuid_param,
    },
    runs::markdown_response,
};
use crate::{
    admission::{self, SubmitOptions},
    wire::at_ns,
};

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(create_sweep, list_sweeps))
        .routes(routes!(get_sweep, patch_sweep, delete_sweep))
        .routes(routes!(cancel_sweep))
        .routes(routes!(sweep_report))
}

fn cell_state(c: &SweepCellView) -> String {
    if c.run_short_id.is_some() {
        "completed".into()
    } else if c.cancel_requested_at.is_some() {
        "cancelled".into()
    } else {
        c.job_state.clone().unwrap_or_else(|| "detached".into())
    }
}

fn resource(
    row: &bb_db::Sweep,
    cells: Option<&[SweepCellView]>,
    with_cells: bool,
) -> Result<Sweep, ApiError> {
    let spec: GridSpec = serde_json::from_value(row.spec.clone())?;
    let axis = |a: &bb_core::compare::GridAxis| SweepAxis {
        axis: a.name().into(),
        labels: (0..a.len()).map(|i| a.value_label(i)).collect(),
    };
    let mut progress = SweepProgress {
        cells_total: spec.cells(),
        ..SweepProgress::default()
    };
    let mut out_cells = Vec::new();
    for c in cells.unwrap_or(&[]) {
        let state = cell_state(c);
        match state.as_str() {
            "completed" => progress.completed += 1,
            "running" | "cancelling" => progress.running += 1,
            "queued" | "created" => progress.queued += 1,
            "failed" => progress.failed += 1,
            "cancelled" => progress.cancelled += 1,
            "expired" => progress.expired += 1,
            _ => {}
        }
        out_cells.push(SweepCell {
            x: c.x,
            y: c.y,
            experiment_id: c.experiment_id.to_string(),
            job_id: c.job_id.map(|j| j.to_string()),
            run_id: c.run_short_id.clone(),
            metrics: c.run_short_id.as_ref().map(|_| CellMetrics {
                net_pnl: c.net_pnl,
                fill_count: c.fill_count,
                fees: c.fees,
                max_drawdown: c.max_drawdown,
            }),
            state,
        });
    }
    let p = &progress;
    let terminal = p.completed + p.failed + p.cancelled + p.expired;
    let state = if row.cancel_requested_at.is_some() {
        "cancelled"
    } else if p.completed == p.cells_total {
        "completed"
    } else if terminal == p.cells_total && terminal > 0 {
        "partial"
    } else if p.running == 0 && p.completed == 0 && terminal == 0 {
        "queued"
    } else {
        "running"
    };
    Ok(Sweep {
        id: row.id.to_string(),
        label: row.label.clone(),
        version: row.version,
        x: axis(&spec.x),
        y: axis(&spec.y),
        state: state.into(),
        progress,
        cells: with_cells.then_some(out_cells),
        spec: with_cells.then_some(spec),
        created_at_ns: at_ns(row.created_at),
    })
}

/// Admit a grid (every cell) and queue its cells as experiments. Idempotent per spec.
#[utoipa::path(post, path = "/sweeps", tag = "sweeps", security(("bearer" = [])),
    params(WorkspaceParam), request_body = CreateSweep,
    responses((status = 202, body = SweepSubmitted), (status = 422, body = ErrorEnvelope),
              (status = 429, body = ErrorEnvelope)))]
pub async fn create_sweep(
    State(state): State<Shared>,
    caller: Caller,
    Query(ws): Query<WorkspaceParam>,
    ApiJson(body): ApiJson<CreateSweep>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(ws.workspace.as_deref())?.clone();
    let label = check_label("label", body.label.as_deref().unwrap_or(""), true)?;
    let spec = body.spec;
    let problems = spec.validate();
    if !problems.is_empty() {
        return Err(ApiError::invalid_config(problems));
    }
    let cells = spec.cells();
    if cells > state.cfg.max_sweep_cells {
        return Err(ApiError::budget_exceeded(
            format!(
                "the sweep has {cells} cells; this server admits at most {}",
                state.cfg.max_sweep_cells
            ),
            json!({"what": "sweep_cells", "limit": state.cfg.max_sweep_cells, "estimate": cells,
                   "profile": state.cfg.profile.as_str()}),
        ));
    }
    // Admit every cell before anything is stored (one admission path, all-or-nothing).
    let (x_len, y_len) = (spec.x.len(), spec.y.len());
    let mut admitted = Vec::with_capacity(cells);
    for yi in 0..y_len {
        for xi in 0..x_len {
            let config = spec.y.apply(&spec.x.apply(&spec.base, xi), yi);
            let cell = admission::admit(
                &state.db,
                workspace.id,
                state.cfg.profile,
                &state.cfg.limits,
                config,
            )
            .await
            .map_err(|e| {
                let mut e = e;
                e.message = format!("cell x={xi} y={yi}: {}", e.message);
                e
            })?;
            admitted.push((xi, yi, cell));
        }
    }
    let spec_value = serde_json::to_value(&spec)?;
    let mut digest = Sha256::new();
    digest.update(crate::canonical::to_vec(&spec_value));
    digest.update(admitted[0].2.version.events_sha256.as_slice());
    digest.update(state.cfg.result_platform.as_bytes());
    let row = state
        .db
        .create_sweep(
            &NewSweep {
                workspace_id: workspace.id,
                spec: spec_value,
                spec_fingerprint: digest.finalize().to_vec(),
                x_len: x_len as i16,
                y_len: y_len as i16,
                label: if label.is_empty() {
                    format!("{} × {}", spec.x.name(), spec.y.name())
                } else {
                    label
                },
                created_by_key_id: caller.key_id(),
            },
            workspace.kind == "demo",
        )
        .await
        .map_err(|e| match e {
            bb_db::Error::Quota(scope) => ApiError::quota_exceeded(scope, 0, 0),
            other => other.into(),
        });
    let row = match row {
        Ok(r) => r,
        Err(e) => {
            return Err(admission::quota_details(&state.db, &workspace, caller.key_id(), e).await);
        }
    };
    let existing = state.db.sweep_cell_views(row.id).await?;
    let created = existing.is_empty();
    if created {
        for (xi, yi, cell) in &admitted {
            let result = admission::submit(
                &state.db,
                &state.hub,
                &state.metrics,
                cell,
                SubmitOptions {
                    workspace: workspace.id,
                    key: caller.key_id(),
                    label: &format!(
                        "{} x={} y={}",
                        row.label,
                        spec.x.value_label(*xi),
                        spec.y.value_label(*yi)
                    ),
                    platform: &state.cfg.result_platform,
                    priority: 50,
                    deadline_ms: state.cfg.deadline_ms,
                    sweep: Some((row.id, *xi as i16, *yi as i16)),
                    bypass_quotas: workspace.kind == "demo",
                    storage_backend: state.storage.backend,
                },
            )
            .await;
            if let Err(e) = result {
                return Err(
                    admission::quota_details(&state.db, &workspace, caller.key_id(), e).await,
                );
            }
        }
    }
    let cells = state.db.sweep_cell_views(row.id).await?;
    json(
        StatusCode::ACCEPTED,
        &SweepSubmitted {
            created,
            sweep: resource(&row, Some(&cells), true)?,
        },
        Cache::NoStore,
    )
}

/// Sweeps of a workspace, newest first (without cells).
#[utoipa::path(get, path = "/sweeps", tag = "sweeps", security((), ("bearer" = [])),
    params(WorkspaceParam, ("cursor" = Option<String>, Query), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = Page<Sweep>)))]
pub async fn list_sweeps(
    State(state): State<Shared>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let p = Params(pairs);
    let workspace = caller.read_workspace(p.get("workspace"))?.clone();
    let limit = p.limit(50, 100)?;
    let filters = p.filters();
    let cursor: Option<Cursor> = p
        .get("cursor")
        .map(|c| decode_cursor(c, &filters))
        .transpose()?;
    let rows = state
        .db
        .sweeps(workspace.id, cursor.as_ref(), super::runs::peek(limit))
        .await?;
    let more = super::runs::has_more(rows.len(), limit);
    let rows = &rows[..rows.len().min(limit as usize)];
    let mut items = Vec::new();
    for r in rows {
        let cells = state.db.sweep_cell_views(r.id).await?;
        items.push(resource(r, Some(&cells), false)?);
    }
    let (items, cut) = bounded(&items, byte_cap(&state, &caller))?;
    let next = if (cut || more) && !items.is_empty() {
        rows.get(items.len() - 1).map(|r| {
            encode_cursor(
                &Cursor {
                    created_at: r.created_at,
                    id: r.id,
                },
                &filters,
            )
        })
    } else {
        None
    };
    json_value(
        StatusCode::OK,
        &json!({"items": items, "next_cursor": next,
                "truncated_by": if cut { Some(TruncatedBy::Bytes) } else if more { Some(TruncatedBy::Limit) } else { None }}),
        Cache::NoStore,
    )
}

async fn visible(state: &Shared, caller: &Caller, id: &str) -> Result<bb_db::Sweep, ApiError> {
    let id = uuid_param("sweep", id)?;
    state
        .db
        .sweep(caller.workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("sweep"))
}

#[utoipa::path(get, path = "/sweeps/{id}", tag = "sweeps", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = Sweep), (status = 404, body = ErrorEnvelope)))]
pub async fn get_sweep(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    let cells = state.db.sweep_cell_views(row.id).await?;
    Ok(conditional(
        &headers,
        json(
            StatusCode::OK,
            &resource(&row, Some(&cells), true)?,
            Cache::NoStore,
        )?,
    ))
}

#[utoipa::path(patch, path = "/sweeps/{id}", tag = "sweeps", security(("bearer" = [])),
    params(("id" = String, Path), ("If-Match" = String, Header)), request_body = LabelPatch,
    responses((status = 200, body = Sweep), (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)))]
pub async fn patch_sweep(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LabelPatch>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("sweep", &id)?;
    let version = if_match(&headers, true)?.unwrap_or_default();
    let label = check_label("label", &body.label, true)?;
    let row = state
        .db
        .rename_sweep(workspace.id, id, version, &label)
        .await
        .map_err(|e| match e {
            bb_db::Error::Conflict => {
                ApiError::conflict("version_mismatch", "the sweep changed or does not exist")
            }
            other => other.into(),
        })?;
    let cells = state.db.sweep_cell_views(row.id).await?;
    json(
        StatusCode::OK,
        &resource(&row, Some(&cells), true)?,
        Cache::NoStore,
    )
}

async fn cancel_local_jobs(state: &Shared, cells: &[SweepCellView]) {
    for c in cells {
        if let Some(job) = c.job_id
            && let Ok(row) = state.db.job(job).await
            && row.state == "cancelling"
        {
            state.hub.cancel(job);
        }
    }
    state.hub.bump();
}

#[utoipa::path(delete, path = "/sweeps/{id}", tag = "sweeps", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 204), (status = 404, body = ErrorEnvelope)))]
pub async fn delete_sweep(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("sweep", &id)?;
    let row = state
        .db
        .sweep(workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("sweep"))?;
    if row.workspace_id != workspace.id {
        return Err(ApiError::not_found("sweep"));
    }
    let cells = state.db.sweep_cell_views(id).await?;
    state.db.delete_sweep(workspace.id, id).await?;
    cancel_local_jobs(&state, &cells).await;
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}

/// Cancel every live cell (cells another workspace still wants keep running).
#[utoipa::path(post, path = "/sweeps/{id}/cancel", tag = "sweeps", security(("bearer" = [])),
    params(("id" = String, Path)), request_body = Empty,
    responses((status = 202, body = Sweep), (status = 404, body = ErrorEnvelope)))]
pub async fn cancel_sweep(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("sweep", &id)?;
    let row = state
        .db
        .cancel_sweep(workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("sweep"))?;
    let cells = state.db.sweep_cell_views(id).await?;
    cancel_local_jobs(&state, &cells).await;
    json(
        StatusCode::ACCEPTED,
        &resource(&row, Some(&cells), true)?,
        Cache::NoStore,
    )
}

/// Markdown table of a sweep's net P&L per cell.
pub fn sweep_markdown(row: &bb_db::Sweep, cells: &[SweepCellView]) -> Result<String, ApiError> {
    let s = resource(row, Some(cells), true)?;
    let mut md = format!(
        "# Sweep: {}\n\nx = {}, y = {}\n\n",
        s.label, s.x.axis, s.y.axis
    );
    md.push_str(&format!("| {} \\ {} |", s.y.axis, s.x.axis));
    for l in &s.x.labels {
        md.push_str(&format!(" {l} |"));
    }
    md.push_str("\n|---|");
    md.push_str(&"---:|".repeat(s.x.labels.len()));
    md.push('\n');
    for (yi, yl) in s.y.labels.iter().enumerate() {
        md.push_str(&format!("| {yl} |"));
        for xi in 0..s.x.labels.len() {
            let cell = s
                .cells
                .as_ref()
                .and_then(|c| c.iter().find(|c| c.x as usize == xi && c.y as usize == yi));
            let text = cell
                .and_then(|c| c.metrics.as_ref())
                .and_then(|m| m.net_pnl)
                .map_or_else(
                    || cell.map_or("-".to_string(), |c| c.state.clone()),
                    |v| format!("{v:.6}"),
                );
            md.push_str(&format!(" {text} |"));
        }
        md.push('\n');
    }
    md.push_str(
        "\nNet P&L per cell. Simulated results only; nothing here is an exchange execution.\n",
    );
    Ok(md)
}

#[utoipa::path(get, path = "/sweeps/{id}/report", tag = "sweeps", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, content_type = "text/markdown", body = String), (status = 404, body = ErrorEnvelope)))]
pub async fn sweep_report(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    let cells = state.db.sweep_cell_views(row.id).await?;
    let mut response = markdown_response(
        sweep_markdown(&row, &cells)?,
        &format!("sweep-{}.md", row.id),
    )?;
    super::respond::apply_cache(response.headers_mut(), Cache::NoStore, None);
    Ok(response)
}
