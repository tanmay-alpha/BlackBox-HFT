//! Process composition for `serve`, `worker`, `migrate` and the database-backed CLI commands
//! (TARGET_ARCHITECTURE §12–§13, DEPLOYMENT.md §15): pool construction, start-up migration,
//! bootstrap admin key, the HTTP server with the in-process worker, and the SIGTERM/SIGINT
//! drain.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use axum::Router;
use bb_db::{BootstrapOutcome, Database};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio_util::sync::CancellationToken;

use crate::{
    artifacts::{BundleCache, Storage},
    config::{AppConfig, Profile},
    metrics::Metrics,
    registry::DatasetRegistry,
    v1::{self, AppState},
    worker::{self, WorkerCtx, WorkerHub},
};

/// Lifetime of a bootstrap admin key created at start-up (SECURITY_REVIEW SEC-26).
pub const BOOTSTRAP_KEY_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Initialises logging: JSON lines in the hosted profile, compact text locally. `RUST_LOG`
/// filters (default `info`).
pub fn init_tracing(profile: Profile) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true);
    let _ = match profile {
        Profile::Hosted => builder.json().with_current_span(false).try_init(),
        Profile::Local => builder.compact().try_init(),
    };
}

/// Opens the pooled connection pool (no minimum connections; every acquisition is counted).
pub async fn connect(cfg: &AppConfig, metrics: Arc<Metrics>) -> Result<Database> {
    let (pooled, _) = cfg.database_urls()?;
    let options: PgConnectOptions = pooled
        .parse()
        .context("DATABASE_URL is not a valid Postgres URL")?;
    let pool = PgPoolOptions::new()
        .min_connections(0)
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(15))
        .idle_timeout(Duration::from_secs(240))
        .max_lifetime(Duration::from_secs(1800))
        .before_acquire(move |_conn, _meta| {
            let metrics = metrics.clone();
            Box::pin(async move {
                Metrics::inc(&metrics.db_acquires_total);
                Ok(true)
            })
        })
        .connect_with(options)
        .await
        .context("connecting to DATABASE_URL")?;
    Ok(Database::new(pool))
}

/// Runs the embedded migrations over the direct (unpooled) URL; sqlx takes an advisory lock.
pub async fn migrate(cfg: &AppConfig) -> Result<()> {
    let (_, direct) = cfg.database_urls()?;
    bb_db::migrate(direct)
        .await
        .map_err(|e| anyhow!("migration failed: {e}"))
}

/// Everything a server or worker process shares.
pub struct Runtime {
    pub cfg: Arc<AppConfig>,
    pub db: Database,
    pub storage: Storage,
    pub cache: Arc<BundleCache>,
    pub registry: Arc<DatasetRegistry>,
    pub hub: Arc<WorkerHub>,
    pub metrics: Arc<Metrics>,
}

impl Runtime {
    pub async fn build(cfg: AppConfig, mode: &'static str, slots: usize) -> Result<Self> {
        let cfg = Arc::new(cfg);
        std::fs::create_dir_all(&cfg.data_dir)
            .with_context(|| format!("creating {}", cfg.data_dir.display()))?;
        let metrics = Arc::new(Metrics::default());
        let db = connect(&cfg, metrics.clone()).await?;
        Self::with_db(cfg, db, metrics, mode, slots, None)
    }

    /// Assembles a runtime around an existing pool (tests inject their store).
    pub fn with_db(
        cfg: Arc<AppConfig>,
        db: Database,
        metrics: Arc<Metrics>,
        mode: &'static str,
        slots: usize,
        storage: Option<Storage>,
    ) -> Result<Self> {
        let storage = match storage {
            Some(s) => s,
            None => Storage::from_config(
                &cfg.storage,
                db.clone(),
                cfg.blob_monthly_reads_cap,
                cfg.blob_monthly_read_bytes_cap,
            )
            .map_err(|e| anyhow!("artefact storage: {e}"))?,
        };
        let cache = Arc::new(
            BundleCache::new(&cfg.data_dir, cfg.artifact_disk_cache_bytes)
                .context("creating the bundle cache")?,
        );
        Ok(Self {
            registry: Arc::new(DatasetRegistry::new(&cfg.data_dir)),
            hub: Arc::new(WorkerHub::new(slots, mode, cfg.heartbeat_interval)),
            cfg,
            db,
            storage,
            cache,
            metrics,
        })
    }

    pub fn worker_ctx(&self) -> Arc<WorkerCtx> {
        Arc::new(WorkerCtx {
            cfg: self.cfg.clone(),
            db: self.db.clone(),
            storage: self.storage.clone(),
            cache: self.cache.clone(),
            registry: self.registry.clone(),
            hub: self.hub.clone(),
            metrics: self.metrics.clone(),
            worker_id: worker::worker_id(&self.cfg),
        })
    }

    pub fn app_state(&self) -> Arc<AppState> {
        Arc::new(AppState::new(
            self.cfg.clone(),
            self.db.clone(),
            self.storage.clone(),
            self.cache.clone(),
            self.registry.clone(),
            self.hub.clone(),
            self.metrics.clone(),
        ))
    }
}

/// Ensures the bootstrap admin key exists (idempotent, never resurrects a revoked one).
pub async fn bootstrap_admin_key(cfg: &AppConfig, db: &Database) -> Result<()> {
    let Some(digest) = cfg.admin_bootstrap_key_sha256 else {
        return Ok(());
    };
    let system = db
        .workspace("system")
        .await
        .map_err(|e| anyhow!("system workspace: {e}"))?;
    match db
        .ensure_bootstrap_admin_key(system.id, &digest, BOOTSTRAP_KEY_TTL)
        .await
        .map_err(|e| anyhow!("bootstrap admin key: {e}"))?
    {
        BootstrapOutcome::ActiveAdminExists => {
            tracing::info!("bootstrap admin key: an active admin key exists; nothing created")
        }
        BootstrapOutcome::Created(key) => {
            tracing::info!(key_id = %key.id, "bootstrap admin key created (expires in 7 days)")
        }
        BootstrapOutcome::Inactive(key) => {
            tracing::warn!(key_id = %key.id, "bootstrap admin key digest is revoked or expired; not re-activated")
        }
    }
    Ok(())
}

/// Resolves when SIGTERM or SIGINT (Ctrl-C) arrives.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Options of `serve`.
pub struct ServeOptions {
    pub addr: SocketAddr,
    /// Local profile only: the legacy `/api` router and the built web UI (deprecated; removed
    /// when the UI switches to `/api/v1`).
    pub legacy: Option<Router>,
}

/// The HTTP application (v1 + health + optional legacy fallback).
pub fn app(state: Arc<AppState>, legacy: Option<Router>) -> Router {
    let router = v1::router(state);
    match legacy {
        Some(legacy) => router.fallback_service(legacy),
        None => router,
    }
}

/// Prepares the database for serving: optional migration, reachability, bootstrap key.
pub async fn prepare(runtime: &Runtime, state: &AppState) -> Result<()> {
    if runtime.cfg.migrate_on_start {
        migrate(&runtime.cfg).await?;
        tracing::info!("migrations applied");
    }
    runtime
        .db
        .workspace("demo")
        .await
        .map_err(|e| anyhow!("database not ready (run `blackbox migrate`?): {e}"))?;
    state.db_ready.store(true, Ordering::SeqCst);
    bootstrap_admin_key(&runtime.cfg, &runtime.db).await?;
    Ok(())
}

/// `blackbox serve`: API plus in-process worker until SIGTERM/SIGINT, then drain and exit.
pub async fn serve(
    runtime: Runtime,
    options: ServeOptions,
    started: CancellationToken,
) -> Result<()> {
    let state = runtime.app_state();
    prepare(&runtime, &state).await?;
    let app = app(state.clone(), options.legacy);
    let listener = tokio::net::TcpListener::bind(options.addr)
        .await
        .with_context(|| format!("binding {}", options.addr))?;
    let local = listener.local_addr()?;
    tracing::info!(addr = %local, "listening");
    println!(
        "BlackBox HFT API listening on http://{local}/ (status: http://{local}/api/v1/status)"
    );
    started.cancel();
    run_until_shutdown(runtime, state, listener, app, shutdown_signal()).await
}

/// Serves `app` on `listener` with the in-process worker until `stop` resolves, then drains:
/// stop claiming, let the running job stop at a checkpoint and release its lease, finish
/// in-flight requests, all within `BB_SHUTDOWN_GRACE_MS`.
pub async fn run_until_shutdown(
    runtime: Runtime,
    state: Arc<AppState>,
    listener: tokio::net::TcpListener,
    app: Router,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let hub = runtime.hub.clone();
    let grace = runtime.cfg.shutdown_grace;
    let worker = if runtime.hub.slots > 0 {
        Some(tokio::spawn(worker::run(runtime.worker_ctx(), false)))
    } else {
        None
    };
    let drained = CancellationToken::new();
    let drained_signal = drained.clone();
    let drain_hub = hub.clone();
    tokio::spawn(async move {
        stop.await;
        tracing::info!("shutdown signal: draining");
        drain_hub.start_drain();
        drained_signal.cancel();
    });
    let shutdown = drained.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
    });
    drained.cancelled().await;
    let deadline = tokio::time::Instant::now() + grace;
    if let Some(worker) = worker
        && tokio::time::timeout_at(deadline, worker).await.is_err()
    {
        tracing::warn!("worker did not stop within the grace period; its lease will expire");
    }
    match tokio::time::timeout_at(deadline, server).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => tracing::warn!(error = %e, "server error during shutdown"),
        Ok(Err(e)) => tracing::warn!(error = %e, "server task failed"),
        Err(_) => tracing::warn!("in-flight requests did not finish within the grace period"),
    }
    let _ = state;
    runtime.db.pool().close().await;
    tracing::info!("stopped");
    Ok(())
}

/// `blackbox worker`: the worker loop only (standalone: polls with backoff while idle).
pub async fn worker_only(runtime: Runtime) -> Result<()> {
    if runtime.cfg.migrate_on_start {
        migrate(&runtime.cfg).await?;
    }
    let ctx = runtime.worker_ctx();
    let hub = runtime.hub.clone();
    let grace = runtime.cfg.shutdown_grace;
    let handle = tokio::spawn(worker::run(ctx, true));
    shutdown_signal().await;
    hub.start_drain();
    let _ = tokio::time::timeout(grace, handle).await;
    runtime.db.pool().close().await;
    Ok(())
}

/// The data directory's legacy dataset root (`<data-dir>/datasets`).
pub fn legacy_datasets(cfg: &AppConfig) -> PathBuf {
    cfg.data_dir.join("datasets")
}
