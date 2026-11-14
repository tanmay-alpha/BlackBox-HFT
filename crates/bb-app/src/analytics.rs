//! Research analytics of runs and datasets (bb-analytics), as stored and served by `/api/v1`.
//!
//! Run analytics (execution E1–E10 + sampled risk) are computed by the worker when a run
//! completes, streamed so per-order details are not retained, embedded in the bundle trailer and
//! stored in `bb.run_analytics` as exact JSON bytes with their sha256. Dataset analytics (market
//! M1–M10) are computed once per dataset version and analytics version, on first request or at
//! `demo seed`, and stored in `bb.dataset_analytics`.

use bb_analytics::{
    ANALYTICS_VERSION,
    execution::{ExecutionConfig, ExecutionReport},
    market::{MarketConfig, MarketReport},
    risk::{RiskConfig, RiskReport},
};
use bb_core::result::RunResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::{datasets::DatasetCtx, error::AppError};

/// `GET /api/v1/runs/{run_id}/analytics`.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunAnalytics {
    pub analytics_version: String,
    /// Exchange-clock execution quality (E1–E10). Per-order details are not retained
    /// (`order_details_streamed = true`).
    pub execution: ExecutionReport,
    /// Sampled risk statistics of the equity path, with caveats.
    pub risk: RiskReport,
    /// Present when execution analytics could not be computed (the reason; risk still is).
    pub execution_unavailable: Option<String>,
    /// Series of the stored report that this response shortened (see [`downsample`]); the
    /// complete points are paged by `GET /runs/{run_id}/analytics/series`. Never stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downsampled: Vec<DownsampledSeries>,
}

/// `GET /api/v1/datasets/{id}/analytics`.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct DatasetAnalytics {
    pub analytics_version: String,
    pub dataset_id: String,
    pub version: i32,
    pub market: MarketReport,
    /// As [`RunAnalytics::downsampled`]; complete points via
    /// `GET /datasets/{id}/versions/{version}/analytics/series`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downsampled: Vec<DownsampledSeries>,
}

/// Most points of one series in an analytics response. Aggregate scalars are never touched; a
/// longer series is replaced by at most this many points evenly spaced by index (the first and
/// last kept) and listed in `downsampled`. Responses halve it until the body fits the caller's
/// byte cap (`v1::runs::analytics_body`).
pub const MAX_SERIES_POINTS: usize = 100;

/// Array fields that hold series: Kaplan–Meier curves, market series points and the interval
/// views. Everything else in a report is a fixed-size aggregate.
const SERIES_FIELDS: [&str; 4] = ["curve", "points", "ofi_intervals", "trade_intervals"];

/// One series shortened in an analytics response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DownsampledSeries {
    /// JSON Pointer of the series in the report, e.g. `/execution/first_fill/km/curve`; pass it
    /// as `path` to the series endpoint.
    pub path: String,
    /// Points of the complete series.
    pub total: usize,
    /// Points in this response.
    pub returned: usize,
}

/// The served form of a stored report (as JSON): every series longer than `max_points` (≥ 2)
/// keeps `max_points` evenly spaced points, and `downsampled` lists what was shortened. Only
/// the kept points are copied.
pub fn downsampled(report: &serde_json::Value, max_points: usize) -> serde_json::Value {
    use serde_json::Value;
    fn walk(v: &Value, path: &mut String, max: usize, out: &mut Vec<DownsampledSeries>) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, child)| {
                        let len = path.len();
                        path.push('/');
                        path.push_str(&key.replace('~', "~0").replace('/', "~1"));
                        let copy = match child {
                            Value::Array(items)
                                if SERIES_FIELDS.contains(&key.as_str()) && items.len() > max =>
                            {
                                out.push(DownsampledSeries {
                                    path: path.clone(),
                                    total: items.len(),
                                    returned: max,
                                });
                                Value::Array(
                                    evenly_spaced(items.len(), max)
                                        .map(|i| items[i].clone())
                                        .collect(),
                                )
                            }
                            _ => walk(child, path, max, out),
                        };
                        path.truncate(len);
                        (key.clone(), copy)
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, child)| {
                        let len = path.len();
                        path.push('/');
                        path.push_str(&i.to_string());
                        let copy = walk(child, path, max, out);
                        path.truncate(len);
                        copy
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let mut out = Vec::new();
    let mut copy = walk(report, &mut String::new(), max_points.max(2), &mut out);
    if let (false, Some(map)) = (out.is_empty(), copy.as_object_mut()) {
        map.insert(
            "downsampled".into(),
            serde_json::to_value(out).expect("plain data serializes"),
        );
    }
    copy
}

/// `max` distinct indices of `0..total` (`total > max >= 2`), evenly spaced, first and last
/// included, ascending.
fn evenly_spaced(total: usize, max: usize) -> impl Iterator<Item = usize> {
    (0..max).map(move |i| i * (total - 1) / (max - 1))
}

/// The complete series at `path` (a JSON Pointer naming a series field) of a stored report.
pub fn series<'a>(report: &'a serde_json::Value, path: &str) -> Option<&'a [serde_json::Value]> {
    let field = path.rsplit('/').next()?;
    if !SERIES_FIELDS.contains(&field) {
        return None;
    }
    report.pointer(path)?.as_array().map(Vec::as_slice)
}

/// `GET …/analytics/series`: one page of the complete points of a downsampled series.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SeriesPage {
    pub path: String,
    /// Points of the complete series.
    pub total: usize,
    pub offset: usize,
    /// The points, as in the stored report.
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<serde_json::Value>,
    pub next_offset: Option<usize>,
    pub truncated_by: Option<crate::v1::respond::TruncatedBy>,
}

/// Exact JSON bytes and their digest.
pub struct Encoded {
    pub json: String,
    pub sha256: Vec<u8>,
}

pub fn encode<T: Serialize>(value: &T) -> Result<Encoded, AppError> {
    let json = serde_json::to_string(value)?;
    let sha256 = Sha256::digest(json.as_bytes()).to_vec();
    Ok(Encoded { json, sha256 })
}

/// Execution and risk analytics of a completed run over its dataset.
pub fn run_analytics(run: &RunResult, ctx: &DatasetCtx) -> Result<RunAnalytics, AppError> {
    let instrument = &ctx.manifest().instrument;
    let risk = bb_analytics::risk::compute_risk(
        &run.equity,
        &run.fills,
        instrument,
        run.metrics.net_pnl,
        &RiskConfig::default(),
    );
    let (execution, execution_unavailable) =
        match bb_analytics::execution::compute_execution_streaming(
            run,
            &ctx.bbo,
            instrument,
            &ExecutionConfig::default(),
            |_order| {},
        ) {
            Ok(report) => (report, None),
            Err(e) => {
                // The report shape stays stable: compute over an empty run, record the reason.
                let empty = RunResult {
                    orders: Vec::new(),
                    fills: Vec::new(),
                    equity: Vec::new(),
                    tape: Vec::new(),
                    ..run.clone()
                };
                let report = bb_analytics::execution::compute_execution_streaming(
                    &empty,
                    &ctx.bbo,
                    instrument,
                    &ExecutionConfig::default(),
                    |_order| {},
                )
                .map_err(|e2| AppError::Internal(format!("execution analytics: {e}; {e2}")))?;
                (report, Some(e.to_string()))
            }
        };
    Ok(RunAnalytics {
        analytics_version: ANALYTICS_VERSION.to_string(),
        execution,
        risk,
        execution_unavailable,
        downsampled: Vec::new(),
    })
}

/// Market analytics of a dataset version.
pub fn dataset_analytics(ctx: &DatasetCtx, version_no: i32) -> Result<DatasetAnalytics, AppError> {
    let manifest = ctx.manifest();
    let config = MarketConfig {
        series_max_points: Some(500),
        ..MarketConfig::default()
    };
    let market = bb_analytics::market::compute_market(
        ctx.dataset.events.as_slice(),
        &manifest.instrument,
        ctx.dataset.origin_ns,
        &config,
    )
    .map_err(|e| AppError::InvalidRequest(format!("market analytics unavailable: {e}")))?;
    Ok(DatasetAnalytics {
        analytics_version: ANALYTICS_VERSION.to_string(),
        dataset_id: manifest.id.clone(),
        version: version_no,
        market,
        downsampled: Vec::new(),
    })
}
