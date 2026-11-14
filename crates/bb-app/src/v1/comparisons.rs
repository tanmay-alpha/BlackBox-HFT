//! Comparisons (API_CONTRACT §8.6): the bb-forensics comparison of two verified runs, stored per
//! workspace; the eligibility gate (AT-1/AT-2) decides whether per-order pairs exist.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bb_core::compare::{Comparison as CoreComparison, PairClass};
use bb_db::{Comparison as Row, Cursor, NewComparison, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    experiments::{Params, WorkspaceParam},
    model::{Comparison, CreateComparison, LabelPatch},
    respond::{
        ApiJson, Cache, Page, TruncatedBy, bounded, byte_cap, check_label, conditional,
        decode_cursor, encode_cursor, if_match, json, json_value, uuid_param,
    },
    runs::{markdown_response, resolve_run, verified_summary},
};
use crate::{artifacts::open_run, wire::at_ns};

/// Version of the comparator whose output is stored (bump when bb-forensics changes it).
pub const COMPARATOR_VERSION: i32 = 2;

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(create_comparison, list_comparisons))
        .routes(routes!(get_comparison, patch_comparison, delete_comparison))
        .routes(routes!(comparison_pairs))
        .routes(routes!(comparison_report))
}

fn stored(row: &Row) -> Result<CoreComparison, ApiError> {
    Ok(serde_json::from_value(row.summary.clone())?)
}

fn resource(row: &Row, with_alignment: bool) -> Result<Comparison, ApiError> {
    let mut comparison = stored(row)?;
    if let Some(alignment) = comparison.alignment.as_mut() {
        alignment.pairs.clear();
    }
    if !with_alignment {
        comparison.alignment = None;
    }
    Ok(Comparison {
        id: row.id.to_string(),
        label: row.label.clone(),
        version: row.version,
        comparator_version: row.comparator_version,
        status: row.eligibility.clone(),
        created_at_ns: at_ns(row.created_at),
        comparison,
    })
}

/// Compare two visible runs (`201` new, `200` existing).
#[utoipa::path(post, path = "/comparisons", tag = "comparisons", security(("bearer" = [])),
    params(WorkspaceParam), request_body = CreateComparison,
    responses((status = 201, body = Comparison), (status = 200, body = Comparison),
              (status = 404, body = ErrorEnvelope), (status = 500, body = ErrorEnvelope)))]
pub async fn create_comparison(
    State(state): State<Shared>,
    caller: Caller,
    Query(ws): Query<WorkspaceParam>,
    ApiJson(body): ApiJson<CreateComparison>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(ws.workspace.as_deref())?.clone();
    let label = check_label("label", body.label.as_deref().unwrap_or(""), true)?;
    let viewer = Caller {
        workspace: workspace.clone(),
        ..caller.clone()
    };
    let platform = body.platform.as_deref();
    let a = resolve_run(&state, &viewer, &body.run_a, platform).await?;
    let b = resolve_run(&state, &viewer, &body.run_b, platform).await?;
    if a.run.id == b.run.id {
        return Err(ApiError::invalid_request(
            "run_a and run_b are the same run",
        ));
    }
    let label_a = verified_summary(&state, &a, false)
        .await?
        .label
        .unwrap_or_default();
    let label_b = verified_summary(&state, &b, false)
        .await?
        .label
        .unwrap_or_default();
    let _permit = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.heavy.acquire(),
    )
    .await
    {
        Ok(Ok(p)) => p,
        _ => return Err(ApiError::unavailable("busy", 2)),
    };
    let limits = state.bundle_limits();
    let open_a = open_run(
        &state.db,
        &state.storage,
        &state.cache,
        &state.metrics,
        limits,
        a.clone(),
    )
    .await?;
    let open_b = open_run(
        &state.db,
        &state.storage,
        &state.cache,
        &state.metrics,
        limits,
        b.clone(),
    )
    .await?;
    let run_a = open_a.full().await?;
    let run_b = open_b.full().await?;
    let instrument = |v: &bb_db::RunView| {
        let v = v.clone();
        let db = state.db.clone();
        async move {
            db.dataset_version(&v.dataset_id, v.dataset_version_no)
                .await
                .ok()
                .and_then(|ver| crate::registry::manifest_of(&ver).ok())
                .map(|m| m.instrument)
        }
    };
    let inst_a = instrument(&a).await;
    let inst_b = instrument(&b).await;
    let comparison = tokio::task::spawn_blocking(move || {
        bb_forensics::compare_runs(
            bb_forensics::CompareInput {
                run: &run_a,
                label: &label_a,
                instrument: inst_a.as_ref(),
            },
            bb_forensics::CompareInput {
                run: &run_b,
                label: &label_b,
                instrument: inst_b.as_ref(),
            },
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("comparison task: {e}")))?;
    let summary = serde_json::to_value(&comparison)?;
    let reasons = if comparison.eligibility.aligned {
        json!([])
    } else {
        serde_json::to_value(&comparison.eligibility.reasons)?
    };
    let existing = state
        .db
        .comparisons(workspace.id, None, 100)
        .await?
        .into_iter()
        .any(|c| {
            c.run_a_id == a.run.id
                && c.run_b_id == b.run.id
                && c.comparator_version == COMPARATOR_VERSION
        });
    let row = state
        .db
        .create_comparison(&NewComparison {
            workspace_id: workspace.id,
            run_a_id: a.run.id,
            run_b_id: b.run.id,
            comparator_version: COMPARATOR_VERSION,
            eligibility: if comparison.eligibility.aligned {
                "aligned"
            } else {
                "metrics_only"
            }
            .into(),
            reasons,
            summary_sha256: crate::canonical::sha256(&summary).to_vec(),
            summary,
            label,
            created_by_key_id: caller.key_id(),
        })
        .await
        .map_err(|e| match e {
            bb_db::Error::NotFound => ApiError::not_found("run"),
            other => other.into(),
        })?;
    state.hub.bump();
    json(
        if existing {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        &resource(&row, true)?,
        Cache::NoStore,
    )
}

/// Comparisons of a workspace, newest first (without alignment).
#[utoipa::path(get, path = "/comparisons", tag = "comparisons", security((), ("bearer" = [])),
    params(WorkspaceParam, ("cursor" = Option<String>, Query), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = Page<Comparison>)))]
pub async fn list_comparisons(
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
        .comparisons(workspace.id, cursor.as_ref(), limit + 1)
        .await?;
    let more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let items: Vec<Comparison> = rows
        .iter()
        .map(|r| resource(r, false))
        .collect::<Result<_, _>>()?;
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
        Cache::Revalidate,
    )
}

async fn visible(state: &Shared, caller: &Caller, id: &str) -> Result<Row, ApiError> {
    let id = uuid_param("comparison", id)?;
    state
        .db
        .comparison(caller.workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("comparison"))
}

/// One comparison (alignment counters without pairs).
#[utoipa::path(get, path = "/comparisons/{id}", tag = "comparisons", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = Comparison), (status = 404, body = ErrorEnvelope)))]
pub async fn get_comparison(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    Ok(conditional(
        &headers,
        json(StatusCode::OK, &resource(&row, true)?, Cache::Revalidate)?,
    ))
}

#[utoipa::path(patch, path = "/comparisons/{id}", tag = "comparisons", security(("bearer" = [])),
    params(("id" = String, Path), ("If-Match" = String, Header)), request_body = LabelPatch,
    responses((status = 200, body = Comparison), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)))]
pub async fn patch_comparison(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LabelPatch>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("comparison", &id)?;
    let version = if_match(&headers, true)?.unwrap_or_default();
    let label = check_label("label", &body.label, true)?;
    let row = state
        .db
        .rename_comparison(workspace.id, id, version, &label)
        .await
        .map_err(|e| match e {
            bb_db::Error::Conflict => ApiError::conflict(
                "version_mismatch",
                "the comparison changed or does not exist",
            ),
            other => other.into(),
        })?;
    json(StatusCode::OK, &resource(&row, true)?, Cache::NoStore)
}

#[utoipa::path(delete, path = "/comparisons/{id}", tag = "comparisons", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 204), (status = 404, body = ErrorEnvelope)))]
pub async fn delete_comparison(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = caller.write_workspace(None)?.clone();
    let id = uuid_param("comparison", &id)?;
    state
        .db
        .delete_comparison(workspace.id, id)
        .await
        .map_err(|_| ApiError::not_found("comparison"))?;
    state.hub.bump();
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PairsQuery {
    /// Only pairs of this class (e.g. `same_outcome_different_timing`).
    pub class: Option<String>,
    pub cursor: Option<String>,
    /// ≤ 1000, default 200.
    pub limit: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct PairCursor {
    index: u64,
}

/// Per-order pairs of an aligned comparison; 409 `alignment_unavailable` when metrics-only.
#[utoipa::path(get, path = "/comparisons/{id}/pairs", tag = "comparisons", security((), ("bearer" = [])),
    params(("id" = String, Path), PairsQuery),
    responses((status = 200, body = Page<bb_core::compare::OrderPair>), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope)))]
pub async fn comparison_pairs(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<PairsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    let comparison = stored(&row)?;
    let Some(alignment) = comparison.alignment else {
        return Err(ApiError::conflict(
            "alignment_unavailable",
            "this comparison is metrics-only; the runs are not order-aligned",
        ));
    };
    let class: Option<PairClass> = q
        .class
        .as_deref()
        .map(|c| {
            serde_json::from_value(Value::String(c.into()))
                .map_err(|_| ApiError::invalid_field("class", format!("unknown class `{c}`")))
        })
        .transpose()?;
    let limit = q.limit.unwrap_or(200);
    if !(1..=1000).contains(&limit) {
        return Err(ApiError::invalid_field(
            "limit",
            "limit must be between 1 and 1000",
        ));
    }
    let filters = format!("pairs|{:?}", q.class);
    let start = q
        .cursor
        .as_deref()
        .map(|c| decode_cursor::<PairCursor>(c, &filters))
        .transpose()?
        .map_or(0, |c| c.index);
    let matching: Vec<(usize, &bb_core::compare::OrderPair)> = alignment
        .pairs
        .iter()
        .enumerate()
        .filter(|(i, p)| *i as u64 >= start && class.is_none_or(|c| p.class == c))
        .take(limit as usize + 1)
        .collect();
    let more = matching.len() as u64 > limit;
    let page: Vec<&bb_core::compare::OrderPair> = matching
        .iter()
        .take(limit as usize)
        .map(|(_, p)| *p)
        .collect();
    let (items, cut) = bounded(&page, byte_cap(&state, &caller))?;
    let next = if cut || more {
        matching
            .get(items.len())
            .map(|(i, _)| encode_cursor(&PairCursor { index: *i as u64 }, &filters))
    } else {
        None
    };
    let body = json!({"items": items, "next_cursor": next,
        "truncated_by": if cut { Some(TruncatedBy::Bytes) } else if more { Some(TruncatedBy::Limit) } else { None }});
    Ok(conditional(
        &headers,
        json_value(StatusCode::OK, &body, Cache::Revalidate)?,
    ))
}

/// Markdown report of a comparison.
pub fn comparison_markdown(row: &Row) -> Result<String, ApiError> {
    let c = stored(row)?;
    let mut md = String::new();
    md.push_str(&format!("# Comparison {} vs {}\n\n", c.run_a, c.run_b));
    md.push_str(&format!(
        "Eligibility: **{}** (like-for-like: {}).\n\n",
        row.eligibility,
        if c.eligibility.like_for_like {
            "yes"
        } else {
            "no"
        }
    ));
    for r in &c.eligibility.reasons {
        md.push_str(&format!(
            "- `{}`: A = {}, B = {}. {}\n",
            r.field, r.a, r.b, r.explanation
        ));
    }
    md.push_str("\n| Metric | A | B | B - A |\n|---|---:|---:|---:|\n");
    let f = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |v| format!("{v:.6}"));
    for m in &c.metrics {
        md.push_str(&format!(
            "| {} ({}) | {} | {} | {} |\n",
            m.label,
            m.unit,
            f(m.a),
            f(m.b),
            f(m.delta)
        ));
    }
    md.push_str("\n## Interpretation\n\n");
    for n in &c.interpretation {
        md.push_str(&format!("- [{:?}] {}\n", n.basis, n.statement));
    }
    md.push_str("\nSimulated results only; nothing here is an exchange execution.\n");
    Ok(md)
}

#[utoipa::path(get, path = "/comparisons/{id}/report", tag = "comparisons", security((), ("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, content_type = "text/markdown", body = String), (status = 404, body = ErrorEnvelope)))]
pub async fn comparison_report(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let row = visible(&state, &caller, &id).await?;
    let _ = Scope::Read;
    markdown_response(
        comparison_markdown(&row)?,
        &format!("comparison-{}.md", row.id),
    )
}
