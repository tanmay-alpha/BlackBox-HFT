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
}

/// `GET /api/v1/datasets/{id}/analytics`.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct DatasetAnalytics {
    pub analytics_version: String,
    pub dataset_id: String,
    pub version: i32,
    pub market: MarketReport,
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
    })
}
