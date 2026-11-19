//! The job worker (TARGET_ARCHITECTURE §4.3, §6, §12): claims leased jobs from Postgres, runs
//! the simulation under a [`RunControl`] that reports progress and stops cooperatively, writes
//! and verifies the bundle, uploads it, reads it back and completes the job in one transaction.
//!
//! No idle polling: an idle in-process worker sleeps until a local submission wakes it
//! ([`WorkerHub::notify`]), the earliest scheduled queue event (`next_wake`) or shutdown. A
//! standalone `blackbox worker` cannot be woken by another process, so it additionally polls
//! with exponential backoff (1 s … `idle_poll_max`).
//!
//! Failures are persisted with a stable category/code and a generic or bounded message; raw
//! dependency, engine and panic text only reaches the log, through the SEC-18 redactor, next to
//! the safe fields `job_id`, `component`, `error_class`, `reason` and `elapsed_ms`. A
//! [`FaultInjector`] (tests only) can make individual steps fail (SEC-17).
//!
//! Cancellation takes precedence over a failed persistence step while the lease is still held.
//! Between persistence steps a known lost lease or cancellation stops further store requests;
//! an upload that already landed keeps its intent for garbage collection, never a committed run.

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bb_artifacts::{ArtifactError, BundleWriter, TrailerAnalytics};
use bb_core::config::RunConfig;
use bb_db::{Database, Job, Lease, NewArtifact, NewRun, WorkerHeartbeat};
use bb_sim::{Progress, RunControl};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    analytics,
    artifacts::{BundleCache, Storage, StorageError, bundle_limits},
    config::{AppConfig, redact, redacted},
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

    fn cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
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
    faults: Mutex<Option<Arc<dyn FaultInjector>>>,
}

/// Test-only fault seam (SECURITY_DISPOSITIONS SEC-17): makes one step of a real job fail the way
/// a production dependency fails, so integration tests can check the persisted job state and the
/// logs. Production never installs one: there is no endpoint, environment variable or flag for
/// it, only in-process code holding the [`WorkerHub`] ([`WorkerHub::inject_faults`]).
pub trait FaultInjector: Send + Sync {
    /// Runs on the simulation thread right before the simulation, inside its panic guard: may
    /// panic, wait on `control` (cancellation, lease loss, deadline, drain) or return an engine
    /// error. `Ok` runs the real simulation.
    fn before_simulation(&self, _control: &JobControl) -> Result<(), AppError> {
        Ok(())
    }

    /// A database failure returned instead of performing `step` for job `job`.
    fn database(&self, _step: DbStep, _job: Uuid) -> Option<bb_db::Error> {
        None
    }
}

/// Database steps of a job that a [`FaultInjector`] can fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbStep {
    /// Reading the job's stored configuration (first query of an attempt).
    ReadConfig,
    /// A lease-extending heartbeat.
    Heartbeat,
    /// The completion transaction.
    Complete,
    /// Recording a failed, released or expired outcome.
    RecordOutcome,
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
            faults: Mutex::new(None),
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

    /// Installs a test [`FaultInjector`] for the jobs this process claims from now on.
    #[doc(hidden)]
    pub fn inject_faults(&self, faults: Arc<dyn FaultInjector>) {
        *self.faults.lock().unwrap_or_else(PoisonError::into_inner) = Some(faults);
    }

    fn faults(&self) -> Option<Arc<dyn FaultInjector>> {
        self.faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
    /// The injected failure for `step`, if a test installed one.
    fn db_fault(&self, step: DbStep, job: Uuid) -> Option<bb_db::Error> {
        self.hub.faults().and_then(|f| f.database(step, job))
    }

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
            tracing::warn!(
                component = "worker",
                error_class = "database",
                reason = "heartbeat_row_not_written",
                error = %redacted(&e),
                "worker heartbeat row not written"
            );
        }
    }

    async fn reap(&self) {
        match self.db.reap().await {
            Ok(reaped) if !reaped.is_empty() => {
                for job in &reaped {
                    if job.state != "expired" {
                        Metrics::inc(&self.metrics.lease_reclaims_total);
                    }
                    tracing::warn!(
                        job_id = %job.id,
                        component = "worker",
                        state = %job.state,
                        reason = "lease_expired",
                        "reaper moved job"
                    );
                }
                self.hub.bump();
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                component = "worker",
                error_class = "database",
                reason = "reap_failed",
                error = %redacted(&e),
                "reaper pass failed"
            ),
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
                let _ = tokio::task::spawn_blocking(release_freed_heap).await;
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    slot,
                    component = "worker",
                    error_class = "database",
                    reason = "claim_failed",
                    error = %redacted(&e),
                    "claim failed"
                );
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

/// Returns the heap a finished job freed to the operating system (AT-5 / SEC-6,
/// `docs/rebuild/HOSTED_MEMORY.md`). A job allocates and frees up to a few hundred MiB (the
/// run, its analytics, bundle pages); glibc keeps freed chunks inside its arenas and only ever
/// gives back an arena's top, so resident memory ratcheted job after job until a 512 MB
/// container was killed. `malloc_trim(0)` releases the free pages of every arena; live
/// allocations are untouched. A no-op on other platforms and C libraries.
fn release_freed_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` takes glibc's own arena locks and only returns pages of free chunks
    // to the kernel; it neither moves nor frees a live allocation.
    unsafe {
        libc::malloc_trim(0);
    }
}

enum Outcome {
    Completed,
    Failed(&'static str, &'static str, String),
    Released,
    /// The job is `cancelling`: record `cancelled` now instead of leaving it to the reaper.
    Cancelled,
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
    let started = Instant::now();
    let span = tracing::info_span!("job", job_id = %job.id, attempt = job.claim_count);
    tracing::info!(parent: &span, job_id = %job.id, component = "worker", "job claimed");
    let outcome = execute(ctx, &job, &lease, &control)
        .instrument(span.clone())
        .await;
    stop.cancel();
    let _ = heartbeat.await;

    let unrecordable = match &outcome {
        Outcome::Completed | Outcome::Abandoned => None,
        _ => ctx.db_fault(DbStep::RecordOutcome, job.id),
    };
    let result = match (&outcome, unrecordable) {
        (Outcome::Completed | Outcome::Abandoned, _) => Ok(()),
        (_, Some(e)) => Err(e),
        (Outcome::Failed(category, code, message), None) => ctx
            .db
            .fail(&lease, category, code, &redact(message))
            .await
            .map(|_| ()),
        (Outcome::Released, None) => ctx.db.release(&lease).await.map(|_| ()),
        (Outcome::Cancelled, None) => ctx.db.acknowledge_cancel(&lease).await.map(|_| ()),
        (Outcome::Expired, None) => ctx
            .db
            .expire_attempt(&lease, "exceeded the 600 s per-attempt deadline")
            .await
            .map(|_| ()),
    };
    // Cancellation can race any failed step, even before a heartbeat or local API notification
    // reaches this worker. `fail`/`expire_attempt` correctly refuse `cancelling`; acknowledge it
    // with the same lease fence instead of abandoning it to the reaper.
    let (outcome, result) = match result {
        Err(bb_db::Error::LeaseLost)
            if matches!(&outcome, Outcome::Failed(..) | Outcome::Expired) =>
        {
            match ctx.db.acknowledge_cancel(&lease).await {
                Ok(_) => (Outcome::Cancelled, Ok(())),
                Err(e) => (outcome, Err(e)),
            }
        }
        result => (outcome, result),
    };
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let job_id = job.id;
    match (&outcome, result) {
        (_, Err(bb_db::Error::LeaseLost)) => tracing::warn!(
            job_id = %job_id,
            component = "worker",
            reason = "lease_lost",
            elapsed_ms,
            "lease lost before the outcome was recorded"
        ),
        (_, Err(e)) => tracing::error!(
            job_id = %job_id,
            component = "worker",
            error_class = "database",
            reason = "outcome_not_recorded",
            elapsed_ms,
            error = %redacted(&e),
            "recording job outcome failed; the lease will expire and the job is retried"
        ),
        (Outcome::Failed(category, code, _), Ok(())) => tracing::warn!(
            job_id = %job_id,
            component = "worker",
            error_class = *category,
            reason = *code,
            elapsed_ms,
            "job failed"
        ),
        (Outcome::Released, Ok(())) => tracing::info!(
            job_id = %job_id,
            component = "worker",
            reason = "released",
            elapsed_ms,
            "job released (cancelled or draining)"
        ),
        (Outcome::Cancelled, Ok(())) => tracing::info!(
            job_id = %job_id,
            component = "worker",
            reason = "cancelled",
            elapsed_ms,
            "job cancelled; nothing stored"
        ),
        (Outcome::Expired, Ok(())) => tracing::warn!(
            job_id = %job_id,
            component = "worker",
            error_class = "deadline",
            reason = "deadline_exceeded",
            elapsed_ms,
            "job attempt expired"
        ),
        (Outcome::Completed | Outcome::Abandoned, Ok(())) => {}
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
        let beat = match ctx.db_fault(DbStep::Heartbeat, lease.job_id) {
            Some(e) => Err(e),
            None => {
                ctx.db
                    .heartbeat(&lease, ctx.cfg.lease_seconds, ticks, tape)
                    .await
            }
        };
        match beat {
            Ok(job) => {
                ctx.hub.update(|s| s.last_heartbeat = Some(Instant::now()));
                if job.state == "cancelling" {
                    control.request_cancel();
                }
            }
            Err(bb_db::Error::LeaseLost) => {
                tracing::warn!(
                    job_id = %lease.job_id,
                    component = "worker",
                    reason = "lease_lost",
                    "lease lost; abandoning the attempt"
                );
                control.lost.store(true, Ordering::SeqCst);
                return;
            }
            Err(e) => tracing::warn!(
                job_id = %lease.job_id,
                component = "worker",
                error_class = "database",
                reason = "heartbeat_failed",
                error = %redacted(&e),
                "heartbeat failed"
            ),
        }
    }
}

async fn execute(
    ctx: &Arc<WorkerCtx>,
    job: &Job,
    lease: &Lease,
    control: &Arc<JobControl>,
) -> Outcome {
    let read = match ctx.db_fault(DbStep::ReadConfig, job.id) {
        Some(e) => Err(e),
        None => ctx.db.config(job.experiment_config_id).await,
    };
    let config_row = match read {
        Ok(row) => row,
        Err(e) => {
            let message = generic(job.id, "database", "config_unreadable", &e);
            return Outcome::Failed("internal", "internal", message);
        }
    };
    // Claim-time de-duplication: a valid run for this identity already exists.
    if let Ok(Some(run)) = ctx.db.existing_run(config_row.id, &job.platform).await {
        return match ctx.db.complete_existing(lease, run.id).await {
            Ok(_) => {
                Metrics::inc(&ctx.metrics.jobs_deduplicated_total);
                Outcome::Completed
            }
            Err(bb_db::Error::LeaseLost) => Outcome::Cancelled,
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
        Err(e) => {
            let message = generic(job.id, "database", "dataset_version_unreadable", &e);
            return Outcome::Failed("internal", "internal", message);
        }
    };
    let dataset = match ctx.registry.load(&version).await {
        Ok(d) => d,
        Err(e) => {
            let message = generic(job.id, "dataset", "dataset_unavailable", &e);
            return Outcome::Failed("internal", "dataset_unavailable", message);
        }
    };

    Metrics::inc(&ctx.metrics.simulations_started_total);
    let limits = ctx.cfg.limits;
    let run_control = control.clone();
    let ds = dataset.clone();
    let faults = ctx.hub.faults();
    let simulated = tokio::task::spawn_blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            if let Some(faults) = &faults {
                faults.before_simulation(run_control.as_ref())?;
            }
            pipeline::execute_with(&ds, &config, &limits, run_control.as_ref())
        }))
    })
    .await;
    let run = match simulated {
        Ok(Ok(Ok(run))) => run,
        Ok(Ok(Err(err))) => return failure(job.id, control, err),
        Ok(Err(panic)) => {
            tracing::error!(
                job_id = %job.id,
                component = "worker",
                error_class = "panic",
                reason = "simulation_failed",
                panic = %redacted(panic_text(&*panic)),
                "simulation panicked"
            );
            return Outcome::Failed(
                "simulation_failed",
                "simulation_failed",
                SIMULATION_FAILED.into(),
            );
        }
        Err(e) => {
            let message = generic(job.id, "task", "simulation_task_failed", &e);
            return Outcome::Failed("internal", "internal", message);
        }
    };
    if control.lost.load(Ordering::SeqCst) {
        return Outcome::Abandoned;
    }
    // A cancel that arrived after the simulation's last checkpoint: store nothing.
    if control.cancel_requested() {
        return Outcome::Cancelled;
    }
    if let Some(elapsed) = ctx.hub.drain_elapsed()
        && elapsed + COMMIT_GRACE > ctx.cfg.shutdown_grace
    {
        return Outcome::Released;
    }
    persist(
        ctx,
        job,
        lease,
        control,
        &config_row.fingerprint,
        run,
        &dataset,
    )
    .await
}

/// Persisted message of an engine error or panic (the detail is only in the redacted log).
const SIMULATION_FAILED: &str = "the simulation failed; the details are in the server log";
/// Persisted message of every other internal failure.
const INTERNAL: &str = "internal worker error";

fn failure(job: Uuid, control: &JobControl, err: AppError) -> Outcome {
    match err {
        AppError::Cancelled(_) => {
            if control.lost.load(Ordering::SeqCst) {
                Outcome::Abandoned
            } else if control.deadline_hit.load(Ordering::SeqCst) {
                Outcome::Expired
            } else if control.cancel_requested() {
                Outcome::Cancelled
            } else {
                // Drain (→ queued), or a cancel this process has not seen yet (→ cancelled):
                // `release` does both.
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
            tracing::error!(
                job_id = %job,
                component = "worker",
                error_class = "engine",
                reason = "simulation_failed",
                error = %redacted(&detail),
                "simulation failed"
            );
            Outcome::Failed(
                "simulation_failed",
                "simulation_failed",
                SIMULATION_FAILED.into(),
            )
        }
        other => {
            let message = generic(job, "internal", "internal", &other);
            Outcome::Failed("internal", "internal", message)
        }
    }
}

/// Logs a failed job step (raw text redacted) and returns the generic persisted message.
fn generic(
    job: Uuid,
    class: &'static str,
    reason: &'static str,
    e: &dyn std::fmt::Display,
) -> String {
    tracing::error!(
        job_id = %job,
        component = "worker",
        error_class = class,
        reason,
        error = %redacted(e),
        "job step failed"
    );
    INTERNAL.into()
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
    control: &JobControl,
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
                tracing::warn!(
                    job_id = %job.id,
                    component = "worker",
                    error_class = "analytics",
                    reason = "analytics_unavailable",
                    error = %redacted(&e),
                    "run analytics unavailable"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    job_id = %job.id,
                    component = "worker",
                    error_class = "task",
                    reason = "analytics_task_failed",
                    error = %redacted(&e),
                    "run analytics task failed"
                );
                None
            }
        };
    // Consumed: the typed report is dropped once encoded, not held through the bundle write.
    let encoded = match analytics.map(|a| analytics::encode(&a)).transpose() {
        Ok(e) => e,
        Err(e) => {
            let message = generic(job.id, "json", "analytics_encoding_failed", &e);
            return Outcome::Failed("internal", "internal", message);
        }
    };
    let trailer_analytics = encoded
        .as_ref()
        .and_then(|e| TrailerAnalytics::from_json(&e.json).ok());

    // 1-3: write to a temp file, hash, validate by re-reading (inside BundleWriter).
    let limits = bundle_limits(ctx.cfg.limits.max_output_bytes);
    let temp_dir = ctx.cache.dir().to_path_buf();
    let for_writer = run.clone();
    let written = tokio::task::spawn_blocking(move || {
        let mut writer = BundleWriter::new(limits)?;
        if let Some(trailer_analytics) = trailer_analytics {
            writer = writer.with_analytics(trailer_analytics);
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
            tracing::error!(
                job_id = %job.id,
                component = "worker",
                error_class = "artifact",
                reason = "integrity_failed",
                error = %redacted(&e),
                "bundle failed self-verification"
            );
            return Outcome::Failed(
                "integrity_failed",
                "integrity_failed",
                "the bundle failed verification after writing; nothing was stored".into(),
            );
        }
        Ok(Err(e)) => {
            let message = generic(job.id, "artifact", "bundle_write_failed", &e);
            return Outcome::Failed("internal", "internal", message);
        }
        Err(e) => {
            let message = generic(job.id, "task", "bundle_task_failed", &e);
            return Outcome::Failed("internal", "internal", message);
        }
    };
    let trust = bundle.trust.clone();
    let run_uuid = Uuid::now_v7();
    let key = format!(
        "runs/{}/{}/{}.bbz",
        hex::encode(fingerprint),
        bb_core::platform::storage_segment(&job.platform),
        run_uuid
    );
    // A lost lease or cancel while analytics and the bundle were built: upload nothing.
    if control.lost.load(Ordering::SeqCst) {
        return Outcome::Abandoned;
    }
    if control.cancel_requested() {
        return Outcome::Cancelled;
    }
    // 4a: upload intent (own transaction) so an orphaned object is garbage collected.
    if let Err(e) = ctx.db.upload_intent(ctx.storage.backend, &key).await {
        let message = generic(job.id, "database", "upload_intent_failed", &e);
        return Outcome::Failed("storage_failed", "storage_failed", message);
    }
    if control.lost.load(Ordering::SeqCst) {
        return Outcome::Abandoned;
    }
    if control.cancel_requested() {
        return Outcome::Cancelled;
    }
    // 4b: publish (never overwrites); every attempt is charged to the submitting workspace.
    match ctx
        .storage
        .put(&key, bundle.path(), job.submitted_by_workspace_id)
        .await
    {
        Ok(()) => {}
        Err(StorageError::Budget) => {
            return Outcome::Failed(
                "storage_failed",
                "storage_budget",
                "the monthly storage upload allowance is spent; nothing was stored".into(),
            );
        }
        Err(e) => {
            tracing::error!(
                job_id = %job.id,
                component = "storage",
                error_class = "transport",
                reason = "upload_failed",
                error = %redacted(&e),
                "upload failed"
            );
            return Outcome::Failed(
                "storage_failed",
                "storage_failed",
                "the bundle could not be stored; resubmit to retry".into(),
            );
        }
    }
    // An upload can have landed before its response arrives. Leave its intent for GC, but do
    // not spend a read-back request for an attempt that already stopped.
    if control.lost.load(Ordering::SeqCst) {
        return Outcome::Abandoned;
    }
    if control.cancel_requested() {
        return Outcome::Cancelled;
    }
    // 4c: read back the trailer and compare its digest.
    let range = match trust.trailer.range() {
        Ok(r) => r,
        Err(e) => {
            let message = generic(job.id, "artifact", "trailer_range_invalid", &e);
            return Outcome::Failed("internal", "internal", message);
        }
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
            tracing::error!(
                job_id = %job.id,
                component = "storage",
                error_class = "transport",
                reason = "read_back_failed",
                error = %redacted(&e),
                "read-back failed"
            );
            return Outcome::Failed(
                "storage_failed",
                "storage_failed",
                "the stored bundle could not be read back; resubmit to retry".into(),
            );
        }
    }
    if control.lost.load(Ordering::SeqCst) {
        return Outcome::Abandoned;
    }
    if control.cancel_requested() {
        return Outcome::Cancelled;
    }
    // 5: transactional completion with the complete BundleTrust as the trust root.
    let page_index = match serde_json::to_value(&trust) {
        Ok(v) => v,
        Err(e) => {
            let message = generic(job.id, "json", "trust_encoding_failed", &e);
            return Outcome::Failed("internal", "internal", message);
        }
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
            Err(e) => {
                let message = generic(job.id, "json", "metrics_encoding_failed", &e);
                return Outcome::Failed("internal", "internal", message);
            }
        },
        engine_check: match serde_json::to_value(&run.engine_check) {
            Ok(v) => v,
            Err(e) => {
                let message = generic(job.id, "json", "engine_check_encoding_failed", &e);
                return Outcome::Failed("internal", "internal", message);
            }
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
    let completed = match ctx.db_fault(DbStep::Complete, job.id) {
        Some(e) => Err(e),
        None => ctx.db.complete(lease, &new_run, &artifact).await,
    };
    match completed {
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
                    tracing::warn!(
                        job_id = %job.id,
                        component = "worker",
                        error_class = "database",
                        reason = "analytics_row_not_stored",
                        error = %redacted(&e),
                        "run analytics row not stored"
                    );
                }
                // 6: keep the verified file in the read cache.
                let size = trust.size_bytes;
                let temp = bundle.into_temp_file();
                match temp.into_temp_path().keep() {
                    Ok(path) => {
                        let _ = ctx.cache.insert(&trust.sha256, &path, size);
                    }
                    Err(e) => tracing::warn!(
                        job_id = %job.id,
                        component = "worker",
                        error_class = "io",
                        reason = "bundle_not_cached",
                        error = %redacted(&e),
                        "bundle not cached"
                    ),
                }
            }
            tracing::info!(
                job_id = %job.id,
                component = "worker",
                run = %completion.run.id,
                reused = completion.reused,
                "job completed"
            );
            Outcome::Completed
        }
        Err(bb_db::Error::Nondeterminism) => {
            Metrics::inc(&ctx.metrics.nondeterminism_total);
            tracing::error!(
                job_id = %job.id,
                component = "worker",
                error_class = "integrity",
                reason = "nondeterminism_detected",
                "nondeterminism detected"
            );
            // The job was already failed inside the completion transaction.
            Outcome::Abandoned
        }
        // Refused because the job is `cancelling` (a cancel that raced the upload, possibly seen
        // by no heartbeat yet): record `cancelled`. A truly lost lease fails that as well and is
        // logged as `lease_lost`.
        Err(bb_db::Error::LeaseLost) => Outcome::Cancelled,
        Err(e) => {
            let message = generic(job.id, "database", "completion_failed", &e);
            Outcome::Failed("internal", "internal", message)
        }
    }
}
