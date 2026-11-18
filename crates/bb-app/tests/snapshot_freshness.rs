//! Phase 6 A3 (docs/rebuild/PHASE6_RELIABILITY.md): freshness of the anonymous demo snapshot
//! against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`; the tests skip without it).
//!
//! Rows written by another process (direct SQL, a second runtime as `blackbox demo seed` or
//! `blackbox datasets` would be, a standalone worker) are not visible before the recheck
//! interval and are visible right after it; changes made through this server are visible at
//! once. Covered: an initially empty database, dataset registration, in-process and external
//! comparison creation, concurrent readers (one check, one build), integrity failures (changed
//! comparison and run rows dropped, the run quarantined), a failed rebuild (no partial
//! snapshot, the database path answers, recovery), a server restart, and a standalone worker
//! completing jobs before and after it restarts.

mod common;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bb_app::{
    metrics::Metrics,
    runtime::{self, Runtime},
};
use bb_db::{Database, Scope};
use common::*;
use serde_json::{Value, json};

macro_rules! db_or_skip {
    () => {
        match TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}

const LONG: Duration = Duration::from_secs(3600);
const SHORT: Duration = Duration::from_millis(100);

/// Another process on the same database and artefact store, with its own data directory.
async fn other_process(tdb: &TestDb, server_dir: &Path, own_dir: &Path, slots: usize) -> Runtime {
    let mut vars = env(&tdb.url);
    vars.insert(
        "BB_ARTIFACT_DIR".into(),
        server_dir.join("artifacts").to_string_lossy().into_owned(),
    );
    let cfg = config(own_dir, &vars);
    let metrics = Arc::new(Metrics::default());
    let db = runtime::connect(&cfg, metrics.clone())
        .await
        .expect("connect");
    Runtime::with_db(Arc::new(cfg), db, metrics, "standalone", slots, None).expect("runtime")
}

fn items(page: &Value) -> Vec<Value> {
    page["items"].as_array().expect("a page").clone()
}

/// An anonymous list read. The anonymous response cache (30 s) keys on the documented query
/// parameters only, so `n` picks a distinct `limit` (3..=100, above every list's length here):
/// each read reaches the handler and therefore the snapshot.
async fn anonymous(server: &Server, path: &str, n: usize) -> Vec<Value> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let url = format!("{path}{sep}limit={}", 3 + n % 98);
    let (status, _, page) = server.get(&url, None).await;
    assert_eq!(status, 200, "{url}: {page}");
    items(&page)
}

/// Lets the recheck interval elapse, so the next anonymous read checks the data version.
async fn recheck_now(server: &Server) {
    server.state.snapshot.set_recheck(SHORT);
    tokio::time::sleep(SHORT * 2).await;
}

fn acquires(server: &Server) -> u64 {
    Metrics::get(&server.metrics.db_acquires_total)
}

async fn tamper_net_pnl(db: &Database, run_id: &str) {
    let pool = db.pool();
    for sql in [
        "ALTER TABLE bb.runs DISABLE TRIGGER runs_immutable",
        "UPDATE bb.runs r SET metrics=jsonb_set(metrics,'{net_pnl}',to_jsonb((metrics->>'net_pnl')::float8+5000.25)) FROM bb.experiment_configs c WHERE c.id=r.experiment_config_id AND c.short_id=$1 AND r.quarantined_at IS NULL",
        "ALTER TABLE bb.runs ENABLE TRIGGER runs_immutable",
    ] {
        let q = sqlx::query(sql);
        let q = if sql.contains("$1") {
            q.bind(run_id)
        } else {
            q
        };
        q.execute(pool).await.unwrap();
    }
}

async fn sql(db: &Database, statement: &str) {
    sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_sees_external_writes_within_the_interval_and_survives_failures() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    server.state.snapshot.set_recheck(LONG);

    // Initially empty, and served from memory.
    assert!(anonymous(&server, "/api/v1/datasets", 0).await.is_empty());
    assert!(anonymous(&server, "/api/v1/runs", 0).await.is_empty());
    assert!(
        anonymous(&server, "/api/v1/comparisons", 0)
            .await
            .is_empty()
    );

    // Another process registers a dataset version: invisible until the interval elapses.
    let external = tdb.pool().await;
    let manifest = register_dataset(&external, dir.path(), 20).await;
    assert!(
        anonymous(&server, "/api/v1/datasets", 1).await.is_empty(),
        "within the interval"
    );
    recheck_now(&server).await;
    let datasets = anonymous(&server, "/api/v1/datasets", 2).await;
    assert_eq!(datasets.len(), 1, "after the interval");
    assert_eq!(datasets[0]["id"], DATASET);
    assert_eq!(datasets[0]["current_version"]["version"], 1);
    let (status, _, presets) = server.get("/api/v1/presets?n=3", None).await;
    assert_eq!(status, 200);
    assert!(
        !presets.as_array().unwrap().is_empty(),
        "the catalogue follows the dataset"
    );
    server.state.snapshot.set_recheck(LONG);

    // Changes made through this server are visible at once.
    let (_, admin) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;
    let (a, _) = run(&server, &admin, config_json(&manifest, 100, 1), "a").await;
    let (b, _) = run(&server, &admin, config_json(&manifest, 200, 1), "b").await;
    let runs = anonymous(&server, "/api/v1/runs", 4).await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    let (status, _, created) = server
        .post(
            "/api/v1/comparisons",
            Some(&admin),
            &json!({"run_a": a, "run_b": b}),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(anonymous(&server, "/api/v1/comparisons", 5).await.len(), 1);

    // Another process stores a comparison (as `demo seed` does).
    let other = other_process(&tdb, dir.path(), other_dir.path(), 0).await;
    let state = other.app_state();
    let demo = other.db.workspace("demo").await.unwrap();
    let platform = other.cfg.result_platform.clone();
    let view = |id: &str| {
        let (db, id, platform) = (other.db.clone(), id.to_string(), platform.clone());
        async move { db.run_view(demo.id, &id, &platform).await.unwrap() }
    };
    let (row, existed) = bb_app::v1::comparisons::compare_and_store(
        &state,
        demo.id,
        &view(&b).await,
        &view(&a).await,
        "reversed".into(),
        &bb_db::Audit {
            workspace: Some(demo.id),
            ..Default::default()
        },
    )
    .await
    .expect("external comparison");
    assert!(!existed);
    assert_eq!(
        anonymous(&server, "/api/v1/comparisons", 6).await.len(),
        1,
        "within the interval"
    );
    recheck_now(&server).await;
    assert_eq!(anonymous(&server, "/api/v1/comparisons", 7).await.len(), 2);
    server.state.snapshot.set_recheck(LONG);
    let before = acquires(&server);
    let (status, _, detail) = server
        .get(&format!("/api/v1/comparisons/{}", row.id), None)
        .await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["label"], "reversed");
    assert_eq!(
        acquires(&server),
        before,
        "an anonymous demo comparison is read from memory"
    );

    // Concurrent readers after another external change: one check, at most one build.
    server.hub.bump();
    let before = acquires(&server);
    server
        .state
        .snapshot
        .get(&server.state)
        .await
        .expect("rebuilt");
    let build_cost = acquires(&server) - before;
    assert!(build_cost > 0);
    sql(
        &external,
        &format!(
            "UPDATE bb.comparisons SET label='renamed' WHERE id='{}'",
            row.id
        ),
    )
    .await;
    // Due at the burst's start; not due again before it ends.
    server.state.snapshot.set_recheck(Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let before = acquires(&server);
    let server = Arc::new(server);
    let readers: Vec<_> = (0..32)
        .map(|i| {
            let server = server.clone();
            tokio::spawn(async move { anonymous(&server, "/api/v1/comparisons", 100 + i).await })
        })
        .collect();
    for reader in readers {
        let page = reader.await.unwrap();
        assert_eq!(page.len(), 2, "never a partial list");
        let label = page.iter().find(|c| c["id"] == row.id.to_string()).unwrap()["label"].clone();
        assert!(label == "reversed" || label == "renamed", "{label}");
    }
    let used = acquires(&server) - before;
    eprintln!(
        "32 concurrent readers after an external change: {used} acquisitions (one build: {build_cost})"
    );
    assert!(
        used <= 1 + build_cost,
        "{used} acquisitions for one check and one build of {build_cost}"
    );
    let page = anonymous(&server, "/api/v1/comparisons", 200).await;
    assert!(page.iter().any(|c| c["label"] == "renamed"));
    let server = Arc::into_inner(server).expect("readers finished");

    // Integrity: a changed comparison row and a changed run row, written behind the triggers.
    sql(
        &external,
        "ALTER TABLE bb.comparisons DISABLE TRIGGER comparisons_immutable",
    )
    .await;
    sql(&external, &format!("UPDATE bb.comparisons SET summary=summary||'{{\"note\":\"edited\"}}'::jsonb WHERE id='{}'", row.id)).await;
    sql(
        &external,
        "ALTER TABLE bb.comparisons ENABLE TRIGGER comparisons_immutable",
    )
    .await;
    tamper_net_pnl(&external, &b).await;
    recheck_now(&server).await;
    let comparisons = anonymous(&server, "/api/v1/comparisons", 300).await;
    assert!(
        comparisons.iter().all(|c| c["id"] != row.id.to_string()),
        "changed comparison dropped"
    );
    let (status, _, body) = server
        .get(&format!("/api/v1/comparisons/{}", row.id), None)
        .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"]["code"], "integrity_failed");
    let runs = anonymous(&server, "/api/v1/runs", 301).await;
    assert!(
        runs.iter().all(|r| r["run_id"] != b.as_str()),
        "changed run dropped"
    );
    let (status, _, body) = server.get(&format!("/api/v1/runs/{b}"), None).await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(
        body["error"]["code"], "integrity_failed",
        "the run is quarantined"
    );

    // A failed version check keeps the last confirmed snapshot. An in-process change still
    // forces a build; a failed build leaves no partial snapshot and the database path answers.
    let before_outage = anonymous(&server, "/api/v1/comparisons", 399).await;
    sql(
        &external,
        "ALTER TABLE bb.workspaces RENAME TO workspaces_moved",
    )
    .await;
    sql(
        &external,
        &format!(
            "UPDATE bb.comparisons SET label='during outage' WHERE id='{}'",
            created["id"].as_str().unwrap()
        ),
    )
    .await;
    recheck_now(&server).await;
    let comparisons = anonymous(&server, "/api/v1/comparisons", 400).await;
    assert_eq!(
        comparisons, before_outage,
        "failed check keeps the snapshot"
    );
    server.hub.bump();
    let comparisons = anonymous(&server, "/api/v1/comparisons", 401).await;
    assert_eq!(comparisons[0]["label"], "during outage", "database path");
    assert!(
        server.state.snapshot.get(&server.state).await.is_none(),
        "no partial snapshot"
    );
    sql(
        &external,
        "ALTER TABLE bb.workspaces_moved RENAME TO workspaces",
    )
    .await;
    server.hub.bump();
    server.state.snapshot.set_recheck(LONG);
    assert!(
        server.state.snapshot.get(&server.state).await.is_some(),
        "rebuilt"
    );
    let before = acquires(&server);
    assert_eq!(
        anonymous(&server, "/api/v1/comparisons", 402).await.len(),
        1
    );
    assert_eq!(acquires(&server), before, "served from memory again");
    server.stop().await;

    // A restarted server builds from the current rows.
    sql(
        &external,
        &format!(
            "UPDATE bb.comparisons SET label='after restart' WHERE id='{}'",
            created["id"].as_str().unwrap()
        ),
    )
    .await;
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let comparisons = anonymous(&server, "/api/v1/comparisons", 500).await;
    assert_eq!(comparisons[0]["label"], "after restart");
    server.stop().await;
    other.db.pool().close().await;
    external.pool().close().await;
    tdb.drop_db().await;
}

/// Runs a standalone worker of `rt` until `job` completed (checked through `server`'s database
/// path with `key`), then drains it.
async fn standalone_worker_completes(rt: &Runtime, server: &Server, key: &str, job: &str) {
    let worker = tokio::spawn(bb_app::worker::run(rt.worker_ctx(), true));
    let done = wait_job(server, Some(key), job).await;
    assert_eq!(done["state"], "completed", "{done}");
    rt.hub.start_drain();
    let _ = worker.await;
}

async fn submit(server: &Server, key: &str, config: Value, label: &str) -> String {
    let (status, _, body) = server
        .post(
            "/api/v1/experiments",
            Some(key),
            &json!({"config": config, "label": label}),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    body["job"]["id"].as_str().unwrap().to_string()
}

fn experiment_status(experiments: &[Value], label: &str) -> String {
    experiments
        .iter()
        .find(|e| e["label"] == label)
        .map(|e| e["status"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_follows_a_standalone_worker_across_its_restart() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            ..Options::default()
        },
    )
    .await;
    server.state.snapshot.set_recheck(LONG);
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, admin) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;

    for (round, seed) in [(1, 11u64), (2, 12)] {
        let label = format!("worker round {round}");
        let job = submit(&server, &admin, config_json(&manifest, 100, seed), &label).await;
        // Submitted through this server: listed at once, queued.
        let experiments = anonymous(&server, "/api/v1/experiments", round * 10).await;
        assert_eq!(experiment_status(&experiments, &label), "queued");
        // A standalone worker (a new process each round: a worker restart) completes it.
        let worker_dir: PathBuf = dir.path().join(format!("worker-{round}"));
        let worker = other_process(&tdb, dir.path(), &worker_dir, 1).await;
        server.state.snapshot.set_recheck(LONG);
        standalone_worker_completes(&worker, &server, &admin, &job).await;
        worker.db.pool().close().await;
        let experiments = anonymous(&server, "/api/v1/experiments", round * 10 + 1).await;
        assert_eq!(
            experiment_status(&experiments, &label),
            "queued",
            "within the interval"
        );
        recheck_now(&server).await;
        let experiments = anonymous(&server, "/api/v1/experiments", round * 10 + 2).await;
        assert_eq!(
            experiment_status(&experiments, &label),
            "completed",
            "after the interval"
        );
        let runs = anonymous(&server, "/api/v1/runs", round * 10 + 3).await;
        assert_eq!(runs.len(), round, "{runs:?}");
    }
    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_quality_check_refreshes_the_anonymous_dataset_summary() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let external = tdb.pool().await;
    register_dataset(&external, dir.path(), 1).await;
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    server.state.snapshot.set_recheck(LONG);
    let before = anonymous(&server, "/api/v1/datasets", 0).await;
    let quality = &before[0]["current_version"]["quality"];
    let initial_checker = quality["checker_version"].as_i64().unwrap();
    let demo = external.workspace("demo").await.unwrap();
    let version = external.demo_data_version(demo.id).await.unwrap();
    sqlx::query("INSERT INTO bb.dataset_quality_reports(dataset_version_id,checker_version,passed,error_count,warning_count,book_valid_fraction,report,report_sha256) SELECT dataset_version_id,checker_version+100,false,1,0,0.5,report,report_sha256 FROM bb.dataset_quality_reports WHERE dataset_version_id=(SELECT id FROM bb.dataset_versions WHERE dataset_id=$1) ORDER BY checker_version DESC LIMIT 1")
        .bind(DATASET)
        .execute(external.pool())
        .await
        .unwrap();
    assert_ne!(version, external.demo_data_version(demo.id).await.unwrap());
    assert_eq!(
        anonymous(&server, "/api/v1/datasets", 1).await[0]["current_version"]["quality"],
        *quality,
        "the existing snapshot is used inside the interval"
    );
    recheck_now(&server).await;
    let after = anonymous(&server, "/api/v1/datasets", 2).await;
    let quality = &after[0]["current_version"]["quality"];
    assert_eq!(quality["checker_version"], initial_checker + 100);
    assert_eq!(quality["passed"], false);
    assert_eq!(quality["error_count"], 1);
    server.stop().await;
    external.pool().close().await;
    tdb.drop_db().await;
}

#[tokio::test]
async fn row_version_fingerprint_detects_equal_sum_updates() {
    let tdb = db_or_skip!();
    let db = tdb.pool().await;
    let demo = db.workspace("demo").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    register_dataset(&db, dir.path(), 1).await;
    // Allocate A < B < C < D; insert in B/C, then update in A/D and commit D before A.
    // Consecutive XIDs reproduce the review's two-row case. Multiplicities also handle
    // unrelated tests allocating cluster-wide XIDs between us, without a timing assumption.
    let mut early = db.pool().begin().await.unwrap();
    let early_xid: i64 = sqlx::query_scalar("SELECT pg_current_xact_id()::text::bigint")
        .fetch_one(&mut *early)
        .await
        .unwrap();
    let mut middle = db.pool().begin().await.unwrap();
    let middle_xid: i64 = sqlx::query_scalar("SELECT pg_current_xact_id()::text::bigint")
        .fetch_one(&mut *middle)
        .await
        .unwrap();
    let mut middle_later = db.pool().begin().await.unwrap();
    let middle_later_xid: i64 = sqlx::query_scalar("SELECT pg_current_xact_id()::text::bigint")
        .fetch_one(&mut *middle_later)
        .await
        .unwrap();
    let mut late = db.pool().begin().await.unwrap();
    let late_xid: i64 = sqlx::query_scalar("SELECT pg_current_xact_id()::text::bigint")
        .fetch_one(&mut *late)
        .await
        .unwrap();
    let early_rows = late_xid - middle_later_xid;
    let late_rows = middle_xid - early_xid;
    for (prefix, count, tx) in [
        ("version-a-", early_rows, &mut middle),
        ("version-b-", late_rows, &mut middle_later),
    ] {
        sqlx::query("INSERT INTO bb.datasets(id,title,kind,level,instrument_id,data_source_id) SELECT $1 || n::text,$1 || n::text,kind,level,instrument_id,data_source_id FROM bb.datasets CROSS JOIN generate_series(1,$2::bigint) AS n WHERE id=$3")
            .bind(prefix)
            .bind(count)
            .bind(DATASET)
            .execute(&mut **tx)
            .await
            .unwrap();
    }
    middle.commit().await.unwrap();
    middle_later.commit().await.unwrap();
    let before = db.demo_data_version(demo.id).await.unwrap();
    let sum_before: i64 =
        sqlx::query_scalar("SELECT sum(xmin::text::bigint)::bigint FROM bb.datasets")
            .fetch_one(db.pool())
            .await
            .unwrap();
    sqlx::query("UPDATE bb.datasets SET title='early update' WHERE starts_with(id,'version-a-')")
        .execute(&mut *early)
        .await
        .unwrap();
    sqlx::query("UPDATE bb.datasets SET title='late update' WHERE starts_with(id,'version-b-')")
        .execute(&mut *late)
        .await
        .unwrap();
    late.commit().await.unwrap();
    early.commit().await.unwrap();
    let sum_after: i64 =
        sqlx::query_scalar("SELECT sum(xmin::text::bigint)::bigint FROM bb.datasets")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(sum_before, sum_after, "the old sum fingerprint collides");
    assert_ne!(before, db.demo_data_version(demo.id).await.unwrap());
    db.pool().close().await;
    tdb.drop_db().await;
}
