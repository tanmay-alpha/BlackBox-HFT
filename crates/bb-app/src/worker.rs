//! The job worker (TARGET_ARCHITECTURE §4.3, §6, §12): claims leased jobs from Postgres, runs
//! the simulation under a [`RunControl`] that reports progress and stops cooperatively, writes
//! and verifies the bundle, uploads it, reads it back and completes the job in one transaction.
//!
//! No idle polling: an idle in-process worker sleeps until a local submission wakes it
//! ([`WorkerHub::notify`]), the earliest scheduled queue event (`next_wake`) or shutdown. A
//! standalone `blackbox worker` cannot be woken by another process, so it additionally polls
//! with exponential backoff (1 s … `idle_poll_max`).

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bb_artifacts::{ArtifactError, BundleWriter};
use bb_core::config::RunConfig;
use bb_db::{Database, Job, Lease, NewArtifact, NewRun, WorkerHeartbeat};
use bb_sim::{Progress, RunControl};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    analytics,
    artifacts::{BundleCache, Storage, bundle_limits},
    config::AppConfig,
    error::AppError,
    metrics::Metrics,
    pipeline,
    registry::DatasetRegistry,
};

/// Grace left when a drain starts after the simulation finished: commit only with this much left.
const COMMIT_GRACE: Duration = Duration::from_secs(10);

/// Live state of one in-flight job, shared between the simulation thread, the heartbeat task
/// and the API (cancellation, progress).
pub struct JobControl {
    pub job_id: Uuid,
    pub attempt: i16,
    cancel: AtomicBool,
    lost: AtomicBool,
    draining: Arc<AtomicBool>,
    deadline: Instant,
    deadline_hit: AtomicBool,
    ticks_done: AtomicU64,
    ticks_total: AtomicU64,
    tape_events: AtomicU64,
    reported: AtomicBool,
}

/// Progress of a job held by this process (`Job.progress`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveProgress {
    pub attempt: i16,
    pub done: u64,
    pub total: u64,
    pub tape_events: u64,
}

impl JobControl {
    fn new(job: &Job, draining: Arc<AtomicBool>) -> Self {
        Self {
            job_id: job.id,
            attempt: job.claim_count,
            cancel: AtomicBool::new(job.state == "cancelling"),
            lost: AtomicBool::new(false),
            draining,
            deadline: Instant::now()
                + Duration::from_millis(u64::try_from(job.deadline_ms).unwrap_or(600_000)),
            deadline_hit: AtomicBool::new(false),
            ticks_done: AtomicU64::new(0),
            ticks_total: AtomicU64::new(u64::try_from(job.ticks_total).unwrap_or(0)),
            tape_events: AtomicU64::new(0),
            reported: AtomicBool::new(false),
        }
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn progress(&self) -> Option<LiveProgress> {
        self.reported.load(Ordering::SeqCst).then(|| LiveProgress {
            attempt: self.attempt,
            done: self.ticks_done.load(Ordering::SeqCst),
            total: self.ticks_total.load(Ordering::SeqCst),
            tape_events: self.tape_events.load(Ordering::SeqCst),
        })
    }

    fn snapshot(&self) -> (i64, i64) {
        let total = self.ticks_total.load(Ordering::SeqCst);
        let done = self.ticks_done.load(Ordering::SeqCst).min(total);
        (
            i64::try_from(done).unwrap_or(i64::MAX),
            i64::try_from(self.tape_events.load(Ordering::SeqCst)).unwrap_or(i64::MAX),
        )
    }
}

impl RunControl for JobControl {
    fn on_progress(&self, p: &Progress) {
        // Monotonic within an attempt (AT-6).
        self.ticks_done.fetch_max(p.ticks_done, Ordering::SeqCst);
        self.tape_events.fetch_max(p.tape_events, Ordering::SeqCst);
        // The stored total is the admission estimate, an upper bound; keep the larger one so
        // `done <= total` always holds.
        self.ticks_total.fetch_max(p.ticks_total, Ordering::SeqCst);
        self.reported.store(true, Ordering::SeqCst);
    }

    fn should_cancel(&self) -> bool {
        if Instant::now() >= self.deadline {
            self.deadline_hit.store(true, Ordering::SeqCst);
            return true;
        }
        self.cancel.load(Ordering::SeqCst)
            || self.lost.load(Ordering::SeqCst)
            || self.draining.load(Ordering::SeqCst)
    }
}

/// What the API's `/status` shows about the worker (memory only).
#[derive(Clone, Debug)]
pub struct WorkerStatus {
    pub state: &'static str,
    pub busy: usize,
    pub last_heartbeat: Option<Instant>,
    pub queue_depth: i64,
    pub running: i64,
    pub queue_as_of: Option<chrono::DateTime<chrono::Utc>>,
}

/// Shared between the API and the in-process worker loops.
pub struct WorkerHub {
    pub notify: Notify,
    pub slots: usize,
    pub mode: &'static str,
    pub heartbeat_interval: Duration,
    jobs: Mutex<HashMap<Uuid, Arc<JobControl>>>,
    status: Mutex<WorkerStatus>,
    draining: Arc<AtomicBool>,
    drain_started: Mutex<Option<Instant>>,
    shutdown: CancellationToken,
    /// Bumped on every state change the API can observe (anonymous response cache).
    pub generation: AtomicU64,
}

impl WorkerHub {
    pub fn new(slots: usize, mode: &'static str, heartbeat_interval: Duration) -> Self {
        Self {
            notify: Notify::new(),
            slots,
            mode,
            heartbeat_interval,
            jobs: Mutex::new(HashMap::new()),
            status: Mutex::new(WorkerStatus {
                state: if slots == 0 { "stopped" } else { "starting" },
                busy: 0,
                last_heartbeat: None,
                queue_depth: 0,
                running: 0,
                queue_as_of: None,
            }),
            draining: Arc::new(AtomicBool::new(false)),
            drain_started: Mutex::new(None),
            shutdown: CancellationToken::new(),
            generation: AtomicU64::new(0),
        }
    }

    /// A local submission: wake an idle worker.
    pub fn submitted(&self) {
        self.bump();
        self.notify.notify_waiters();
        self.notify.notify_one();
    }

    pub fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn job(&self, id: Uuid) -> Option<Arc<JobControl>> {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    /// The API asked to cancel `id`: stop it at the next checkpoint if this process runs it.
    pub fn cancel(&self, id: Uuid) {
        if let Some(job) = self.job(id) {
            job.request_cancel();
        }
        self.bump();
    }

    pub fn status(&self) -> WorkerStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_state(&self, state: &'static str) {
        let mut s = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        s.state = state;
        s.last_heartbeat = Some(Instant::now());
    }

    fn update(&self, f: impl FnOnce(&mut WorkerStatus)) {
        f(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// SIGTERM: stop claiming and make running simulations stop at their next checkpoint.
    pub fn start_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
        self.drain_started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(Instant::now);
        self.shutdown.cancel();
        self.notify.notify_waiters();
        let mut s = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        if s.state != "stopped" {
            s.state = "draining";
        }
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    fn drain_elapsed(&self) -> Option<Duration> {
        self.drain_started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|t| t.elapsed())
    }
}

/// Everything a worker loop needs.
pub struct WorkerCtx {
    pub cfg: Arc<AppConfig>,
    pub db: Database,
    pub storage: Storage,
    pub cache: Arc<BundleCache>,
    pub registry: Arc<DatasetRegistry>,
    pub hub: Arc<WorkerHub>,
    pub metrics: Arc<Metrics>,
    pub worker_id: String,
}

/// Worker id: Render instance id (or host name) plus the process id.
pub fn worker_id(cfg: &AppConfig) -> String {
    let host = cfg
        .instance_id
        .clone()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "local".into());
    format!("{host}:{}", std::process::id())
}

impl WorkerCtx {
    async fn heartbeat_row(&self, state: &str, job: Option<Uuid>) {
        let now = chrono::Utc::now();
        let row = WorkerHeartbeat {
            worker_id: self.worker_id.clone(),
            hostname: self.worker_id.split(':').next().unwrap_or("local").into(),
            pid: i32::try_from(std::process::id()).unwrap_or(0),
            blackbox_version: env!("CARGO_PKG_VERSION").into(),
            platform: self.cfg.worker_platform.clone(),
            mode: self.hub.mode.into(),
            slots: i16::try_from(self.hub.slots).unwrap_or(1),
            state: state.into(),
            current_job_id: job,
            heartbeat_interval_ms: i32::try_from(self.cfg.heartbeat_interval.as_millis())
                .unwrap_or(15_000),
            started_at: now,
            last_heartbeat_at: now,
            stopped_at: None,
        };
        if let Err(e) = self.db.worker_heartbeat(&row).await {
            tracing::warn!(error = %e, "worker heartbeat row not written");
        }
    }

    async fn reap(&self) {
        match self.db.reap().await {
            Ok(reaped) if !reaped.is_empty() => {
                for job in &reaped {
                    if job.state != "expired" {
                        Metrics::inc(&self.metrics.lease_reclaims_total);
                    }
                    tracing::warn!(job_id = %job.id, state = %job.state, "reaper moved job");
                }
                self.hub.bump();
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "reaper pass failed"),
        }
    }

    async fn refresh_queue(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        if let Ok(stats) = self.db.queue_stats(Some(&self.cfg.worker_platform)).await {
            self.metrics
                .queue_depth
                .store(u64::try_from(stats.queued).unwrap_or(0), Ordering::Relaxed);
            self.hub.update(|s| {
                s.queue_depth = stats.queued;
                s.running = stats.running;
                s.queue_as_of = Some(chrono::Utc::now());
            });
        }
        self.db
            .next_wake(&self.cfg.worker_platform)
            .await
            .ok()
            .flatten()
    }
}

/// Runs `slots` worker loops until shutdown; returns when they have all stopped.
pub async fn run(ctx: Arc<WorkerCtx>, standalone: bool) {
    if ctx.hub.slots == 0 {
        return;
    }
    ctx.heartbeat_row("starting", None).await;
    ctx.reap().await;
    ctx.hub.set_state("idle");
    let liveness = tokio::spawn(idle_liveness(ctx.hub.clone()));
    let mut loops = Vec::new();
    for slot in 0..ctx.hub.slots {
        let ctx = ctx.clone();
        loops.push(tokio::spawn(async move {
            slot_loop(ctx, slot, standalone).await
        }));
    }
    for handle in loops {
        let _ = handle.await;
    }
    liveness.abort();
    ctx.hub.set_state("stopped");
    ctx.heartbeat_row("stopped", None).await;
}

/// Memory-only liveness while idle. An idle in-process worker parks without polling Postgres
/// (Neon must be allowed to scale to zero), so its `/status` heartbeat is refreshed here, every
/// heartbeat interval and without any database access: an idle worker reports `idle` with a
/// fresh heartbeat instead of looking stale. A busy worker's heartbeat stays the job heartbeat
/// (lease extension), so a stuck job still shows a stale age.
async fn idle_liveness(hub: Arc<WorkerHub>) {
    let mut tick = tokio::time::interval(hub.heartbeat_interval.max(Duration::from_millis(10)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        hub.update(|s| {
            if s.busy == 0 && s.state == "idle" {
                s.last_heartbeat = Some(Instant::now());
            }
        });
    }
}

async fn slot_loop(ctx: Arc<WorkerCtx>, slot: usize, standalone: bool) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if ctx.hub.is_draining() {
            break;
        }
        match ctx
            .db
            .claim_fair(
                &ctx.worker_id,
                &ctx.cfg.worker_platform,
                ctx.cfg.lease_seconds,
            )
            .await
        {
            Ok(Some(job)) => {
                backoff = Duration::from_secs(1);
                ctx.hub.bump();
                process(&ctx, job).await;
                ctx.reap().await;
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(slot, error = %e, "claim failed");
            }
        }
        // Idle.
        let wake_at = ctx.refresh_queue().await;
        ctx.hub.update(|s| {
            if s.busy == 0 && s.state != "draining" {
                s.state = "idle";
            }
        });
        if slot == 0 {
            ctx.heartbeat_row("idle", None).await;
        }
        let now = chrono::Utc::now();
        let sleep_for = match wake_at {
            Some(at) if at > now => (at - now)
                .to_std()
                .unwrap_or(backoff)
                .max(Duration::from_millis(200)),
            // Work is due but could not be claimed (another platform, a racing worker): back off.
            Some(_) => {
                let d = backoff;
                backoff = (backoff * 2).min(ctx.cfg.idle_poll_max);
                d
            }
            None if standalone => {
                let d = backoff;
                backoff = (backoff * 2).min(ctx.cfg.idle_poll_max);
                d
            }
            None => Duration::MAX,
        };
        let notified = ctx.hub.notify.notified();
        tokio::select! {
            _ = notified => { backoff = Duration::from_secs(1); }
            _ = sleep_or_forever(sleep_for) => { ctx.reap().await; }
            _ = ctx.hub.shutdown.cancelled() => break,
        }
    }
}

async fn sleep_or_forever(d: Duration) {
    if d == Duration::MAX {
        std::future::pending::<()>().await;
    } else {
        tokio::time::sleep(d).await;
    }
}

enum Outcome {
    Completed,
    Failed(&'static str, &'static str, String),
    Released,
    Expired,
    Abandoned,
}

async fn process(ctx: &Arc<WorkerCtx>, job: Job) {
    let lease = match job.lease() {
        Ok(lease) => lease,
        Err(_) => return,
    };
    let control = Arc::new(JobControl::new(&job, ctx.hub.draining.clone()));
    ctx.hub
        .jobs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(job.id, control.clone());
    ctx.hub.update(|s| {
        s.busy += 1;
        s.state = "busy";
        s.last_heartbeat = Some(Instant::now());
    });
    Metrics::inc(&ctx.metrics.worker_busy);
    ctx.heartbeat_row("busy", Some(job.id)).await;

    let stop = CancellationToken::new();
    let heartbeat = tokio::spawn(heartbeat_loop(
        ctx.clone(),
        lease.clone(),
        control.clone(),
        stop.clone(),
    ));
    let span = tracing::info_span!("job", job_id = %job.id, attempt = job.claim_count);
    let outcome = {
        let _enter = span.enter();
        tracing::info!("job claimed");
        drop(_enter);
        execute(ctx, &job, &lease, &control).await
    };
    stop.cancel();
    let _ = heartbeat.await;

    let result = match &outcome {
        Outcome::Completed | Outcome::Abandoned => Ok(()),
        Outcome::Failed(category, code, message) => ctx
            .db
            .fail(&lease, category, code, message)
            .await
            .map(|_| ()),
        Outcome::Released => ctx.db.release(&lease).await.map(|_| ()),
        Outcome::Expired => ctx
            .db
            .expire_attempt(&lease, "exceeded the 600 s per-attempt deadline")
            .await
            .map(|_| ()),
    };
    match (&outcome, result) {
        (_, Err(bb_db::Error::LeaseLost)) => {
            tracing::warn!(job_id = %job.id, "lease lost before the outcome was recorded")
        }
        (_, Err(e)) => {
            tracing::error!(job_id = %job.id, error = %e, "recording job outcome failed")
        }
        (Outcome::Failed(category, code, _), Ok(())) => {
            tracing::warn!(job_id = %job.id, category, code, "job failed")
        }
        _ => {}
    }
    ctx.hub
        .jobs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&job.id);
    ctx.metrics.worker_busy.fetch_sub(1, Ordering::Relaxed);
    ctx.hub.update(|s| {
        s.busy = s.busy.saturating_sub(1);
        s.last_heartbeat = Some(Instant::now());
        if s.busy == 0 && s.state == "busy" {
            s.state = "idle";
        }
    });
    ctx.hub.bump();
}

async fn heartbeat_loop(
    ctx: Arc<WorkerCtx>,
    lease: Lease,
    control: Arc<JobControl>,
    stop: CancellationToken,
) {
    let mut interval = tokio::time::interval(ctx.cfg.heartbeat_interval);
    interval.tick().await;
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = interval.tick() => {}
        }
        let (ticks, tape) = control.snapshot();
        match ctx
            .db
            .heartbeat(&lease, ctx.cfg.lease_seconds, ticks, tape)
            .await
        {
            Ok(job) => {
                ctx.hub.update(|s| s.last_heartbeat = Some(Instant::now()));
                if job.state == "cancelling" {
                    control.request_cancel();
                }
            }
            Err(bb_db::Error::LeaseLost) => {
                tracing::warn!(job_id = %lease.job_id, "lease lost; abandoning the attempt");
                control.lost.store(true, Ordering::SeqCst);
                return;
            }
            Err(e) => tracing::warn!(job_id = %lease.job_id, error = %e, "heartbeat failed"),
        }
    }
}

async fn execute(
    ctx: &Arc<WorkerCtx>,
    job: &Job,
    lease: &Lease,
    control: &Arc<JobControl>,
) -> Outcome {
    let config_row = match ctx.db.config(job.experiment_config_id).await {
        Ok(row) => row,
        Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
    };
    // Claim-time de-duplication: a valid run for this identity already exists.
    if let Ok(Some(run)) = ctx.db.existing_run(config_row.id, &job.platform).await {
        return match ctx.db.complete_existing(lease, run.id).await {
            Ok(_) => {
                Metrics::inc(&ctx.metrics.jobs_deduplicated_total);
                Outcome::Completed
            }
            Err(bb_db::Error::LeaseLost) => Outcome::Abandoned,
            Err(_) => Outcome::Released,
        };
    }
    let config: RunConfig = match serde_json::from_value(config_row.config.clone()) {
        Ok(c) => c,
        Err(e) => {
            return Outcome::Failed(
                "invalid_config",
                "invalid_config",
                format!("stored configuration is unreadable: {e}"),
            );
        }
    };
    let version = match ctx
        .db
        .dataset_version_by_id(config_row.dataset_version_id)
        .await
    {
        Ok(v) => v,
        Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
    };
    let dataset = match ctx.registry.load(&version).await {
        Ok(d) => d,
        Err(e) => return Outcome::Failed("internal", "dataset_unavailable", generic(&e)),
    };

    Metrics::inc(&ctx.metrics.simulations_started_total);
    let limits = ctx.cfg.limits;
    let run_control = control.clone();
    let ds = dataset.clone();
    let simulated = tokio::task::spawn_blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            pipeline::execute_with(&ds, &config, &limits, run_control.as_ref())
        }))
    })
    .await;
    let run = match simulated {
        Ok(Ok(Ok(run))) => run,
        Ok(Ok(Err(err))) => return failure(control, err),
        Ok(Err(panic)) => {
            tracing::error!(job_id = %job.id, panic = ?panic_text(&*panic), "simulation panicked");
            return Outcome::Failed(
                "simulation_failed",
                "simulation_failed",
                "the simulation failed; the details are in the server log".into(),
            );
        }
        Err(e) => {
            tracing::error!(job_id = %job.id, error = %e, "simulation task failed");
            return Outcome::Failed("internal", "internal", "internal worker error".into());
        }
    };
    if control.lost.load(Ordering::SeqCst) {
        return Outcome::Abandoned;
    }
    if let Some(elapsed) = ctx.hub.drain_elapsed()
        && elapsed + COMMIT_GRACE > ctx.cfg.shutdown_grace
    {
        return Outcome::Released;
    }
    persist(ctx, job, lease, &config_row.fingerprint, run, &dataset).await
}

fn failure(control: &JobControl, err: AppError) -> Outcome {
    match err {
        AppError::Cancelled(_) => {
            if control.lost.load(Ordering::SeqCst) {
                Outcome::Abandoned
            } else if control.deadline_hit.load(Ordering::SeqCst) {
                Outcome::Expired
            } else {
                // Drain (→ queued) or cancel (cancelling → cancelled): `release` does both.
                Outcome::Released
            }
        }
        AppError::BudgetExceeded(message) => {
            Outcome::Failed("budget_exceeded", "budget_exceeded", message)
        }
        AppError::InvalidConfig(problems) => Outcome::Failed(
            "invalid_config",
            "invalid_config",
            problems.into_iter().take(20).collect::<Vec<_>>().join("; "),
        ),
        AppError::PlatformMismatch(message) => {
            Outcome::Failed("internal", "platform_mismatch", message)
        }
        AppError::SimulationFailed(detail) => {
            tracing::error!(error = %detail, "simulation failed");
            Outcome::Failed(
                "simulation_failed",
                "simulation_failed",
                "the simulation failed; the details are in the server log".into(),
            )
        }
        other => {
            tracing::error!(error = %other, "job failed");
            Outcome::Failed("internal", "internal", "internal worker error".into())
        }
    }
}

fn generic(e: &dyn std::fmt::Display) -> String {
    tracing::error!(error = %e, "job failed");
    "internal worker error".into()
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no panic message".into())
}

/// Completion protocol (TARGET_ARCHITECTURE §6.2) steps 1–6.
async fn persist(
    ctx: &Arc<WorkerCtx>,
    job: &Job,
    lease: &Lease,
    fingerprint: &[u8],
    run: bb_core::result::RunResult,
    dataset: &Arc<crate::datasets::DatasetCtx>,
) -> Outcome {
    // Analytics (streamed, bounded) are part of the bundle trailer and of the DB row.
    let ds = dataset.clone();
    let run = Arc::new(run);
    let for_analytics = run.clone();
    let analytics =
        match tokio::task::spawn_blocking(move || analytics::run_analytics(&for_analytics, &ds))
            .await
        {
            Ok(Ok(a)) => Some(a),
            Ok(Err(e)) => {
                tracing::warn!(job_id = %job.id, error = %e, "run analytics unavailable");
                None
            }
            Err(e) => {
                tracing::warn!(job_id = %job.id, error = %e, "run analytics task failed");
                None
            }
        };
    let encoded = match analytics.as_ref().map(analytics::encode).transpose() {
        Ok(e) => e,
        Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
    };
    let analytics_value: Option<serde_json::Value> = encoded
        .as_ref()
        .and_then(|e| serde_json::from_str(&e.json).ok());

    // 1-3: write to a temp file, hash, validate by re-reading (inside BundleWriter).
    let limits = bundle_limits(ctx.cfg.limits.max_output_bytes);
    let temp_dir = ctx.cache.dir().to_path_buf();
    let for_writer = run.clone();
    let written = tokio::task::spawn_blocking(move || {
        let mut writer = BundleWriter::new(limits)?;
        if let Some(value) = analytics_value {
            writer = writer.with_analytics(value);
        }
        writer.write_run(&temp_dir, &for_writer)
    })
    .await;
    let bundle = match written {
        Ok(Ok(bundle)) => bundle,
        Ok(Err(ArtifactError::LimitExceeded(what))) => {
            return Outcome::Failed(
                "budget_exceeded",
                "budget_exceeded",
                format!(
                    "the run's bundle exceeds this server's output limit ({what}); nothing was stored"
                ),
            );
        }
        Ok(Err(ArtifactError::Integrity(e))) => {
            tracing::error!(job_id = %job.id, error = %e, "bundle failed self-verification");
            return Outcome::Failed(
                "integrity_failed",
                "integrity_failed",
                "the bundle failed verification after writing; nothing was stored".into(),
            );
        }
        Ok(Err(e)) => return Outcome::Failed("internal", "internal", generic(&e)),
        Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
    };
    let trust = bundle.trust.clone();
    let run_uuid = Uuid::now_v7();
    let key = format!(
        "runs/{}/{}/{}.bbz",
        hex::encode(fingerprint),
        bb_core::platform::storage_segment(&job.platform),
        run_uuid
    );
    // 4a: upload intent (own transaction) so an orphaned object is garbage collected.
    if let Err(e) = ctx.db.upload_intent(ctx.storage.backend, &key).await {
        return Outcome::Failed("storage_failed", "storage_failed", generic(&e));
    }
    // 4b: publish (never overwrites).
    if let Err(e) = ctx.storage.put(&key, bundle.path()).await {
        tracing::error!(job_id = %job.id, error = %e, "upload failed");
        return Outcome::Failed(
            "storage_failed",
            "storage_failed",
            "the bundle could not be stored; resubmit to retry".into(),
        );
    }
    // 4c: read back the trailer and compare its digest.
    let range = match trust.trailer.range() {
        Ok(r) => r,
        Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
    };
    match ctx.storage.get_range(&key, range).await {
        Ok(bytes) if hex::encode(Sha256::digest(&bytes)) == trust.trailer.sha256 => {}
        Ok(_) => {
            return Outcome::Failed(
                "storage_failed",
                "storage_failed",
                "the stored bundle did not read back identically; resubmit to retry".into(),
            );
        }
        Err(e) => {
            tracing::error!(job_id = %job.id, error = %e, "read-back failed");
            return Outcome::Failed(
                "storage_failed",
                "storage_failed",
                "the stored bundle could not be read back; resubmit to retry".into(),
            );
        }
    }
    // 5: transactional completion with the complete BundleTrust as the trust root.
    let page_index = match serde_json::to_value(&trust) {
        Ok(v) => v,
        Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
    };
    let page_index_sha256 = crate::canonical::sha256(&page_index).to_vec();
    let decode = |h: &str| hex::decode(h).unwrap_or_default();
    let count = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    let new_run = NewRun {
        id: run_uuid,
        blackbox_version: env!("CARGO_PKG_VERSION").into(),
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .into(),
        result_sha256: decode(&trust.result_sha256),
        summary_sha256: decode(&trust.summary_sha256),
        ticks: count(trust.counts.ticks),
        order_count: count(trust.counts.orders),
        fill_count: count(trust.counts.fills),
        tape_len: count(trust.counts.tape),
        equity_len: count(trust.counts.equity),
        wall_time_ms: i64::try_from(run.manifest.wall_time_ms).unwrap_or(i64::MAX),
        peak_rss_bytes: crate::metrics::resident_bytes().map(|(_, hwm)| hwm as i64),
        metrics: match serde_json::to_value(&run.metrics) {
            Ok(v) => v,
            Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
        },
        engine_check: match serde_json::to_value(&run.engine_check) {
            Ok(v) => v,
            Err(e) => return Outcome::Failed("internal", "internal", generic(&e)),
        },
    };
    let artifact = NewArtifact {
        format_version: i32::try_from(trust.format_version).unwrap_or(1),
        storage_backend: ctx.storage.backend.into(),
        storage_key: key,
        sha256: decode(&trust.sha256),
        size_bytes: count(trust.size_bytes),
        uncompressed_bytes: count(trust.uncompressed_bytes),
        page_index,
        page_index_sha256,
    };
    match ctx.db.complete(lease, &new_run, &artifact).await {
        Ok(completion) => {
            if !completion.reused {
                if let Some(encoded) = &encoded
                    && let Err(e) = ctx
                        .db
                        .put_run_analytics(
                            completion.run.id,
                            bb_analytics::ANALYTICS_VERSION,
                            &encoded.json,
                            &encoded.sha256,
                        )
                        .await
                {
                    tracing::warn!(error = %e, "run analytics row not stored");
                }
                // 6: keep the verified file in the read cache.
                let size = trust.size_bytes;
                let temp = bundle.into_temp_file();
                match temp.into_temp_path().keep() {
                    Ok(path) => {
                        let _ = ctx.cache.insert(&trust.sha256, &path, size);
                    }
                    Err(e) => tracing::warn!(error = %e, "bundle not cached"),
                }
            }
            tracing::info!(job_id = %job.id, run = %completion.run.id, reused = completion.reused, "job completed");
            Outcome::Completed
        }
        Err(bb_db::Error::Nondeterminism) => {
            Metrics::inc(&ctx.metrics.nondeterminism_total);
            tracing::error!(job_id = %job.id, "nondeterminism detected");
            // The job was already failed inside the completion transaction.
            Outcome::Abandoned
        }
        Err(bb_db::Error::LeaseLost) => Outcome::Abandoned,
        Err(e) => Outcome::Failed("internal", "internal", generic(&e)),
    }
}
