//! FINAL_AUDIT §4 items 5–7 against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`, else skip):
//! row digests on DB-only list paths, the anonymous demo snapshot, quotas, the per-workspace
//! upload reservation and transactional audit rows.

mod common;

use bb_db::{Database, Scope};
use common::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

macro_rules! db_or_skip {
    () => {
        match TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}

/// Adds `delta` to a run's stored `metrics.net_pnl` behind the immutability trigger, as an
/// attacker with database write access would; returns the new value.
async fn tamper_net_pnl(db: &Database, run_id: &str, delta: f64) -> f64 {
    let pool = db.pool();
    sqlx::query("ALTER TABLE bb.runs DISABLE TRIGGER runs_immutable")
        .execute(pool)
        .await
        .unwrap();
    let (value,): (f64,) = sqlx::query_as("UPDATE bb.runs r SET metrics=jsonb_set(metrics,'{net_pnl}',to_jsonb((metrics->>'net_pnl')::float8+$2)) FROM bb.experiment_configs c WHERE c.id=r.experiment_config_id AND c.short_id=$1 AND r.quarantined_at IS NULL RETURNING (r.metrics->>'net_pnl')::float8")
        .bind(run_id)
        .bind(delta)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE bb.runs ENABLE TRIGGER runs_immutable")
        .execute(pool)
        .await
        .unwrap();
    value
}

async fn quarantined(db: &Database, run_id: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.runs r JOIN bb.experiment_configs c ON c.id=r.experiment_config_id WHERE c.short_id=$1 AND r.quarantined_at IS NOT NULL)")
        .bind(run_id)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

fn items(page: &Value) -> &Vec<Value> {
    page["items"].as_array().expect("a page")
}

/// No list item is the tampered run's summary and no list body carries the tampered value.
/// (Experiments name their identity `run_id` too; only an embedded summary would leak it.)
fn assert_absent(page: &Value, run_id: &str, tampered: f64, path: &str) {
    for item in items(page) {
        if item.get("metrics").is_some() {
            assert_ne!(item["run_id"], run_id, "{path} lists the tampered run");
        }
        assert_ne!(
            item["run"]["run_id"], run_id,
            "{path} embeds the tampered run"
        );
    }
    assert!(
        !page.to_string().contains(&tampered.to_string()),
        "{path} serves the tampered net_pnl {tampered}"
    );
}

/// FINAL_AUDIT §4 item 5: a changed `metrics.net_pnl` in a run row cannot escape through any
/// list path (runs in every order and filter, experiments, comparisons, reports, the anonymous
/// demo lists); the first list read quarantines the run and invalidates the anonymous caches.
/// Changed comparison and report rows are left out of lists and refused on detail reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tampered_summary_rows_never_escape_a_list() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let (_, admin) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;
    let (run1, _) = run(&server, &key, config_json(&manifest, 100, 1), "kept").await;
    let (run2, _) = run(&server, &key, config_json(&manifest, 200, 1), "tampered").await;
    let (status, _, comparison) = server
        .post(
            "/api/v1/comparisons",
            Some(&key),
            &json!({"run_a": run1, "run_b": run2}),
        )
        .await;
    assert_eq!(status, 201, "{comparison}");
    let comparison_id = comparison["id"].as_str().unwrap().to_string();
    let (status, _, report) = server
        .post(
            "/api/v1/reports",
            Some(&key),
            &json!({"kind": "comparison", "subject_id": comparison_id, "title": "pair"}),
        )
        .await;
    assert_eq!(status, 201, "{report}");
    let report_id = report["id"].as_str().unwrap().to_string();
    // A demo run, listed anonymously before the change (warms the anonymous caches).
    let (demo_run, _) = run(&server, &admin, config_json(&manifest, 300, 2), "public").await;
    let (status, _, anonymous) = server.get("/api/v1/runs", None).await;
    assert_eq!(status, 200);
    assert!(items(&anonymous).iter().any(|r| r["run_id"] == demo_run));

    let tampered = tamper_net_pnl(&server.db, &run2, 1000.25).await;
    let demo_tampered = tamper_net_pnl(&server.db, &demo_run, 2000.25).await;
    for path in [
        "/api/v1/runs".to_string(),
        "/api/v1/runs?sort=net_pnl_desc".into(),
        "/api/v1/runs?sort=net_pnl_asc".into(),
        "/api/v1/runs?sort=max_drawdown_asc".into(),
        format!("/api/v1/runs?run_id={run2}"),
        "/api/v1/runs?label_prefix=tamp".into(),
        "/api/v1/runs?limit=1".into(),
        "/api/v1/experiments".into(),
        "/api/v1/experiments?status=completed".into(),
        "/api/v1/comparisons".into(),
        "/api/v1/reports".into(),
    ] {
        let (status, _, page) = server.get(&path, Some(&key)).await;
        assert_eq!(status, 200, "{path}: {page}");
        assert_absent(&page, &run2, tampered, &path);
    }
    assert!(quarantined(&server.db, &run2).await);
    let (_, _, experiments) = server.get("/api/v1/experiments", Some(&key)).await;
    let changed = items(&experiments)
        .iter()
        .find(|e| e["label"] == "tampered")
        .expect("the experiment stays listed");
    assert!(changed["run"].is_null(), "{changed}");
    let (status, _, body) = server
        .get(&format!("/api/v1/runs/{run2}"), Some(&key))
        .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "integrity_failed");
    // The stored comparison was computed from verified data and still lists; the kept run too.
    let (_, _, runs) = server.get("/api/v1/runs", Some(&key)).await;
    assert!(items(&runs).iter().any(|r| r["run_id"] == run1));
    let (_, _, comparisons) = server.get("/api/v1/comparisons", Some(&key)).await;
    assert_eq!(items(&comparisons).len(), 1);

    // Anonymous demo lists: the quarantine above changed the read version, so the warmed
    // response is not served again; the changed demo run is quarantined on this read.
    for path in [
        "/api/v1/runs",
        "/api/v1/runs?sort=net_pnl_desc",
        "/api/v1/experiments",
    ] {
        let (status, _, page) = server.get(path, None).await;
        assert_eq!(status, 200, "{path}: {page}");
        assert_absent(&page, &demo_run, demo_tampered, path);
    }
    assert!(quarantined(&server.db, &demo_run).await);

    // A changed comparison row and a changed report body.
    let pool = server.db.pool();
    for sql in [
        "ALTER TABLE bb.comparisons DISABLE TRIGGER comparisons_immutable",
        "UPDATE bb.comparisons SET summary=summary||'{\"note\":\"edited\"}'::jsonb",
        "ALTER TABLE bb.comparisons ENABLE TRIGGER comparisons_immutable",
        "ALTER TABLE bb.research_reports DISABLE TRIGGER research_reports_immutable",
        "UPDATE bb.research_reports SET body=body||' edited'",
        "ALTER TABLE bb.research_reports ENABLE TRIGGER research_reports_immutable",
    ] {
        sqlx::query(sql).execute(pool).await.unwrap();
    }
    let (_, _, comparisons) = server.get("/api/v1/comparisons", Some(&key)).await;
    assert!(items(&comparisons).is_empty(), "{comparisons}");
    let (status, _, body) = server
        .get(&format!("/api/v1/comparisons/{comparison_id}"), Some(&key))
        .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "integrity_failed");
    let (_, _, reports) = server.get("/api/v1/reports", Some(&key)).await;
    assert!(items(&reports).is_empty(), "{reports}");
    let (status, _, body) = server
        .get(&format!("/api/v1/reports/{report_id}"), Some(&key))
        .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "integrity_failed");
    server.stop().await;
    tdb.drop_db().await;
}

/// A run whose stored row changed after its sweep finished is left out of the sweep view and
/// the sweep report (`sweep_cell_views` checks `summary_row_sha256`): no run id, no metrics and
/// an integrity error for its cell instead of a value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sweep_views_and_reports_leave_out_tampered_runs() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let spec = json!({
        "base": config_json(&manifest, 250, 11),
        "design": {"type": "seeds", "count": 3},
    });
    let (status, _, body) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": spec}))
        .await;
    assert_eq!(status, 202, "{body}");
    let path = format!("/api/v1/sweeps/{}", body["sweep"]["id"].as_str().unwrap());
    let mut sweep = Value::Null;
    for _ in 0..1200 {
        sweep = server.get(&path, Some(&key)).await.2;
        if sweep["report_ready"] == true {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert_eq!(sweep["state"], "completed", "{sweep}");
    let run_of = |s: &Value, cell: usize| s["cells"][cell]["runs"][0]["run_id"].clone();
    let changed = run_of(&sweep, 0).as_str().unwrap().to_string();
    let kept = run_of(&sweep, 1).as_str().unwrap().to_string();
    tamper_net_pnl(&server.db, &changed, 3000.25).await;

    let (status, _, sweep) = server.get(&path, Some(&key)).await;
    assert_eq!(status, 200, "{sweep}");
    let cell = &sweep["cells"][0]["runs"][0];
    assert!(
        cell["run_id"].is_null() && cell["metrics"].is_null(),
        "{cell}"
    );
    assert_eq!(run_of(&sweep, 1), json!(kept));
    let (status, _, report) = server.get(&format!("{path}/report"), Some(&key)).await;
    assert_eq!(status, 200, "{report}");
    let cell = &report["cells"][0];
    assert!(
        cell["in_sample"].is_null() && cell["in_sample_run_id"].is_null(),
        "{cell}"
    );
    assert!(
        cell["error"].as_str().unwrap().contains("integrity"),
        "{cell}"
    );
    assert!(!report.to_string().contains(&changed), "{report}");
    assert!(report.to_string().contains(&kept));
    server.stop().await;
    tdb.drop_db().await;
}

/// FINAL_AUDIT §4 item 6 (SEC-10): anonymous demo lists and catalogues are answered from the
/// in-memory demo snapshot. Its pages equal the database path's for the same query, and after
/// warm-up 1,000 anonymous requests with varied query strings (filters, sorts, cursors, limits,
/// unknown parameters, missing datasets) add no database acquisition.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warm_anonymous_demo_reads_with_varied_queries_use_no_database() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, admin) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;
    let mut runs = Vec::new();
    for (i, label) in ["demo a", "demo b", "other c"].into_iter().enumerate() {
        let config = config_json(&manifest, 100 * (i as i64 + 1), 3);
        runs.push(run(&server, &admin, config, label).await.0);
    }
    let (status, _, body) = server
        .post(
            "/api/v1/comparisons",
            Some(&admin),
            &json!({"run_a": runs[0], "run_b": runs[1]}),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let (status, _, body) = server
        .post(
            "/api/v1/reports",
            Some(&admin),
            &json!({"kind": "run", "subject_id": runs[0], "title": "A"}),
        )
        .await;
    assert_eq!(status, 201, "{body}");

    // The snapshot answers exactly what the database path answers (the admin key reads the
    // demo workspace through the database).
    for path in [
        "/api/v1/runs?workspace=demo",
        "/api/v1/runs?workspace=demo&sort=net_pnl_desc",
        "/api/v1/runs?workspace=demo&sort=net_pnl_asc&limit=1",
        "/api/v1/runs?workspace=demo&sort=max_drawdown_asc&limit=2",
        "/api/v1/runs?workspace=demo&label_prefix=demo&limit=1",
        "/api/v1/comparisons?workspace=demo",
        "/api/v1/reports?workspace=demo",
        "/api/v1/datasets",
        "/api/v1/presets",
        "/api/v1/demos",
        "/api/v1/presets?dataset_id=test-synth",
    ] {
        let mut path = path.to_string();
        loop {
            let (status, _, anonymous) = server.get(&path, None).await;
            let (_, _, keyed) = server.get(&path, Some(&admin)).await;
            assert_eq!(status, 200, "{path}: {anonymous}");
            assert_eq!(anonymous, keyed, "{path}");
            match anonymous["next_cursor"].as_str() {
                Some(c) => path = format!("{}&cursor={c}", path.split("&cursor=").next().unwrap()),
                None => break,
            }
        }
    }
    let ids =
        |page: &Value| -> Vec<Value> { items(page).iter().map(|e| e["id"].clone()).collect() };
    for path in [
        "/api/v1/experiments?workspace=demo",
        "/api/v1/experiments?workspace=demo&status=completed&label_prefix=demo",
    ] {
        let (_, _, anonymous) = server.get(path, None).await;
        let (_, _, keyed) = server.get(path, Some(&admin)).await;
        assert_eq!(ids(&anonymous), ids(&keyed), "{path}");
    }
    let (_, _, page) = server.get("/api/v1/runs?limit=1", None).await;
    assert_eq!(items(&page).len(), 1);
    let cursor = page["next_cursor"].as_str().unwrap().to_string();

    let path = |i: usize| -> String {
        match i % 13 {
            0 => format!("/api/v1/runs?limit={}&v={i}", 1 + i % 100),
            1 => format!("/api/v1/runs?sort=net_pnl_desc&label_prefix=demo&n={i}"),
            2 => format!("/api/v1/runs?run_id={i:016x}"),
            3 => format!(
                "/api/v1/runs?sort=max_drawdown_asc&dataset_id=test-synth&created_after_ns={i}"
            ),
            4 => format!(
                "/api/v1/experiments?status=completed&limit={}&n={i}",
                1 + i % 50
            ),
            5 => format!("/api/v1/experiments?label_prefix=x{i}"),
            6 => format!("/api/v1/comparisons?limit={}&z={i}", 1 + i % 10),
            7 => format!("/api/v1/reports?workspace=demo&n={i}"),
            8 => format!("/api/v1/datasets?limit={}&n={i}", 1 + i % 100),
            9 => format!("/api/v1/presets?dataset_id=test-synth&n={i}"),
            10 => format!("/api/v1/demos?n={i}"),
            11 => format!("/api/v1/demos?dataset_id=missing-{i}"),
            _ => format!("/api/v1/runs?limit={}&cursor={cursor}", 1 + i % 3),
        }
    };
    // Warm-up until a full round needs no database: background work after the last change
    // (snapshot rebuild, egress counter load) has settled.
    let metrics = &server.metrics;
    let acquires = || bb_app::metrics::Metrics::get(&metrics.db_acquires_total);
    let flushes = || bb_app::metrics::Metrics::get(&metrics.egress_flushes_total);
    let mut settled = false;
    for _ in 0..20 {
        let (before, flushed) = (acquires(), flushes());
        for i in 0..13 {
            server.get(&path(i), None).await;
        }
        if acquires() == before && flushes() == flushed {
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
    assert!(settled, "anonymous reads kept using the database");

    let (before, flushed) = (acquires(), flushes());
    let mut bytes = 0;
    for i in 13..1013 {
        let path = path(i);
        let (status, _, body) = server.get(&path, None).await;
        let expected = if i % 13 == 11 { 404 } else { 200 };
        assert_eq!(status, expected, "{path}: {body}");
        bytes += body.to_string().len();
    }
    // The one documented exception: the egress brake persists its daily counter once per 5 MB
    // of responses (`egress_flushes_total`); a flush's write runs in the background.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let flushed = flushes() - flushed;
    assert!(
        flushed as usize <= bytes / 5_000_000 + 1,
        "{flushed} flushes for {bytes} B"
    );
    assert_eq!(
        acquires() - before,
        flushed,
        "warm anonymous reads with varied query strings used the database"
    );
    server.stop().await;
    tdb.drop_db().await;
}

/// A filesystem store whose next `fail_puts` uploads answer 503, counting every PUT it sees.
struct FlakyStore {
    inner: bb_artifacts::FsArtifactStore,
    fail_puts: AtomicUsize,
    puts: AtomicUsize,
}

#[async_trait::async_trait]
impl bb_artifacts::ArtifactStore for FlakyStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &std::path::Path,
    ) -> Result<bb_artifacts::ArtifactHead, bb_artifacts::ArtifactError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        // One upload at a time reaches this store, so load-then-decrement is exact.
        if self.fail_puts.load(Ordering::SeqCst) > 0 {
            self.fail_puts.fetch_sub(1, Ordering::SeqCst);
            return Err(bb_artifacts::ArtifactError::Http(503));
        }
        self.inner.put_bundle_from_file(key, path).await
    }
    async fn get_range(
        &self,
        key: &str,
        range: std::ops::Range<u64>,
    ) -> Result<bytes::Bytes, bb_artifacts::ArtifactError> {
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

async fn workspace_attempts(db: &Database, workspace: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT coalesce(sum(attempts),0)::bigint FROM bb.workspace_uploads_monthly WHERE workspace_id=$1")
        .bind(workspace)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// FINAL_AUDIT §4 item 7 (SEC-8/SEC-11): every Blob upload attempt, retries included, is
/// reserved against the submitting workspace's monthly share before the PUT; a spent share
/// refuses the upload (no PUT is sent, the job fails `storage_budget`) and new submissions
/// (429 `storage`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blob_uploads_are_reserved_per_attempt_against_the_workspace_share() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FlakyStore {
        inner: bb_artifacts::FsArtifactStore::new(dir.path().join("objects")).unwrap(),
        fail_puts: AtomicUsize::new(1),
        puts: AtomicUsize::new(0),
    });
    let storage = bb_app::artifacts::Storage::custom(
        store.clone(),
        "vercel_blob",
        Some((tdb.pool().await, 1_000_000, i64::MAX)),
    );
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            storage: Some(storage),
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (ws, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let workspace = server.db.workspace("alpha").await.unwrap();
    assert_eq!(workspace.quota_uploads_per_month, 300, "default share");
    sqlx::query("UPDATE bb.workspaces SET quota_uploads_per_month=3 WHERE id=$1")
        .bind(ws)
        .execute(server.db.pool())
        .await
        .unwrap();

    // One transient failure, then success: two attempts, both charged.
    run(&server, &key, config_json(&manifest, 100, 1), "first").await;
    assert_eq!(store.puts.load(Ordering::SeqCst), 2);
    assert_eq!(workspace_attempts(&server.db, ws).await, 2);

    // Admitted (2 < 3); the first attempt takes the last unit and fails, the retry is refused
    // before any PUT.
    store.fail_puts.store(5, Ordering::SeqCst);
    let (status, _, body) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config_json(&manifest, 200, 1), "label": "second"}),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    let job = wait_job(&server, Some(&key), body["job"]["id"].as_str().unwrap()).await;
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["error"]["code"], "storage_budget", "{job}");
    assert_eq!(store.puts.load(Ordering::SeqCst), 3);
    assert_eq!(workspace_attempts(&server.db, ws).await, 3);

    // The share is spent: new work is refused at admission.
    let (status, _, body) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config_json(&manifest, 300, 1), "label": "third"}),
        )
        .await;
    assert_eq!(status, 429, "{body}");
    assert_eq!(body["error"]["details"]["scope"], "storage", "{body}");
    // Another workspace still has its own share.
    store.fail_puts.store(0, Ordering::SeqCst);
    let (_, other) = workspace_key(&server.db, "beta", &[Scope::Run]).await;
    run(&server, &other, config_json(&manifest, 400, 1), "beta").await;
    server.stop().await;
    tdb.drop_db().await;
}

/// FINAL_AUDIT §4 item 7 (SEC-8): hosted default `quota_active_jobs` is 5 live admission units;
/// a sweep is one unit however many runs it has. The sweep goes through `POST /sweeps`, so every
/// run reaches `Database::submit` with its sweep id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_jobs_admit_five_units_and_a_sweep_counts_once() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    // No worker: every job stays live.
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    assert_eq!(
        server
            .db
            .workspace("alpha")
            .await
            .unwrap()
            .quota_active_jobs,
        5
    );

    // An 8-run seed ensemble (more runs than the quota) is admitted into an empty workspace.
    let spec = json!({
        "base": config_json(&manifest, 250, 100),
        "design": {"type": "seeds", "count": 8},
    });
    let (status, _, body) = server
        .post("/api/v1/sweeps", Some(&key), &json!({"spec": spec}))
        .await;
    assert_eq!(status, 202, "{body}");
    let live: std::collections::BTreeSet<&str> = body["sweep"]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["runs"].as_array().unwrap())
        .filter(|r| r["state"] == "queued")
        .filter_map(|r| r["job_id"].as_str())
        .collect();
    assert_eq!(live.len(), 8, "{body}");
    // The sweep is one unit: four single experiments fit, the fifth does not.
    for seed in 1..=5u64 {
        let (status, _, body) = server
            .post(
                "/api/v1/experiments",
                Some(&key),
                &json!({"config": config_json(&manifest, 100, seed), "label": format!("single {seed}")}),
            )
            .await;
        if seed <= 4 {
            assert_eq!(status, 202, "single {seed}: {body}");
        } else {
            assert_eq!(status, 429, "{body}");
            assert_eq!(body["error"]["details"]["scope"], "active_jobs");
            assert_eq!(body["error"]["details"]["limit"], 5);
            assert_eq!(body["error"]["details"]["used"], 5);
        }
    }
    server.stop().await;
    tdb.drop_db().await;
}

/// The audit rows of `action`: (api key id, request id, target id).
async fn audit_rows(
    db: &Database,
    action: &str,
) -> Vec<(Option<uuid::Uuid>, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT api_key_id,request_id,target_id FROM bb.audit_events WHERE action=$1 AND outcome='ok' ORDER BY id")
        .bind(action)
        .fetch_all(db.pool())
        .await
        .unwrap()
}

async fn key_id(db: &Database, plaintext: &str) -> uuid::Uuid {
    sqlx::query_scalar("SELECT id FROM bb.api_keys WHERE display_prefix=$1")
        .bind(&plaintext[..8])
        .fetch_one(db.pool())
        .await
        .unwrap()
}

fn request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

async fn send(
    server: &Server,
    method: reqwest::Method,
    path: &str,
    key: &str,
    if_match: Option<&str>,
    body: Option<Value>,
) -> (u16, reqwest::header::HeaderMap, Value) {
    let mut req = server
        .http
        .request(method, server.url(path))
        .bearer_auth(key);
    if let Some(tag) = if_match {
        req = req.header("If-Match", tag);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    response(req.send().await.unwrap()).await
}

/// FINAL_AUDIT §4 item 7 (SEC-25): every v1 mutation writes exactly one `audit_events` row with
/// its request id, key id, action, target and outcome inside the mutation's transaction; a
/// mutation that rolls back (stale version, terminal job, missing target) leaves no row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_v1_mutation_writes_one_transactional_audit_row() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    use reqwest::Method;

    // Phase 1 without a worker: submit, rename, cancel and delete a queued experiment.
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let (_, admin) = workspace_key(&server.db, "ops", &[Scope::Admin]).await;
    let kid = key_id(&server.db, &key).await;
    let (status, headers, submitted) = send(
        &server,
        Method::POST,
        "/api/v1/experiments",
        &key,
        None,
        Some(json!({"config": config_json(&manifest, 100, 9), "label": "queued"})),
    )
    .await;
    assert_eq!(status, 202, "{submitted}");
    let experiment_id = submitted["experiment"]["id"].as_str().unwrap().to_string();
    let job_id = submitted["job"]["id"].as_str().unwrap().to_string();
    let rows = audit_rows(&server.db, "experiment.create").await;
    assert_eq!(
        rows,
        vec![(Some(kid), request_id(&headers), Some(experiment_id.clone()))]
    );

    let path = format!("/api/v1/experiments/{experiment_id}");
    let version = submitted["experiment"]["version"].as_i64().unwrap();
    let (status, headers, _) = send(
        &server,
        Method::PATCH,
        &path,
        &key,
        Some(&format!("\"v{version}\"")),
        Some(json!({"label": "renamed"})),
    )
    .await;
    assert_eq!(status, 200);
    let rename_request = request_id(&headers);
    // Stale version: the update matches nothing and the transaction rolls back.
    let (status, _, _) = send(
        &server,
        Method::PATCH,
        &path,
        &key,
        Some(&format!("\"v{version}\"")),
        Some(json!({"label": "stale"})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(
        audit_rows(&server.db, "experiment.rename").await,
        vec![(Some(kid), rename_request, Some(experiment_id.clone()))]
    );

    let (status, headers, _) = send(
        &server,
        Method::POST,
        &format!("/api/v1/jobs/{job_id}/cancel"),
        &key,
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(
        audit_rows(&server.db, "job.cancel").await,
        vec![(Some(kid), request_id(&headers), Some(job_id.clone()))]
    );
    let (status, headers, _) = send(&server, Method::DELETE, &path, &key, None, None).await;
    assert_eq!(status, 204);
    assert_eq!(
        audit_rows(&server.db, "experiment.delete").await,
        vec![(Some(kid), request_id(&headers), Some(experiment_id.clone()))]
    );

    // Sweeps: create, rename (a stale version rolls back), cancel, delete (a second delete
    // finds nothing and rolls back).
    let spec = json!({
        "base": config_json(&manifest, 250, 5),
        "design": {"type": "seeds", "count": 2},
    });
    let (status, headers, created) = send(
        &server,
        Method::POST,
        "/api/v1/sweeps",
        &key,
        None,
        Some(json!({"spec": spec})),
    )
    .await;
    assert_eq!(status, 202, "{created}");
    let sweep_id = created["sweep"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        audit_rows(&server.db, "sweep.create").await,
        vec![(Some(kid), request_id(&headers), Some(sweep_id.clone()))]
    );
    let spath = format!("/api/v1/sweeps/{sweep_id}");
    let version = created["sweep"]["version"].as_i64().unwrap();
    let (status, headers, _) = send(
        &server,
        Method::PATCH,
        &spath,
        &key,
        Some(&format!("\"v{version}\"")),
        Some(json!({"label": "ensemble"})),
    )
    .await;
    assert_eq!(status, 200);
    let rename_request = request_id(&headers);
    let (status, _, _) = send(
        &server,
        Method::PATCH,
        &spath,
        &key,
        Some(&format!("\"v{version}\"")),
        Some(json!({"label": "stale"})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(
        audit_rows(&server.db, "sweep.rename").await,
        vec![(Some(kid), rename_request, Some(sweep_id.clone()))]
    );
    let (status, headers, _) = send(
        &server,
        Method::POST,
        &format!("{spath}/cancel"),
        &key,
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(
        audit_rows(&server.db, "sweep.cancel").await,
        vec![(Some(kid), request_id(&headers), Some(sweep_id.clone()))]
    );
    let (status, headers, _) = send(&server, Method::DELETE, &spath, &key, None, None).await;
    assert_eq!(status, 204);
    let (status, _, _) = send(&server, Method::DELETE, &spath, &key, None, None).await;
    assert_eq!(status, 404);
    assert_eq!(
        audit_rows(&server.db, "sweep.delete").await,
        vec![(Some(kid), request_id(&headers), Some(sweep_id.clone()))]
    );
    server.stop().await;

    // Phase 2 with a worker: comparisons, reports and a key revocation.
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let (run1, job1) = run(&server, &key, config_json(&manifest, 100, 1), "one").await;
    let (run2, _) = run(&server, &key, config_json(&manifest, 200, 1), "two").await;
    // A terminal job cannot be cancelled: rolled back, no row.
    let (status, _, _) = send(
        &server,
        Method::POST,
        &format!("/api/v1/jobs/{}/cancel", job1["id"].as_str().unwrap()),
        &key,
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(audit_rows(&server.db, "job.cancel").await.len(), 1);

    let (status, headers, comparison) = send(
        &server,
        Method::POST,
        "/api/v1/comparisons",
        &key,
        None,
        Some(json!({"run_a": run1, "run_b": run2})),
    )
    .await;
    assert_eq!(status, 201, "{comparison}");
    let comparison_id = comparison["id"].as_str().unwrap().to_string();
    assert_eq!(
        audit_rows(&server.db, "comparison.create").await,
        vec![(Some(kid), request_id(&headers), Some(comparison_id.clone()))]
    );
    let cpath = format!("/api/v1/comparisons/{comparison_id}");
    let (status, headers, _) = send(
        &server,
        Method::PATCH,
        &cpath,
        &key,
        Some("\"v1\""),
        Some(json!({"label": "pair"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        audit_rows(&server.db, "comparison.rename").await,
        vec![(Some(kid), request_id(&headers), Some(comparison_id.clone()))]
    );
    let (status, headers, report) = send(
        &server,
        Method::POST,
        "/api/v1/reports",
        &key,
        None,
        Some(json!({"kind": "run", "subject_id": run1, "title": "One"})),
    )
    .await;
    assert_eq!(status, 201, "{report}");
    let report_id = report["id"].as_str().unwrap().to_string();
    assert_eq!(
        audit_rows(&server.db, "report.create").await,
        vec![(Some(kid), request_id(&headers), Some(report_id.clone()))]
    );
    let (status, headers, _) = send(
        &server,
        Method::DELETE,
        &format!("/api/v1/reports/{report_id}"),
        &key,
        None,
        None,
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(
        audit_rows(&server.db, "report.delete").await,
        vec![(Some(kid), request_id(&headers), Some(report_id.clone()))]
    );
    let (status, headers, _) = send(&server, Method::DELETE, &cpath, &key, None, None).await;
    assert_eq!(status, 204);
    // Deleting it again finds nothing: rolled back, no second row.
    let (status, _, _) = send(&server, Method::DELETE, &cpath, &key, None, None).await;
    assert_eq!(status, 404);
    assert_eq!(
        audit_rows(&server.db, "comparison.delete").await,
        vec![(Some(kid), request_id(&headers), Some(comparison_id.clone()))]
    );

    let (status, headers, _) = send(
        &server,
        Method::POST,
        &format!("/api/v1/system/api-keys/{kid}/revoke"),
        &admin,
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200);
    let admin_id = key_id(&server.db, &admin).await;
    let revokes = audit_rows(&server.db, "key.revoke").await;
    assert_eq!(
        revokes,
        vec![(Some(admin_id), request_id(&headers), Some(kid.to_string()))]
    );
    server.stop().await;
    tdb.drop_db().await;
}
