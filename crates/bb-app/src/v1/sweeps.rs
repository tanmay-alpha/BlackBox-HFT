//! Sweeps (API_CONTRACT §8.7): a declared `SweepSpec` (grid, random or seed-ensemble design,
//! optional in-sample / out-of-sample partition) admitted as one unit under the deployment's
//! sweep limits and fanned out into ordinary experiments through the same admission and the
//! same job queue (no second executor; TARGET_ARCHITECTURE §5.5). Windowed runs execute on
//! derived slice datasets (`registry::register_slice`). Once every run is terminal the sweep's
//! report is built: `bb_forensics::analyze_sweep` for grid and random designs,
//! `bb_forensics::analyze_ensemble` for seed ensembles (AT-12).

use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bb_artifacts::Section;
use bb_core::{
    result::{EquitySample, Metrics},
    sweep::{MAX_SWEEP_CELLS, Segment, SweepDesign, SweepLimits, SweepRefusal, SweepSpec},
};
use bb_db::{Cursor, NewSweep, SweepCellView};
use bb_forensics::{
    Availability, EnsembleRunOutcome, SweepCellOutcome, analyze_ensemble, analyze_sweep,
    objective_blocks,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    experiments::{Params, WorkspaceParam},
    model::{
        CellMetrics, CreateSweep, Empty, LabelPatch, Sweep, SweepAxis, SweepCell, SweepCellRun,
        SweepProgress, SweepReportCell, SweepReportResponse, SweepSubmitted,
    },
    respond::{
        ApiJson, Cache, Page, TruncatedBy, bounded, byte_cap, check_label, conditional,
        decode_cursor, encode_cursor, if_match, json, json_value, uuid_param,
    },
    runs::markdown_response,
};
use crate::{
    admission::{self, SubmitOptions},
    artifacts::open_run,
    config::Profile,
    registry::{self, manifest_of},
    wire::at_ns,
};

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(create_sweep, list_sweeps))
        .routes(routes!(get_sweep, patch_sweep, delete_sweep))
        .routes(routes!(cancel_sweep))
        .routes(routes!(sweep_report))
}

/// Run states after which nothing more happens to a run.
const TERMINAL: [&str; 5] = ["completed", "failed", "cancelled", "expired", "detached"];

fn segment_key(s: Segment) -> &'static str {
    match s {
        Segment::Full => "full",
        Segment::InSample => "in_sample",
        Segment::OutOfSample => "out_of_sample",
    }
}

fn segment_of(key: &str) -> Option<Segment> {
    match key {
        "full" => Some(Segment::Full),
        "in_sample" => Some(Segment::InSample),
        "out_of_sample" => Some(Segment::OutOfSample),
        _ => None,
    }
}

fn design_key(d: &SweepDesign) -> &'static str {
    match d {
        SweepDesign::Grid { .. } => "grid",
        SweepDesign::Random { .. } => "random",
        SweepDesign::Seeds { .. } => "seeds",
    }
}

fn run_state(c: &SweepCellView) -> String {
    if c.run_short_id.is_some() {
        "completed".into()
    } else if c.cancel_requested_at.is_some() {
        "cancelled".into()
    } else {
        c.job_state.clone().unwrap_or_else(|| "detached".into())
    }
}

fn spec_of(row: &bb_db::Sweep) -> Result<SweepSpec, ApiError> {
    serde_json::from_value(row.spec.clone())
        .map_err(|e| ApiError::internal(format!("sweep {} has an unreadable spec: {e}", row.id)))
}

fn view_of(views: &[SweepCellView], cell: u32, segment: Segment) -> Option<&SweepCellView> {
    views
        .iter()
        .find(|v| v.cell as u32 == cell && v.segment == segment_key(segment))
}

fn resource(
    row: &bb_db::Sweep,
    views: &[SweepCellView],
    with_cells: bool,
) -> Result<Sweep, ApiError> {
    let spec = spec_of(row)?;
    let cells = spec
        .cells()
        .map_err(|p| ApiError::internal(format!("sweep {}: {}", row.id, p.join("; "))))?;
    let cancelled = row.cancel_requested_at.is_some();
    let mut progress = SweepProgress {
        cells_total: cells.len(),
        ..SweepProgress::default()
    };
    let mut terminal = 0;
    let mut out_cells = Vec::new();
    for cell in &cells {
        let mut runs = Vec::new();
        for r in &cell.runs {
            let view = view_of(views, cell.index, r.segment);
            let state = match view {
                Some(v) => run_state(v),
                None if cancelled => "cancelled".into(),
                None => "not_submitted".into(),
            };
            progress.runs_total += 1;
            match state.as_str() {
                "completed" => progress.completed += 1,
                "running" | "cancelling" => progress.running += 1,
                "queued" | "created" => progress.queued += 1,
                "failed" => progress.failed += 1,
                "cancelled" => progress.cancelled += 1,
                "expired" => progress.expired += 1,
                _ => {}
            }
            if TERMINAL.contains(&state.as_str()) {
                terminal += 1;
            }
            if with_cells {
                runs.push(SweepCellRun {
                    segment: r.segment,
                    experiment_id: view.map(|v| v.experiment_id.to_string()),
                    job_id: view.and_then(|v| v.job_id).map(|j| j.to_string()),
                    run_id: view.and_then(|v| v.run_short_id.clone()),
                    error: view
                        .filter(|_| state == "failed")
                        .and_then(|v| v.error_message.clone()),
                    metrics: view
                        .filter(|v| v.run_short_id.is_some())
                        .map(|v| CellMetrics {
                            net_pnl: v.net_pnl,
                            fill_count: v.fill_count,
                            fees: v.fees,
                            max_drawdown: v.max_drawdown,
                        }),
                    state,
                });
            }
        }
        if with_cells {
            out_cells.push(SweepCell {
                index: cell.index,
                coords: cell.coords.clone(),
                params: cell.params.clone(),
                runs,
            });
        }
    }
    let p = &progress;
    let finished = p.completed + p.failed + p.cancelled + p.expired;
    let state = if cancelled {
        "cancelled"
    } else if p.completed == p.runs_total {
        "completed"
    } else if terminal == p.runs_total {
        "partial"
    } else if p.running == 0 && p.completed == 0 && finished == 0 {
        "queued"
    } else {
        "running"
    };
    let axes = match &spec.design {
        SweepDesign::Grid { axes } => axes
            .iter()
            .map(|a| SweepAxis {
                axis: a.name().into(),
                labels: (0..a.len()).map(|i| a.value_label(i)).collect(),
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(Sweep {
        id: row.id.to_string(),
        label: row.label.clone(),
        version: row.version,
        design: design_key(&spec.design).into(),
        fingerprint: spec.fingerprint(),
        axes,
        partition: spec.partition,
        objective: spec.objective,
        state: state.into(),
        report_ready: terminal == progress.runs_total,
        progress,
        cells: with_cells.then_some(out_cells),
        spec: with_cells.then_some(spec),
        created_at_ns: at_ns(row.created_at),
    })
}

fn default_label(spec: &SweepSpec) -> String {
    let mut label = match &spec.design {
        SweepDesign::Grid { axes } => axes
            .iter()
            .map(|a| a.name())
            .collect::<Vec<_>>()
            .join(" × "),
        SweepDesign::Random {
            samples, params, ..
        } => format!(
            "random {samples} × {}",
            params
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        SweepDesign::Seeds { .. } => format!("{} seeds", spec.cell_count()),
    };
    if spec.partition.is_some() {
        label.push_str(" (IS/OOS)");
    }
    label
}

fn refusal(state: &Shared, r: SweepRefusal) -> ApiError {
    match r.budget_details() {
        None => match r {
            SweepRefusal::Invalid(problems) => ApiError::invalid_config(problems),
            other => ApiError::invalid_config(vec![other.to_string()]),
        },
        Some((what, limit, predicted)) => ApiError::budget_exceeded(
            r.to_string(),
            json!({"what": what, "limit": limit, "estimate": predicted,
                   "profile": state.cfg.profile.as_str()}),
        ),
    }
}

/// Admit a `SweepSpec` (or a legacy `GridSpec`) under the deployment's sweep limits, register
/// the slice datasets of a partition, and queue every run through the common admission path.
/// Idempotent per spec, dataset version and platform: a resubmission returns the same sweep and
/// submits only runs an interrupted submission left out.
#[utoipa::path(post, path = "/sweeps", tag = "sweeps", security(("bearer" = [])),
    params(WorkspaceParam), request_body = CreateSweep,
    responses((status = 202, body = SweepSubmitted), (status = 404, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)))]
pub async fn create_sweep(
    State(state): State<Shared>,
    caller: Caller,
    Query(ws): Query<WorkspaceParam>,
    ApiJson(body): ApiJson<CreateSweep>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(ws.workspace.as_deref())?.clone();
    let label = check_label("label", body.label.as_deref().unwrap_or(""), true)?;
    let spec = body.spec.into_spec();
    let not_found = |e: bb_db::Error| match e {
        bb_db::Error::NotFound => ApiError::not_found("dataset"),
        other => other.into(),
    };
    // The parent dataset must be visible to the workspace; the sweep binds its current version.
    state
        .db
        .dataset(workspace.id, &spec.base.dataset_id)
        .await
        .map_err(not_found)?;
    let parent = state
        .db
        .current_version(&spec.base.dataset_id)
        .await
        .map_err(not_found)?;
    let manifest = manifest_of(&parent)?;
    let limits = SweepLimits::from_execution(
        state.cfg.limits,
        state.cfg.max_sweep_cells.min(MAX_SWEEP_CELLS) as u32,
    );
    let plan = spec
        .admit(&manifest, &limits)
        .map_err(|r| refusal(&state, r))?;

    // Derived datasets of the windows, then every run through the one admission path (all
    // admitted before anything is queued).
    let mut windows = Vec::new();
    for w in plan.runs.iter().filter_map(|r| r.window) {
        if !windows.contains(&w) {
            windows.push(w);
        }
    }
    for w in windows {
        registry::register_slice(
            &state.db,
            &state.registry,
            &parent,
            w,
            state.cfg.profile == Profile::Hosted,
        )
        .await?;
    }
    let mut admitted = Vec::with_capacity(plan.runs.len());
    for run in &plan.runs {
        let cell = admission::admit(
            &state.db,
            workspace.id,
            state.cfg.profile,
            &state.cfg.limits,
            run.config.clone(),
        )
        .await
        .map_err(|mut e| {
            e.message = format!(
                "cell {} ({}): {}",
                run.cell,
                segment_key(run.segment),
                e.message
            );
            e
        })?;
        admitted.push((run.cell, run.segment, cell));
    }

    let mut digest = Sha256::new();
    digest.update(plan.fingerprint.as_bytes());
    digest.update(parent.events_sha256.as_slice());
    digest.update(state.cfg.result_platform.as_bytes());
    let row = state
        .db
        .create_sweep(
            &NewSweep {
                workspace_id: workspace.id,
                spec: serde_json::to_value(&spec)?,
                spec_fingerprint: digest.finalize().to_vec(),
                cell_count: plan.cells.len() as i16,
                run_count: plan.runs.len() as i16,
                label: if label.is_empty() {
                    default_label(&spec)
                } else {
                    label
                },
                created_by_key_id: caller.key_id(),
            },
            workspace.kind == "demo",
            &caller.audit(&workspace),
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
    if row.cancel_requested_at.is_none() {
        for (cell, segment, admitted) in &admitted {
            if view_of(&existing, *cell, *segment).is_some() {
                continue;
            }
            let params = plan.cells[*cell as usize]
                .params
                .iter()
                .map(|p| format!("{}={}", p.param, p.label))
                .collect::<Vec<_>>()
                .join(", ");
            let run_label: String = format!("{} · {params} · {}", row.label, segment_key(*segment))
                .chars()
                .take(200)
                .collect();
            let result = admission::submit(
                &state.db,
                &state.hub,
                &state.metrics,
                admitted,
                SubmitOptions {
                    workspace: workspace.id,
                    key: caller.key_id(),
                    label: &run_label,
                    platform: &state.cfg.result_platform,
                    priority: 50,
                    deadline_ms: state.cfg.deadline_ms,
                    sweep: Some((row.id, *cell as i16, segment_key(*segment))),
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
    let views = state.db.sweep_cell_views(row.id).await?;
    json(
        StatusCode::ACCEPTED,
        &SweepSubmitted {
            created,
            sweep: resource(&row, &views, true)?,
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
    let filters = p.filters("/api/v1/sweeps");
    let cursor: Option<Cursor> = p
        .get("cursor")
        .map(|c| decode_cursor(c, &filters))
        .transpose()?;
    let snapshot = state.snapshot.for_read(&state, &caller).await;
    let rows = match &snapshot {
        Some(snapshot) => snapshot.sweeps(cursor.as_ref(), super::runs::peek(limit) as usize),
        None => {
            state
                .db
                .sweeps(workspace.id, cursor.as_ref(), super::runs::peek(limit))
                .await?
        }
    };
    let more = super::runs::has_more(rows.len(), limit);
    let rows = &rows[..rows.len().min(limit as usize)];
    let mut items = Vec::new();
    for r in rows {
        let views;
        let cells = match &snapshot {
            Some(snapshot) => snapshot.sweep_cells(r.id),
            None => {
                views = state.db.sweep_cell_views(r.id).await?;
                &views
            }
        };
        items.push(resource(r, cells, false)?);
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

/// A sweep with every cell, each with the job state of its runs (per segment).
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
    let views = state.db.sweep_cell_views(row.id).await?;
    Ok(conditional(
        &headers,
        json(
            StatusCode::OK,
            &resource(&row, &views, true)?,
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
        .rename_sweep(workspace.id, id, version, &label, &caller.audit(&workspace))
        .await
        .map_err(|e| match e {
            bb_db::Error::Conflict => {
                ApiError::conflict("version_mismatch", "the sweep changed or does not exist")
            }
            other => other.into(),
        })?;
    let views = state.db.sweep_cell_views(row.id).await?;
    json(
        StatusCode::OK,
        &resource(&row, &views, true)?,
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
    state
        .db
        .delete_sweep(workspace.id, id, &caller.audit(&workspace))
        .await?;
    cancel_local_jobs(&state, &cells).await;
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}

/// Cancel every live run (runs another workspace still wants keep running).
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
        .cancel_sweep(workspace.id, id, &caller.audit(&workspace))
        .await
        .map_err(|_| ApiError::not_found("sweep"))?;
    let views = state.db.sweep_cell_views(id).await?;
    cancel_local_jobs(&state, &views).await;
    json(
        StatusCode::ACCEPTED,
        &resource(&row, &views, true)?,
        Cache::NoStore,
    )
}

/// The objective's per-block contributions of a completed run (CSCV input), read from its
/// verified bundle's equity samples.
async fn run_blocks(
    state: &Shared,
    caller: &Caller,
    view: &SweepCellView,
    spec: &SweepSpec,
    blocks: u32,
) -> Result<Option<Vec<f64>>, ApiError> {
    let Some(run) = view.run_uuid else {
        return Ok(None);
    };
    let run = state.db.run_view_by_id(caller.workspace.id, run).await?;
    let window = match spec.partition {
        Some(p) => p.in_sample,
        None => {
            let version = state
                .db
                .dataset_version(&run.dataset_id, run.dataset_version_no)
                .await?;
            spec.selection_window(&manifest_of(&version)?)
        }
    };
    let open = open_run(
        &state.db,
        &state.storage,
        &state.cache,
        &state.metrics,
        state.bundle_limits(),
        run,
    )
    .await?;
    let mut equity: Vec<EquitySample> = Vec::new();
    for (page, _, _) in open.pages(Section::Equity) {
        equity.extend(open.page::<EquitySample>(page).await?);
    }
    Ok(objective_blocks(spec.objective.metric, &equity, window, blocks).ok())
}

/// The analysis of a sweep whose runs are all terminal (`409 conflict` before that).
pub async fn build_report(
    state: &Shared,
    caller: &Caller,
    row: &bb_db::Sweep,
) -> Result<SweepReportResponse, ApiError> {
    let views = state.db.sweep_cell_views(row.id).await?;
    let sweep = resource(row, &views, false)?;
    if !sweep.report_ready {
        let p = &sweep.progress;
        return Err(ApiError::conflict(
            "sweep_not_finished",
            format!(
                "{} of {} runs are completed, failed, cancelled or expired; the report is built \
                 once every run is",
                p.completed + p.failed + p.cancelled + p.expired,
                p.runs_total
            ),
        ));
    }
    let spec = spec_of(row)?;
    let cells = spec.cells().map_err(|p| ApiError::internal(p.join("; ")))?;
    let metrics_of = |v: &SweepCellView| -> Option<Metrics> {
        v.run_short_id.as_ref()?;
        serde_json::from_value(v.metrics.clone()?).ok()
    };
    // A failed or expired run is an error of its cell; cancelled and never-run cells are missing.
    // A completed job without a listed run had its run quarantined or its stored row changed
    // (`sweep_cell_views` leaves such runs out): an error, never a value.
    let error_of = |v: Option<&SweepCellView>| -> Option<String> {
        let v = v?;
        match run_state(v).as_str() {
            "failed" => Some(
                v.error_message
                    .clone()
                    .unwrap_or_else(|| "the run failed".into()),
            ),
            "expired" => Some("the run expired before it completed".into()),
            "completed" if v.run_short_id.is_none() => {
                Some("the run is quarantined or its stored row failed the integrity check".into())
            }
            _ => None,
        }
    };
    let selection = if spec.partition.is_some() {
        Segment::InSample
    } else {
        Segment::Full
    };
    let metric = spec.objective.metric;
    let mut report_cells = Vec::with_capacity(cells.len());
    for c in &cells {
        let is_view = view_of(&views, c.index, selection);
        let oos_view = spec
            .partition
            .and_then(|_| view_of(&views, c.index, Segment::OutOfSample));
        report_cells.push(SweepReportCell {
            index: c.index,
            params: c.params.clone(),
            in_sample: is_view.and_then(metrics_of).and_then(|m| metric.value(&m)),
            out_of_sample: oos_view.and_then(metrics_of).and_then(|m| metric.value(&m)),
            in_sample_run_id: is_view.and_then(|v| v.run_short_id.clone()),
            out_of_sample_run_id: oos_view.and_then(|v| v.run_short_id.clone()),
            error: error_of(is_view).or_else(|| error_of(oos_view)),
        });
    }
    let (analysis, ensemble) = if matches!(spec.design, SweepDesign::Seeds { .. }) {
        let outcomes: Vec<EnsembleRunOutcome> = views
            .iter()
            .filter_map(|v| {
                Some(EnsembleRunOutcome {
                    cell: v.cell as u32,
                    segment: segment_of(&v.segment)?,
                    run_id: v.run_short_id.clone(),
                    metrics: metrics_of(v),
                    error: error_of(Some(v)),
                })
            })
            .collect();
        let report = analyze_ensemble(&spec, &outcomes).map_err(ApiError::internal)?;
        (None, Some(report))
    } else {
        // Per-block values need every run's equity samples: an artefact-heavy read.
        let _permit = match spec.cscv_blocks {
            Some(_) => Some(
                tokio::time::timeout(Duration::from_secs(2), state.heavy.acquire())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .ok_or_else(|| ApiError::unavailable("busy", 2))?,
            ),
            None => None,
        };
        let mut outcomes = Vec::with_capacity(cells.len());
        for rc in &report_cells {
            let blocks = match (spec.cscv_blocks, rc.in_sample) {
                (Some(b), Some(_)) => match view_of(&views, rc.index, selection) {
                    Some(v) => run_blocks(state, caller, v, &spec, b).await?,
                    None => None,
                },
                _ => None,
            };
            outcomes.push(SweepCellOutcome {
                cell: rc.index,
                in_sample: rc.in_sample,
                out_of_sample: rc.out_of_sample,
                blocks,
                error: rc.error.clone(),
            });
        }
        let report = analyze_sweep(&spec, &outcomes).map_err(ApiError::internal)?;
        (Some(report), None)
    };
    Ok(SweepReportResponse {
        sweep_id: row.id.to_string(),
        label: row.label.clone(),
        design: design_key(&spec.design).into(),
        state: sweep.state,
        objective: spec.objective,
        partition: spec.partition,
        cells: report_cells,
        analysis,
        ensemble,
    })
}

/// The report as Markdown (`?format=markdown`, saved research reports), in the evidence
/// structure of docs/research/REPORT_FORMAT.md.
pub fn report_markdown(r: &SweepReportResponse) -> String {
    use std::fmt::Write as _;

    use bb_core::compare::duration_label;

    use crate::report::{
        metric_name, num as n, param_name, queue_model_name, serde_code, unit_name,
    };

    let num = |v: Option<f64>| v.map_or_else(|| "–".to_string(), n);
    let metric = r.objective.metric;
    let objective = format!("{} (`{}`)", metric_name(metric.key()), metric.key());
    let unit = unit_name(metric.unit());
    let direction = format!(
        "{} (`{}`)",
        match r.objective.direction {
            bb_core::sweep::Direction::Maximize => "higher is better",
            bb_core::sweep::Direction::Minimize => "lower is better",
        },
        serde_code(&r.objective.direction)
    );
    let seeds = r.ensemble.is_some();
    let design = match r.design.as_str() {
        "grid" => "Grid of every combination",
        "random" => "Seeded random sample of configurations",
        "seeds" => "Seed ensemble of one configuration",
        _ => "Sweep design",
    };
    let (fingerprint, analysis_version) = match (&r.analysis, &r.ensemble) {
        (Some(a), _) => (Some(a.fingerprint.as_str()), Some(a.analysis_version)),
        (None, Some(e)) => (Some(e.fingerprint.as_str()), Some(e.analysis_version)),
        (None, None) => (None, None),
    };
    let param_cell = |p: &bb_core::sweep::ParamValue| {
        let value = if p.param == "queue_model" {
            serde_json::from_value::<bb_core::config::QueueModelConfig>(p.value.clone())
                .map(|q| format!("{} (`{}`)", queue_model_name(&q), q.label()))
                .unwrap_or_else(|_| format!("`{}`", p.value))
        } else {
            let raw = match &p.value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if raw == p.label {
                format!("`{raw}`")
            } else {
                format!("{} (`{raw}`)", p.label)
            }
        };
        format!("{} (`{}`) = {value}", param_name(&p.param), p.param)
    };
    let mut md = String::new();
    let w = &mut md;

    let _ = write!(
        w,
        "# Sweep report: {}\n\n\
         Research only. Every value below comes from simulated runs under stated latency, queue \
         and fee assumptions; simulated orders never move replayed prices or consume displayed \
         depth (no market impact). Nothing here is a real exchange execution.\n\n",
        r.label
    );

    // ----- Research question and hypothesis -----
    let tried = r
        .analysis
        .as_ref()
        .map_or(r.cells.len() as u32, |a| a.configurations.tried);
    let _ = write!(
        w,
        "## Research question\n\n{}\n\n## Hypothesis\n\n{}\n\n",
        if let Some(e) = &r.ensemble {
            format!(
                "How much do the metrics of one configuration vary across {} seeds?",
                e.seed_count
            )
        } else {
            format!(
                "Which of the {tried} declared configurations scores best on {objective}, \
                 {direction}{}?",
                if r.partition.is_some() {
                    ", in sample, and does that choice hold out of sample"
                } else {
                    ""
                }
            )
        },
        if seeds {
            "Descriptive ensemble, no hypothesis: the spread over seeds is measured, not tested."
        } else {
            "Selection study, no hypothesis: the objective ranks the configurations; no stated \
             expectation is tested."
        }
    );

    // ----- Data source, provenance, limitations -----
    w.push_str(
        "## Data source\n\n\
         Every cell is an ordinary run. Its own report (`/reports/<run id>`) names the dataset \
         and whether it is a synthetic session generated by BlackBox or recorded market data.\n",
    );
    if let Some(p) = &r.partition {
        let _ = write!(
            w,
            "\nIn-sample window: {} to {} from the dataset origin ({}); out-of-sample window: {} \
             to {} ({}) (`partition`, exchange-clock offsets). Each window runs on a slice \
             dataset derived from the base dataset.\n",
            duration_label(p.in_sample.start),
            duration_label(p.in_sample.end),
            duration_label(p.in_sample.len_ns()),
            duration_label(p.out_of_sample.start),
            duration_label(p.out_of_sample.end),
            duration_label(p.out_of_sample.len_ns()),
        );
    }
    w.push_str(
        "\n## Dataset provenance\n\n\
         The run ids of every cell are listed under Results; each run's report carries its \
         dataset events SHA-256, source input hashes and timestamp provenance.\n\n\
         ## Dataset limitations\n\n\
         Stated per run in each run's report; the sweep adds none of its own.\n\n",
    );

    // ----- Model assumptions and configuration -----
    let mut swept: Vec<&str> = Vec::new();
    for c in &r.cells {
        for p in &c.params {
            if !swept.contains(&p.param.as_str()) {
                swept.push(&p.param);
            }
        }
    }
    let swept_list = swept
        .iter()
        .map(|p| format!("{} (`{p}`)", param_name(p)))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(
        w,
        "## Model assumptions\n\n\
         Every cell runs the sweep's base configuration with only {} changed; every other \
         assumption (fill, queue and latency models, fees, risk limits) is the base \
         configuration's, stated in each run's report.\n\n\
         ## Experiment configuration\n\n\
         - Design: {design} (`{design_code}`), {cells} cells.\n\
         - Objective: {objective} in {unit}, {direction}.\n\
         - Partition: {partition}.\n\
         - State: `{state}`.\n\n",
        if swept_list.is_empty() {
            "the cell's parameters".to_string()
        } else {
            swept_list
        },
        design_code = r.design,
        cells = r.cells.len(),
        partition = if r.partition.is_some() {
            "in-sample and out-of-sample windows"
        } else {
            "none, every cell runs on the whole dataset"
        },
        state = r.state,
    );

    // ----- Methods -----
    w.push_str("## Methods\n\n");
    if seeds {
        w.push_str(
            "- The base configuration ran once per seed. Mean, standard deviation, minimum and \
             maximum of every metric are taken over the completed seeds.\n\n",
        );
    } else {
        let _ = write!(
            w,
            "- Each cell ran {}.\n\
             - The selected cell has the best {} value in the objective's direction (the lowest \
             index wins ties).\n\
             - Spearman's rank correlation compares the in-sample and out-of-sample ranks of the \
             cells; the probability of backtest overfitting (PBO) uses combinatorially symmetric \
             cross-validation (CSCV) over equal time blocks when blocks were requested.\n\n",
            if r.partition.is_some() {
                "once on the in-sample window and once on the out-of-sample window"
            } else {
                "once on the whole dataset"
            },
            if r.partition.is_some() {
                "in-sample"
            } else {
                "whole-dataset"
            },
        );
    }

    // ----- Results -----
    let (first, second) = if r.partition.is_some() {
        ("In sample", "Out of sample")
    } else {
        ("Whole dataset", "Out of sample")
    };
    let _ = write!(
        w,
        "## Results\n\n\
         Objective values ({objective}, {unit}) per cell, rounded to six decimals.\n\n\
         | Cell | Parameters | {first} | {second} | Runs |\n|---:|---|---:|---:|---|\n"
    );
    for c in &r.cells {
        let params = c
            .params
            .iter()
            .map(param_cell)
            .collect::<Vec<_>>()
            .join(", ");
        let runs = [&c.in_sample_run_id, &c.out_of_sample_run_id]
            .into_iter()
            .flatten()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            w,
            "| {} | {params} | {} | {} | {} |",
            c.index,
            num(c.in_sample),
            num(c.out_of_sample),
            c.error.clone().unwrap_or(runs)
        );
    }
    if let Some(a) = &r.analysis {
        let k = &a.configurations;
        let _ = write!(
            w,
            "\n### Selection\n\n\
             Configurations tried: {} (`configurations.tried`): {} completed in sample, {} out of \
             sample, {} failed, {} missing.\n\n",
            k.tried, k.completed_in_sample, k.completed_out_of_sample, k.failed, k.missing
        );
        match &a.selected {
            Some(s) => {
                let tie = if s.tied_with > 0 {
                    format!(
                        "; tied with {} other cells (`selected.tied_with`)",
                        s.tied_with
                    )
                } else {
                    String::new()
                };
                if r.partition.is_some() {
                    let _ = write!(
                        w,
                        "Selected cell {} (`selected.cell`): in sample {}, out of sample {} \
                         (rank {} of {}), degradation {} (`selected.degradation`){tie}.\n\n",
                        s.cell,
                        n(s.in_sample),
                        num(s.out_of_sample),
                        s.out_of_sample_rank.map_or("–".into(), |v| v.to_string()),
                        s.out_of_sample_of,
                        num(s.degradation),
                    );
                } else {
                    let _ = write!(
                        w,
                        "Selected cell {} (`selected.cell`): whole-dataset value {}{tie}. No \
                         out-of-sample window was declared, so no degradation is measured.\n\n",
                        s.cell,
                        n(s.in_sample),
                    );
                }
            }
            None => w.push_str("No cell has an in-sample value; nothing was selected.\n\n"),
        }
    }
    if let Some(e) = &r.ensemble {
        let _ = write!(w, "\n### Seed ensemble ({} seeds)\n\n", e.seed_count);
        if let Some(reason) = &e.collapse_reason {
            let _ = write!(w, "**{reason}**\n\n");
        }
        for s in &e.segments {
            let segment = match s.segment {
                Segment::Full => "Whole dataset",
                Segment::InSample => "In sample",
                Segment::OutOfSample => "Out of sample",
            };
            let _ = write!(
                w,
                "#### {segment} (`{}`): {} of {} completed\n\n\
                 | Metric | Key | Unit | Mean | Std dev | Min | Max |\n\
                 |---|---|---|---:|---:|---:|---:|\n",
                segment_key(s.segment),
                s.completed,
                s.members.len()
            );
            for m in &s.metrics {
                let _ = writeln!(
                    w,
                    "| {} | `{}` | {} | {} | {} | {} | {} |",
                    metric_name(&m.metric),
                    m.metric,
                    unit_name(&m.unit),
                    n(m.mean),
                    num(m.std_dev),
                    n(m.min),
                    n(m.max)
                );
            }
            let members = s
                .members
                .iter()
                .map(|m| {
                    format!(
                        "seed `{}`: {}",
                        m.seed,
                        m.run_id
                            .as_deref()
                            .map_or_else(|| "no run".to_string(), |id| format!("run `{id}`"))
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let _ = write!(w, "\nRuns: {members}.\n\n");
        }
    }

    // ----- Visual evidence -----
    let _ = write!(
        w,
        "\n## Visual evidence\n\n\
         Paths are relative to the BlackBox console.\n\n\
         - Sweep result matrix: `/compare/sweeps/{}`\n",
        r.sweep_id
    );
    if let Some(s) = r.analysis.as_ref().and_then(|a| a.selected.as_ref())
        && let Some(c) = r.cells.iter().find(|c| c.index == s.cell)
    {
        for (label, id) in [
            ("in-sample", &c.in_sample_run_id),
            ("out-of-sample", &c.out_of_sample_run_id),
        ] {
            if let Some(id) = id {
                let _ = writeln!(
                    w,
                    "- Selected cell {}, {label} run: `/forensics/{id}`",
                    s.cell
                );
            }
        }
    }

    // ----- Sensitivity -----
    w.push_str("\n## Sensitivity\n\n");
    match &r.analysis {
        Some(a) => {
            for (label, d) in [
                (first, &a.dispersion.in_sample),
                ("Out of sample", &a.dispersion.out_of_sample),
            ] {
                if let Some(d) = d {
                    let _ = writeln!(
                        w,
                        "- {label} spread of the objective over {} cells: minimum {}, median {}, \
                         maximum {}, standard deviation {} ({unit}).",
                        d.n,
                        n(d.min),
                        n(d.median),
                        n(d.max),
                        num(d.std_dev)
                    );
                }
            }
            if a.dispersion.in_sample.is_none() && a.dispersion.out_of_sample.is_none() {
                w.push_str("- No completed cell, so no spread exists.\n");
            }
        }
        None => {
            w.push_str("- Only the seed varies; its effect is under Statistical uncertainty.\n")
        }
    }

    // ----- Statistical uncertainty -----
    w.push_str("\n## Statistical uncertainty\n\n");
    if let Some(a) = &r.analysis {
        match &a.rank_correlation {
            Availability::Available(rc) => {
                let _ = writeln!(
                    w,
                    "- Spearman rank correlation in sample vs out of sample: {:.3} over {} cells \
                     (`rank_correlation`).",
                    rc.spearman_rho, rc.cells
                );
            }
            Availability::Unavailable { reason } => {
                let _ = writeln!(w, "- Spearman rank correlation: unavailable ({reason}).");
            }
        }
        let b = &a.selection_bias;
        match (b.selected_z, b.expected_max_z_under_noise) {
            (Some(z), Some(e)) => {
                let _ = writeln!(
                    w,
                    "- Selection bias over {} trials (`selection_bias`): the selected value lies \
                     {:.3} in-sample standard deviations above the mean; pure noise would give \
                     about {:.3}.",
                    b.trials, z, e
                );
            }
            _ => {
                let _ = writeln!(
                    w,
                    "- Selection bias over {} trials (`selection_bias`): not computable for this \
                     sweep.",
                    b.trials
                );
            }
        }
        match &a.pbo {
            Availability::Available(p) => {
                let _ = writeln!(
                    w,
                    "- Probability of backtest overfitting, PBO (CSCV, {} blocks, {} splits): \
                     {:.3} (`pbo.pbo`).",
                    p.blocks, p.combinations, p.pbo
                );
            }
            Availability::Unavailable { reason } => {
                let _ = writeln!(w, "- PBO: unavailable ({reason}).");
            }
        }
    }
    if let Some(e) = &r.ensemble {
        let _ = writeln!(
            w,
            "- The seed {} the result (`seed_affects_result` = `{}`); seeds collapse to one \
             result: {} (`collapsed` = `{}`).",
            if e.seed_affects_result {
                "can change"
            } else {
                "cannot change"
            },
            e.seed_affects_result,
            if e.collapsed { "yes" } else { "no" },
            e.collapsed
        );
    }

    // ----- Reproduction -----
    let _ = write!(
        w,
        "\n## Reproduction\n\n\
         - Sweep id: `{}`\n\
         - Spec fingerprint: `{}`\n\
         - Analysis version: {}\n\n\
         Each cell run's report carries its config SHA-256, result SHA-256, dataset events \
         SHA-256, simulation revision and reproduction recipe.\n\n",
        r.sweep_id,
        fingerprint.unwrap_or("–"),
        analysis_version.map_or_else(|| "–".to_string(), |v| v.to_string()),
    );

    // ----- Limitations -----
    w.push_str("## Limitations\n\n");
    let caveats = r
        .analysis
        .iter()
        .flat_map(|a| &a.caveats)
        .chain(r.ensemble.iter().flat_map(|e| &e.caveats));
    // bb-forensics caveats name the objective by its key; the report names it readably.
    let key_word = format!(" {} ", metric.key());
    let named = format!(" {objective} ");
    for c in caveats {
        let _ = writeln!(w, "- {} (`{}`)", c.text.replace(&key_word, &named), c.code);
    }
    w.push_str(
        "- A sweep shows how the model's output moves with its assumptions; it is not a \
         prediction of live performance.\n",
    );
    md
}

#[derive(Deserialize, IntoParams)]
pub struct ReportFormat {
    /// `json` (default) or `markdown`.
    pub format: Option<String>,
}

/// The analysis of a finished sweep: configurations tried, the in-sample selection and its
/// out-of-sample degradation, Spearman, PBO (or why it is unavailable) and caveats; for a seeds
/// design the per-metric ensemble dispersion and an explicit collapse statement.
#[utoipa::path(get, path = "/sweeps/{id}/report", tag = "sweeps", security((), ("bearer" = [])),
    params(("id" = String, Path), ReportFormat),
    responses((status = 200, content(
                  (SweepReportResponse = "application/json"),
                  (String = "text/markdown"))),
              (status = 404, body = ErrorEnvelope),
              (status = 409, description = "Not every run is terminal yet", body = ErrorEnvelope)))]
pub async fn sweep_report(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<ReportFormat>,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    let report = build_report(&state, &caller, &row).await?;
    match q.format.as_deref() {
        None | Some("json") => json(StatusCode::OK, &report, Cache::NoStore),
        Some("markdown") => {
            let mut response =
                markdown_response(report_markdown(&report), &format!("sweep-{}.md", row.id))?;
            super::respond::apply_cache(response.headers_mut(), Cache::NoStore, None);
            Ok(response)
        }
        Some(_) => Err(ApiError::invalid_field(
            "format",
            "must be `json` or `markdown`",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_keys_round_trip() {
        for s in [Segment::Full, Segment::InSample, Segment::OutOfSample] {
            assert_eq!(segment_of(segment_key(s)), Some(s));
        }
    }
}
