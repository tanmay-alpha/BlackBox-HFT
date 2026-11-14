//! The public demo workflow against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`) at demo size:
//! `demo seed --wait` materialises the curated comparisons and saved reports (idempotently),
//! anonymous analytics of every demo run stay under the 1 MiB anonymous cap with series paging,
//! comparison creation of two demo-sized runs finishes within its own timeout, the idle worker
//! reports a fresh memory-only heartbeat, and the central response cap refuses an oversize body
//! while the egress counter adds the true body size. Without the variable the tests skip.

mod common;

use std::{path::Path, sync::Arc, time::Instant};

use bb_app::{
    metrics::Metrics,
    runtime::{self, Runtime},
};
use bb_db::Scope;
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

const MIB: usize = 1 << 20;

/// `blackbox demo seed --wait` in its own runtime (as the CLI does).
async fn seed(tdb: &TestDb, dir: &Path) -> bb_app::seed::SeedReport {
    let cfg = config(dir, &env(&tdb.url));
    let metrics = Arc::new(Metrics::default());
    let db = runtime::connect(&cfg, metrics.clone())
        .await
        .expect("connect");
    let rt = Runtime::with_db(Arc::new(cfg), db, metrics, "standalone", 1, None).expect("runtime");
    let platform = rt.cfg.result_platform.clone();
    let report = bb_app::seed::seed(&rt, &platform, true)
        .await
        .expect("seed");
    rt.db.pool().close().await;
    report
}

/// GET returning (status, body bytes); the body length is what the server sent.
async fn get_raw(server: &Server, path: &str, key: Option<&str>) -> (u16, Vec<u8>) {
    let mut req = server.http.get(server.url(path));
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    let r = req.send().await.expect("request");
    let status = r.status().as_u16();
    (status, r.bytes().await.expect("body").to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_seed_curates_and_demo_responses_stay_bounded() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();

    // Seeding computes the demo runs, then curates: 5 comparisons and 3 saved reports.
    let first = seed(&tdb, dir.path()).await;
    assert_eq!(first.experiments.len(), 8);
    let count = |r: &bb_app::seed::SeedReport, kind: &str, status: &str| {
        r.curated
            .iter()
            .filter(|c| c.kind == kind && c.status == status)
            .count()
    };
    assert_eq!(count(&first, "comparison", "created"), 5);
    assert_eq!(count(&first, "report", "created"), 3);
    assert_eq!(first.curated.len(), 8);
    // Idempotent: nothing new the second time.
    let second = seed(&tdb, dir.path()).await;
    assert_eq!(count(&second, "comparison", "existing"), 5);
    assert_eq!(count(&second, "report", "existing"), 3);
    assert_eq!(second.curated.len(), 8);

    let server = Server::start(&tdb, dir.path(), Options::default()).await;

    // Anonymous visitors read every curated comparison and report.
    let (status, _, comparisons) = server.get("/api/v1/comparisons?limit=20", None).await;
    assert_eq!(status, 200, "{comparisons}");
    let comparisons = comparisons["items"].as_array().unwrap().clone();
    assert_eq!(comparisons.len(), 5);
    for c in &comparisons {
        let id = c["id"].as_str().unwrap();
        let (status, _, body) = server.get(&format!("/api/v1/comparisons/{id}"), None).await;
        assert_eq!(status, 200, "{body}");
        let (status, body) =
            get_raw(&server, &format!("/api/v1/comparisons/{id}/report"), None).await;
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&body).starts_with('#'));
    }
    let (status, _, reports) = server.get("/api/v1/reports?limit=20", None).await;
    assert_eq!(status, 200, "{reports}");
    let reports = reports["items"].as_array().unwrap().clone();
    assert_eq!(reports.len(), 3);
    for r in &reports {
        let id = r["id"].as_str().unwrap();
        let (status, body) = get_raw(&server, &format!("/api/v1/reports/{id}/content"), None).await;
        assert_eq!(status, 200);
        assert_eq!(body.len(), r["size_bytes"].as_u64().unwrap() as usize);
    }

    // Anonymous run analytics of every demo run: < 1 MiB, egress counts the exact body.
    let (status, _, runs) = server.get("/api/v1/runs?limit=20", None).await;
    assert_eq!(status, 200, "{runs}");
    let runs: Vec<String> = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["run_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(runs.len(), 6);
    let mut largest = (0usize, String::new(), Value::Null);
    for run in &runs {
        let before = server.state.egress.today(true);
        let (status, body) = get_raw(&server, &format!("/api/v1/runs/{run}/analytics"), None).await;
        let after = server.state.egress.today(true);
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        assert!(
            body.len() < MIB,
            "run {run} analytics is {} bytes",
            body.len()
        );
        assert_eq!(after - before, body.len() as i64, "egress of run {run}");
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            value["risk"]["net_pnl"].is_object(),
            "aggregates stay complete"
        );
        eprintln!("run {run}: anonymous analytics {} bytes", body.len());
        if body.len() > largest.0 {
            largest = (body.len(), run.clone(), value);
        }
    }
    // Every downsampled series of the largest one is pageable in full.
    let (_, run, value) = largest;
    let downsampled = value["downsampled"].as_array().cloned().unwrap_or_default();
    for s in &downsampled {
        let path = s["path"].as_str().unwrap();
        let total = s["total"].as_u64().unwrap() as usize;
        assert!(s["returned"].as_u64().unwrap() as usize <= bb_app::analytics::MAX_SERIES_POINTS);
        let (mut offset, mut seen) = (0usize, 0usize);
        loop {
            let (status, _, page) = server
                .get(
                    &format!(
                        "/api/v1/runs/{run}/analytics/series?path={path}&offset={offset}&limit=5000"
                    ),
                    None,
                )
                .await;
            assert_eq!(status, 200, "{page}");
            assert_eq!(page["total"].as_u64().unwrap() as usize, total);
            seen += page["items"].as_array().unwrap().len();
            match page["next_offset"].as_u64() {
                Some(next) => offset = next as usize,
                None => break,
            }
        }
        assert_eq!(seen, total, "{path}");
    }
    let (status, _, refused) = server
        .get(
            &format!("/api/v1/runs/{run}/analytics/series?path=/risk/net_pnl"),
            None,
        )
        .await;
    assert_eq!(status, 400, "{refused}");

    // Anonymous dataset analytics.
    let before = server.state.egress.today(true);
    let (status, body) = get_raw(&server, "/api/v1/datasets/synthetic-demo/analytics", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(
        body.len() < MIB,
        "dataset analytics is {} bytes",
        body.len()
    );
    assert_eq!(server.state.egress.today(true) - before, body.len() as i64);
    eprintln!("dataset analytics: {} bytes", body.len());
    let market: Value = serde_json::from_slice(&body).unwrap();
    if let Some(s) = market["downsampled"].as_array().and_then(|d| d.first()) {
        let (status, _, page) = server
            .get(
                &format!(
                    "/api/v1/datasets/synthetic-demo/versions/{}/analytics/series?path={}",
                    market["version"],
                    s["path"].as_str().unwrap()
                ),
                None,
            )
            .await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["total"], s["total"]);
    }

    // A keyed comparison of two aligned demo-sized runs completes within its own timeout.
    let (_, key) = workspace_key(&server.db, "lab", &[Scope::Run]).await;
    let demo_runs = |label: &str| {
        first
            .experiments
            .iter()
            .find(|(l, _, _)| l == label)
            .map(|(_, id, _)| id.clone())
            .unwrap()
    };
    let a = demo_runs("gross_vs_net: 2 bp maker / 4 bp taker fees");
    let b = demo_runs("gross_vs_net: 1 bp maker rebate / 4 bp taker fee");
    let started = Instant::now();
    let (status, _, comparison) = server
        .post(
            "/api/v1/comparisons",
            Some(&key),
            &json!({"run_a": a, "run_b": b, "label": "timed"}),
        )
        .await;
    let elapsed = started.elapsed();
    assert_eq!(status, 201, "{comparison}");
    assert_eq!(comparison["status"], "aligned", "{comparison}");
    assert!(
        elapsed < server.state.cfg.comparison_timeout,
        "comparison took {elapsed:?}"
    );
    eprintln!("aligned demo comparison: {elapsed:?}");

    // The idle in-process worker keeps a fresh in-memory heartbeat (no stale flag).
    let interval = server.state.cfg.heartbeat_interval;
    tokio::time::sleep(interval * 4).await;
    let (_, _, status) = server.get("/api/v1/status", None).await;
    assert_eq!(status["worker"]["state"], "idle", "{status}");
    let age = status["worker"]["last_heartbeat_age_ms"].as_u64().unwrap();
    assert!(
        u128::from(age) < interval.as_millis() * 3,
        "idle heartbeat age {age} ms"
    );

    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversize_bodies_are_refused_and_egress_counts_true_bytes() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            tweak: Box::new(|c| c.anonymous_max_response_bytes = 4096),
            ..Options::default()
        },
    )
    .await;
    let (_, key) = workspace_key(&server.db, "lab", &[Scope::Read]).await;

    // The OpenAPI document is far above 4 KiB: refused for anonymous callers, counted exactly.
    let before = server.state.egress.today(true);
    let (status, body) = get_raw(&server, "/api/v1/openapi.json", None).await;
    assert_eq!(status, 500);
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "internal", "{error}");
    assert_eq!(error["error"]["details"]["reason"], "response_too_large");
    assert_eq!(error["error"]["details"]["limit_bytes"], 4096);
    assert_eq!(server.state.egress.today(true) - before, body.len() as i64);

    // Keyed callers have the 4 MiB cap; the egress counter adds the real body length.
    let before = server.state.egress.today(false);
    let (status, body) = get_raw(&server, "/api/v1/openapi.json", Some(&key)).await;
    assert_eq!(status, 200);
    assert!(body.len() > 4096);
    assert_eq!(server.state.egress.today(false) - before, body.len() as i64);

    // Small anonymous responses still pass and are counted.
    let before = server.state.egress.today(true);
    let (status, body) = get_raw(&server, "/api/v1/status", None).await;
    assert_eq!(status, 200);
    assert!(!body.is_empty());
    assert_eq!(server.state.egress.today(true) - before, body.len() as i64);

    server.stop().await;
    tdb.drop_db().await;
}
