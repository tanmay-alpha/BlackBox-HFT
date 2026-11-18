//! Public synthetic L3 workbench. Private artifacts are deliberately never read by this router.
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bb_analytics::l3::{self, Comparison, L3Report, Metrics};
use bb_data::{
    l3synth::{self, L3SynthConfig},
    l3workbench::{CuratedExample, MAX_DETAIL_LEVELS, MAX_DETAIL_ORDERS, Workbench},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    Shared,
    auth::Caller,
    error::{ApiError, ErrorEnvelope},
    respond::{self, Cache, Page, TruncatedBy},
};

const DEMO_ID: &str = "l3-demo";
const PAGE_BYTES: usize = 256 * 1024;
const DEMO_IDLE: Duration = Duration::from_secs(30);
static DEMO: Mutex<Option<(Arc<Demo>, Instant)>> = Mutex::const_new(None);

struct Demo {
    workbench: Workbench,
    report: L3Report,
    summary: Summary,
    examples: Vec<CuratedExample>,
}

#[derive(Clone, Serialize, ToSchema)]
pub struct MarketQuality {
    pub passed: bool,
    pub quarantined_total: u64,
    pub checks: Vec<Value>,
}
#[derive(Clone, Serialize, ToSchema)]
pub struct Summary {
    pub dataset_id: String,
    pub dataset_version: String,
    pub data_class: String,
    pub symbol: String,
    pub venue: String,
    pub session: String,
    pub timestamp_basis: String,
    pub events: u64,
    pub first_ns: Option<String>,
    pub last_ns: Option<String>,
    pub messages_by_type: BTreeMap<String, u64>,
    pub quality: MarketQuality,
    pub generator: Value,
    pub license_note: String,
}
#[derive(Serialize, ToSchema)]
pub struct Catalogue {
    pub items: Vec<Summary>,
}

// Schema-only mirrors: bb-data has no OpenAPI dependency. Runtime serializes the original records.
#[derive(ToSchema)]
pub struct MarketEvent {
    pub event_index: u64,
    pub message_type: String,
    pub ts_ns: Option<String>,
    pub order_ref: Option<String>,
    pub new_order_ref: Option<String>,
    pub side: Option<String>,
    pub price4: Option<u32>,
    pub shares: Option<u64>,
    pub best_bid_price4: Option<u32>,
    pub best_ask_price4: Option<u32>,
    pub applied: String,
}
#[derive(ToSchema)]
pub struct MarketOrder {
    pub order_ref: String,
    pub shares: u64,
    pub queue_position: u32,
    pub shares_ahead: u64,
    pub entered_ns: Option<String>,
    pub entered_event_index: u64,
}
#[derive(ToSchema)]
pub struct MarketLevel {
    pub price4: u32,
    pub shares: u64,
    pub order_count: u32,
    pub orders: Vec<MarketOrder>,
    pub orders_truncated: bool,
}
#[derive(ToSchema)]
pub struct MarketBook {
    pub event_index: u64,
    pub ts_ns: Option<String>,
    pub levels: usize,
    pub bids: Vec<MarketLevel>,
    pub asks: Vec<MarketLevel>,
}
/// One resting order in FIFO order; its queue position is its array index + 1.
#[derive(ToSchema)]
pub struct MarketQueueSlot {
    pub order_ref: String,
    pub shares: u64,
}
#[derive(ToSchema)]
pub struct MarketDepthLevel {
    pub price4: u32,
    /// Total of all resting orders at the price (the L2 aggregate).
    pub shares: u64,
    pub order_count: u32,
    pub orders: Vec<MarketQueueSlot>,
    pub orders_truncated: bool,
}
#[derive(ToSchema)]
pub struct MarketDepth {
    pub bids: Vec<MarketDepthLevel>,
    pub asks: Vec<MarketDepthLevel>,
}
/// A price level the event changed, complete before and after it.
#[derive(ToSchema)]
pub struct MarketAffectedLevel {
    pub side: String,
    pub price4: u32,
    /// `order`, or for a replace `replaced` (same price), `replaced_from`, `replaced_to`.
    pub role: String,
    pub before: MarketLevel,
    pub after: MarketLevel,
}
#[derive(ToSchema)]
pub struct MarketOrderPosition {
    pub order_ref: String,
    pub side: String,
    pub price4: u32,
    pub shares: u64,
    /// One-based FIFO position at its price.
    pub queue_position: u32,
    pub level_orders: u32,
    /// Shares of the orders ahead of it at its price, excluding itself.
    pub shares_ahead: u64,
    pub level_shares: u64,
    pub entered_ns: Option<String>,
    pub entered_event_index: u64,
    /// Event timestamp minus the accepted add/replace timestamp, decimal nanoseconds.
    pub age_ns: Option<String>,
}
#[derive(ToSchema)]
pub struct MarketLevelChange {
    pub side: String,
    pub price4: u32,
    pub before_shares: u64,
    pub after_shares: u64,
    pub before_orders: u32,
    pub after_orders: u32,
}
#[derive(ToSchema)]
pub struct MarketSameTimestampEvent {
    pub event_index: u64,
    pub message_type: String,
    pub applied: String,
    pub level_changes: Vec<MarketLevelChange>,
}
/// Events up to and including the selected one that share its source timestamp.
#[derive(ToSchema)]
pub struct MarketSameTimestamp {
    pub ts_ns: Option<String>,
    pub position: u32,
    pub first_event_index: u64,
    pub events: Vec<MarketSameTimestampEvent>,
    pub events_truncated: bool,
}
/// The book immediately before and after one event, from source frames up to that event only.
#[derive(ToSchema)]
pub struct MarketEventDetail {
    pub event: MarketEvent,
    pub before_event_index: Option<u64>,
    pub levels: usize,
    pub orders_per_level: usize,
    pub before: MarketDepth,
    pub after: MarketDepth,
    pub affected: Vec<MarketAffectedLevel>,
    pub order_before: Option<MarketOrderPosition>,
    pub order_after: Option<MarketOrderPosition>,
    /// Completed lifetime when this event removed or replaced the referenced order.
    pub lifetime_ns: Option<String>,
    pub same_timestamp: MarketSameTimestamp,
}
#[derive(Serialize, ToSchema)]
pub struct MarketExample {
    pub id: String,
    pub title: String,
    pub event_index: u64,
    /// The deterministic selection rule the event satisfies.
    pub rule: String,
}
#[derive(Serialize, ToSchema)]
pub struct MarketExamples {
    pub items: Vec<MarketExample>,
}

pub fn routes() -> OpenApiRouter<Shared> {
    OpenApiRouter::new()
        .routes(routes!(catalogue))
        .routes(routes!(summary))
        .routes(routes!(events))
        .routes(routes!(event_detail))
        .routes(routes!(examples))
        .routes(routes!(book))
        .routes(routes!(comparison))
        .routes(routes!(metrics))
}

fn valid_id(id: &str) -> Result<(), ApiError> {
    if id == DEMO_ID {
        Ok(())
    } else {
        Err(ApiError::not_found("market dataset"))
    }
}

/// Retain the lazily built workbench while it is being browsed, not throughout later jobs.
pub(super) fn expire_idle() {
    if let Ok(mut cached) = DEMO.try_lock()
        && cached
            .as_ref()
            .is_some_and(|(_, used)| used.elapsed() >= DEMO_IDLE)
    {
        *cached = None;
    }
}

async fn demo(state: &Shared) -> Result<Arc<Demo>, ApiError> {
    let mut cached = DEMO.lock().await;
    if let Some((demo, used)) = cached.as_mut() {
        *used = Instant::now();
        return Ok(demo.clone());
    }
    let demo = async {
        let permit = tokio::time::timeout(std::time::Duration::from_secs(2), state.heavy.clone().acquire_owned())
            .await.map_err(|_| ApiError::unavailable("busy", 1))?.map_err(ApiError::internal)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let config = L3SynthConfig::default();
            let bytes = l3synth::generate(&config).map_err(ApiError::internal)?;
            let mut workbench = Workbench::from_bytes("synthetic-l3-demo.itch", &bytes, &config.symbol, 1024)
                .map_err(ApiError::internal)?;
            let report = l3::compute(&workbench).map_err(ApiError::internal)?;
            let examples = workbench.curated_examples().map_err(ApiError::internal)?;
            // Only the analytics computation consumes these arrays. Book and event detail replay
            // source frames/checkpoints; retaining both arrays duplicates already computed data.
            workbench.observations = Vec::new();
            workbench.l2 = Vec::new();
            let manifest = &workbench.manifest;
            let checks = manifest.checks.iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>()?;
            let summary = Summary {
                dataset_id: DEMO_ID.into(), dataset_version: manifest.source.sha256.clone(),
                data_class: "synthetic".into(), symbol: config.symbol.clone(),
                venue: "SIMULATED (NASDAQ ITCH 5.0 format)".into(),
                session: "synthetic session; not an exchange trading day".into(),
                timestamp_basis: "nanoseconds since synthetic session midnight; source frame order".into(),
                events: manifest.frames, first_ns: manifest.timestamps.first_ns.map(|x| x.to_string()),
                last_ns: manifest.timestamps.last_ns.map(|x| x.to_string()),
                messages_by_type: manifest.messages_by_type.clone(),
                quality: MarketQuality { passed: manifest.checks.iter().all(|c| c.passed), quarantined_total: manifest.book.quarantined_total, checks },
                generator: serde_json::to_value(config)?,
                license_note: "Generated by BlackBox; redistributable synthetic bytes. Not observed NASDAQ data. Restricted private artifacts are never served.".into(),
            };
            Ok::<_, ApiError>(Arc::new(Demo { workbench, report, summary, examples }))
        }).await.map_err(ApiError::internal)?
    }.await?;
    *cached = Some((demo.clone(), Instant::now()));
    Ok(demo)
}

fn response<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<Response, ApiError> {
    Ok(respond::conditional(
        headers,
        respond::json(StatusCode::OK, value, Cache::Immutable)?,
    ))
}

#[utoipa::path(get, operation_id="market_catalogue", path="/market/datasets", tag="market", security((), ("bearer"=[])), responses((status=200,body=Catalogue),(status=503,body=ErrorEnvelope)))]
pub async fn catalogue(
    State(state): State<Shared>,
    _caller: Caller,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    response(
        &headers,
        &Catalogue {
            items: vec![demo(&state).await?.summary.clone()],
        },
    )
}
#[utoipa::path(get, operation_id="market_summary", path="/market/datasets/{dataset_id}", tag="market", security((), ("bearer"=[])), params(("dataset_id"=String,Path)), responses((status=200,body=Summary),(status=404,body=ErrorEnvelope)))]
pub async fn summary(
    State(state): State<Shared>,
    _caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    response(&headers, &demo(&state).await?.summary)
}

#[derive(Default, Deserialize, IntoParams)]
#[into_params(parameter_in=Query)]
pub struct EventsQuery {
    pub cursor: Option<String>,
    /// Default 100; 1..500.
    pub limit: Option<i64>,
    /// One ITCH message-type letter.
    pub message_type: Option<String>,
    /// Inclusive nanoseconds since session midnight.
    pub t0_ns: Option<String>,
    pub t1_ns: Option<String>,
    /// Inclusive source frame index; 0..99999.
    pub from_event_index: Option<u64>,
}
impl EventsQuery {
    fn bounds(&self) -> Result<(usize, Option<u64>, Option<u64>, u64), ApiError> {
        let limit = respond::limit_param(self.limit, 100, 500)? as usize;
        if self
            .message_type
            .as_ref()
            .is_some_and(|s| s.len() != 1 || !s.as_bytes()[0].is_ascii_uppercase())
        {
            return Err(ApiError::invalid_field(
                "message_type",
                "expected one uppercase ITCH type letter",
            ));
        }
        let parse = |key, v: Option<&str>| -> Result<Option<u64>, ApiError> {
            let n = respond::ns_param(key, v)?;
            if n.is_some_and(|n| !(0..86_400_000_000_000i64).contains(&n)) {
                return Err(ApiError::invalid_field(
                    key,
                    "outside synthetic session day",
                ));
            }
            Ok(n.map(|n| n as u64))
        };
        let lo = parse("t0_ns", self.t0_ns.as_deref())?;
        let hi = parse("t1_ns", self.t1_ns.as_deref())?;
        if lo.zip(hi).is_some_and(|(a, b)| b < a) {
            return Err(ApiError::invalid_field("t1_ns", "must be >= t0_ns"));
        }
        let from = self.from_event_index.unwrap_or(0);
        if from > 99_999 {
            return Err(ApiError::invalid_field(
                "from_event_index",
                "must be <=99999",
            ));
        }
        Ok((limit, lo, hi, from))
    }
}
#[utoipa::path(get,operation_id="market_events",path="/market/datasets/{dataset_id}/events",tag="market",security((),("bearer"=[])),params(("dataset_id"=String,Path),EventsQuery),responses((status=200,body=Page<MarketEvent>),(status=400,body=ErrorEnvelope),(status=404,body=ErrorEnvelope)))]
pub async fn events(
    State(state): State<Shared>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    let (limit, lo, hi, from) = q.bounds()?;
    let demo = demo(&state).await?;
    let filters = format!(
        "market-events|{}|{:?}|{lo:?}|{hi:?}|{from}",
        demo.summary.dataset_version, q.message_type
    );
    let start = q
        .cursor
        .as_deref()
        .map(|c| respond::decode_cursor::<u64>(c, &filters))
        .transpose()?
        .unwrap_or(from);
    let matches: Vec<_> = demo
        .workbench
        .events
        .iter()
        .filter(|e| {
            let ts = e.ts_ns.as_deref().and_then(|s| s.parse::<u64>().ok());
            e.event_index >= start
                && e.event_index >= from
                && q.message_type.as_ref().is_none_or(|t| t == &e.message_type)
                && lo.is_none_or(|v| ts.is_some_and(|ts| ts >= v))
                && hi.is_none_or(|v| ts.is_some_and(|ts| ts <= v))
        })
        .take(limit + 1)
        .collect();
    let take = matches.len().min(limit);
    let (items, by_bytes) = respond::bounded(
        &matches[..take],
        respond::byte_cap(&state, &caller).min(PAGE_BYTES),
    )?;
    let more = by_bytes || matches.len() > take;
    let next_cursor = if more {
        items
            .last()
            .and_then(|v| v["event_index"].as_u64())
            .map(|i| respond::encode_cursor(&(i + 1), &filters))
    } else {
        None
    };
    response(
        &headers,
        &Page {
            items,
            next_cursor,
            truncated_by: if more {
                Some(if by_bytes {
                    TruncatedBy::Bytes
                } else {
                    TruncatedBy::Limit
                })
            } else {
                None
            },
        },
    )
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in=Query)]
pub struct EventDetailQuery {
    /// Depth levels per side, default 5; 1..10.
    pub levels: Option<i64>,
    /// Listed orders per level, default 50; 1..100. Selected orders are always listed.
    pub orders_per_level: Option<i64>,
}
#[utoipa::path(get,operation_id="market_event_detail",path="/market/datasets/{dataset_id}/events/{event_index}",tag="market",security((),("bearer"=[])),params(("dataset_id"=String,Path),("event_index"=u64,Path),EventDetailQuery),responses((status=200,body=MarketEventDetail),(status=400,body=ErrorEnvelope),(status=404,body=ErrorEnvelope)))]
pub async fn event_detail(
    State(state): State<Shared>,
    _caller: Caller,
    Path((id, index)): Path<(String, String)>,
    Query(q): Query<EventDetailQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    let index: u64 = index
        .parse()
        .map_err(|_| ApiError::invalid_field("event_index", "expected a decimal event index"))?;
    let levels = bounded_param(q.levels, 5, MAX_DETAIL_LEVELS as i64, "levels")? as usize;
    let orders = bounded_param(
        q.orders_per_level,
        50,
        MAX_DETAIL_ORDERS as i64,
        "orders_per_level",
    )? as usize;
    let demo = demo(&state).await?;
    if index >= demo.summary.events {
        return Err(ApiError::not_found("market event"));
    }
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.heavy.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ApiError::unavailable("busy", 1))?
    .map_err(ApiError::internal)?;
    let demo = Arc::clone(&demo);
    let detail = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        demo.workbench.event_detail(index, levels, orders)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::not_found("market event"))?;
    response(&headers, &detail)
}

#[utoipa::path(get,operation_id="market_examples",path="/market/datasets/{dataset_id}/examples",tag="market",security((),("bearer"=[])),params(("dataset_id"=String,Path)),responses((status=200,body=MarketExamples),(status=404,body=ErrorEnvelope)))]
pub async fn examples(
    State(state): State<Shared>,
    _caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    let demo = demo(&state).await?;
    response(
        &headers,
        &MarketExamples {
            items: demo
                .examples
                .iter()
                .map(|e| MarketExample {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    event_index: e.event_index,
                    rule: e.rule.clone(),
                })
                .collect(),
        },
    )
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in=Query)]
pub struct BookQuery {
    pub event_index: u64,
    pub levels: Option<i64>,
    pub orders_per_level: Option<i64>,
}
#[utoipa::path(get,operation_id="market_book",path="/market/datasets/{dataset_id}/book",tag="market",security((),("bearer"=[])),params(("dataset_id"=String,Path),BookQuery),responses((status=200,body=MarketBook),(status=400,body=ErrorEnvelope),(status=404,body=ErrorEnvelope)))]
pub async fn book(
    State(state): State<Shared>,
    _caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<BookQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    let levels = bounded_param(q.levels, 5, 20, "levels")? as usize;
    let orders = bounded_param(q.orders_per_level, 20, 100, "orders_per_level")? as usize;
    let demo = demo(&state).await?;
    if q.event_index >= demo.summary.events {
        return Err(ApiError::not_found("market event"));
    }
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.heavy.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ApiError::unavailable("busy", 1))?
    .map_err(ApiError::internal)?;
    let demo = Arc::clone(&demo);
    let book = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        demo.workbench.book(q.event_index, levels, orders)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    response(&headers, &book)
}
#[utoipa::path(get,operation_id="market_comparison",path="/market/datasets/{dataset_id}/comparison",tag="market",security((),("bearer"=[])),params(("dataset_id"=String,Path)),responses((status=200,body=Comparison),(status=404,body=ErrorEnvelope)))]
pub async fn comparison(
    State(state): State<Shared>,
    _caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    response(&headers, &demo(&state).await?.report.comparison)
}
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in=Query)]
pub struct MetricsQuery {
    /// Default100; 2..500.
    pub max_points: Option<i64>,
}
#[utoipa::path(get,operation_id="market_metrics",path="/market/datasets/{dataset_id}/metrics",tag="market",security((),("bearer"=[])),params(("dataset_id"=String,Path),MetricsQuery),responses((status=200,body=Metrics),(status=400,body=ErrorEnvelope),(status=404,body=ErrorEnvelope)))]
pub async fn metrics(
    State(state): State<Shared>,
    _caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<MetricsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    valid_id(&id)?;
    let max_points = bounded_param(q.max_points, 100, 500, "max_points")? as usize;
    if max_points < 2 {
        return Err(ApiError::invalid_field("max_points", "must be >=2"));
    }
    let demo = demo(&state).await?;
    let metrics = &demo.report.metrics;
    let series = l3::downsampled(&metrics.series, max_points);
    response(
        &headers,
        &MetricsView {
            basis: &metrics.basis,
            definitions: &metrics.definitions,
            counts: &metrics.counts,
            rates_per_second: &metrics.rates_per_second,
            order_lifetime_ns: &metrics.order_lifetime_ns,
            queue_position_at_execution: &metrics.queue_position_at_execution,
            series,
            l2: &metrics.l2,
        },
    )
}

/// A bounded integer query parameter (1..=max, default) whose error names the parameter itself.
fn bounded_param(
    value: Option<i64>,
    default: i64,
    max: i64,
    field: &'static str,
) -> Result<i64, ApiError> {
    let value = value.unwrap_or(default);
    if !(1..=max).contains(&value) {
        return Err(ApiError::invalid_field(
            field,
            format!("{field} must be between 1 and {max}"),
        ));
    }
    Ok(value)
}

#[derive(Serialize)]
struct MetricsView<'a> {
    basis: &'a str,
    definitions: &'a BTreeMap<String, String>,
    counts: &'a BTreeMap<String, u64>,
    rates_per_second: &'a BTreeMap<String, Option<f64>>,
    order_lifetime_ns: &'a l3::LifetimeDistribution,
    queue_position_at_execution: &'a l3::QueuePositionDistribution,
    series: Vec<&'a l3::MetricPoint>,
    l2: &'a bb_analytics::market::MarketReport,
}

/// Offline artifact producer. File input is never read or published by the public router.
#[derive(clap::Subcommand)]
pub enum MarketCommand {
    /// Write generated, redistributable ITCH bytes and a precomputed workbench artifact.
    L3Synth {
        #[arg(long, default_value_t = 7)]
        seed: u64,
        #[arg(long, default_value_t = 30000)]
        events: u32,
        #[arg(long, default_value = "SYNTH")]
        symbol: String,
        #[arg(long)]
        inject_unknown_execute: bool,
        #[arg(long)]
        inject_duplicate_add: bool,
        /// A NEW directory; never overwrites existing artifacts.
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// Inspect a private ITCH file locally. Outputs remain private and are never served.
    Workbench {
        #[arg(long)]
        input: std::path::PathBuf,
        #[arg(long)]
        symbol: String,
        #[arg(long, default_value_t = 100000)]
        max_messages: u64,
        #[arg(long)]
        out: std::path::PathBuf,
    },
}

pub fn run_cli(command: MarketCommand) -> anyhow::Result<()> {
    use std::io::Write;
    let (workbench, out, source, class, generator) = match command {
        MarketCommand::L3Synth {
            seed,
            events,
            symbol,
            inject_unknown_execute,
            inject_duplicate_add,
            out,
        } => {
            let config = L3SynthConfig {
                seed,
                events,
                symbol,
                inject_unknown_execute,
                inject_duplicate_add,
            };
            let bytes = l3synth::generate(&config).map_err(|e| anyhow::anyhow!("{e}"))?;
            let workbench = Workbench::from_bytes("synthetic.itch", &bytes, &config.symbol, 1024)
                .map_err(anyhow::Error::msg)?;
            (
                workbench,
                out,
                Some(bytes),
                "synthetic",
                Some(serde_json::to_value(config)?),
            )
        }
        MarketCommand::Workbench {
            input,
            symbol,
            max_messages,
            out,
        } => {
            anyhow::ensure!(
                max_messages > 0 && max_messages <= 100000,
                "--max-messages must be 1..100000"
            );
            let workbench = Workbench::from_file(&input, &symbol, max_messages, 1024)
                .map_err(anyhow::Error::msg)?;
            (workbench, out, None, "restricted_local_only", None)
        }
    };
    let report = l3::compute(&workbench).map_err(anyhow::Error::msg)?;
    workbench.save(&out).map_err(anyhow::Error::msg)?;
    let write_new = |name: &str, bytes: &[u8]| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(out.join(name))?;
        file.write_all(bytes)?;
        Ok(())
    };
    if let Some(bytes) = source {
        write_new("synthetic.itch", &bytes)?;
    }
    write_new("analytics.json", &serde_json::to_vec_pretty(&report)?)?;
    write_new(
        "provenance.json",
        &serde_json::to_vec_pretty(&serde_json::json!({
            "data_class": class, "generator": generator,
            "public_api_loadable": false,
            "license_note": if class == "synthetic" { "Generated, redistributable; not observed exchange data." } else { "Restricted input and event-derived outputs: local only; never commit, upload, or host." }
        }))?,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "out": out, "data_class": class, "frames": workbench.manifest.frames,
            "source_sha256": workbench.manifest.source.sha256,
            "quarantined_total": workbench.manifest.book.quarantined_total,
            "comparison": report.comparison, "counts": report.metrics.counts
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discarded_analytics_inputs_do_not_change_workbench_reads() {
        let config = L3SynthConfig::default();
        let bytes = l3synth::generate(&config).unwrap();
        let mut workbench =
            Workbench::from_bytes("synthetic-l3-demo.itch", &bytes, &config.symbol, 1024).unwrap();
        let indices = [0, 1023, 1024, 20000, workbench.manifest.frames - 1];
        let books: Vec<_> = indices
            .iter()
            .map(|&index| workbench.book(index, 20, 100).unwrap())
            .collect();
        let details: Vec<_> = indices
            .iter()
            .map(|&index| workbench.event_detail(index, 10, 100).unwrap())
            .collect();
        let examples = workbench.curated_examples().unwrap();
        assert!(!workbench.observations.is_empty() && !workbench.l2.is_empty());
        workbench.observations = Vec::new();
        workbench.l2 = Vec::new();
        for (n, &index) in indices.iter().enumerate() {
            assert_eq!(workbench.book(index, 20, 100).unwrap(), books[n]);
            assert_eq!(workbench.event_detail(index, 10, 100).unwrap(), details[n]);
        }
        assert_eq!(workbench.curated_examples().unwrap(), examples);
    }
}
