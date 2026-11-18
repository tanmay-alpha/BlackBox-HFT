//! HTTP API `/api/v1` (docs/engineering/API_CONTRACT.md), health endpoints and the OpenAPI
//! document. Routers are built with `utoipa_axum::OpenApiRouter`, so the served
//! `/api/v1/openapi.json` (and `blackbox openapi`) is generated from the handlers themselves.

pub mod auth;
pub mod comparisons;
pub mod datasets;
pub mod edge;
pub mod error;
pub mod experiments;
pub mod health;
pub mod jobs;
pub mod market;
pub mod model;
pub mod reports;
pub mod respond;
pub mod runs;
pub mod snapshot;
pub mod sweeps;
pub mod system;

use std::{
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Instant,
};

use axum::{Router, middleware};
use bb_db::Database;
use tokio::sync::Semaphore;
use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use utoipa_axum::router::OpenApiRouter;

use crate::{
    artifacts::{BundleCache, Storage, bundle_limits},
    config::AppConfig,
    metrics::Metrics,
    registry::DatasetRegistry,
    worker::WorkerHub,
};

/// Everything the handlers share.
pub struct AppState {
    pub cfg: Arc<AppConfig>,
    pub db: Database,
    pub storage: Storage,
    pub cache: Arc<BundleCache>,
    pub registry: Arc<DatasetRegistry>,
    pub hub: Arc<WorkerHub>,
    pub metrics: Arc<Metrics>,
    /// Set once the start-up migration/DB check succeeded; `/readyz` answers from it (SEC-10).
    pub db_ready: AtomicBool,
    pub limiters: auth::Limiters,
    pub workspaces: auth::WorkspaceCache,
    pub anon_cache: edge::AnonCache,
    /// Anonymous demo lists and catalogues from memory (FINAL_AUDIT §4 item 6).
    pub snapshot: snapshot::DemoSnapshot,
    pub egress: edge::Egress,
    /// Concurrency limit of artefact-heavy routes (API_CONTRACT §5). Shared so a permit can move
    /// into blocking work and stay held until that work actually ends.
    pub heavy: Arc<Semaphore>,
    /// Requests in flight across the server (SEC-9).
    pub inflight: Arc<Semaphore>,
    pub started: Instant,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: Arc<AppConfig>,
        db: Database,
        storage: Storage,
        cache: Arc<BundleCache>,
        registry: Arc<DatasetRegistry>,
        hub: Arc<WorkerHub>,
        metrics: Arc<Metrics>,
    ) -> Self {
        respond::set_cursor_key(cfg.api_key_pepper.expose().as_bytes());
        Self {
            limiters: auth::Limiters::new(&cfg),
            workspaces: auth::WorkspaceCache::default(),
            anon_cache: edge::AnonCache::new(cfg.response_cache_bytes),
            snapshot: snapshot::DemoSnapshot::default(),
            egress: edge::Egress::default(),
            heavy: Arc::new(Semaphore::new(cfg.heavy_read_concurrency)),
            inflight: Arc::new(Semaphore::new(cfg.max_in_flight)),
            db_ready: AtomicBool::new(false),
            started: Instant::now(),
            cfg,
            db,
            storage,
            cache,
            registry,
            hub,
            metrics,
        }
    }

    pub fn bundle_limits(&self) -> bb_artifacts::BundleLimits {
        bundle_limits(self.cfg.limits.max_output_bytes)
    }

    /// Changes whenever data a reader may see changed in this process: any state change
    /// (`WorkerHub::generation`) or a run quarantined on read. Versions the anonymous response
    /// cache and the demo snapshot.
    pub fn data_generation(&self) -> u64 {
        self.hub
            .generation
            .load(std::sync::atomic::Ordering::SeqCst)
            .wrapping_add(crate::metrics::Metrics::get(
                &self.metrics.runs_quarantined_total,
            ))
    }
}

pub type Shared = Arc<AppState>;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("bbk_<43 base64url characters>")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "BlackBox HFT API",
        version = "1",
        description = "Research-only simulated execution forensics. Every nanosecond quantity \
                       (`*_ns`, `ts`, `*_ts`) and every seed is a decimal string. Errors use the \
                       envelope `ErrorEnvelope`. See docs/engineering/API_CONTRACT.md.",
        license(name = "MIT")
    ),
    modifiers(&BearerAuth),
    components(schemas(error::ErrorEnvelope))
)]
struct ApiDoc;

/// The `/api/v1` routes plus health routes, with their OpenAPI document.
pub fn api_router() -> OpenApiRouter<Shared> {
    let v1 = OpenApiRouter::new()
        .merge(health::v1_routes())
        .merge(datasets::routes())
        .merge(market::routes())
        .merge(experiments::routes())
        .merge(jobs::routes())
        .merge(runs::routes())
        .merge(comparisons::routes())
        .merge(sweeps::routes())
        .merge(reports::routes())
        .merge(system::routes());
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(health::root_routes())
        .nest("/api/v1", v1)
}

/// The OpenAPI document as pretty JSON (`blackbox openapi`, `/api/v1/openapi.json`).
pub static OPENAPI_JSON: LazyLock<String> = LazyLock::new(|| {
    let (_, api) = api_router().split_for_parts();
    api.to_pretty_json()
        .expect("the OpenAPI document always serializes")
});

/// The complete application: `/api/v1`, health, middleware (outermost first: `edge::guard`,
/// `auth::authenticate`, `edge::egress`, `edge::anon_cache`).
pub fn router(state: Shared) -> Router {
    let (routes, _) = api_router().split_for_parts();
    routes
        .layer(middleware::from_fn_with_state(
            state.clone(),
            edge::anon_cache,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), edge::egress))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .fallback(edge::no_such_endpoint)
        .layer(middleware::from_fn_with_state(state.clone(), edge::guard))
        .with_state(state)
}
