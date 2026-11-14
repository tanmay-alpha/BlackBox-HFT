//! FINAL_AUDIT §4 items 5–7 against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`, else skip):
//! row digests on DB-only list paths, the anonymous demo snapshot, quotas, the per-workspace
//! upload reservation and transactional audit rows.

mod common;

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
