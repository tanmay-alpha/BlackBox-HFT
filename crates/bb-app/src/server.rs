//! The HTTP API of `docs/API.md`, plus optional static serving of the web UI.
//!
//! Handlers never block the async runtime: everything that reads a dataset or a stored run, or
//! runs a simulation, executes on the blocking thread pool.

use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        OriginalUri, Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bb_core::{
    CONTRACT_VERSION, RelNs,
    compare::{Comparison, GridResult, GridSpec},
    dataset::DatasetManifest,
    flight::FlightEvent,
    market::ReplayWindow,
    result::{EquitySample, FillRecord, OrderRecord, RunResult, RunSummary},
    time::{NS_PER_MS, NS_PER_S},
};
use bb_data::book::ReplayRequest;
use tower_http::services::{ServeDir, ServeFile};

use crate::{
    DISCLAIMER,
    api::{
        CompareRequest, Demo, Health, OrderDetail, OrdersPage, OrdersQuery, Preset, PresetsQuery,
        ReplayQuery, RunRequest, RunTimeline, TapePage, TapeQuery, TimelineQuery,
    },
    datasets::{self, DatasetCache},
    demos,
    error::AppError,
    pipeline, presets, report,
    store::RunStore,
};

/// Default replay window length when `t1` is not given.
const DEFAULT_REPLAY_SPAN_NS: i64 = 60 * NS_PER_S;
/// Default number of depth frames across the replay window.
const DEFAULT_REPLAY_FRAMES: i64 = 300;
const DEFAULT_REPLAY_LEVELS: u32 = 25;
const MAX_REPLAY_LEVELS: u32 = 100;
const DEFAULT_REPLAY_TRADES: u32 = 5_000;
const MAX_REPLAY_TRADES: u32 = 50_000;
/// Upper bound on depth frames in one replay response. Each frame carries up to
/// `2 * MAX_REPLAY_LEVELS` levels, so an unbounded `(t1 - t0) / step_ns` could ask the server
/// for an arbitrarily large response.
pub const MAX_REPLAY_FRAMES: i64 = 5_000;

const DEFAULT_ORDERS_LIMIT: u64 = 200;
const MAX_ORDERS_LIMIT: u64 = 2_000;
const DEFAULT_TAPE_LIMIT: u64 = 500;
const MAX_TAPE_LIMIT: u64 = 5_000;
/// Longest accepted run label. Labels are display text stored with every run.
const MAX_LABEL_CHARS: usize = 200;

/// Everything the handlers share.
pub struct AppState {
    /// Dataset root: one sub-directory per dataset (`<data-dir>/datasets`).
    pub data_root: PathBuf,
    pub datasets: DatasetCache,
    pub store: Arc<dyn RunStore>,
    /// Worker threads used by one sensitivity grid.
    pub grid_threads: usize,
}

type Shared = State<Arc<AppState>>;

/// Builds the application router. With `web_dir`, files in it are served for every non-API
/// path and unknown paths fall back to its `index.html`, as a single-page app expects.
pub fn router(state: Arc<AppState>, web_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/health", get(get_health))
        .route("/datasets", get(list_datasets))
        .route("/datasets/{id}", get(get_dataset))
        .route("/datasets/{id}/replay", get(get_replay))
        .route("/presets", get(list_presets))
        .route("/demos", get(list_demos))
        .route("/runs", get(list_runs).post(create_run))
        .route("/runs/{run_id}", get(get_run).delete(delete_run))
        .route("/runs/{run_id}/engine-check", get(get_engine_check))
        .route("/runs/{run_id}/orders", get(list_orders))
        .route("/runs/{run_id}/orders/{order_id}", get(get_order))
        .route("/runs/{run_id}/fills", get(list_fills))
        .route("/runs/{run_id}/equity", get(list_equity))
        .route("/runs/{run_id}/tape", get(get_tape))
        .route("/runs/{run_id}/timeline", get(get_timeline))
        .route("/runs/{run_id}/report", get(get_report))
        .route("/runs/{run_id}/export", get(export_run))
        .route("/compare", post(compare_runs))
        .route("/grids", post(run_grid))
        // Unknown API paths answer with the JSON error body, never with the web UI's HTML.
        .fallback(no_such_endpoint)
        .with_state(state);

    let app = Router::new().nest("/api", api);
    // No CORS layer: the UI is same-origin. Cross-origin and DNS-rebinding access is refused by
    // `access::guard`, which the binary wraps around this router.
    match web_dir {
        Some(dir) => {
            let index = ServeFile::new(dir.join("index.html"));
            app.fallback_service(ServeDir::new(dir).fallback(index))
        }
        None => app.fallback(no_such_endpoint),
    }
}

/// Serves `app` on `listener` until SIGTERM or Ctrl-C.
pub async fn serve(listener: tokio::net::TcpListener, app: Router) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(crate::runtime::shutdown_signal())
        .await
}

/// The health payload.
pub fn health() -> Health {
    Health {
        status: "ok".to_string(),
        blackbox_version: env!("CARGO_PKG_VERSION").to_string(),
        contract_version: CONTRACT_VERSION,
        engine: pipeline::engine_info(),
        disclaimer: DISCLAIMER.to_string(),
    }
}

/// Resolves the replay query against the dataset's exchange-clock span `[start, end]`, applying
/// the documented defaults and caps.
pub fn replay_request(
    start: RelNs,
    end: RelNs,
    query: &ReplayQuery,
) -> Result<ReplayRequest, AppError> {
    let t0 = query.t0.unwrap_or(start);
    let t1 = query
        .t1
        .unwrap_or_else(|| end.min(t0.saturating_add(DEFAULT_REPLAY_SPAN_NS)));
    if t1 <= t0 {
        return Err(AppError::InvalidRequest(format!(
            "t1 ({t1}) must be greater than t0 ({t0})"
        )));
    }
    let span = t1.checked_sub(t0).ok_or_else(|| {
        AppError::InvalidRequest(format!(
            "the window from t0 ({t0}) to t1 ({t1}) is too long"
        ))
    })?;
    let step_ns = query
        .step_ns
        .unwrap_or_else(|| (span / DEFAULT_REPLAY_FRAMES).max(NS_PER_MS));
    if step_ns <= 0 {
        return Err(AppError::InvalidRequest(format!(
            "step_ns ({step_ns}) must be positive"
        )));
    }
    // Compare before adding the first frame, so `span / step_ns + 1` cannot overflow.
    let frames = span / step_ns;
    if frames >= MAX_REPLAY_FRAMES {
        let frames = frames.saturating_add(1);
        return Err(AppError::InvalidRequest(format!(
            "the window would hold {frames} depth frames; the limit is {MAX_REPLAY_FRAMES}. \
             Increase step_ns or shorten the window"
        )));
    }
    Ok(ReplayRequest {
        t0,
        t1,
        step_ns,
        levels: query
            .levels
            .unwrap_or(DEFAULT_REPLAY_LEVELS)
            .min(MAX_REPLAY_LEVELS),
        max_trades: query
            .max_trades
            .unwrap_or(DEFAULT_REPLAY_TRADES)
            .min(MAX_REPLAY_TRADES),
    })
}

/// Runs `work` on the blocking thread pool.
async fn blocking<T, F>(work: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|join_error| {
            Err(AppError::Internal(format!(
                "a worker task failed: {join_error}"
            )))
        })
}

/// At most this many single-run simulations execute at once; further requests wait their turn.
/// A run is single-threaded and bounded by the configuration limits, so this caps the server's
/// CPU and memory under concurrent requests.
const MAX_CONCURRENT_RUNS: usize = 4;
static RUN_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_CONCURRENT_RUNS);
/// One sensitivity grid at a time; a grid already uses `grid_threads` workers internally.
static GRID_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Like [`blocking`], after taking a slot from `slots`.
async fn admitted<T, F>(slots: &'static tokio::sync::Semaphore, work: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    let _slot = slots
        .acquire()
        .await
        .map_err(|_| AppError::Internal("the simulation admission queue is closed".into()))?;
    blocking(work).await
}

fn load_run(state: &AppState, run_id: &str) -> Result<Arc<RunResult>, AppError> {
    state
        .store
        .get(run_id)?
        .ok_or_else(|| run_not_found(run_id))
}

fn run_not_found(run_id: &str) -> AppError {
    AppError::NotFound(format!("run '{run_id}' not found"))
}

async fn no_such_endpoint(OriginalUri(uri): OriginalUri) -> AppError {
    AppError::NotFound(format!("no such endpoint: {}", uri.path()))
}

async fn get_health() -> Json<Health> {
    Json(health())
}

async fn list_datasets(State(state): Shared) -> Result<Json<Vec<DatasetManifest>>, AppError> {
    blocking(move || datasets::list_manifests(&state.data_root))
        .await
        .map(Json)
}

async fn get_dataset(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<DatasetManifest>, AppError> {
    let Path(id) = path?;
    blocking(move || datasets::find_manifest(&state.data_root, &id))
        .await
        .map(Json)
}

async fn get_replay(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<ReplayQuery>, QueryRejection>,
) -> Result<Json<ReplayWindow>, AppError> {
    let Path(id) = path?;
    let Query(query) = query?;
    blocking(move || {
        let ctx = state.datasets.get_or_load(&state.data_root, &id)?;
        let manifest = ctx.manifest();
        let request = replay_request(manifest.start, manifest.end, &query)?;
        Ok(bb_data::book::replay_window(
            ctx.dataset.events.as_slice(),
            &manifest.instrument,
            ctx.dataset.origin_ns,
            &request,
        ))
    })
    .await
    .map(Json)
}

async fn list_presets(
    State(state): Shared,
    query: Result<Query<PresetsQuery>, QueryRejection>,
) -> Result<Json<Vec<Preset>>, AppError> {
    let Query(query) = query?;
    blocking(move || {
        let manifest = match query.dataset_id {
            Some(id) => datasets::find_manifest(&state.data_root, &id)?,
            // Presets are sized for an instrument; with no dataset there is nothing to size
            // them for, so the catalogue is empty rather than made up.
            None => match datasets::list_manifests(&state.data_root)?
                .into_iter()
                .next()
            {
                Some(first) => first,
                None => return Ok(Vec::new()),
            },
        };
        Ok(presets::presets(&manifest))
    })
    .await
    .map(Json)
}

async fn list_demos(
    State(state): Shared,
    query: Result<Query<PresetsQuery>, QueryRejection>,
) -> Result<Json<Vec<Demo>>, AppError> {
    let Query(query) = query?;
    blocking(move || {
        let manifest = match query.dataset_id {
            Some(id) => datasets::find_manifest(&state.data_root, &id)?,
            None => match datasets::list_manifests(&state.data_root)?
                .into_iter()
                .next()
            {
                Some(first) => first,
                None => return Ok(Vec::new()),
            },
        };
        Ok(demos::demos(&manifest))
    })
    .await
    .map(Json)
}

async fn create_run(
    State(state): Shared,
    payload: Result<Json<RunRequest>, JsonRejection>,
) -> Result<Json<RunSummary>, AppError> {
    let Json(request) = payload?;
    admitted(&RUN_SLOTS, move || {
        // Validate before looking for the dataset so a broken config is reported as such.
        pipeline::check_config(&request.config)?;
        if let Some(label) = &request.label
            && label.chars().count() > MAX_LABEL_CHARS
        {
            return Err(AppError::InvalidRequest(format!(
                "label is longer than {MAX_LABEL_CHARS} characters"
            )));
        }
        let ctx = state
            .datasets
            .get_or_load(&state.data_root, &request.config.dataset_id)?;
        let label = request
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| request.config.strategy.kind.label());
        // Computes the run id first and returns the stored run when this exact experiment was
        // already simulated, which is what makes the endpoint idempotent.
        pipeline::find_or_execute(&ctx, &request.config, state.store.as_ref(), label)
    })
    .await
    .map(Json)
}

async fn list_runs(State(state): Shared) -> Result<Json<Vec<RunSummary>>, AppError> {
    blocking(move || state.store.list()).await.map(Json)
}

async fn get_run(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Json<RunSummary>, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        state
            .store
            .summary(&run_id)?
            .ok_or_else(|| run_not_found(&run_id))
    })
    .await
    .map(Json)
}

async fn delete_run(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        if state.store.delete(&run_id)? {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(run_not_found(&run_id))
        }
    })
    .await
}

async fn list_orders(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<OrdersQuery>, QueryRejection>,
) -> Result<Json<OrdersPage>, AppError> {
    let Path(run_id) = path?;
    let Query(query) = query?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        let offset = query.offset.unwrap_or(0);
        let limit = query
            .limit
            .unwrap_or(DEFAULT_ORDERS_LIMIT)
            .min(MAX_ORDERS_LIMIT);
        let mut total = 0u64;
        let mut orders = Vec::new();
        let matching = run.orders.iter().filter(|order| {
            query.outcome.is_none_or(|outcome| order.outcome == outcome)
                && query.side.is_none_or(|side| order.side == side)
        });
        for order in matching {
            if total >= offset && (orders.len() as u64) < limit {
                orders.push(order.clone());
            }
            total += 1;
        }
        Ok(OrdersPage {
            total,
            offset,
            orders,
        })
    })
    .await
    .map(Json)
}

async fn get_order(
    State(state): Shared,
    path: Result<Path<(String, u64)>, PathRejection>,
) -> Result<Json<OrderDetail>, AppError> {
    let Path((run_id, order_id)) = path?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        if !run.orders.iter().any(|order| order.order_id == order_id) {
            return Err(AppError::NotFound(format!(
                "run '{run_id}' has no order {order_id}"
            )));
        }
        let dataset = datasets::manifest_for_run(&state.data_root, &run.manifest)?;
        pipeline::order_detail(&run, &dataset, order_id)
            .ok_or_else(|| AppError::NotFound(format!("run '{run_id}' has no order {order_id}")))
    })
    .await
    .map(Json)
}

async fn list_fills(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        // Serialized here, from the shared run, instead of cloning the whole vector first.
        Ok(Json(&run.fills).into_response())
    })
    .await
}

async fn list_equity(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        Ok(Json(&run.equity).into_response())
    })
    .await
}

/// The cross-check of BlackBox's ledger against the engine's own local account for one run.
async fn get_engine_check(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        Ok(Json(&run.engine_check).into_response())
    })
    .await
}

async fn get_tape(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<TapeQuery>, QueryRejection>,
) -> Result<Json<TapePage>, AppError> {
    let Path(run_id) = path?;
    let Query(query) = query?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        let from_seq = query.from_seq.unwrap_or(0);
        let limit = query
            .limit
            .unwrap_or(DEFAULT_TAPE_LIMIT)
            .min(MAX_TAPE_LIMIT) as usize;
        let page = match query.order_id {
            None => {
                // The tape is ordered by `seq`, so the page start is found by bisection.
                let start = run.tape.partition_point(|event| event.seq < from_seq);
                TapePage {
                    total: run.tape.len() as u64,
                    events: run.tape[start..].iter().take(limit).cloned().collect(),
                }
            }
            Some(order_id) => {
                let mut total = 0u64;
                let mut events = Vec::new();
                for event in run.tape.iter().filter(|e| e.order_id == Some(order_id)) {
                    total += 1;
                    if event.seq >= from_seq && events.len() < limit {
                        events.push(event.clone());
                    }
                }
                TapePage { total, events }
            }
        };
        Ok(page)
    })
    .await
    .map(Json)
}

/// The resolved inclusive bounds of a timeline window and the run span.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimelineWindow {
    pub t0: i64,
    pub t1: i64,
    /// First and last equity sample ts (0 without samples).
    pub run_start: i64,
    pub run_end: i64,
}

/// Resolves `t0` (default: run start) and `t1` (default: `min(run end, t0 + 60 s)`).
pub fn timeline_window(
    run: &RunResult,
    t0: Option<i64>,
    t1: Option<i64>,
) -> Result<TimelineWindow, AppError> {
    let run_start = run.equity.first().map_or(0, |s| s.ts);
    let run_end = run.equity.last().map_or(0, |s| s.ts);
    let t0 = t0.unwrap_or(run_start);
    let t1 = t1.unwrap_or_else(|| run_end.min(t0.saturating_add(DEFAULT_REPLAY_SPAN_NS)));
    if t1 < t0 {
        return Err(AppError::InvalidRequest("timeline t1 must be >= t0".into()));
    }
    Ok(TimelineWindow {
        t0,
        t1,
        run_start,
        run_end,
    })
}

/// Tape events with `ts` in `[t0, t1]` (on either clock; the tape is not time-sorted) and
/// `seq >= from_seq`, in `seq` order.
pub fn timeline_events(
    run: &RunResult,
    w: TimelineWindow,
    from_seq: u64,
) -> impl Iterator<Item = &FlightEvent> {
    let start = run.tape.partition_point(|event| event.seq < from_seq);
    run.tape[start..]
        .iter()
        .filter(move |event| (w.t0..=w.t1).contains(&event.ts))
}

/// What a window shows besides its events: fills in it, the equity samples in it plus the last
/// one before `t0`, and the orders alive at some point in it (ascending `order_id`).
pub struct TimelineContext {
    pub fills: Vec<FillRecord>,
    pub equity: Vec<EquitySample>,
    pub orders: Vec<OrderRecord>,
}

pub fn timeline_context(run: &RunResult, w: TimelineWindow) -> TimelineContext {
    let TimelineWindow {
        t0, t1, run_end, ..
    } = w;
    let start = run.equity.partition_point(|s| s.ts < t0);
    let end = run.equity.partition_point(|s| s.ts <= t1);
    let equity = run.equity[start.saturating_sub(1)..end].to_vec();
    let mut orders: Vec<_> = run
        .orders
        .iter()
        .filter(|order| {
            let last_ts = order.terminal_ts.unwrap_or_else(|| {
                run.tape
                    .get(order.last_seq as usize)
                    .filter(|event| {
                        event.seq == order.last_seq && event.order_id == Some(order.order_id)
                    })
                    .map_or(run_end, |event| event.ts)
            });
            order.decision_ts <= t1 && last_ts >= t0
        })
        .cloned()
        .collect();
    orders.sort_unstable_by_key(|order| order.order_id);
    TimelineContext {
        fills: run
            .fills
            .iter()
            .filter(|fill| (t0..=t1).contains(&fill.ts))
            .cloned()
            .collect(),
        equity,
        orders,
    }
}

/// Selects inclusive windows without assuming the tape's two clocks are time-sorted.
pub fn timeline(run: &RunResult, query: &TimelineQuery) -> Result<RunTimeline, AppError> {
    let w = timeline_window(run, query.t0, query.t1)?;
    let limit = query.max_events.unwrap_or(5_000).min(50_000) as usize;
    let mut events = Vec::new();
    let mut events_total = 0;
    for event in timeline_events(run, w, 0) {
        events_total += 1;
        if events.len() < limit {
            events.push(event.clone());
        }
    }
    let context = timeline_context(run, w);
    Ok(RunTimeline {
        run_id: run.manifest.run_id.clone(),
        dataset_id: run.manifest.dataset.id.clone(),
        t0: w.t0,
        t1: w.t1,
        events_truncated: events_total > events.len() as u64,
        events,
        events_total,
        fills: context.fills,
        equity: context.equity,
        orders: context.orders,
        run_start: w.run_start,
        run_end: w.run_end,
    })
}

async fn get_timeline(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<TimelineQuery>, QueryRejection>,
) -> Result<Json<RunTimeline>, AppError> {
    let Path(run_id) = path?;
    let Query(query) = query?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        timeline(&run, &query)
    })
    .await
    .map(Json)
}

async fn get_report(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        let dataset = datasets::find_manifest(&state.data_root, &run.manifest.dataset.id)?;
        let markdown = report::markdown(&run, &dataset)?;
        Ok((
            [
                (
                    header::CONTENT_TYPE,
                    "text/markdown; charset=utf-8".to_string(),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"blackbox-run-{run_id}.md\""),
                ),
            ],
            markdown,
        )
            .into_response())
    })
    .await
}

async fn export_run(
    State(state): Shared,
    path: Result<Path<String>, PathRejection>,
) -> Result<Response, AppError> {
    let Path(run_id) = path?;
    blocking(move || {
        let run = load_run(&state, &run_id)?;
        Ok((
            [(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"blackbox-run-{run_id}.json\""),
            )],
            Json(run.as_ref()),
        )
            .into_response())
    })
    .await
}

async fn compare_runs(
    State(state): Shared,
    payload: Result<Json<CompareRequest>, JsonRejection>,
) -> Result<Json<Comparison>, AppError> {
    let Json(request) = payload?;
    blocking(move || {
        let label = |run_id: &str| -> Result<String, AppError> {
            Ok(state
                .store
                .summary(run_id)?
                .ok_or_else(|| run_not_found(run_id))?
                .label)
        };
        let label_a = label(&request.a)?;
        let label_b = label(&request.b)?;
        let a = load_run(&state, &request.a)?;
        let b = load_run(&state, &request.b)?;
        // A run whose dataset is gone or changed still compares; its instrument is then
        // reported as unverified rather than assumed.
        let instrument = |run: &RunResult| {
            datasets::manifest_for_run(&state.data_root, &run.manifest)
                .ok()
                .map(|m| m.instrument)
        };
        let (inst_a, inst_b) = (instrument(&a), instrument(&b));
        Ok(bb_forensics::compare_runs(
            bb_forensics::CompareInput {
                run: &a,
                label: &label_a,
                instrument: inst_a.as_ref(),
            },
            bb_forensics::CompareInput {
                run: &b,
                label: &label_b,
                instrument: inst_b.as_ref(),
            },
        ))
    })
    .await
    .map(Json)
}

async fn run_grid(
    State(state): Shared,
    payload: Result<Json<GridSpec>, JsonRejection>,
) -> Result<Json<GridResult>, AppError> {
    let Json(spec) = payload?;
    admitted(&GRID_SLOTS, move || {
        // Validate before looking for the dataset so a broken grid is reported as such.
        pipeline::check_grid(&spec)?;
        let ctx = state
            .datasets
            .get_or_load(&state.data_root, &spec.base.dataset_id)?;
        pipeline::run_grid(&ctx, &spec, state.store.as_ref(), state.grid_threads)
    })
    .await
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: RelNs = 120 * NS_PER_S;

    fn resolve(query: ReplayQuery) -> ReplayRequest {
        match replay_request(0, END, &query) {
            Ok(request) => request,
            Err(err) => panic!("query {query:?} was rejected: {err}"),
        }
    }

    fn rejected(query: ReplayQuery) -> bool {
        matches!(
            replay_request(0, END, &query),
            Err(AppError::InvalidRequest(_))
        )
    }

    #[test]
    fn replay_defaults_follow_the_api_document() {
        let r = resolve(ReplayQuery::default());
        assert_eq!((r.t0, r.t1), (0, 60 * NS_PER_S));
        assert_eq!(r.step_ns, 60 * NS_PER_S / 300);
        assert_eq!((r.levels, r.max_trades), (25, 5_000));

        // A window starting near the end stops at the dataset end.
        let r = resolve(ReplayQuery {
            t0: Some(100 * NS_PER_S),
            ..ReplayQuery::default()
        });
        assert_eq!(r.t1, END);

        // A very short window still steps by at least one millisecond.
        let r = resolve(ReplayQuery {
            t0: Some(0),
            t1: Some(30 * NS_PER_MS),
            ..ReplayQuery::default()
        });
        assert_eq!(r.step_ns, NS_PER_MS);
    }

    #[test]
    fn replay_caps_are_applied() {
        let r = resolve(ReplayQuery {
            levels: Some(10_000),
            max_trades: Some(10_000_000),
            ..ReplayQuery::default()
        });
        assert_eq!((r.levels, r.max_trades), (100, 50_000));
    }

    #[test]
    fn replay_rejects_empty_windows_bad_steps_and_oversized_responses() {
        assert!(rejected(ReplayQuery {
            t0: Some(10),
            t1: Some(10),
            ..ReplayQuery::default()
        }));
        assert!(rejected(ReplayQuery {
            t0: Some(10),
            t1: Some(5),
            ..ReplayQuery::default()
        }));
        // Starting at or after the dataset end leaves no default window.
        assert!(rejected(ReplayQuery {
            t0: Some(END),
            ..ReplayQuery::default()
        }));
        for step_ns in [0, -1] {
            assert!(rejected(ReplayQuery {
                step_ns: Some(step_ns),
                ..ReplayQuery::default()
            }));
        }
        assert!(rejected(ReplayQuery {
            step_ns: Some(1),
            ..ReplayQuery::default()
        }));
        assert!(rejected(ReplayQuery {
            t0: Some(i64::MIN),
            t1: Some(i64::MAX),
            ..ReplayQuery::default()
        }));
    }

    #[test]
    fn health_reports_the_pinned_engine_and_the_disclaimer() {
        let h = health();
        assert_eq!(h.status, "ok");
        assert_eq!(h.contract_version, CONTRACT_VERSION);
        assert_eq!(h.engine.name, "hftbacktest");
        assert_eq!(h.engine.version, "0.9.4");
        assert_eq!(h.engine.features, ["backtest"]);
        assert!(h.disclaimer.contains("simulation"));
    }
}
