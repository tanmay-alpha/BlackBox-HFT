//! Cancellation of a running job through the real worker and Postgres, at both ends of the
//! attempt: while the simulation runs (it stops at its next checkpoint) and after it finished
//! (the completion is refused and the attempt is recorded `cancelled`). Either way the job reaches
//! `cancelled` promptly — never by waiting for the lease reaper — the transition is logged and
//! nothing is stored.
mod common;

use std::{
    io::Write,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bb_app::{
    config::Profile,
    runtime,
    worker::{DbStep, FaultInjector},
};
use bb_db::{Database, Job, Scope};
use common::*;
use serde_json::{Value, json};
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

struct Fixture {
    tdb: TestDb,
    _dir: tempfile::TempDir,
    server: Server,
    key: String,
    manifest: bb_core::dataset::DatasetManifest,
}

async fn fixture(dataset_seconds: i64) -> Option<Fixture> {
    LazyLock::force(&LOGS);
    let tdb = TestDb::create().await?;
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
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
    let Some(fx) = fixture(1_800).await else {
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
    let Some(fx) = fixture(60).await else {
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
