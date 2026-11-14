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
/// longer series is replaced by this many points evenly spaced by index (the first and last
/// kept) and listed in `downsampled`.
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

/// Bounds a stored report (as JSON) for serving: every series longer than `max_points` keeps
/// `max_points` evenly spaced points, and `downsampled` lists what was shortened.
pub fn downsample(report: &mut serde_json::Value, max_points: usize) {
    fn walk(
        v: &mut serde_json::Value,
        path: &mut String,
        max: usize,
        out: &mut Vec<DownsampledSeries>,
    ) {
        match v {
            serde_json::Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&key.replace('~', "~0").replace('/', "~1"));
                    match child {
                        serde_json::Value::Array(items)
                            if SERIES_FIELDS.contains(&key.as_str()) && items.len() > max =>
                        {
                            let total = items.len();
                            let kept: Vec<serde_json::Value> = evenly_spaced(total, max)
                                .map(|i| std::mem::take(&mut items[i]))
                                .collect();
                            *items = kept;
                            out.push(DownsampledSeries {
                                path: path.clone(),
                                total,
                                returned: items.len(),
                            });
                        }
                        _ => walk(child, path, max, out),
                    }
                    path.truncate(len);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, child) in items.iter_mut().enumerate() {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&i.to_string());
                    walk(child, path, max, out);
                    path.truncate(len);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(report, &mut String::new(), max_points.max(2), &mut out);
    if let (false, Some(map)) = (out.is_empty(), report.as_object_mut()) {
        map.insert(
            "downsampled".into(),
            serde_json::to_value(out).expect("plain data serializes"),
        );
    }
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
