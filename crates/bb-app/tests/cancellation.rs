//! Cancellation through the real worker and Postgres, during simulation and persistence. A
//! cancelled attempt reaches `cancelled` promptly — never by waiting for the lease reaper —
//! even when a store response fails concurrently. No run or artifact row is committed; an
//! upload that already landed retains its garbage-collection intent.
mod common;

use std::{
    io::Write,
    path::Path,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bb_app::{
    artifacts::Storage,
    config::Profile,
    runtime,
    worker::{DbStep, FaultInjector},
};
use bb_db::{Database, Job, Scope};
use bb_sim::RunControl;
use common::*;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Lease of these servers: long, like production, so a reaper-driven `cancelled` would take a
/// minute and cannot pass for a prompt one.
const LEASE_SECONDS: i32 = 60;
/// Upper bound from the cancel request to the job's `cancelled` row.
const PROMPT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The production log pipeline writing into memory (installed once per test binary).
static LOGS: LazyLock<Sink> = LazyLock::new(|| {
    let sink = Sink(Arc::new(Mutex::new(Vec::new())));
    let writer = sink.clone();
    // The app's panic hook logs into the sink; keep the default one so failures stay readable.
    let default_hook = std::panic::take_hook();
    runtime::init_tracing_with(Profile::Hosted, move || writer.clone());
    let app_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        app_hook(info);
        default_hook(info);
    }));
    sink
});

/// The log line of `job` carrying `reason`.
fn logged(job: Uuid, reason: &str) -> Option<String> {
    let logs = String::from_utf8_lossy(&LOGS.0.lock().unwrap()).into_owned();
    logs.lines()
        .find(|l| {
            l.contains(&format!("\"job_id\":\"{job}\""))
                && l.contains(&format!("\"reason\":\"{reason}\""))
        })
        .map(str::to_string)
}

/// Holds the worker right before the completion transaction until `go`: the simulation, the
/// bundle and the upload are done, the job is still `running`.
#[derive(Default)]
struct AtCompletion {
    reached: AtomicBool,
    go: AtomicBool,
}

impl FaultInjector for AtCompletion {
    fn database(&self, step: DbStep, _job: Uuid) -> Option<bb_db::Error> {
        if step == DbStep::Complete {
            self.reached.store(true, Ordering::SeqCst);
            while !self.go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreStep {
    Upload,
    Readback,
}

/// Pauses a store response after the actual operation, with an optional permanent failure.
/// An upload can already exist when cancellation or lease loss arrives.
struct PausedStore {
    inner: bb_artifacts::FsArtifactStore,
    step: StoreStep,
    fail: bool,
    reached: AtomicBool,
    go: Semaphore,
    gets: AtomicUsize,
}

impl PausedStore {
    fn new(root: &Path, step: StoreStep, fail: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: bb_artifacts::FsArtifactStore::new(root).unwrap(),
            step,
            fail,
            reached: AtomicBool::new(false),
            go: Semaphore::new(0),
            gets: AtomicUsize::new(0),
        })
    }

    async fn pause(&self, step: StoreStep) -> Result<(), bb_artifacts::ArtifactError> {
        if self.step == step {
            self.reached.store(true, Ordering::SeqCst);
            self.go.acquire().await.unwrap().forget();
            if self.fail {
                return Err(bb_artifacts::ArtifactError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "store response lost",
                )));
            }
        }
        Ok(())
    }

    async fn wait_reached(&self) {
        let give_up = Instant::now() + Duration::from_secs(120);
        while !self.reached.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < give_up,
                "store never reached {:?}",
                self.step
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[async_trait::async_trait]
impl bb_artifacts::ArtifactStore for PausedStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<bb_artifacts::ArtifactHead, bb_artifacts::ArtifactError> {
        let head = self.inner.put_bundle_from_file(key, path).await?;
        self.pause(StoreStep::Upload).await?;
        Ok(head)
    }

    async fn get_range(
        &self,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<bytes::Bytes, bb_artifacts::ArtifactError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        let bytes = self.inner.get_range(key, range).await?;
        self.pause(StoreStep::Readback).await?;
        Ok(bytes)
    }

    async fn head(
        &self,
        key: &str,
    ) -> Result<bb_artifacts::ArtifactHead, bb_artifacts::ArtifactError> {
        self.inner.head(key).await
    }

    async fn delete(&self, key: &str) -> Result<(), bb_artifacts::ArtifactError> {
        self.inner.delete(key).await
    }
}

struct Fixture {
    tdb: TestDb,
    _dir: tempfile::TempDir,
    server: Server,
    key: String,
    manifest: bb_core::dataset::DatasetManifest,
}

async fn fixture(dataset_seconds: i64, storage: Option<Storage>) -> Option<Fixture> {
    LazyLock::force(&LOGS);
    let tdb = TestDb::create().await?;
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            storage,
            tweak: Box::new(|cfg| cfg.lease_seconds = LEASE_SECONDS),
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), dataset_seconds).await;
    let (_, key) = workspace_key(&server.db, "cancel", &[Scope::Run]).await;
    Some(Fixture {
        tdb,
        _dir: dir,
        server,
        key,
        manifest,
    })
}

async fn submit(server: &Server, key: &str, config: Value) -> Uuid {
    let (status, _, body) = server
        .post(
            "/api/v1/experiments",
            Some(key),
            &json!({ "config": config }),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    body["job"]["id"].as_str().unwrap().parse().unwrap()
}

async fn cancel(server: &Server, key: &str, id: Uuid) {
    let (status, _, body) = server
        .post(&format!("/api/v1/jobs/{id}/cancel"), Some(key), &json!({}))
        .await;
    assert_eq!(status, 202, "{body}");
    // The worker may already have recorded the cancel before the response view is read.
    assert!(
        body["state"] == "cancelling" || body["state"] == "cancelled",
        "{body}"
    );
}

/// Polls the job row until it is terminal (twice the lease, so a reaper-driven end is observed).
async fn terminal(db: &Database, id: Uuid) -> Job {
    let give_up = Instant::now() + Duration::from_secs(2 * LEASE_SECONDS as u64);
    loop {
        let job = db.job(id).await.unwrap();
        if matches!(
            job.state.as_str(),
            "completed" | "failed" | "cancelled" | "expired"
        ) {
            return job;
        }
        assert!(Instant::now() < give_up, "job {id} never ended: {job:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A job cancelled by its own worker: no reaper involvement, attempt closed `cancelled`, the
/// transition logged, nothing stored.
async fn assert_cancelled_by_worker(server: &Server, job: &Job) {
    assert_eq!(job.state, "cancelled", "{job:?}");
    assert_eq!(
        job.lease_losses, 0,
        "the lease reaper ended the job: {job:?}"
    );
    assert!(job.run_id.is_none() && job.error_code.is_none(), "{job:?}");
    let attempt: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT outcome, error_code FROM bb.job_attempts WHERE job_id = $1 AND attempt_no = 1",
    )
    .bind(job.id)
    .fetch_one(server.db.pool())
    .await
    .unwrap();
    assert_eq!(attempt, (Some("cancelled".into()), None), "{job:?}");
    let stored: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM bb.runs), (SELECT count(*) FROM bb.run_artifacts)",
    )
    .fetch_one(server.db.pool())
    .await
    .unwrap();
    assert_eq!(stored, (0, 0), "a cancelled job stored a run");
    let line = logged(job.id, "cancelled").expect("no `cancelled` log line");
    assert!(line.contains("\"component\":\"worker\""), "{line}");
}

async fn finish(fx: Fixture) {
    fx.server.stop().await;
    fx.tdb.drop_db().await;
}

/// Cancel while the real simulation runs: it stops at its next checkpoint (every
/// `PROGRESS_EVERY_TICKS` ticks), well before the end, and the worker records `cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_running_simulation_stops_it_and_records_cancelled_promptly() {
    let Some(fx) = fixture(1_800, None).await else {
        return;
    };
    let server = &fx.server;
    let id = submit(server, &fx.key, config_json(&fx.manifest, 1, 1)).await;
    // Wait for the simulation to report its first checkpoint.
    let give_up = Instant::now() + Duration::from_secs(60);
    let control = loop {
        if let Some(control) = server.hub.job(id)
            && control.progress().is_some_and(|p| p.done > 0)
        {
            break control;
        }
        assert!(Instant::now() < give_up, "the simulation never progressed");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let at_cancel = control.progress().unwrap();
    let asked = Instant::now();
    cancel(server, &fx.key, id).await;
    let job = terminal(&server.db, id).await;
    let recorded = asked.elapsed();
    let stopped = control.progress().unwrap();
    eprintln!(
        "mid-run cancel: {} of {} ticks done at cancel, stopped at {}; cancelled after {recorded:?}",
        at_cancel.done, at_cancel.total, stopped.done
    );
    assert!(
        stopped.done < stopped.total,
        "the simulation ran to the end: {stopped:?}"
    );
    assert!(recorded < PROMPT, "cancelled only after {recorded:?}");
    assert_cancelled_by_worker(server, &job).await;
    finish(fx).await;
}

/// Cancel after the simulation finished, while the attempt is about to complete: the completion
/// is refused (`cancelling`), and instead of silently abandoning the attempt to the lease reaper
/// the worker records `cancelled` at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_after_the_simulation_finished_records_cancelled_promptly() {
    let Some(fx) = fixture(60, None).await else {
        return;
    };
    let server = &fx.server;
    let hold = Arc::new(AtCompletion::default());
    server.hub.inject_faults(hold.clone());
    let id = submit(server, &fx.key, config_json(&fx.manifest, 100, 2)).await;
    let give_up = Instant::now() + Duration::from_secs(120);
    while !hold.reached.load(Ordering::SeqCst) {
        assert!(Instant::now() < give_up, "the job never reached completion");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let asked = Instant::now();
    cancel(server, &fx.key, id).await;
    hold.go.store(true, Ordering::SeqCst);
    let job = terminal(&server.db, id).await;
    let recorded = asked.elapsed();
    eprintln!("post-simulation cancel: cancelled after {recorded:?}");
    assert!(recorded < PROMPT, "cancelled only after {recorded:?}");
    assert_cancelled_by_worker(server, &job).await;
    finish(fx).await;
}

/// A cancellation that races either store response must win even when the response is an
/// error. Failed outcome recording must not leave `cancelling` to the lease reaper.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_during_upload_or_readback_records_cancelled_even_on_store_failure() {
    for step in [StoreStep::Upload, StoreStep::Readback] {
        for fail in [false, true] {
            let objects = tempfile::tempdir().unwrap();
            let store = PausedStore::new(objects.path(), step, fail);
            let storage = Storage::filesystem(store.clone());
            let Some(fx) = fixture(20, Some(storage)).await else {
                return;
            };
            let id = submit(&fx.server, &fx.key, config_json(&fx.manifest, 100, 3)).await;
            store.wait_reached().await;
            let asked = Instant::now();
            cancel(&fx.server, &fx.key, id).await;
            store.go.add_permits(1);
            let job = terminal(&fx.server.db, id).await;
            assert!(asked.elapsed() < PROMPT, "{step:?}, fail={fail}: {job:?}");
            assert_cancelled_by_worker(&fx.server, &job).await;
            assert_eq!(
                store.gets.load(Ordering::SeqCst),
                usize::from(step == StoreStep::Readback),
                "cancelled upload must not spend a read-back request"
            );
            let intents: i64 =
                sqlx::query_scalar("SELECT count(*) FROM bb.blob_gc WHERE reason='upload_intent'")
                    .fetch_one(fx.server.db.pool())
                    .await
                    .unwrap();
            assert_eq!(
                intents, 1,
                "the uncommitted upload must remain eligible for GC"
            );
            finish(fx).await;
        }
    }
}

/// Once a heartbeat has fenced an upload's lease, its late response must not spend a read
/// budget or create a run. The orphan still has an upload intent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_the_lease_during_upload_skips_readback_and_completion() {
    let objects = tempfile::tempdir().unwrap();
    let store = PausedStore::new(objects.path(), StoreStep::Upload, false);
    let Some(fx) = fixture(20, Some(Storage::filesystem(store.clone()))).await else {
        return;
    };
    let id = submit(&fx.server, &fx.key, config_json(&fx.manifest, 100, 4)).await;
    store.wait_reached().await;
    let control = fx.server.hub.job(id).unwrap();
    sqlx::query("UPDATE bb.jobs SET lease_expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(fx.server.db.pool())
        .await
        .unwrap();
    let give_up = Instant::now() + PROMPT;
    while !control.should_cancel() {
        assert!(
            Instant::now() < give_up,
            "heartbeat did not fence the lease"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    store.go.add_permits(1);
    while fx.server.hub.job(id).is_some() {
        assert!(Instant::now() < give_up, "the stale attempt never stopped");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(store.gets.load(Ordering::SeqCst), 0);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM bb.runs), (SELECT count(*) FROM bb.run_artifacts), \
         (SELECT count(*) FROM bb.blob_gc WHERE reason='upload_intent')",
    )
    .fetch_one(fx.server.db.pool())
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 1));
    finish(fx).await;
}
