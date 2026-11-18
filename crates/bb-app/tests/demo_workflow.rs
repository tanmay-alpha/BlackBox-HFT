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

    // Seeding computes the demo runs, then curates: 11 comparisons and 4 saved reports.
    let first = seed(&tdb, dir.path()).await;
    assert_eq!(first.experiments.len(), 15);
    let count = |r: &bb_app::seed::SeedReport, kind: &str, status: &str| {
        r.curated
            .iter()
            .filter(|c| c.kind == kind && c.status == status)
            .count()
    };
    assert_eq!(count(&first, "comparison", "created"), 11);
    assert_eq!(count(&first, "report", "created"), 4);
    assert_eq!(first.curated.len(), 15);
    // Idempotent: nothing new the second time.
    let second = seed(&tdb, dir.path()).await;
    assert_eq!(count(&second, "comparison", "existing"), 11);
    assert_eq!(count(&second, "report", "existing"), 4);
    assert_eq!(second.curated.len(), 15);

    let server = Server::start(&tdb, dir.path(), Options::default()).await;

    // Anonymous visitors read every curated comparison and report.
    let (status, _, comparisons) = server.get("/api/v1/comparisons?limit=20", None).await;
    assert_eq!(status, 200, "{comparisons}");
    let comparisons = comparisons["items"].as_array().unwrap().clone();
    assert_eq!(comparisons.len(), 11);
    for c in &comparisons {
        let id = c["id"].as_str().unwrap();
        let (status, _, body) = server.get(&format!("/api/v1/comparisons/{id}"), None).await;
        assert_eq!(status, 200, "{body}");
        // Markdown bodies are counted exactly too.
        let before = server.state.egress.today(true);
        let (status, body) =
            get_raw(&server, &format!("/api/v1/comparisons/{id}/report"), None).await;
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&body).starts_with('#'));
        assert!(!body.is_empty());
        assert_eq!(server.state.egress.today(true) - before, body.len() as i64);
    }
    let (status, _, reports) = server.get("/api/v1/reports?limit=20", None).await;
    assert_eq!(status, 200, "{reports}");
    let reports = reports["items"].as_array().unwrap().clone();
    assert_eq!(reports.len(), 4);
    for r in &reports {
        let id = r["id"].as_str().unwrap();
        let before = server.state.egress.today(true);
        let (status, body) = get_raw(&server, &format!("/api/v1/reports/{id}/content"), None).await;
        assert_eq!(status, 200);
        assert_eq!(body.len(), r["size_bytes"].as_u64().unwrap() as usize);
        assert_eq!(server.state.egress.today(true) - before, body.len() as i64);
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
    assert_eq!(runs.len(), 12);
    let demo = server.state.db.workspace("demo").await.unwrap();
    let caller = bb_app::v1::auth::Caller {
        key: None,
        workspace: demo.clone(),
        demo,
    };
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
        let view = server
            .state
            .db
            .run_view(caller.workspace.id, run, &server.state.cfg.result_platform)
            .await
            .unwrap();
        let stored = server
            .state
            .db
            .run_analytics(view.run.id)
            .await
            .unwrap()
            .unwrap();
        let old: Value = serde_json::from_str(&stored.report_json).unwrap();
        assert_eq!(
            body,
            bb_app::v1::runs::analytics_body(&old, &server.state, &caller).unwrap(),
            "demo {run}: old and borrowed analytics bodies",
        );
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

    // Keyed run analytics stay within the keyed cap (4 MiB) and are counted exactly; so is the
    // run's Markdown report.
    let keyed_cap = server.state.cfg.max_response_bytes;
    assert_eq!(keyed_cap, 4 * MIB);
    for run in &runs {
        let before = server.state.egress.today(false);
        let (status, body) = get_raw(
            &server,
            &format!("/api/v1/runs/{run}/analytics"),
            Some(&key),
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        assert!(
            body.len() <= keyed_cap,
            "keyed analytics {} bytes",
            body.len()
        );
        assert_eq!(server.state.egress.today(false) - before, body.len() as i64);
    }
    let before = server.state.egress.today(true);
    let (status, body) = get_raw(&server, &format!("/api/v1/runs/{}/report", runs[0]), None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(String::from_utf8_lossy(&body).starts_with('#'));
    assert_eq!(server.state.egress.today(true) - before, body.len() as i64);

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

    // Small caps on the same data: anonymous analytics halve their series points until the
    // body fits (aggregates intact); a keyed cap below the aggregates answers 422.
    const SMALL: usize = 128 * 1024;
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            tweak: Box::new(|c| {
                c.anonymous_max_response_bytes = SMALL;
                c.max_response_bytes = 4096;
            }),
            ..Options::default()
        },
    )
    .await;
    // The restarted process adds today's persisted anonymous bytes (flushed by the first server)
    // once, at its first non-error response above 2 KiB.
    let mut stored = server.db.egress_today("anonymous").await.unwrap();
    assert!(stored > 0, "the first server flushed its anonymous egress");
    for path in runs
        .iter()
        .map(|run| format!("/api/v1/runs/{run}/analytics"))
        .chain(["/api/v1/datasets/synthetic-demo/analytics".to_string()])
    {
        let before = server.state.egress.today(true);
        let (status, body) = get_raw(&server, &path, None).await;
        assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&body));
        assert!(body.len() <= SMALL, "{path}: {} bytes", body.len());
        let expected = std::mem::take(&mut stored) + body.len() as i64;
        assert_eq!(server.state.egress.today(true) - before, expected);
        let value: Value = serde_json::from_slice(&body).unwrap();
        let downsampled = value["downsampled"].as_array().expect("series shortened");
        assert!(
            downsampled
                .iter()
                .all(|s| s["returned"].as_u64().unwrap()
                    < bb_app::analytics::MAX_SERIES_POINTS as u64),
            "{path}: {downsampled:?}"
        );
        eprintln!("{path}: {} bytes within a {SMALL}-byte cap", body.len());
    }
    let (status, body) = get_raw(
        &server,
        &format!("/api/v1/runs/{}/analytics", runs[0]),
        Some(&key),
    )
    .await;
    assert_eq!(status, 422, "{}", String::from_utf8_lossy(&body));
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "response_too_large", "{error}");
    assert_eq!(error["error"]["details"]["limit_bytes"], 4096);
    assert!(error["error"]["details"]["hint"].is_string());

    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversize_bodies_are_refused_and_egress_counts_true_bytes() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    const KEYED_BUDGET: i64 = 1_000_000;
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            tweak: Box::new(|c| {
                c.anonymous_max_response_bytes = 4096;
                c.egress_keyed_daily_bytes = KEYED_BUDGET;
            }),
            ..Options::default()
        },
    )
    .await;
    let (_, key) = workspace_key(&server.db, "lab", &[Scope::Read]).await;

    // The OpenAPI document is far above 4 KiB: refused for anonymous callers with 422
    // `response_too_large` (limit and hint), counted exactly.
    let before = server.state.egress.today(true);
    let (status, body) = get_raw(&server, "/api/v1/openapi.json", None).await;
    assert_eq!(status, 422);
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "response_too_large", "{error}");
    assert_eq!(error["error"]["details"]["limit_bytes"], 4096);
    assert!(
        error["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("API key")
    );
    assert_eq!(server.state.egress.today(true) - before, body.len() as i64);

    // Small anonymous responses still pass and are counted.
    let before = server.state.egress.today(true);
    let (status, body) = get_raw(&server, "/api/v1/status", None).await;
    assert_eq!(status, 200);
    assert!(!body.is_empty());
    assert_eq!(server.state.egress.today(true) - before, body.len() as i64);

    // Today's persisted keyed counter cannot be read: a large keyed response is refused (fail
    // closed), a small one is still served and counted.
    let pool = server.db.pool();
    sqlx::query("ALTER TABLE bb.egress_usage_daily RENAME TO egress_usage_daily_hidden")
        .execute(pool)
        .await
        .unwrap();
    let (status, body) = get_raw(&server, "/api/v1/openapi.json", Some(&key)).await;
    assert_eq!(status, 503, "{}", String::from_utf8_lossy(&body));
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["details"]["reason"], "database", "{error}");
    assert_eq!(server.state.egress.today(false), 0);
    let (status, body) = get_raw(&server, "/api/v1/whoami", Some(&key)).await;
    assert_eq!(status, 200);
    let small = body.len() as i64;
    assert_eq!(server.state.egress.today(false), small);
    sqlx::query("ALTER TABLE bb.egress_usage_daily_hidden RENAME TO egress_usage_daily")
        .execute(pool)
        .await
        .unwrap();

    // Bytes persisted by an earlier process today count against the budget after a restart:
    // the first large keyed response adds the stored value once, plus its exact length. The stored
    // value is derived from the document's size so the arithmetic holds as the API grows: one more
    // document fits the budget, two do not.
    let doc_len = bb_app::v1::OPENAPI_JSON.len() as i64;
    let stored = KEYED_BUDGET - small - doc_len - doc_len / 2;
    assert!(
        stored > 0,
        "KEYED_BUDGET is too small for a {doc_len}-byte document"
    );
    server.db.add_egress("keyed", stored).await.unwrap();
    let (status, body) = get_raw(&server, "/api/v1/openapi.json", Some(&key)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let doc = body.len() as i64;
    assert!(doc > 64 * 1024 && stored + small + doc <= KEYED_BUDGET);
    assert_eq!(server.state.egress.today(false), stored + small + doc);
    // The next one would pass the budget: refused, and its reservation is released.
    assert!(stored + small + 2 * doc > KEYED_BUDGET);
    let (status, body) = get_raw(&server, "/api/v1/openapi.json", Some(&key)).await;
    assert_eq!(status, 503, "{}", String::from_utf8_lossy(&body));
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["details"]["budget"], "egress", "{error}");
    assert_eq!(server.state.egress.today(false), stored + small + doc);

    // Once the budget is spent, every non-error body above 2 KiB is refused, not only large
    // ones, so small pages cannot keep draining it; tiny bodies and errors are still served.
    // The medium body: the admin list of a few dozen API keys.
    let mut admin = String::new();
    for _ in 0..30 {
        admin = workspace_key(&server.db, "ops", &[Scope::Read, Scope::Admin])
            .await
            .1;
    }
    server.stop().await;
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let (status, body) = get_raw(&server, "/api/v1/system/api-keys", Some(&admin)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(
        (4 * 1024..32 * 1024).contains(&body.len()),
        "{} bytes",
        body.len()
    );
    server.stop().await;
    // A restart with today's persisted keyed bytes above the budget: the first such body loads
    // the persisted counter and is refused.
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            tweak: Box::new(|c| c.egress_keyed_daily_bytes = 1_000),
            ..Options::default()
        },
    )
    .await;
    let (status, body) = get_raw(&server, "/api/v1/system/api-keys", Some(&admin)).await;
    assert_eq!(status, 503, "{}", String::from_utf8_lossy(&body));
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["details"]["budget"], "egress", "{error}");
    let (status, body) = get_raw(&server, "/api/v1/whoami", Some(&key)).await;
    assert_eq!(status, 200);
    assert!(body.len() <= 2048);
    let (status, _) = get_raw(&server, "/api/v1/runs/0000000000000000", Some(&key)).await;
    assert_eq!(status, 404);
    assert!(server.state.egress.today(false) > 1_000);

    server.stop().await;
    tdb.drop_db().await;
}
