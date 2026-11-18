//! SEC-17 forced-failure matrix through the real worker: an injected [`FaultInjector`] (the
//! test-only seam), a failing artefact store and real database manipulation make individual job
//! steps fail. For each case the persisted job state, the generic error text, the absence of
//! partial artefacts and duplicate runs, and the captured logs (SEC-18) are checked.
mod common;

use std::{
    collections::HashSet,
    io::Write,
    path::Path,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use bb_app::{
    artifacts::{ReadBudget, Storage},
    config::{Profile, Secret, StorageConfig},
    error::AppError,
    runtime,
    worker::{DbStep, FaultInjector, JobControl},
};
use bb_db::{Database, Job, Scope};
use bb_sim::RunControl;
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

/// Every sentinel contains this marker; it must never reach a log line, a job row or a body.
const MARKER: &str = "S3NT1NEL";
/// Fake credentials, spelled so `scripts/ci/secret-scan.sh` does not take them for real ones.
const DB_URL: &str = concat!(
    "postgres",
    "://ops:S3NT1NEL-db-pw-41c2@db.prod.example.net/bb?sslmode=verify-full"
);
const BLOB_TOKEN: &str = "vercel_blob_rw_S3NT1NELworkertoken41c2";

const SIMULATION_FAILED: &str = "the simulation failed; the details are in the server log";
const INTERNAL: &str = "internal worker error";

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
    let default_hook = std::panic::take_hook();
    runtime::init_tracing_with(Profile::Hosted, move || writer.clone());
    let app_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        app_hook(info);
        default_hook(info);
    }));
    sink
});

fn captured() -> String {
    String::from_utf8_lossy(&LOGS.0.lock().unwrap()).into_owned()
}

fn assert_no_sentinel(what: &str, text: &str) {
    assert!(
        !text.contains(MARKER),
        "{what} leaked a sentinel secret:\n{text}"
    );
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Sim {
    #[default]
    Real,
    Panic,
    EngineError,
    BudgetBreach,
    /// Waits until released or until the worker asks the simulation to stop.
    Hold,
}

/// The injected faults of one server.
#[derive(Default)]
struct Faults {
    sim: Mutex<Sim>,
    db: Mutex<Option<DbStep>>,
    release: AtomicBool,
    holding: AtomicUsize,
}

impl Faults {
    fn set(&self, sim: Sim, db: Option<DbStep>) {
        *self.sim.lock().unwrap() = sim;
        *self.db.lock().unwrap() = db;
        self.release.store(false, Ordering::SeqCst);
    }
}

impl FaultInjector for Faults {
    fn before_simulation(&self, control: &JobControl) -> Result<(), AppError> {
        let sim = *self.sim.lock().unwrap();
        match sim {
            Sim::Real => Ok(()),
            Sim::Panic => panic!("simulator exploded; book dump uses {DB_URL} and {BLOB_TOKEN}"),
            Sim::EngineError => Err(AppError::SimulationFailed(format!(
                "matching engine invariant broken while reading {DB_URL} with {BLOB_TOKEN}"
            ))),
            Sim::BudgetBreach => Err(AppError::BudgetExceeded(
                "the run exceeded max_tape_events (10) at tick 42; nothing was stored".into(),
            )),
            Sim::Hold => {
                self.holding.fetch_add(1, Ordering::SeqCst);
                while !self.release.load(Ordering::SeqCst) && !control.should_cancel() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                self.holding.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    fn database(&self, step: DbStep, _job: Uuid) -> Option<bb_db::Error> {
        (*self.db.lock().unwrap() == Some(step)).then(|| {
            bb_db::Error::Database(sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                format!("connection to {DB_URL} reset by peer"),
            )))
        })
    }
}

/// A filesystem store whose uploads or reads fail with a transport error echoing a credential.
struct FaultyStore {
    inner: bb_artifacts::FsArtifactStore,
    fail_put: AtomicBool,
    fail_get: AtomicBool,
}

fn transport_error(op: &str) -> bb_artifacts::ArtifactError {
    bb_artifacts::ArtifactError::Io(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        format!("{op} https://store:{BLOB_TOKEN}@blob.example/runs: connection reset"),
    ))
}

#[async_trait::async_trait]
impl bb_artifacts::ArtifactStore for FaultyStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<bb_artifacts::ArtifactHead, bb_artifacts::ArtifactError> {
        if self.fail_put.load(Ordering::SeqCst) {
            return Err(transport_error("PUT"));
        }
        self.inner.put_bundle_from_file(key, path).await
    }
    async fn get_range(
        &self,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<bytes::Bytes, bb_artifacts::ArtifactError> {
        if self.fail_get.load(Ordering::SeqCst) {
            return Err(transport_error("GET"));
        }
        self.inner.get_range(key, range).await
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

/// Options of a server whose configuration holds the sentinel secrets. The configured Blob token
/// is never used: every server here gets an explicit local `storage`.
fn options(storage: Storage, deadline_ms: Option<i32>) -> Options {
    Options {
        slots: 1,
        storage: Some(storage),
        tweak: Box::new(move |cfg| {
            cfg.database_url_unpooled = Some(Secret::new(DB_URL));
            cfg.storage = StorageConfig::VercelBlob(Secret::new(BLOB_TOKEN));
            if let Some(ms) = deadline_ms {
                cfg.deadline_ms = ms;
            }
        }),
    }
}

fn local_storage(objects: &Path) -> Storage {
    Storage::filesystem(Arc::new(
        bb_artifacts::FsArtifactStore::new(objects).unwrap(),
    ))
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

async fn wait_job(db: &Database, id: Uuid, done: impl Fn(&Job) -> bool) -> Job {
    for _ in 0..1200 {
        let job = db.job(id).await.unwrap();
        if done(&job) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "job {id} never reached the expected state: {:?}",
        db.job(id).await
    );
}

fn terminal(job: &Job) -> bool {
    matches!(
        job.state.as_str(),
        "completed" | "failed" | "cancelled" | "expired"
    )
}

async fn wait_holding(faults: &Faults) {
    for _ in 0..1200 {
        if faults.holding.load(Ordering::SeqCst) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the simulation never started");
}

/// Stored runs and run artefact rows for the job's experiment configuration.
async fn stored(db: &Database, job: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(DISTINCT r.id), count(a.id) FROM bb.jobs j \
         JOIN bb.runs r ON r.experiment_config_id = j.experiment_config_id \
         LEFT JOIN bb.run_artifacts a ON a.run_id = r.id WHERE j.id = $1",
    )
    .bind(job)
    .fetch_one(db.pool())
    .await
    .unwrap()
}

/// A failed job: exact category, code and message, nothing stored, no secret anywhere.
async fn assert_failed(
    server: &Server,
    key: &str,
    job: &Job,
    category: &str,
    code: &str,
    message: &str,
) {
    assert_eq!(job.state, "failed", "{job:?}");
    assert_eq!(job.error_category.as_deref(), Some(category), "{job:?}");
    assert_eq!(job.error_code.as_deref(), Some(code), "{job:?}");
    assert_eq!(job.error_message.as_deref(), Some(message), "{job:?}");
    assert!(job.run_id.is_none(), "{job:?}");
    assert_eq!(job.claim_count, 1, "{job:?}");
    assert_eq!(stored(&server.db, job.id).await, (0, 0), "{job:?}");
    let (status, _, body) = server
        .get(&format!("/api/v1/jobs/{}", job.id), Some(key))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "failed", "{body}");
    assert_no_sentinel("job API body", &body.to_string());
}

/// The log line of `job` carrying `reason`, with the safe fields present.
fn assert_logged(job: Uuid, reason: &str) -> String {
    let logs = captured();
    let line = logs
        .lines()
        .find(|l| {
            l.contains(&format!("\"job_id\":\"{job}\""))
                && l.contains(&format!("\"reason\":\"{reason}\""))
        })
        .unwrap_or_else(|| panic!("no `{reason}` log line for job {job}:\n{logs}"))
        .to_string();
    assert!(line.contains("\"component\":"), "{line}");
    line
}

async fn upload_intents(db: &Database) -> HashSet<String> {
    sqlx::query_scalar("SELECT storage_key FROM bb.blob_gc WHERE reason = 'upload_intent'")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect()
}

fn files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

/// No partial artefact survives a failure: every cached bundle and every stored object belongs
/// to a committed run, or (stored objects only) is recorded for garbage collection.
async fn assert_no_partial_artifacts(db: &Database, data_dir: &Path, objects: &Path) {
    let committed: Vec<(String, String)> =
        sqlx::query_as("SELECT storage_key, encode(sha256, 'hex') FROM bb.run_artifacts")
            .fetch_all(db.pool())
            .await
            .unwrap();
    let digests: HashSet<String> = committed.iter().map(|(_, d)| format!("{d}.bbz")).collect();
    let keys: HashSet<String> = committed.into_iter().map(|(k, _)| k).collect();
    // A dropped temp file can stay visible for a moment on Windows while another process (an
    // anti-virus scanner) still holds it open; a real leak stays.
    let cache = data_dir.join("cache").join("bundles");
    let uncommitted = || -> Vec<String> {
        files(&cache)
            .into_iter()
            .filter(|f| !digests.contains(f))
            .collect()
    };
    for _ in 0..100 {
        if uncommitted().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let left = uncommitted();
    assert!(
        left.is_empty(),
        "uncommitted bundles left in the cache: {left:?}"
    );
    let intents = upload_intents(db).await;
    for key in files(objects) {
        assert!(
            keys.contains(&key) || intents.contains(&key),
            "object {key} is neither a committed artefact nor recorded for collection"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_job_failure_class_persists_generic_state_and_stores_nothing() {
    LazyLock::force(&LOGS);
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    let pool = tdb.pool().await;
    let store = Arc::new(FaultyStore {
        inner: bb_artifacts::FsArtifactStore::new(&objects).unwrap(),
        fail_put: AtomicBool::new(false),
        fail_get: AtomicBool::new(false),
    });
    let storage = Storage::metered(
        store.clone(),
        "vercel_blob",
        ReadBudget::new(pool.clone(), 1_000_000, i64::MAX),
    );
    let server = Server::start(&tdb, dir.path(), options(storage, None)).await;
    let faults = Arc::new(Faults::default());
    server.hub.inject_faults(faults.clone());
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (ws, key) = workspace_key(&server.db, "alpha", &[Scope::Read, Scope::Run]).await;
    let config = |seed: u64| config_json(&manifest, 100, seed);
    let db = &server.db;

    // Simulator panic.
    faults.set(Sim::Panic, None);
    let id = submit(&server, &key, config(1)).await;
    let job = wait_job(db, id, terminal).await;
    assert_failed(
        &server,
        &key,
        &job,
        "simulation_failed",
        "simulation_failed",
        SIMULATION_FAILED,
    )
    .await;
    let line = assert_logged(id, "simulation_failed");
    assert!(line.contains("simulator exploded"), "{line}");
    assert!(line.contains("<redacted>"), "{line}");
    let logs = captured();
    let panic_line = logs
        .lines()
        .find(|l| l.contains("thread panicked") && l.contains("simulator exploded"))
        .unwrap_or_else(|| panic!("no panic event:\n{logs}"));
    assert!(panic_line.contains("worker_faults.rs:"), "{panic_line}");

    // Engine error.
    faults.set(Sim::EngineError, None);
    let id = submit(&server, &key, config(2)).await;
    let job = wait_job(db, id, terminal).await;
    assert_failed(
        &server,
        &key,
        &job,
        "simulation_failed",
        "simulation_failed",
        SIMULATION_FAILED,
    )
    .await;
    assert!(assert_logged(id, "simulation_failed").contains("\"error_class\":\"engine\""));

    // Engine budget breach: the bounded validation message is kept.
    faults.set(Sim::BudgetBreach, None);
    let id = submit(&server, &key, config(3)).await;
    let job = wait_job(db, id, terminal).await;
    assert_failed(
        &server,
        &key,
        &job,
        "budget_exceeded",
        "budget_exceeded",
        "the run exceeded max_tape_events (10) at tick 42; nothing was stored",
    )
    .await;
    assert_logged(id, "budget_exceeded");

    // Database unavailable at the first query of the attempt.
    faults.set(Sim::Real, Some(DbStep::ReadConfig));
    let id = submit(&server, &key, config(4)).await;
    let job = wait_job(db, id, terminal).await;
    assert_failed(&server, &key, &job, "internal", "internal", INTERNAL).await;
    assert!(assert_logged(id, "config_unreadable").contains("\"error_class\":\"database\""));

    // Database unavailable at the completion transaction: the uploaded object stays recorded
    // for garbage collection, no run row exists.
    faults.set(Sim::Real, Some(DbStep::Complete));
    let id = submit(&server, &key, config(5)).await;
    let job = wait_job(db, id, terminal).await;
    assert_failed(&server, &key, &job, "internal", "internal", INTERNAL).await;
    assert_logged(id, "completion_failed");

    // Artefact transport failure on upload.
    faults.set(Sim::Real, None);
    store.fail_put.store(true, Ordering::SeqCst);
    let id = submit(&server, &key, config(6)).await;
    let job = wait_job(db, id, terminal).await;
    store.fail_put.store(false, Ordering::SeqCst);
    assert_failed(
        &server,
        &key,
        &job,
        "storage_failed",
        "storage_failed",
        "the bundle could not be stored; resubmit to retry",
    )
    .await;
    let line = assert_logged(id, "upload_failed");
    assert!(line.contains("\"component\":\"storage\""), "{line}");
    assert!(line.contains("https://<redacted>@blob.example"), "{line}");

    // Artefact transport failure on read-back.
    store.fail_get.store(true, Ordering::SeqCst);
    let id = submit(&server, &key, config(7)).await;
    let job = wait_job(db, id, terminal).await;
    store.fail_get.store(false, Ordering::SeqCst);
    assert_failed(
        &server,
        &key,
        &job,
        "storage_failed",
        "storage_failed",
        "the stored bundle could not be read back; resubmit to retry",
    )
    .await;
    assert_logged(id, "read_back_failed");

    // Storage budget refusal: the workspace's upload share is spent while the job runs.
    faults.set(Sim::Hold, None);
    let id = submit(&server, &key, config(8)).await;
    wait_holding(&faults).await;
    sqlx::query(
        "UPDATE bb.workspaces SET quota_uploads_per_month = (SELECT coalesce(sum(attempts), 0) \
         FROM bb.workspace_uploads_monthly WHERE workspace_id = $1) WHERE id = $1",
    )
    .bind(ws)
    .execute(db.pool())
    .await
    .unwrap();
    faults.release.store(true, Ordering::SeqCst);
    let job = wait_job(db, id, terminal).await;
    assert_failed(
        &server,
        &key,
        &job,
        "storage_failed",
        "storage_budget",
        "the monthly storage upload allowance is spent; nothing was stored",
    )
    .await;
    assert_logged(id, "storage_budget");
    sqlx::query("UPDATE bb.workspaces SET quota_uploads_per_month = 1000 WHERE id = $1")
        .bind(ws)
        .execute(db.pool())
        .await
        .unwrap();

    // Invalid dataset: the worker's copy of a registered dataset is corrupt.
    faults.set(Sim::Real, None);
    let (version, corrupt) = bb_app::registry::register_synthetic(
        db,
        &bb_app::registry::DatasetRegistry::new(&dir.path().join("elsewhere")),
        &bb_data::synthetic::SyntheticConfig {
            id: "corrupt-synth".into(),
            duration_ns: 10_000_000_000,
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    let sha = hex::encode(&version.events_sha256);
    let copy = dir
        .path()
        .join("registry")
        .join(&sha[..16])
        .join("corrupt-synth");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("dataset.json"), b"{ corrupted").unwrap();
    let id = submit(&server, &key, config_json(corrupt.manifest(), 100, 9)).await;
    let job = wait_job(db, id, terminal).await;
    assert_failed(
        &server,
        &key,
        &job,
        "internal",
        "dataset_unavailable",
        INTERNAL,
    )
    .await;
    assert!(assert_logged(id, "dataset_unavailable").contains("\"error_class\":\"dataset\""));

    assert_no_partial_artifacts(db, dir.path(), &objects).await;

    // Cancellation of a running job: cancelled, nothing stored.
    faults.set(Sim::Hold, None);
    let id = submit(&server, &key, config(10)).await;
    wait_holding(&faults).await;
    let (status, _, body) = server
        .post(&format!("/api/v1/jobs/{id}/cancel"), Some(&key), &json!({}))
        .await;
    assert_eq!(status, 202, "{body}");
    let job = wait_job(db, id, terminal).await;
    assert_eq!(job.state, "cancelled", "{job:?}");
    assert!(job.error_code.is_none() && job.run_id.is_none(), "{job:?}");
    assert_eq!(stored(db, id).await, (0, 0));
    assert_logged(id, "released");

    // Lease expiration while running: the attempt is abandoned (no outcome written by the
    // stale worker), the job is re-queued once, and the retry stores exactly one run.
    faults.set(Sim::Hold, None);
    let id = submit(&server, &key, config(11)).await;
    wait_holding(&faults).await;
    sqlx::query("UPDATE bb.jobs SET lease_expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let job = wait_job(db, id, |j| j.lease_losses == 1 && j.state == "queued").await;
    assert!(job.error_code.is_none() && job.run_id.is_none(), "{job:?}");
    assert_eq!(stored(db, id).await, (0, 0));
    assert_logged(id, "lease_lost");
    faults.set(Sim::Real, None);
    retry_now(&server, id).await;
    let job = wait_job(db, id, terminal).await;
    assert_eq!(job.state, "completed", "{job:?}");
    assert_eq!(job.claim_count, 2, "{job:?}");
    assert_eq!(stored(db, id).await, (1, 1));

    // Resubmitting the same configuration reuses the run: no duplicate result.
    let (status, _, body) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({ "config": config(11) }),
        )
        .await;
    assert!(status == 200 || status == 202, "{body}");
    assert_eq!(body["created"], false, "{body}");
    assert!(body["run"].is_object(), "{body}");
    assert_eq!(stored(db, id).await, (1, 1));

    // Database unavailable when the outcome is recorded: no false failure or success; the
    // lease expires, the job is retried and completes once.
    faults.set(Sim::EngineError, Some(DbStep::RecordOutcome));
    let id = submit(&server, &key, config(12)).await;
    let job = wait_job(db, id, |j| j.lease_losses == 1 && j.state == "queued").await;
    assert!(job.error_code.is_none() && job.run_id.is_none(), "{job:?}");
    assert_eq!(stored(db, id).await, (0, 0));
    assert_logged(id, "outcome_not_recorded");
    faults.set(Sim::Real, None);
    retry_now(&server, id).await;
    let job = wait_job(db, id, terminal).await;
    assert_eq!(job.state, "completed", "{job:?}");
    assert_eq!(job.claim_count, 2, "{job:?}");
    assert_eq!(stored(db, id).await, (1, 1));

    assert_no_partial_artifacts(db, dir.path(), &objects).await;
    server.stop().await;

    // Runtime budget breach: the per-attempt deadline expires the attempt, nothing stored.
    let server = Server::start(
        &tdb,
        dir.path(),
        options(local_storage(&objects), Some(1_500)),
    )
    .await;
    let faults = Arc::new(Faults::default());
    server.hub.inject_faults(faults.clone());
    faults.set(Sim::Hold, None);
    let id = submit(&server, &key, config(13)).await;
    let job = wait_job(&server.db, id, terminal).await;
    assert_eq!(job.state, "expired", "{job:?}");
    assert!(
        job.expired_reason
            .as_deref()
            .unwrap_or("")
            .contains("deadline"),
        "{job:?}"
    );
    assert!(job.run_id.is_none(), "{job:?}");
    assert_eq!(stored(&server.db, id).await, (0, 0));
    assert_logged(id, "deadline_exceeded");
    server.stop().await;

    // Process restart: worker A freezes (simulation stuck, heartbeats failing) and loses its
    // lease; a restarted process B reclaims and completes the job; when A wakes up its late
    // completion is refused. Exactly one run, no false success from A.
    let a = Server::start(&tdb, dir.path(), options(local_storage(&objects), None)).await;
    let frozen = Arc::new(Faults::default());
    a.hub.inject_faults(frozen.clone());
    frozen.set(Sim::Hold, Some(DbStep::Heartbeat));
    let id = submit(&a, &key, config(14)).await;
    wait_holding(&frozen).await;
    let b = Server::start(&tdb, dir.path(), options(local_storage(&objects), None)).await;
    wait_job(&b.db, id, |j| j.lease_losses == 1 && j.state == "queued").await;
    assert_logged(id, "heartbeat_failed");
    retry_now(&b, id).await;
    let job = wait_job(&b.db, id, terminal).await;
    assert_eq!(job.state, "completed", "{job:?}");
    assert_eq!(job.claim_count, 2, "{job:?}");
    let run_id = job.run_id;
    frozen.release.store(true, Ordering::SeqCst);
    for _ in 0..600 {
        if a.hub.status().busy == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        a.hub.status().busy,
        0,
        "worker A never finished its stale attempt"
    );
    let job = b.db.job(id).await.unwrap();
    assert_eq!(job.state, "completed", "{job:?}");
    assert_eq!(job.run_id, run_id, "{job:?}");
    assert_eq!(stored(&b.db, id).await, (1, 1));
    a.stop().await;
    b.stop().await;
    assert_no_partial_artifacts(&pool, dir.path(), &objects).await;

    let logs = captured();
    assert_no_sentinel("captured logs", &logs);
    let rows: Vec<Option<String>> = sqlx::query_scalar("SELECT error_message FROM bb.jobs")
        .fetch_all(pool.pool())
        .await
        .unwrap();
    for message in rows.into_iter().flatten() {
        assert_no_sentinel("persisted job error", &message);
    }
    pool.pool().close().await;
    tdb.drop_db().await;
}

/// Makes a re-queued job runnable now (the reaper backs off 30 s) and wakes the worker.
async fn retry_now(server: &Server, id: Uuid) {
    sqlx::query("UPDATE bb.jobs SET run_after = now() WHERE id = $1")
        .bind(id)
        .execute(server.db.pool())
        .await
        .unwrap();
    server.hub.submitted();
}
