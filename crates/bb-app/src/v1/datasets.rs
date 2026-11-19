//! Datasets, versions, quality, replay windows, market analytics, presets and demos
//! (API_CONTRACT §8.2).

use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use bb_db::DatasetVersion as DbVersion;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    model::{
        DataSourceRef, Dataset, DatasetVersion, Demo, DemoRun, InstrumentRef, Preset, Quality,
        QualitySummary,
    },
    respond::{
        Cache, Page, Query, byte_cap, conditional, json, json_bytes, json_value, limit_param,
        ns_param,
    },
    runs::{SeriesQuery, analytics_body, series_page},
};
use crate::{
    analytics::{self, DatasetAnalytics},
    api::ReplayQuery,
    datasets::is_valid_dataset_id,
    pipeline,
    registry::{QUALITY_CHECKER_VERSION, manifest_of},
    wire::{self, at_ns},
};

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(list_datasets))
        .routes(routes!(get_dataset))
        .routes(routes!(list_versions))
        .routes(routes!(get_version))
        .routes(routes!(get_quality))
        .routes(routes!(version_replay))
        .routes(routes!(current_replay))
        .routes(routes!(version_analytics))
        .routes(routes!(current_analytics))
        .routes(routes!(version_analytics_series))
        .routes(routes!(list_presets))
        .routes(routes!(list_demos))
}

async fn version_resource(
    state: &Shared,
    v: &DbVersion,
    with_manifest: bool,
) -> Result<DatasetVersion, ApiError> {
    let manifest = manifest_of(v)?;
    let dataset = state
        .db
        .dataset(uuid::Uuid::nil(), &v.dataset_id)
        .await
        .ok();
    let source = match &dataset {
        Some(d) => state.db.data_source(&d.data_source_id).await.ok(),
        None => None,
    };
    let quality = state
        .db
        .quality_report(v.id, None)
        .await
        .ok()
        .map(|q| QualitySummary {
            checker_version: q.checker_version,
            passed: q.passed,
            error_count: q.error_count,
            warning_count: q.warning_count,
        });
    Ok(DatasetVersion {
        dataset_id: v.dataset_id.clone(),
        version: v.version_no,
        events_sha256: hex::encode(&v.events_sha256),
        normalization_version: v.normalization_version,
        contract_version: v.contract_version,
        instrument: InstrumentRef {
            exchange: manifest.source.exchange.clone(),
            symbol: manifest.source.symbol.clone(),
            tick_size: v.tick_size,
            lot_size: v.lot_size,
            contract_size: v.contract_size,
        },
        origin_ns: v.origin_ns.to_string(),
        start_ns: v.start_rel_ns.to_string(),
        end_ns: v.end_rel_ns.to_string(),
        timestamp_unit: v.timestamp_unit.clone(),
        rows: v.row_count,
        events_bytes: v.events_bytes,
        source: source.map(|s| DataSourceRef {
            id: s.id,
            vendor: s.vendor,
            licence: s.licence,
            hostable: s.hostable,
        }),
        quality,
        retired_at_ns: v.retired_at.map(at_ns),
        manifest: if with_manifest { Some(manifest) } else { None },
    })
}

pub(super) async fn dataset_resource(
    state: &Shared,
    d: &bb_db::Dataset,
    with_manifest: bool,
) -> Result<Dataset, ApiError> {
    let current = match state.db.current_version(&d.id).await {
        Ok(v) => Some(version_resource(state, &v, with_manifest).await?),
        Err(bb_db::Error::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    Ok(Dataset {
        id: d.id.clone(),
        title: d.title.clone(),
        kind: d.kind.clone(),
        level: d.level.clone(),
        owner: if d.owner_workspace_id.is_some() {
            "workspace"
        } else {
            "global"
        }
        .into(),
        current_version: current,
        created_at_ns: at_ns(d.created_at),
    })
}

/// A dataset visible to the caller, or 404.
async fn visible_dataset(
    state: &Shared,
    caller: &Caller,
    id: &str,
) -> Result<bb_db::Dataset, ApiError> {
    if !is_valid_dataset_id(id) || id.len() > 64 {
        return Err(ApiError::not_found("dataset"));
    }
    state
        .db
        .dataset(caller.workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("dataset"))
}

async fn visible_version(
    state: &Shared,
    caller: &Caller,
    id: &str,
    version: Option<i32>,
) -> Result<DbVersion, ApiError> {
    visible_dataset(state, caller, id).await?;
    match version {
        Some(v) => state.db.dataset_version(id, v).await,
        None => state.db.current_version(id).await,
    }
    .map_err(|_| ApiError::not_found("dataset version"))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Dataset id to continue after (opaque cursor).
    pub cursor: Option<String>,
    /// ≤ 100, default 50.
    pub limit: Option<i64>,
}

/// Datasets visible to the caller, by id.
#[utoipa::path(get, path = "/datasets", tag = "datasets", security((), ("bearer" = [])), params(ListQuery),
    responses((status = 200, body = Page<Dataset>)))]
pub async fn list_datasets(
    State(state): State<Shared>,
    caller: Caller,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limit = limit_param(q.limit, 50, 100)?;
    let after = q
        .cursor
        .as_deref()
        .map(|c| super::respond::decode_cursor::<String>(c, "datasets"))
        .transpose()?;
    let snapshot = state.snapshot.for_read(&state, &caller).await;
    let rendered: Vec<Dataset>;
    let (items, more): (Vec<&Dataset>, bool) = match &snapshot {
        Some(s) => {
            let rows = s.datasets(after.as_deref(), super::runs::peek(limit) as usize);
            let more = super::runs::has_more(rows.len(), limit);
            (rows.into_iter().take(limit as usize).collect(), more)
        }
        None => {
            let rows = state
                .db
                .datasets(
                    caller.workspace.id,
                    after.as_deref(),
                    super::runs::peek(limit),
                )
                .await?;
            let more = super::runs::has_more(rows.len(), limit);
            let mut items = Vec::new();
            for d in rows.iter().take(limit as usize) {
                items.push(dataset_resource(&state, d, false).await?);
            }
            rendered = items;
            (rendered.iter().collect(), more)
        }
    };
    let next = if more {
        items
            .last()
            .map(|d| super::respond::encode_cursor(&d.id, "datasets"))
    } else {
        None
    };
    let response = json(
        StatusCode::OK,
        &json!({"items": items, "next_cursor": next, "truncated_by": if more { Some("limit") } else { None }}),
        Cache::Revalidate,
    )?;
    Ok(conditional(&headers, response))
}

/// A dataset with its current version (including the full manifest).
#[utoipa::path(get, path = "/datasets/{dataset_id}", tag = "datasets", security((), ("bearer" = [])),
    params(("dataset_id" = String, Path)),
    responses((status = 200, body = Dataset), (status = 404, body = ErrorEnvelope)))]
pub async fn get_dataset(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let d = visible_dataset(&state, &caller, &id).await?;
    let resource = dataset_resource(&state, &d, true).await?;
    Ok(conditional(
        &headers,
        json(StatusCode::OK, &resource, Cache::Revalidate)?,
    ))
}

/// Versions of a dataset, newest first (without manifests).
#[utoipa::path(get, path = "/datasets/{dataset_id}/versions", tag = "datasets", security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ListQuery),
    responses((status = 200, body = Page<DatasetVersion>), (status = 404, body = ErrorEnvelope)))]
pub async fn list_versions(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    visible_dataset(&state, &caller, &id).await?;
    let limit = limit_param(q.limit, 50, 100)?;
    let filters = format!("versions|{id}");
    let before = q
        .cursor
        .as_deref()
        .map(|c| super::respond::decode_cursor::<i32>(c, &filters))
        .transpose()?;
    let rows = state
        .db
        .dataset_versions(&id, before, super::runs::peek(limit))
        .await?;
    let more = super::runs::has_more(rows.len(), limit);
    let mut items = Vec::new();
    for v in rows.iter().take(limit as usize) {
        items.push(version_resource(&state, v, false).await?);
    }
    let next = if more {
        items
            .last()
            .map(|v| super::respond::encode_cursor(&v.version, &filters))
    } else {
        None
    };
    json(
        StatusCode::OK,
        &json!({"items": items, "next_cursor": next, "truncated_by": if more { Some("limit") } else { None }}),
        Cache::Revalidate,
    )
}

fn version_param(v: &str) -> Result<i32, ApiError> {
    v.parse::<i32>()
        .ok()
        .filter(|v| *v >= 1)
        .ok_or_else(|| ApiError::not_found("dataset version"))
}

/// One immutable dataset version with its manifest.
#[utoipa::path(get, path = "/datasets/{dataset_id}/versions/{version}", tag = "datasets", security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ("version" = i32, Path)),
    responses((status = 200, body = DatasetVersion), (status = 404, body = ErrorEnvelope)))]
pub async fn get_version(
    State(state): State<Shared>,
    caller: Caller,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, Some(version_param(&version)?)).await?;
    let resource = version_resource(&state, &v, true).await?;
    Ok(conditional(
        &headers,
        json(StatusCode::OK, &resource, Cache::Immutable)?,
    ))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct QualityQuery {
    /// Default: the latest checker.
    pub checker_version: Option<i32>,
}

/// The data-quality report of a version, verified against its stored digest.
#[utoipa::path(get, path = "/datasets/{dataset_id}/versions/{version}/quality", tag = "datasets",
    security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ("version" = i32, Path), QualityQuery),
    responses((status = 200, body = Quality), (status = 404, body = ErrorEnvelope),
              (status = 500, body = ErrorEnvelope)))]
pub async fn get_quality(
    State(state): State<Shared>,
    caller: Caller,
    Path((id, version)): Path<(String, String)>,
    Query(q): Query<QualityQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, Some(version_param(&version)?)).await?;
    let report = state
        .db
        .quality_report(v.id, q.checker_version.or(Some(QUALITY_CHECKER_VERSION)))
        .await
        .map_err(|_| ApiError::not_found("quality report"))?;
    if crate::canonical::sha256(&report.report).as_slice() != report.report_sha256 {
        return Err(ApiError::integrity_failed(
            &format!("{}@{}", v.dataset_id, v.version_no),
            "quality_report",
            &hex::encode(&report.report_sha256),
        ));
    }
    let body = Quality {
        checker_version: report.checker_version,
        passed: report.passed,
        error_count: report.error_count,
        warning_count: report.warning_count,
        book_valid_fraction: report.book_valid_fraction,
        report: serde_json::from_value(report.report)?,
        report_sha256: hex::encode(&report.report_sha256),
    };
    Ok(conditional(
        &headers,
        json(StatusCode::OK, &body, Cache::Immutable)?,
    ))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReplayParams {
    pub t0_ns: Option<String>,
    pub t1_ns: Option<String>,
    pub step_ns: Option<String>,
    /// ≤ 100.
    pub levels: Option<u32>,
    /// ≤ 50000.
    pub max_trades: Option<u32>,
}

async fn replay(
    state: &Shared,
    v: &DbVersion,
    q: &ReplayParams,
    cap: usize,
) -> Result<Value, ApiError> {
    let query = ReplayQuery {
        t0: ns_param("t0_ns", q.t0_ns.as_deref())?,
        t1: ns_param("t1_ns", q.t1_ns.as_deref())?,
        step_ns: ns_param("step_ns", q.step_ns.as_deref())?,
        levels: q.levels,
        max_trades: q.max_trades,
    };
    let _permit = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.heavy.acquire(),
    )
    .await
    {
        Ok(Ok(p)) => p,
        _ => return Err(ApiError::unavailable("busy", 2)),
    };
    let ctx = state.registry.load(v).await?;
    let window = tokio::task::spawn_blocking(move || -> Result<_, crate::error::AppError> {
        let manifest = ctx.manifest();
        let request = crate::server::replay_request(manifest.start, manifest.end, &query)?;
        Ok(bb_data::book::replay_window(
            ctx.dataset.events.as_slice(),
            &manifest.instrument,
            ctx.dataset.origin_ns,
            &request,
        ))
    })
    .await
    .map_err(|e| ApiError::internal(format!("replay task: {e}")))??;
    let mut value = serde_json::to_value(&window)?;
    wire::to_wire(&mut value).map_err(ApiError::internal)?;
    let size = serde_json::to_vec(&value)?.len();
    if size > cap {
        return Err(ApiError::invalid_request(format!(
            "the replay window is {size} bytes; the limit for this caller is {cap}. Shorten the \
             window, increase step_ns or lower levels/max_trades"
        )));
    }
    Ok(value)
}

/// Replay window (depth frames and trades) of one version.
#[utoipa::path(get, path = "/datasets/{dataset_id}/versions/{version}/replay", tag = "datasets",
    security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ("version" = i32, Path), ReplayParams),
    responses((status = 200, body = bb_core::market::ReplayWindow), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)))]
pub async fn version_replay(
    State(state): State<Shared>,
    caller: Caller,
    Path((id, version)): Path<(String, String)>,
    Query(q): Query<ReplayParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, Some(version_param(&version)?)).await?;
    let value = replay(&state, &v, &q, byte_cap(&state, &caller)).await?;
    Ok(conditional(
        &headers,
        json_value(StatusCode::OK, &value, Cache::Immutable)?,
    ))
}

/// Replay window of the current version (`Content-Location` names the versioned URL).
#[utoipa::path(get, path = "/datasets/{dataset_id}/replay", tag = "datasets", security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ReplayParams),
    responses((status = 200, body = bb_core::market::ReplayWindow), (status = 404, body = ErrorEnvelope)))]
pub async fn current_replay(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<ReplayParams>,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, None).await?;
    let value = replay(&state, &v, &q, byte_cap(&state, &caller)).await?;
    let mut response = json_value(StatusCode::OK, &value, Cache::Revalidate)?;
    if let Ok(loc) = HeaderValue::from_str(&format!(
        "/api/v1/datasets/{}/versions/{}/replay",
        v.dataset_id, v.version_no
    )) {
        response.headers_mut().insert(header::CONTENT_LOCATION, loc);
    }
    Ok(response)
}

/// The stored market analytics report of a version (exact JSON, digest checked), computed and
/// stored first if missing.
async fn market_analytics_report(state: &Shared, v: &DbVersion) -> Result<Value, ApiError> {
    let version_name = bb_analytics::ANALYTICS_VERSION;
    let text = match state.db.dataset_analytics(v.id, version_name).await? {
        Some(row) => {
            if Sha256::digest(row.report_json.as_bytes()).as_slice() != row.report_sha256 {
                return Err(ApiError::integrity_failed(
                    &format!("{}@{}", v.dataset_id, v.version_no),
                    "analytics",
                    &hex::encode(&row.report_sha256),
                ));
            }
            row.report_json
        }
        None => {
            let _permit = match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                state.heavy.acquire(),
            )
            .await
            {
                Ok(Ok(p)) => p,
                _ => return Err(ApiError::unavailable("busy", 2)),
            };
            let ctx = state.registry.load(v).await?;
            let version_no = v.version_no;
            let report: DatasetAnalytics =
                tokio::task::spawn_blocking(move || analytics::dataset_analytics(&ctx, version_no))
                    .await
                    .map_err(|e| ApiError::internal(format!("analytics task: {e}")))??;
            let encoded = analytics::encode(&report)?;
            state
                .db
                .put_dataset_analytics(v.id, version_name, &encoded.json, &encoded.sha256)
                .await?;
            encoded.json
        }
    };
    Ok(serde_json::from_str(&text)?)
}

/// Market microstructure analytics (M1–M10) of one version, computed once and cached.
/// Aggregates are complete; series are downsampled to fit the caller's byte cap exactly as for
/// `/runs/{run_id}/analytics` (≤ 100 points, halved down to 2, listed in `downsampled`; complete
/// points via `/analytics/series`); 422 `response_too_large` only when even 2 points do not fit.
#[utoipa::path(get, path = "/datasets/{dataset_id}/versions/{version}/analytics", tag = "datasets",
    security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ("version" = i32, Path)),
    responses((status = 200, body = DatasetAnalytics), (status = 404, body = ErrorEnvelope),
              (status = 422, description = "`response_too_large`", body = ErrorEnvelope)))]
pub async fn version_analytics(
    State(state): State<Shared>,
    caller: Caller,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, Some(version_param(&version)?)).await?;
    let report = market_analytics_report(&state, &v).await?;
    let body = analytics_body(&report, &state, &caller)?;
    Ok(conditional(
        &headers,
        json_bytes(StatusCode::OK, body, Cache::Immutable),
    ))
}

/// One page of the complete points of a market analytics series of one version.
#[utoipa::path(get, path = "/datasets/{dataset_id}/versions/{version}/analytics/series", tag = "datasets",
    security((), ("bearer" = [])),
    params(("dataset_id" = String, Path), ("version" = i32, Path), SeriesQuery),
    responses((status = 200, body = analytics::SeriesPage), (status = 400, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)))]
pub async fn version_analytics_series(
    State(state): State<Shared>,
    caller: Caller,
    Path((id, version)): Path<(String, String)>,
    Query(q): Query<SeriesQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, Some(version_param(&version)?)).await?;
    let report = market_analytics_report(&state, &v).await?;
    let page = series_page(&report, &q, byte_cap(&state, &caller))?;
    Ok(conditional(
        &headers,
        json(StatusCode::OK, &page, Cache::Immutable)?,
    ))
}

/// Market analytics of the current version, bounded as the versioned route.
#[utoipa::path(get, path = "/datasets/{dataset_id}/analytics", tag = "datasets", security((), ("bearer" = [])),
    params(("dataset_id" = String, Path)),
    responses((status = 200, body = DatasetAnalytics), (status = 404, body = ErrorEnvelope),
              (status = 422, description = "`response_too_large`", body = ErrorEnvelope)))]
pub async fn current_analytics(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let v = visible_version(&state, &caller, &id, None).await?;
    let report = market_analytics_report(&state, &v).await?;
    let body = analytics_body(&report, &state, &caller)?;
    Ok(json_bytes(StatusCode::OK, body, Cache::Revalidate))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CatalogueQuery {
    /// Dataset to size the catalogue for (default: the first visible dataset).
    pub dataset_id: Option<String>,
}

async fn catalogue_manifest(
    state: &Shared,
    caller: &Caller,
    dataset_id: Option<&str>,
) -> Result<Option<(DbVersion, bb_core::dataset::DatasetManifest)>, ApiError> {
    let id = match dataset_id {
        Some(id) => id.to_string(),
        None => match state
            .db
            .datasets(caller.workspace.id, None, 1)
            .await?
            .into_iter()
            .next()
        {
            Some(d) => d.id,
            None => return Ok(None),
        },
    };
    let v = visible_version(state, caller, &id, None).await?;
    let manifest = manifest_of(&v)?;
    Ok(Some((v, manifest)))
}

/// Curated presets sized for a dataset.
#[utoipa::path(get, path = "/presets", tag = "datasets", security((), ("bearer" = [])), params(CatalogueQuery),
    responses((status = 200, body = Vec<Preset>), (status = 404, body = ErrorEnvelope)))]
pub async fn list_presets(
    State(state): State<Shared>,
    caller: Caller,
    Query(q): Query<CatalogueQuery>,
) -> Result<Response, ApiError> {
    let id = q.dataset_id.as_deref();
    if let Some(s) = state.snapshot.for_read(&state, &caller).await {
        if let Some(presets) = s.presets(id) {
            return json(StatusCode::OK, presets, Cache::Revalidate);
        }
        if id.is_some_and(|id| !s.has_dataset(id)) {
            return Err(ApiError::not_found("dataset"));
        }
    }
    json(
        StatusCode::OK,
        &presets_for(&state, &caller, id).await?,
        Cache::Revalidate,
    )
}

/// The presets of `GET /presets` (database path; the demo snapshot keeps them).
pub async fn presets_for(
    state: &Shared,
    caller: &Caller,
    dataset_id: Option<&str>,
) -> Result<Vec<Preset>, ApiError> {
    Ok(match catalogue_manifest(state, caller, dataset_id).await? {
        None => Vec::new(),
        Some((_, manifest)) => crate::presets::presets(&manifest)
            .into_iter()
            .map(|p| Preset {
                id: p.id,
                title: p.title,
                description: p.description,
                demonstrates: p.demonstrates,
                config: p.config,
            })
            .collect(),
    })
}

/// Controlled experiments, each run linked to its simulated result when one is visible.
#[utoipa::path(get, path = "/demos", tag = "datasets", security((), ("bearer" = [])), params(CatalogueQuery),
    responses((status = 200, body = Vec<Demo>), (status = 404, body = ErrorEnvelope)))]
pub async fn list_demos(
    State(state): State<Shared>,
    caller: Caller,
    Query(q): Query<CatalogueQuery>,
) -> Result<Response, ApiError> {
    let id = q.dataset_id.as_deref();
    if let Some(s) = state.snapshot.for_read(&state, &caller).await {
        if let Some(demos) = s.demos(id) {
            return json(StatusCode::OK, demos, Cache::Revalidate);
        }
        if id.is_some_and(|id| !s.has_dataset(id)) {
            return Err(ApiError::not_found("dataset"));
        }
    }
    json(
        StatusCode::OK,
        &demos_for(&state, &caller, id).await?,
        Cache::Revalidate,
    )
}

/// The demos of `GET /demos` (database path; the demo snapshot keeps them).
pub async fn demos_for(
    state: &Shared,
    caller: &Caller,
    dataset_id: Option<&str>,
) -> Result<Vec<Demo>, ApiError> {
    let Some((_, manifest)) = catalogue_manifest(state, caller, dataset_id).await? else {
        return Ok(Vec::new());
    };
    let mut demos = crate::demos::demos(&manifest);
    // The other datasets of `execution_validation`: each one's current manifest, read once from
    // its version row (never its events). An unknown or invisible one leaves that demo out.
    let mut manifests = HashMap::from([(manifest.id.clone(), Some(manifest))]);
    for id in crate::demos::EXECUTION_VALIDATION_DATASETS {
        if !manifests.contains_key(id) {
            let version = visible_version(state, caller, id, None).await.ok();
            let other = version.and_then(|v| manifest_of(&v).ok());
            manifests.insert(id.to_string(), other);
        }
    }
    demos.extend(crate::demos::execution_validation(|id| {
        manifests.get(id).and_then(Option::as_ref)
    }));
    let identities = demo_identities(&demos, &manifests);
    let fingerprints: Vec<Vec<u8>> = identities
        .iter()
        .flatten()
        .filter_map(|fp| hex::decode(fp).ok())
        .collect();
    let visible = state
        .db
        .visible_identities(
            caller.workspace.id,
            &fingerprints,
            &state.cfg.result_platform,
        )
        .await?;
    let mut identities = identities.into_iter();
    Ok(demos
        .into_iter()
        .map(|d| Demo {
            id: d.id,
            title: d.title,
            question: d.question,
            varied: d.varied,
            controlled: d.controlled,
            how_to_read: d.how_to_read,
            runs: d
                .runs
                .into_iter()
                .map(|r| {
                    let fp = identities.next().flatten();
                    let hit = fp
                        .and_then(|fp| visible.iter().find(|v| hex::encode(&v.fingerprint) == fp));
                    DemoRun {
                        label: r.label,
                        run_id: hit.map(|v| v.short_id.clone()),
                        experiment_id: hit.map(|v| v.experiment_id.to_string()),
                        config: r.config,
                    }
                })
                .collect(),
            reproduce: d.reproduce,
        })
        .collect())
}

/// The identity (`config_sha256`) of every demo run in order, on its own dataset's manifest;
/// `None` for a run whose dataset has none here.
fn demo_identities(
    demos: &[crate::api::Demo],
    manifests: &HashMap<String, Option<bb_core::dataset::DatasetManifest>>,
) -> Vec<Option<String>> {
    demos
        .iter()
        .flat_map(|d| d.runs.iter())
        .map(|r| {
            let manifest = manifests.get(&r.config.dataset_id)?.as_ref()?;
            Some(pipeline::run_identity(manifest, &r.config).config_sha256)
        })
        .collect()
}

#[allow(dead_code)]
fn _schema_refs(_: Option<Value>) {}

#[cfg(test)]
mod tests {
    use bb_core::dataset::DatasetManifest;
    use bb_data::synthetic::{SyntheticConfig, generate};

    use super::*;

    /// Run ids of the canonical demos on the default synthetic dataset, as seeded before
    /// `execution_validation` existed (local `demo seed` databases and RESPONSIVE_RESULTS.md).
    const CANONICAL: [&str; 8] = [
        "253bbc23b342f373",
        "d6a57aed972e18d0",
        "bc2d1df47e2b91fe",
        "130d67fc371bd27f",
        "253bbc23b342f373",
        "253bbc23b342f373",
        "6bcdf19136fd3059",
        "586374188aae19b2",
    ];

    fn run_ids(identities: &[Option<String>]) -> Vec<&str> {
        identities
            .iter()
            .map(|fp| &fp.as_deref().expect("resolved")[..16])
            .collect()
    }

    /// The manifest a registered version stores: generated, then saved (hashing the events).
    fn saved(config: &SyntheticConfig) -> DatasetManifest {
        let dir = tempfile::tempdir().unwrap();
        bb_data::store::save(dir.path(), &generate(config).unwrap()).unwrap()
    }

    #[test]
    fn demo_run_ids_resolve_on_each_runs_own_dataset() {
        let manifest = saved(&SyntheticConfig::default());
        let mut manifests = HashMap::from([(manifest.id.clone(), Some(manifest.clone()))]);
        let canonical = crate::demos::demos(&manifest);
        assert_eq!(run_ids(&demo_identities(&canonical, &manifests)), CANONICAL);

        // An unknown stress dataset leaves `execution_validation` out, nothing else.
        manifests.insert(crate::demos::DEPLETED_DATASET.into(), None);
        assert!(
            crate::demos::execution_validation(|id| manifests.get(id).and_then(Option::as_ref))
                .is_none()
        );

        for config in crate::demos::stress_datasets() {
            let m = saved(&config);
            manifests.insert(m.id.clone(), Some(m));
        }
        let mut all = canonical;
        all.extend(crate::demos::execution_validation(|id| {
            manifests.get(id).and_then(Option::as_ref)
        }));
        let identities = demo_identities(&all, &manifests);
        let ids = run_ids(&identities);
        assert_eq!(ids.len(), 15);
        // The canonical demos resolve exactly as before.
        assert_eq!(ids[..8], CANONICAL);
        // The normal-liquidity baseline is the existing 1 ms run; regime runs hash their own
        // dataset's events.
        let ev = &all[3];
        assert_eq!(ids[8], CANONICAL[0]);
        for (run, id) in ev.runs.iter().zip(&ids[8..]) {
            let own = manifests[&run.config.dataset_id].as_ref().unwrap();
            assert_eq!(*id, pipeline::run_identity(own, &run.config).run_id);
            assert_eq!(
                run.config.dataset_id == manifest.id,
                own.events_sha256 == manifest.events_sha256
            );
        }
    }
}
