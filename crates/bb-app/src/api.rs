//! Request and response types that exist only at the HTTP boundary (see `docs/API.md`). Every
//! other payload is a `bb_core` type serialized as is.

use serde::{Deserialize, Serialize};

use bb_core::{
    Side,
    config::RunConfig,
    dataset::InstrumentMeta,
    explain::Explanation,
    flight::FlightEvent,
    result::{EngineInfo, EquitySample, FillRecord, OrderOutcome, OrderRecord},
};

/// `GET /api/health`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// Always `"ok"` when the server answers.
    pub status: String,
    pub blackbox_version: String,
    pub contract_version: u32,
    pub engine: EngineInfo,
    /// Research-only / simulated-results statement.
    pub disclaimer: String,
}

/// Query of `GET /api/datasets/{id}/replay`. Every field is optional.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayQuery {
    /// Default: dataset start.
    pub t0: Option<i64>,
    /// Default: `min(dataset end, t0 + 60 s)`.
    pub t1: Option<i64>,
    /// Default: `(t1 - t0) / 300`, at least 1 ms.
    pub step_ns: Option<i64>,
    /// Default 25, capped at 100.
    pub levels: Option<u32>,
    /// Default 5000, capped at 50000.
    pub max_trades: Option<u32>,
}

/// Query of `GET /api/presets`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetsQuery {
    /// Dataset the presets are sized for. Default: the first dataset by id.
    pub dataset_id: Option<String>,
}

/// A curated, ready-to-run configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub title: String,
    pub description: String,
    /// What the preset is meant to demonstrate, in one sentence.
    pub demonstrates: String,
    pub config: RunConfig,
}

/// Body of `POST /api/runs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRequest {
    pub config: RunConfig,
    #[serde(default)]
    pub label: Option<String>,
}

/// Query of `GET /api/runs/{run_id}/orders`. Every field is optional.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdersQuery {
    pub outcome: Option<OrderOutcome>,
    pub side: Option<Side>,
    /// Default 0.
    pub offset: Option<u64>,
    /// Default 200, capped at 2000.
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrdersPage {
    /// Number of orders after filtering, before paging.
    pub total: u64,
    pub offset: u64,
    pub orders: Vec<OrderRecord>,
}

/// `GET /api/runs/{run_id}/orders/{order_id}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderDetail {
    pub order: OrderRecord,
    /// Every tape event of this order, in `seq` order.
    pub events: Vec<FlightEvent>,
    pub fills: Vec<FillRecord>,
    pub explanation: Explanation,
    pub instrument: InstrumentMeta,
}

/// Query of `GET /api/runs/{run_id}/tape`. Every field is optional.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeQuery {
    /// Default 0.
    pub from_seq: Option<u64>,
    /// Default 500, capped at 5000.
    pub limit: Option<u64>,
    pub order_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapePage {
    /// Tape length (after `order_id` filtering, before paging).
    pub total: u64,
    pub events: Vec<FlightEvent>,
}

/// Body of `POST /api/compare`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareRequest {
    /// Run id of scenario A.
    pub a: String,
    /// Run id of scenario B.
    pub b: String,
}

/// Query of `GET /api/runs/{run_id}/timeline`. Bounds are inclusive.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineQuery {
    pub t0: Option<i64>,
    pub t1: Option<i64>,
    /// Default 5000, capped at 50000.
    pub max_events: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunTimeline {
    pub run_id: String,
    pub dataset_id: String,
    pub t0: i64,
    pub t1: i64,
    pub events: Vec<FlightEvent>,
    pub events_total: u64,
    pub events_truncated: bool,
    pub fills: Vec<FillRecord>,
    pub equity: Vec<EquitySample>,
    pub orders: Vec<OrderRecord>,
    pub run_start: i64,
    pub run_end: i64,
}

/// A reproducible experiment; `varied` is the union of JSON config differences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Demo {
    pub id: String,
    pub title: String,
    pub question: String,
    pub varied: Vec<String>,
    pub controlled: Vec<String>,
    pub how_to_read: String,
    pub runs: Vec<DemoRun>,
    pub reproduce: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DemoRun {
    pub label: String,
    pub config: RunConfig,
}

/// Body of every non-2xx response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// `not_found`, `invalid_request`, `invalid_config`, `simulation_failed` or `internal`.
    pub code: String,
    pub message: String,
}
