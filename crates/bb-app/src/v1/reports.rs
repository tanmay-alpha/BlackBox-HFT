//! Saved report snapshots (API_CONTRACT §8.8): immutable Markdown bodies with their sha256.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bb_db::{Cursor, NewReport, ResearchReport};
use serde_json::json;
use sha2::{Digest, Sha256};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    comparisons::comparison_markdown,
    error::{ApiError, ErrorEnvelope},
    experiments::{Params, WorkspaceParam},
    model::{CreateReport, Report},
    respond::{
        ApiJson, Cache, Page, TruncatedBy, bounded, byte_cap, check_label, conditional,
        decode_cursor, encode_cursor, json, json_value, uuid_param,
    },
    runs::{markdown_response, resolve_run, run_markdown, verified_summary},
    sweeps::sweep_markdown,
};
use crate::{artifacts::open_run, wire::at_ns};

const GENERATOR_VERSION: i32 = 1;

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(create_report, list_reports))
        .routes(routes!(get_report, delete_report))
        .routes(routes!(report_content))
}

fn resource(r: &ResearchReport) -> Report {
    Report {
        id: r.id.to_string(),
        kind: r.kind.clone(),
        subject_id: r
            .run_id
            .or(r.comparison_id)
            .or(r.sweep_id)
            .map(|id| id.to_string()),
        title: r.title.clone(),
        generator_version: r.generator_version,
        format: r.format.clone(),
        body_sha256: hex::encode(&r.body_sha256),
        size_bytes: r.body.len(),
        created_at_ns: at_ns(r.created_at),
    }
}

/// Snapshot a run, comparison or sweep report.
#[utoipa::path(post, path = "/reports", tag = "reports", security(("bearer" = [])),
    params(WorkspaceParam), request_body = CreateReport,
    responses((status = 201, body = Report), (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)))]
pub async fn create_report(
    State(state): State<Shared>,
    caller: Caller,
    Query(ws): Query<WorkspaceParam>,
    ApiJson(body): ApiJson<CreateReport>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(ws.workspace.as_deref())?.clone();
    let title = check_label("title", &body.title, false)?;
    let viewer = Caller {
        workspace: workspace.clone(),
        ..caller.clone()
    };
    let (markdown, run_id, comparison_id, sweep_id) = match body.kind.as_str() {
        "run" => {
            let view = resolve_run(&state, &viewer, &body.subject_id, None).await?;
            verified_summary(&state, &view, false).await?;
            let open = open_run(
                &state.db,
                &state.storage,
                &state.cache,
                &state.metrics,
                state.bundle_limits(),
                view.clone(),
            )
            .await?;
            (
                run_markdown(&state, &open).await?,
                Some(view.run.id),
                None,
                None,
            )
        }
        "comparison" => {
            let id = uuid_param("comparison", &body.subject_id)?;
            let row = state
                .db
                .comparison(workspace.id, id)
                .await
                .map_err(|_| ApiError::not_found("comparison"))?;
            if !super::comparisons::intact(&row) {
                return Err(ApiError::integrity_failed(
                    &row.id.to_string(),
                    "comparison",
                    &hex::encode(&row.summary_sha256),
                ));
            }
            (comparison_markdown(&row)?, None, Some(id), None)
        }
        "sweep" => {
            let id = uuid_param("sweep", &body.subject_id)?;
            let row = state
                .db
                .sweep(workspace.id, id)
                .await
                .map_err(|_| ApiError::not_found("sweep"))?;
            let cells = state.db.sweep_cell_views(id).await?;
            (sweep_markdown(&row, &cells)?, None, None, Some(id))
        }
        other => {
            return Err(ApiError::invalid_field(
                "kind",
                format!("kind must be run, comparison or sweep, not `{other}`"),
            ));
        }
    };
    let row = store(
        &state,
        NewReport {
            workspace_id: workspace.id,
            kind: body.kind.clone(),
            run_id,
            comparison_id,
            sweep_id,
            title,
            generator_version: GENERATOR_VERSION,
            body_sha256: Sha256::digest(markdown.as_bytes()).to_vec(),
            body: markdown,
            created_by_key_id: caller.key_id(),
        },
    )
    .await?;
    json(StatusCode::CREATED, &resource(&row), Cache::NoStore)
}

/// Saves a report snapshot (`POST /reports`, `blackbox demo seed`).
pub async fn store(state: &Shared, report: NewReport) -> Result<ResearchReport, ApiError> {
    let row = state.db.save_report(&report).await.map_err(|e| match e {
        bb_db::Error::NotFound => ApiError::not_found(&report.kind),
        other => other.into(),
    })?;
    state.hub.bump();
    Ok(row)
}

/// A saved snapshot of a stored comparison's Markdown report.
pub fn comparison_snapshot(
    workspace: uuid::Uuid,
    comparison: &bb_db::Comparison,
    title: String,
) -> Result<NewReport, ApiError> {
    let markdown = comparison_markdown(comparison)?;
    Ok(NewReport {
        workspace_id: workspace,
        kind: "comparison".into(),
        run_id: None,
        comparison_id: Some(comparison.id),
        sweep_id: None,
        title,
        generator_version: GENERATOR_VERSION,
        body_sha256: Sha256::digest(markdown.as_bytes()).to_vec(),
        body: markdown,
        created_by_key_id: None,
    })
}

/// Saved reports of a workspace, newest first.
#[utoipa::path(get, path = "/reports", tag = "reports", security((), ("bearer" = [])),
    params(WorkspaceParam, ("cursor" = Option<String>, Query), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = Page<Report>)))]
pub async fn list_reports(
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
        .reports(workspace.id, cursor.as_ref(), super::runs::peek(limit))
        .await?;
    let more = super::runs::has_more(rows.len(), limit);
    let rows = &rows[..rows.len().min(limit as usize)];
    report_page(&state, &caller, rows, more, &filters)
}

/// Whether a saved report's body still matches its `body_sha256`. A changed body is never
/// served: lists leave it out, detail reads answer `integrity_failed`.
fn intact(row: &ResearchReport) -> bool {
    if Sha256::digest(row.body.as_bytes()).as_slice() != row.body_sha256 {
        tracing::error!(report = %row.id, "saved report failed its body digest");
        return false;
    }
    true
}

/// One page of saved reports (DB rows or the demo snapshot), newest first.
pub fn report_page(
    state: &Shared,
    caller: &Caller,
    rows: &[ResearchReport],
    more: bool,
    filters: &str,
) -> Result<Response, ApiError> {
    let kept: Vec<&ResearchReport> = rows.iter().filter(|r| intact(r)).collect();
    let items: Vec<Report> = kept.iter().map(|r| resource(r)).collect();
    let (items, cut) = bounded(&items, byte_cap(state, caller))?;
    let cursor = |r: &ResearchReport| {
        encode_cursor(
            &Cursor {
                created_at: r.created_at,
                id: r.id,
            },
            filters,
        )
    };
    let next = if cut && !items.is_empty() {
        kept.get(items.len() - 1).map(|r| cursor(r))
    } else if more {
        rows.last().map(cursor)
    } else {
        None
    };
    json_value(
        StatusCode::OK,
        &json!({"items": items, "next_cursor": next,
                "truncated_by": if cut { Some(TruncatedBy::Bytes) } else if more { Some(TruncatedBy::Limit) } else { None }}),
        Cache::Revalidate,
    )
}

async fn visible(state: &Shared, caller: &Caller, id: &str) -> Result<ResearchReport, ApiError> {
    let id = uuid_param("report", id)?;
    let row = state
        .db
        .report(caller.workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("report"))?;
    if !intact(&row) {
        return Err(ApiError::integrity_failed(
            &row.id.to_string(),
            "report",
            &hex::encode(&row.body_sha256),
        ));
    }
    Ok(row)
}

#[utoipa::path(get, path = "/reports/{id}", tag = "reports", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = Report), (status = 404, body = ErrorEnvelope)))]
pub async fn get_report(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    Ok(conditional(
        &headers,
        json(StatusCode::OK, &resource(&row), Cache::Revalidate)?,
    ))
}

/// The immutable Markdown body (`ETag` = body sha256).
#[utoipa::path(get, path = "/reports/{id}/content", tag = "reports", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, content_type = "text/markdown", body = String), (status = 404, body = ErrorEnvelope)))]
pub async fn report_content(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    let response = markdown_response(row.body, &format!("report-{}.md", row.id))?;
    Ok(conditional(&headers, response))
}

#[utoipa::path(delete, path = "/reports/{id}", tag = "reports", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 204), (status = 404, body = ErrorEnvelope)))]
pub async fn delete_report(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("report", &id)?;
    state
        .db
        .delete_report(workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("report"))?;
    state.hub.bump();
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}
