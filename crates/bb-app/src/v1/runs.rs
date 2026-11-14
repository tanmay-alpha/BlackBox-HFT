//! Runs (completed results) and their data (API_CONTRACT §8.4), with integrity on read
//! (TARGET_ARCHITECTURE §7, AT-3): every summary is checked against `summary_sha256`, every
//! bundle against the trust root stored in Postgres, every page against its digest. A failure
//! answers 500 `integrity_failed` and quarantines the run.

use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use bb_artifacts::{BundleCounts, Section, summary_sha256};
use bb_core::{
    Side,
    flight::FlightEvent,
    result::{
        EngineStateCheck, EquitySample, FillRecord, Metrics, OrderOutcome, OrderRecord, RunResult,
    },
};
use bb_db::{RunSort, RunView, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::SemaphorePermit;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    experiments::{Params, experiment_query},
    model::{BundleRef, RunSummary},
    respond::{
        Cache, ENVELOPE_RESERVE, Page, TruncatedBy, bounded, byte_cap, conditional, decode_cursor,
        encode_cursor, json, json_bytes, json_value, ns_param, wire_item,
    },
};
use crate::{
    analytics::{self, RunAnalytics},
    artifacts::{OpenRun, open_run, quarantine},
    registry::manifest_of,
    wire,
};

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(list_runs))
        .routes(routes!(get_run))
        .routes(routes!(engine_check))
        .routes(routes!(list_orders))
        .routes(routes!(get_order))
        .routes(routes!(list_fills))
        .routes(routes!(list_equity))
        .routes(routes!(list_tape))
        .routes(routes!(timeline))
        .routes(routes!(report))
        .routes(routes!(export))
        .routes(routes!(run_analytics))
        .routes(routes!(run_analytics_series))
}

/// 16- or 64-character lowercase hex.
pub fn valid_identity(id: &str) -> bool {
    (id.len() == 16 || id.len() == 64)
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The visible live run of `identity`, or `integrity_failed` for a quarantined one.
pub async fn resolve_run(
    state: &Shared,
    caller: &Caller,
    identity: &str,
    platform: Option<&str>,
) -> Result<RunView, ApiError> {
    let identity = identity.to_ascii_lowercase();
    if !valid_identity(&identity) {
        return Err(ApiError::not_found("run"));
    }
    let platform = platform.unwrap_or(&state.cfg.result_platform);
    match state
        .db
        .run_view(caller.workspace.id, &identity, platform)
        .await
    {
        Ok(view) => Ok(view),
        Err(bb_db::Error::NotFound) => {
            match state
                .db
                .quarantined_run(caller.workspace.id, &identity, platform)
                .await?
            {
                Some(run) => Err(ApiError::integrity_failed(
                    &identity[..16],
                    "bundle",
                    &hex::encode(run.result_sha256),
                )),
                None => Err(ApiError::not_found("run")),
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn counts(view: &RunView) -> BundleCounts {
    let n = |v: i64| u64::try_from(v).unwrap_or(0);
    BundleCounts {
        ticks: n(view.run.ticks),
        orders: n(view.run.order_count),
        fills: n(view.run.fill_count),
        equity: n(view.run.equity_len),
        tape: n(view.run.tape_len),
    }
}

/// The summary of a run after checking the stored row against `summary_row_sha256` (DB only)
/// and its metrics against `summary_sha256`. A `summary_sha256` mismatch that is only JSONB
/// number normalisation (e.g. `-0.0` stored as `0`) is resolved against the verified bundle
/// trailer; any real difference quarantines the run.
pub async fn verified_summary(
    state: &Shared,
    view: &RunView,
    include_config: bool,
) -> Result<RunSummary, ApiError> {
    if !view.summary_intact {
        let expected = hex::encode(&view.run.summary_row_sha256);
        return Err(quarantine(&state.db, &state.metrics, view, "summary", &expected).await);
    }
    let metrics: Metrics = serde_json::from_value(view.run.metrics.clone()).map_err(|_| {
        ApiError::integrity_failed(
            &view.short_id,
            "summary",
            &hex::encode(&view.run.summary_sha256),
        )
    })?;
    let engine_check: EngineStateCheck = serde_json::from_value(view.run.engine_check.clone())
        .map_err(|_| {
            ApiError::integrity_failed(
                &view.short_id,
                "summary",
                &hex::encode(&view.run.summary_sha256),
            )
        })?;
    let expected = hex::encode(&view.run.summary_sha256);
    let computed = summary_sha256(&metrics, &engine_check, &counts(view))?;
    if computed != expected {
        let open = open(state, view.clone()).await?;
        let trailer = open.trailer().await?;
        if trailer.metrics != metrics || trailer.engine_check != engine_check {
            return Err(quarantine(&state.db, &state.metrics, view, "summary", &expected).await);
        }
    }
    RunSummary::from_view(view, include_config)
}

/// The summary of a run for DB-only list paths (runs, experiments, the demo snapshot): the row
/// is checked against `summary_row_sha256` without opening artefacts (FINAL_AUDIT §4 item 5).
/// A changed row is quarantined (which also invalidates the anonymous caches) and left out.
pub async fn listed_summary(
    state: &Shared,
    view: &RunView,
) -> Result<Option<RunSummary>, ApiError> {
    if !view.summary_intact {
        let expected = hex::encode(&view.run.summary_row_sha256);
        let _ = quarantine(&state.db, &state.metrics, view, "summary", &expected).await;
        return Ok(None);
    }
    RunSummary::from_view(view, false).map(Some)
}

async fn open(state: &Shared, view: RunView) -> Result<OpenRun, ApiError> {
    open_run(
        &state.db,
        &state.storage,
        &state.cache,
        &state.metrics,
        state.bundle_limits(),
        view,
    )
    .await
}

/// A permit of the artefact-heavy concurrency limit (4 in flight, 2 s wait).
async fn heavy(state: &Shared) -> Result<SemaphorePermit<'_>, ApiError> {
    match tokio::time::timeout(Duration::from_secs(2), state.heavy.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(ApiError::unavailable("busy", 2)),
    }
}

/// Makes sure the run's bundle was verified on this instance (first use fetches it once;
/// afterwards this is a cache hit), so stored-object tampering surfaces on every run read.
async fn verify_bundle(state: &Shared, view: &RunView) -> Result<(), ApiError> {
    let _permit = heavy(state).await?;
    open(state, view.clone()).await.map(|_| ())
}

/// Resolve + verify summary + open the bundle.
async fn run_data(
    state: &Shared,
    caller: &Caller,
    run_id: &str,
    platform: Option<&str>,
) -> Result<OpenRun, ApiError> {
    let view = resolve_run(state, caller, run_id, platform).await?;
    verified_summary(state, &view, false).await?;
    open(state, view).await
}

fn immutable<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<Response, ApiError> {
    Ok(conditional(
        headers,
        json(StatusCode::OK, value, Cache::Immutable)?,
    ))
}

fn immutable_value(headers: &HeaderMap, value: &Value) -> Result<Response, ApiError> {
    Ok(conditional(
        headers,
        json_value(StatusCode::OK, value, Cache::Immutable)?,
    ))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PlatformQuery {
    /// Platform class (default: this deployment's result platform, `linux-x86_64` hosted).
    pub platform: Option<String>,
}

#[derive(IntoParams)]
#[into_params(parameter_in = Query)]
#[allow(dead_code)]
pub struct RunListQuery {
    pub workspace: Option<String>,
    pub platform: Option<String>,
    pub dataset_id: Option<String>,
    pub strategy_kind: Option<String>,
    pub sweep_id: Option<String>,
    pub label_prefix: Option<String>,
    pub created_after_ns: Option<String>,
    pub created_before_ns: Option<String>,
    pub mine: Option<bool>,
    pub run_id: Option<String>,
    /// `created_desc` (default), `net_pnl_desc`, `net_pnl_asc`, `max_drawdown_asc`.
    pub sort: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// Completed runs of a workspace, from Postgres only (no artefact reads).
#[utoipa::path(get, path = "/runs", tag = "runs", security((), ("bearer" = [])), params(RunListQuery),
    responses((status = 200, body = Page<RunSummary>), (status = 400, body = ErrorEnvelope),
              (status = 500, body = ErrorEnvelope)))]
pub async fn list_runs(
    State(state): State<Shared>,
    caller: Caller,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let p = Params(pairs);
    let workspace = caller.read_workspace(p.get("workspace"))?.clone();
    let q = experiment_query(&p, &caller)?;
    let sort: RunSort = match p.get("sort") {
        None => RunSort::CreatedDesc,
        Some(s) => serde_json::from_value(Value::String(s.into()))
            .map_err(|_| ApiError::invalid_field("sort", format!("unknown sort `{s}`")))?,
    };
    let platform = p
        .get("platform")
        .unwrap_or(&state.cfg.result_platform)
        .to_string();
    let limit = p.limit(50, 100)?;
    let filters = p.filters();
    let after: Option<bb_db::RunKey> = p
        .get("cursor")
        .map(|c| decode_cursor(c, &filters))
        .transpose()?;
    let views = match state.snapshot.for_read(&state, &caller).await {
        Some(s) => s.run_views(&platform, &q, sort, after.as_ref(), peek(limit) as usize),
        None => {
            state
                .db
                .run_views(
                    workspace.id,
                    &platform,
                    &q,
                    sort,
                    after.as_ref(),
                    peek(limit),
                )
                .await?
        }
    };
    let more = has_more(views.len(), limit);
    let views = &views[..views.len().min(limit as usize)];
    // Lists come from Postgres only (AT-10): no artefact reads, so one unreadable bundle can
    // never fail a page. Each row is checked against its stored digest; detail reads also
    // verify the summary against the bundle trust root and the bundle itself.
    let mut listed = Vec::with_capacity(views.len());
    for view in views {
        if let Some(summary) = listed_summary(&state, view).await? {
            listed.push((view, summary));
        }
    }
    let summaries: Vec<&RunSummary> = listed.iter().map(|(_, s)| s).collect();
    let (items, cut) = bounded(&summaries, byte_cap(&state, &caller))?;
    // A page cut by bytes continues after its last item; a full page continues after its last
    // row even when a changed row was left out.
    let next = if cut && !items.is_empty() {
        listed
            .get(items.len() - 1)
            .map(|(v, _)| encode_cursor(&v.key(sort), &filters))
    } else if more {
        views.last().map(|v| encode_cursor(&v.key(sort), &filters))
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

/// One run: verified summary, configuration and bundle reference.
#[utoipa::path(get, path = "/runs/{run_id}", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path, description = "16-hex run id or 64-hex fingerprint"), PlatformQuery),
    responses((status = 200, body = RunSummary), (status = 404, body = ErrorEnvelope),
              (status = 500, body = ErrorEnvelope)))]
pub async fn get_run(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<PlatformQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let view = resolve_run(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let mut summary = verified_summary(&state, &view, true).await?;
    verify_bundle(&state, &view).await?;
    let artifact = state.db.artifact(view.run.id).await?;
    summary.bundle = Some(BundleRef {
        format_version: artifact.format_version,
        sha256: hex::encode(&artifact.sha256),
        size_bytes: artifact.size_bytes,
    });
    immutable(&headers, &summary)
}

/// The ledger-versus-engine cross-check of a run.
#[utoipa::path(get, path = "/runs/{run_id}/engine-check", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), PlatformQuery),
    responses((status = 200, body = EngineStateCheck), (status = 404, body = ErrorEnvelope)))]
pub async fn engine_check(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<PlatformQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let view = resolve_run(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let summary = verified_summary(&state, &view, false).await?;
    verify_bundle(&state, &view).await?;
    immutable(&headers, &summary.engine_check)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OrdersQuery {
    pub platform: Option<String>,
    pub outcome: Option<String>,
    /// `buy` or `sell`.
    pub side: Option<String>,
    pub offset: Option<u64>,
    /// ≤ 2000, default 200.
    pub limit: Option<u64>,
}

#[derive(Serialize, ToSchema)]
pub struct OrdersPage {
    pub items: Vec<OrderRecord>,
    pub offset: u64,
    /// Orders matching the filters.
    pub total: u64,
    pub next_offset: Option<u64>,
    pub truncated_by: Option<TruncatedBy>,
}

fn enum_param<T: serde::de::DeserializeOwned>(
    field: &str,
    value: Option<&str>,
) -> Result<Option<T>, ApiError> {
    value
        .map(|v| {
            serde_json::from_value(Value::String(v.into()))
                .map_err(|_| ApiError::invalid_field(field, format!("unknown {field} `{v}`")))
        })
        .transpose()
}

/// Order lifecycle records, offset-paged with the filtered total.
#[utoipa::path(get, path = "/runs/{run_id}/orders", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), OrdersQuery),
    responses((status = 200, body = OrdersPage), (status = 404, body = ErrorEnvelope),
              (status = 500, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)))]
pub async fn list_orders(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<OrdersQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let outcome: Option<OrderOutcome> = enum_param("outcome", q.outcome.as_deref())?;
    let side: Option<Side> = enum_param("side", q.side.as_deref())?;
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(200);
    if !(1..=2000).contains(&limit) {
        return Err(ApiError::invalid_field(
            "limit",
            "limit must be between 1 and 2000",
        ));
    }
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let mut total = 0u64;
    let mut window: Vec<OrderRecord> = Vec::new();
    for (page_no, _, _) in run.pages(Section::Orders) {
        let rows: Vec<OrderRecord> = run.page(page_no).await?;
        for order in rows {
            if outcome.is_none_or(|o| order.outcome == o) && side.is_none_or(|s| order.side == s) {
                if total >= offset && (window.len() as u64) < limit {
                    window.push(order);
                }
                total += 1;
            }
        }
    }
    let (items, cut) = bounded(&window, byte_cap(&state, &caller))?;
    let shown = items.len() as u64;
    let next = offset + shown;
    let body = json!({
        "items": items,
        "offset": offset,
        "total": total,
        "next_offset": if next < total { Some(next) } else { None },
        "truncated_by": if cut { Some(TruncatedBy::Bytes) } else if next < total { Some(TruncatedBy::Limit) } else { None },
    });
    immutable_value(&headers, &body)
}

/// One order: its record, tape events, fills and the rule-based explanation.
#[derive(Serialize, ToSchema)]
pub struct OrderDetail {
    pub order: OrderRecord,
    pub events: Vec<FlightEvent>,
    pub fills: Vec<FillRecord>,
    pub explanation: bb_core::explain::Explanation,
    pub instrument: bb_core::dataset::InstrumentMeta,
}

#[utoipa::path(get, path = "/runs/{run_id}/orders/{order_id}", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), ("order_id" = u64, Path), PlatformQuery),
    responses((status = 200, body = OrderDetail), (status = 404, body = ErrorEnvelope)))]
pub async fn get_order(
    State(state): State<Shared>,
    caller: Caller,
    Path((run_id, order_id)): Path<(String, String)>,
    Query(q): Query<PlatformQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let order_id: u64 = order_id.parse().map_err(|_| ApiError::not_found("order"))?;
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let mut order = None;
    for (page_no, _, _) in run.pages(Section::Orders) {
        let rows: Vec<OrderRecord> = run.page(page_no).await?;
        if let Some(found) = rows.into_iter().find(|o| o.order_id == order_id) {
            order = Some(found);
            break;
        }
    }
    let order = order.ok_or_else(|| ApiError::not_found("order"))?;
    let (first, last) = (order.first_seq, order.last_seq.saturating_add(1));
    let tape_len = run.trust.counts.tape;
    let range = first.min(tape_len)..last.min(tape_len);
    let events: Vec<FlightEvent> = run
        .read(move |reader, path| {
            let bytes = reader.rows(path, Section::Tape, range.clone())?;
            bytes
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(|l| serde_json::from_slice(l).map_err(bb_artifacts::ArtifactError::from))
                .collect()
        })
        .await?;
    let events: Vec<FlightEvent> = events
        .into_iter()
        .filter(|e| e.order_id == Some(order_id))
        .collect();
    let mut fills = Vec::new();
    for (page_no, _, _) in run.pages(Section::Fills) {
        let rows: Vec<FillRecord> = run.page(page_no).await?;
        fills.extend(rows.into_iter().filter(|f| f.order_id == order_id));
    }
    let version = state
        .db
        .dataset_version(&run.view.dataset_id, run.view.dataset_version_no)
        .await?;
    let dataset = manifest_of(&version)?;
    let config: bb_core::config::RunConfig = serde_json::from_value(run.view.config.clone())?;
    let explanation = bb_forensics::explain_order(
        &order,
        &events,
        &dataset.instrument,
        &config,
        dataset.timestamps.local_ts,
    );
    immutable(
        &headers,
        &OrderDetail {
            order,
            events,
            fills,
            explanation,
            instrument: dataset.instrument,
        },
    )
}

#[derive(Deserialize, Serialize, Debug)]
struct RowCursor {
    row: u64,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FillsQuery {
    pub platform: Option<String>,
    /// Inclusive exchange-clock window (decimal ns).
    pub t0_ns: Option<String>,
    pub t1_ns: Option<String>,
    pub cursor: Option<String>,
    /// ≤ 5000, default 1000.
    pub limit: Option<u64>,
}

/// Scans `section` from row `start`, keeping rows accepted by `keep`, until `limit` rows or the
/// byte cap. Returns wire values, the next row to resume from and why the page ended.
async fn scan<T, F>(
    run: &OpenRun,
    section: Section,
    start: u64,
    limit: u64,
    cap: usize,
    skip_page: impl Fn(&bb_artifacts::PageIndex) -> bool,
    keep: F,
) -> Result<(Vec<Value>, Option<u64>, Option<TruncatedBy>), ApiError>
where
    T: serde::de::DeserializeOwned + Serialize + Send + 'static,
    F: Fn(&T) -> bool,
{
    let budget = cap.saturating_sub(4096);
    let mut out = Vec::new();
    let mut used = 0usize;
    for (page_no, first_row, rows) in run.pages(section) {
        if first_row + rows <= start || skip_page(&run.page_index()[page_no]) {
            continue;
        }
        let page: Vec<T> = run.page(page_no).await?;
        for (i, row) in page.iter().enumerate() {
            let row_no = first_row + i as u64;
            if row_no < start || !keep(row) {
                continue;
            }
            if out.len() as u64 == limit {
                return Ok((out, Some(row_no), Some(TruncatedBy::Limit)));
            }
            let mut v = serde_json::to_value(row)?;
            wire::to_wire(&mut v).map_err(ApiError::internal)?;
            let size = serde_json::to_vec(&v)?.len() + 1;
            if used + size > budget {
                return Ok((out, Some(row_no), Some(TruncatedBy::Bytes)));
            }
            used += size;
            out.push(v);
        }
    }
    Ok((out, None, None))
}

fn ts_overlaps(p: &bb_artifacts::PageIndex, clock: &str, t0: Option<i64>, t1: Option<i64>) -> bool {
    match (p.ts_min.get(clock), p.ts_max.get(clock)) {
        (Some(min), Some(max)) => t0.is_none_or(|t0| *max >= t0) && t1.is_none_or(|t1| *min <= t1),
        _ => true,
    }
}

/// Fills in an exchange-clock window, cursor-paged.
#[utoipa::path(get, path = "/runs/{run_id}/fills", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), FillsQuery),
    responses((status = 200, body = Page<FillRecord>), (status = 404, body = ErrorEnvelope)))]
pub async fn list_fills(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<FillsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let t0 = ns_param("t0_ns", q.t0_ns.as_deref())?;
    let t1 = ns_param("t1_ns", q.t1_ns.as_deref())?;
    let limit = q.limit.unwrap_or(1000);
    if !(1..=5000).contains(&limit) {
        return Err(ApiError::invalid_field(
            "limit",
            "limit must be between 1 and 5000",
        ));
    }
    let filters = format!("fills|{t0:?}|{t1:?}|{:?}", q.platform);
    let start = q
        .cursor
        .as_deref()
        .map(|c| decode_cursor::<RowCursor>(c, &filters))
        .transpose()?
        .map_or(0, |c| c.row);
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let (items, next, why) = scan::<FillRecord, _>(
        &run,
        Section::Fills,
        start,
        limit,
        byte_cap(&state, &caller),
        |p| !ts_overlaps(p, "exchange", t0, t1),
        |f| t0.is_none_or(|t0| f.ts >= t0) && t1.is_none_or(|t1| f.ts <= t1),
    )
    .await?;
    let body = json!({
        "items": items,
        "next_cursor": next.map(|row| encode_cursor(&RowCursor { row }, &filters)),
        "truncated_by": why,
    });
    immutable_value(&headers, &body)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TapeQuery {
    pub platform: Option<String>,
    /// First sequence number (default 0); alternatively `cursor`.
    pub from_seq: Option<u64>,
    pub order_id: Option<u64>,
    pub cursor: Option<String>,
    /// ≤ 5000, default 500.
    pub limit: Option<u64>,
}

/// The flight tape, cursor-paged by sequence number.
#[utoipa::path(get, path = "/runs/{run_id}/tape", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), TapeQuery),
    responses((status = 200, body = Page<FlightEvent>), (status = 404, body = ErrorEnvelope)))]
pub async fn list_tape(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<TapeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limit = q.limit.unwrap_or(500);
    if !(1..=5000).contains(&limit) {
        return Err(ApiError::invalid_field(
            "limit",
            "limit must be between 1 and 5000",
        ));
    }
    let filters = format!("tape|{:?}|{:?}", q.order_id, q.platform);
    let start = match &q.cursor {
        Some(c) => decode_cursor::<RowCursor>(c, &filters)?.row,
        None => q.from_seq.unwrap_or(0),
    };
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let order_id = q.order_id;
    let (items, next, why) = scan::<FlightEvent, _>(
        &run,
        Section::Tape,
        start,
        limit,
        byte_cap(&state, &caller),
        |_| false,
        move |e| order_id.is_none_or(|id| e.order_id == Some(id)),
    )
    .await?;
    let body = json!({
        "items": items,
        "next_cursor": next.map(|row| encode_cursor(&RowCursor { row }, &filters)),
        "truncated_by": why,
    });
    immutable_value(&headers, &body)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EquityQuery {
    pub platform: Option<String>,
    pub t0_ns: Option<String>,
    pub t1_ns: Option<String>,
    /// ≤ 5000, default 2000. More samples in the window are downsampled into buckets.
    pub max_points: Option<u64>,
    pub cursor: Option<String>,
}

/// A downsampled equity bucket.
#[derive(Serialize, ToSchema)]
pub struct EquityBucket {
    pub t0_ns: String,
    pub t1_ns: String,
    pub first: f64,
    pub last: f64,
    pub min_equity: f64,
    pub max_equity: f64,
    pub n: u64,
}

#[derive(Serialize, ToSchema)]
pub struct EquityPage {
    /// `EquitySample[]`, or `EquityBucket[]` when `downsampled`.
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<Value>,
    pub downsampled: bool,
    pub bucket_ns: Option<String>,
    pub next_cursor: Option<String>,
    pub truncated_by: Option<TruncatedBy>,
}

/// The equity series of a window: raw samples, or min/max buckets beyond `max_points`.
#[utoipa::path(get, path = "/runs/{run_id}/equity", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), EquityQuery),
    responses((status = 200, body = EquityPage), (status = 404, body = ErrorEnvelope)))]
pub async fn list_equity(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<EquityQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let t0 = ns_param("t0_ns", q.t0_ns.as_deref())?;
    let t1 = ns_param("t1_ns", q.t1_ns.as_deref())?;
    let max_points = q.max_points.unwrap_or(2000);
    if !(1..=5000).contains(&max_points) {
        return Err(ApiError::invalid_field(
            "max_points",
            "max_points must be between 1 and 5000",
        ));
    }
    let filters = format!("equity|{t0:?}|{t1:?}|{max_points}|{:?}", q.platform);
    let start = q
        .cursor
        .as_deref()
        .map(|c| decode_cursor::<RowCursor>(c, &filters))
        .transpose()?
        .map_or(0, |c| c.row);
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let in_window =
        |s: &EquitySample| t0.is_none_or(|t0| s.ts >= t0) && t1.is_none_or(|t1| s.ts <= t1);
    // Count the samples in the window from the page index first (cheap), then read.
    let mut samples: Vec<EquitySample> = Vec::new();
    for (page_no, _, _) in run.pages(Section::Equity) {
        if !ts_overlaps(&run.page_index()[page_no], "local", t0, t1) {
            continue;
        }
        let rows: Vec<EquitySample> = run.page(page_no).await?;
        samples.extend(rows.into_iter().filter(|s| in_window(s)));
    }
    let body = if samples.len() as u64 <= max_points {
        let rest = &samples[(start as usize).min(samples.len())..];
        let (items, cut) = bounded(rest, byte_cap(&state, &caller))?;
        let next = cut.then(|| {
            encode_cursor(
                &RowCursor {
                    row: start + items.len() as u64,
                },
                &filters,
            )
        });
        json!({ "items": items, "downsampled": false, "bucket_ns": null, "next_cursor": next,
                "truncated_by": if cut { Some(TruncatedBy::Bytes) } else { None } })
    } else {
        let lo = samples.first().map_or(0, |s| s.ts);
        let hi = samples.last().map_or(0, |s| s.ts);
        let span = (hi - lo).max(1) as u64 + 1;
        let bucket = span.div_ceil(max_points).max(1) as i64;
        let mut buckets: Vec<EquityBucket> = Vec::new();
        for s in &samples {
            let b0 = lo + ((s.ts - lo) / bucket) * bucket;
            match buckets.last_mut() {
                Some(b) if b.t0_ns == b0.to_string() => {
                    b.last = s.equity;
                    b.min_equity = b.min_equity.min(s.equity);
                    b.max_equity = b.max_equity.max(s.equity);
                    b.n += 1;
                }
                _ => buckets.push(EquityBucket {
                    t0_ns: b0.to_string(),
                    t1_ns: (b0 + bucket - 1).to_string(),
                    first: s.equity,
                    last: s.equity,
                    min_equity: s.equity,
                    max_equity: s.equity,
                    n: 1,
                }),
            }
        }
        let (items, cut) = bounded(&buckets, byte_cap(&state, &caller))?;
        json!({ "items": items, "downsampled": true, "bucket_ns": bucket.to_string(), "next_cursor": null,
                "truncated_by": if cut { Some(TruncatedBy::Bytes) } else { None } })
    };
    immutable_value(&headers, &body)
}

#[derive(Deserialize, IntoParams, Default, Clone)]
#[into_params(parameter_in = Query)]
pub struct TimelineQuery {
    pub platform: Option<String>,
    /// Inclusive window start (default: run start).
    pub t0_ns: Option<String>,
    /// Inclusive window end (default: `min(run end, t0 + 60 s)`).
    pub t1_ns: Option<String>,
    /// Events per page: default 5000; values below 100 are raised to 100 and values above
    /// 50000 are capped. `0` returns no events (counts and context only) and no `next_cursor`;
    /// it is refused on `cursor` pages.
    pub max_events: Option<u32>,
    /// `next_cursor` of the previous page; `t0_ns`, `t1_ns` and `platform` must be unchanged.
    pub cursor: Option<String>,
}

/// One page of everything that happened in a window, for the timeline view.
#[derive(Serialize, ToSchema)]
pub struct RunTimeline {
    pub run_id: String,
    pub dataset_id: String,
    pub t0_ns: String,
    pub t1_ns: String,
    /// This page's tape events with `ts` in `[t0_ns, t1_ns]` (either clock), in `seq` order,
    /// continuing after the previous page.
    #[schema(value_type = Vec<FlightEvent>)]
    pub events: Vec<Value>,
    /// Events of the whole window, over all pages.
    pub events_total: u64,
    /// Pass back as `cursor` for the window's next events; `null` when this page ends the window.
    pub next_cursor: Option<String>,
    /// Why this page stopped before the window's last event: `limit` (`max_events`) or `bytes`
    /// (the response cap). `null` exactly when `next_cursor` is `null`.
    pub truncated_by: Option<TruncatedBy>,
    /// Fills with exchange ts in the window. First page only (empty on `cursor` pages).
    #[schema(value_type = Vec<FillRecord>)]
    pub fills: Vec<Value>,
    /// Samples in the window plus the last one before `t0_ns`. First page only.
    #[schema(value_type = Vec<EquitySample>)]
    pub equity: Vec<Value>,
    /// Orders alive at some point in the window, ascending `order_id`. First page only.
    #[schema(value_type = Vec<OrderRecord>)]
    pub orders: Vec<Value>,
    pub run_start_ns: String,
    pub run_end_ns: String,
}

/// Everything that happened in a window, for the timeline view, cursor-paged over its events.
///
/// Events are paged in tape `seq` order (a stable unique key, so equal timestamps never split
/// or repeat across pages); follow `next_cursor` until it is `null` to get every event counted
/// by `events_total`. A page holds at most `max_events` events and fits the caller's response
/// cap (anonymous: 1 MiB): it ends early with `truncated_by = "bytes"` instead of failing.
/// `fills`, `equity` and `orders` are sent on the first page (no `cursor`) only and are empty
/// on `cursor` pages; when they alone exceed the cap the request is refused with 400
/// `invalid_request` (`details.reason = "window_too_large"`): request a shorter window.
#[utoipa::path(get, path = "/runs/{run_id}/timeline", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), TimelineQuery),
    responses((status = 200, body = RunTimeline), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)))]
pub async fn timeline(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<TimelineQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let full = run.full_shared().await?;
    let page = timeline_page(&full, &run.view.short_id, &q, byte_cap(&state, &caller))?;
    immutable_value(&headers, &serde_json::to_value(&page)?)
}

/// Smallest number of events on a timeline page (except count-only first pages).
const MIN_TIMELINE_PAGE: u32 = 100;

/// Position of a timeline cursor page: the next event's `seq` and the window's event count
/// (carried so cursor pages do not recount the window).
#[derive(Serialize, Deserialize)]
struct TimelineCursor {
    row: u64,
    total: u64,
}

/// One page of [`timeline`] over a loaded run, bounded to `cap` response bytes.
pub fn timeline_page(
    run: &RunResult,
    run_id: &str,
    q: &TimelineQuery,
    cap: usize,
) -> Result<RunTimeline, ApiError> {
    let t0 = ns_param("t0_ns", q.t0_ns.as_deref())?;
    let t1 = ns_param("t1_ns", q.t1_ns.as_deref())?;
    let filters = format!("timeline|{run_id}|{t0:?}|{t1:?}|{:?}", q.platform);
    let cursor = q
        .cursor
        .as_deref()
        .map(|c| decode_cursor::<TimelineCursor>(c, &filters))
        .transpose()?;
    let limit = match (q.max_events, &cursor) {
        (Some(0), Some(_)) => {
            return Err(ApiError::invalid_field(
                "max_events",
                "max_events must be at least 1 on cursor pages",
            ));
        }
        (Some(0), None) => 0,
        (Some(n), _) => n.clamp(MIN_TIMELINE_PAGE, 50_000) as usize,
        (None, _) => 5_000,
    };
    let from_seq = cursor.as_ref().map(|c| c.row);
    let w = crate::server::timeline_window(run, t0, t1)?;
    let events_total = match &cursor {
        Some(c) => c.total,
        None => crate::server::timeline_events(run, w, 0).count() as u64,
    };

    let (mut fills, mut equity, mut orders, mut context_bytes) = (vec![], vec![], vec![], 0);
    if cursor.is_none() {
        let context = crate::server::timeline_context(run, w);
        for (out, items) in [
            (&mut fills, wire_items(&context.fills)?),
            (&mut equity, wire_items(&context.equity)?),
            (&mut orders, wire_items(&context.orders)?),
        ] {
            *out = items.0;
            context_bytes += items.1;
        }
    }
    let Some(budget) = cap.checked_sub(ENVELOPE_RESERVE + context_bytes) else {
        return Err(ApiError::invalid_request(format!(
            "the fills, equity and orders of this window take {context_bytes} bytes, above the \
             {cap}-byte response limit; request a shorter window (t0_ns..t1_ns)"
        ))
        .with_details(
            json!({ "field": "t1_ns", "reason": "window_too_large", "limit_bytes": cap }),
        ));
    };

    let mut events = Vec::new();
    let mut used = 0usize;
    let mut stop = None;
    // Count-only pages (`max_events = 0`) read no events and end without a cursor.
    let events_from = crate::server::timeline_events(run, w, from_seq.unwrap_or(0));
    for event in events_from.take_while(|_| limit > 0) {
        if events.len() == limit {
            stop = Some((event.seq, TruncatedBy::Limit));
            break;
        }
        let (v, size) = wire_item(event)?;
        if used + size > budget {
            // A first page may end before its first event (the context took the room); the
            // cursor page after it has the whole budget, so paging always progresses.
            if events.is_empty() && from_seq.is_some() {
                return Err(ApiError::internal(
                    "a single event exceeds the response cap",
                ));
            }
            stop = Some((event.seq, TruncatedBy::Bytes));
            break;
        }
        used += size;
        events.push(v);
    }
    Ok(RunTimeline {
        run_id: run_id.to_string(),
        dataset_id: run.manifest.dataset.id.clone(),
        t0_ns: w.t0.to_string(),
        t1_ns: w.t1.to_string(),
        events,
        events_total,
        next_cursor: stop.map(|(row, _)| {
            encode_cursor(
                &TimelineCursor {
                    row,
                    total: events_total,
                },
                &filters,
            )
        }),
        truncated_by: stop.map(|(_, why)| why),
        fills,
        equity,
        orders,
        run_start_ns: w.run_start.to_string(),
        run_end_ns: w.run_end.to_string(),
    })
}

/// Wire values of `items` and their serialized size (as counted by [`wire_item`]).
fn wire_items<T: Serialize>(items: &[T]) -> Result<(Vec<Value>, usize), ApiError> {
    let mut bytes = 0;
    let values = items
        .iter()
        .map(|item| {
            let (v, size) = wire_item(item)?;
            bytes += size;
            Ok(v)
        })
        .collect::<Result<_, ApiError>>()?;
    Ok((values, bytes))
}

/// The deterministic Markdown research report of a run.
#[utoipa::path(get, path = "/runs/{run_id}/report", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), PlatformQuery),
    responses((status = 200, content_type = "text/markdown", body = String),
              (status = 404, body = ErrorEnvelope)))]
pub async fn report(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<PlatformQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let markdown = run_markdown(&state, &run).await?;
    let response = markdown_response(markdown, &format!("run-{}-report.md", run.view.short_id))?;
    Ok(conditional(&headers, response))
}

pub async fn run_markdown(state: &Shared, run: &OpenRun) -> Result<String, ApiError> {
    let full = run.full().await?;
    let version = state
        .db
        .dataset_version(&run.view.dataset_id, run.view.dataset_version_no)
        .await?;
    let dataset = manifest_of(&version)?;
    Ok(crate::report::markdown(&full, &dataset)?)
}

pub fn markdown_response(markdown: String, filename: &str) -> Result<Response, ApiError> {
    let etag = format!(
        "\"{}\"",
        &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(markdown.as_bytes()))[..32]
    );
    let mut response = Response::new(Body::from(markdown));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    super::respond::apply_cache(h, Cache::Immutable, Some(&etag));
    Ok(response)
}

/// The verified bundle (`.bbz`, concatenated gzip NDJSON). Requires a key (SEC-12).
#[utoipa::path(get, path = "/runs/{run_id}/export", tag = "runs", security(("bearer" = [])),
    params(("run_id" = String, Path), PlatformQuery),
    responses((status = 200, content_type = "application/gzip", body = Vec<u8>),
              (status = 401, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)))]
pub async fn export(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<PlatformQuery>,
) -> Result<Response, ApiError> {
    caller.require(Scope::Read)?;
    let _permit = heavy(&state).await?;
    let run = run_data(&state, &caller, &run_id, q.platform.as_deref()).await?;
    // Re-verify the cached copy end to end before serving it.
    run.read(|reader, path| reader.verify_file(path).map(|_| ()))
        .await?;
    let bytes = tokio::fs::read(run.cached_path())
        .await
        .map_err(|e| ApiError::internal(format!("export read: {e}")))?;
    let platform: String = run
        .view
        .run
        .platform
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let filename = format!("run-{}-{platform}.bbz", run.view.short_id);
    let mut response = Response::new(Body::from(bytes));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/gzip"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    super::respond::apply_cache(
        h,
        Cache::Immutable,
        Some(&format!("\"{}\"", run.trust.sha256)),
    );
    Ok(response)
}

/// Execution (E1–E10) and risk analytics of a run, with units, unavailability reasons and
/// caveats; verified against the stored digest. Aggregates are complete. Series longer than
/// 100 points are downsampled (evenly spaced by index, first and last kept) and listed in
/// `downsampled` with `total` and `returned`; the points per series halve (50, 25, …, 2) until
/// the body fits the caller's byte cap (anonymous 1 MiB, keyed `BB_MAX_RESPONSE_BYTES`). Page
/// the complete points with `/analytics/series`. 422 `response_too_large` (`details.limit_bytes`,
/// `details.hint`) only when even 2 points per series do not fit.
#[utoipa::path(get, path = "/runs/{run_id}/analytics", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), PlatformQuery),
    responses((status = 200, body = RunAnalytics), (status = 404, body = ErrorEnvelope),
              (status = 422, description = "`response_too_large`", body = ErrorEnvelope),
              (status = 500, body = ErrorEnvelope)))]
pub async fn run_analytics(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<PlatformQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let view = resolve_run(&state, &caller, &run_id, q.platform.as_deref()).await?;
    let report: Value = serde_json::from_str(&run_analytics_json(&state, &view).await?)?;
    let body = analytics_body(&report, &state, &caller)?;
    Ok(conditional(
        &headers,
        json_bytes(StatusCode::OK, body, Cache::Immutable),
    ))
}

/// The served body of a stored analytics report (run or dataset) within the caller's byte cap:
/// aggregates intact, series downsampled to [`analytics::MAX_SERIES_POINTS`] points and halved
/// down to 2 until it fits; 422 `response_too_large` when even that does not fit.
pub fn analytics_body(
    report: &Value,
    state: &Shared,
    caller: &Caller,
) -> Result<Vec<u8>, ApiError> {
    let cap = byte_cap(state, caller);
    let mut points = analytics::MAX_SERIES_POINTS;
    loop {
        let mut value = analytics::downsampled(report, points);
        wire::to_wire(&mut value).map_err(ApiError::internal)?;
        let body = serde_json::to_vec(&value)?;
        if body.len() <= cap {
            return Ok(body);
        }
        if points <= 2 {
            return Err(super::edge::response_too_large(cap, caller.is_anonymous()));
        }
        points = (points / 2).max(2);
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SeriesQuery {
    /// JSON Pointer of the series, as listed in `downsampled[].path`.
    pub path: String,
    /// First point (default 0).
    pub offset: Option<i64>,
    /// Points per page, ≤ 5000 (default 1000); a page also ends at the byte cap.
    pub limit: Option<i64>,
}

/// One page of the complete points of an analytics series.
#[utoipa::path(get, path = "/runs/{run_id}/analytics/series", tag = "runs", security((), ("bearer" = [])),
    params(("run_id" = String, Path), SeriesQuery, PlatformQuery),
    responses((status = 200, body = analytics::SeriesPage), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)))]
pub async fn run_analytics_series(
    State(state): State<Shared>,
    caller: Caller,
    Path(run_id): Path<String>,
    Query(q): Query<SeriesQuery>,
    Query(p): Query<PlatformQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let view = resolve_run(&state, &caller, &run_id, p.platform.as_deref()).await?;
    let report: Value = serde_json::from_str(&run_analytics_json(&state, &view).await?)?;
    let page = series_page(&report, &q, byte_cap(&state, &caller))?;
    immutable(&headers, &page)
}

/// A page of the series at `q.path` of a stored analytics report.
pub fn series_page(
    report: &Value,
    q: &SeriesQuery,
    cap: usize,
) -> Result<analytics::SeriesPage, ApiError> {
    let points = analytics::series(report, &q.path).ok_or_else(|| {
        ApiError::invalid_field("path", "path must name a series listed in `downsampled`")
    })?;
    let offset = usize::try_from(q.offset.unwrap_or(0))
        .map_err(|_| ApiError::invalid_field("offset", "offset must be ≥ 0"))?;
    let limit = super::respond::limit_param(q.limit, 1000, 5000)? as usize;
    let total = points.len();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let (items, by_bytes) = bounded(&points[start..end], cap)?;
    let next = start + items.len();
    Ok(analytics::SeriesPage {
        path: q.path.clone(),
        total,
        offset: start,
        next_offset: (next < total).then_some(next),
        truncated_by: if by_bytes {
            Some(TruncatedBy::Bytes)
        } else if next < total {
            Some(TruncatedBy::Limit)
        } else {
            None
        },
        items,
    })
}

/// The stored analytics report of a verified run (exact JSON, digest checked). Computed and
/// stored first if missing.
async fn run_analytics_json(state: &Shared, view: &RunView) -> Result<String, ApiError> {
    verified_summary(state, view, false).await?;
    verify_bundle(state, view).await?;
    Ok(match state.db.run_analytics(view.run.id).await? {
        Some(row) => {
            let digest = <sha2::Sha256 as sha2::Digest>::digest(row.report_json.as_bytes());
            if digest.as_slice() != row.report_sha256 {
                return Err(quarantine(
                    &state.db,
                    &state.metrics,
                    view,
                    "analytics",
                    &hex::encode(&row.report_sha256),
                )
                .await);
            }
            row.report_json
        }
        None => {
            // Not stored (a completion that crashed before the analytics row, or an older
            // bundle): take it from the verified trailer, or compute it, and store it.
            let _permit = heavy(state).await?;
            let run = open(state, view.clone()).await?;
            let trailer = run.trailer().await?;
            let report: RunAnalytics = match trailer.analytics {
                Some(v) => serde_json::from_value(v)?,
                None => {
                    let full = run.full().await?;
                    let version = state
                        .db
                        .dataset_version_by_id(
                            state
                                .db
                                .config(view.run.experiment_config_id)
                                .await?
                                .dataset_version_id,
                        )
                        .await?;
                    let ctx = state.registry.load(&version).await?;
                    tokio::task::spawn_blocking(move || analytics::run_analytics(&full, &ctx))
                        .await
                        .map_err(|e| ApiError::internal(format!("analytics task: {e}")))??
                }
            };
            let encoded = analytics::encode(&report)?;
            state
                .db
                .put_run_analytics(
                    view.run.id,
                    &report.analytics_version,
                    &encoded.json,
                    &encoded.sha256,
                )
                .await?;
            encoded.json
        }
    })
}

/// Rows to fetch for a page of `limit` (one extra to detect a next page; bb-db caps at 100).
pub fn peek(limit: i64) -> i64 {
    (limit + 1).min(100)
}

/// Whether another page follows. At the 100-row cap the extra row cannot be fetched, so a full
/// page always offers a cursor (the next page may then be empty).
pub fn has_more(len: usize, limit: i64) -> bool {
    len as i64 > limit || (limit >= 100 && len as i64 >= 100)
}
