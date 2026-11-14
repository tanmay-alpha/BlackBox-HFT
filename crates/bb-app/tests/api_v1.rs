//! End-to-end tests of `/api/v1` against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`) and a
//! filesystem artefact store, with the real server and in-process worker on 127.0.0.1:0.
//! Each test creates and drops its own database. Without the variable the tests skip.
//!
//! Covered: AT-3 (tamper detection + quarantine + re-simulation), AT-4 (single flight),
//! AT-5 (admission budget), AT-6 (progress, cancel queued/running, lease expiry requeue,
//! restart recovery), AT-10 (bounded pages), AT-11 / SEC-1 / SEC-2 / SEC-4 / SEC-5 / SEC-10
//! (auth matrix over openapi.json, IDOR, CORS/Host, size limits, forged keys without DB access),
//! the SIGTERM drain and restart persistence, and the wire rules on every response.

mod common;

use std::time::Duration;

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

fn strip_request_id(mut v: Value) -> Value {
    if let Some(e) = v.get_mut("error").and_then(Value::as_object_mut) {
        e.remove("request_id");
    }
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_matrix_idor_cors_limits_and_wire() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key_a) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let (_, read_a) = workspace_key(&server.db, "alpha", &[Scope::Read]).await;
    let (_, key_b) = workspace_key(&server.db, "beta", &[Scope::Run]).await;
    let (_, demo_read) = workspace_key(&server.db, "demo", &[Scope::Read]).await;
    let mut responses: Vec<(String, Value)> = Vec::new();

    // A runs two experiments, compares them and saves a report.
    let (run1, job1) = run(&server, &key_a, config_json(&manifest, 100, 1), "first").await;
    let (run2, _) = run(&server, &key_a, config_json(&manifest, 200, 1), "second").await;
    let (status, _, comparison) = server
        .post(
            "/api/v1/comparisons",
            Some(&key_a),
            &json!({"run_a": run1, "run_b": run2}),
        )
        .await;
    assert_eq!(status, 201, "{comparison}");
    let comparison_id = comparison["id"].as_str().unwrap().to_string();
    let (status, _, report) = server
        .post(
            "/api/v1/reports",
            Some(&key_a),
            &json!({"kind": "run", "subject_id": run1, "title": "Run one"}),
        )
        .await;
    assert_eq!(status, 201, "{report}");
    let report_id = report["id"].as_str().unwrap().to_string();
    let (_, _, experiments) = server.get("/api/v1/experiments", Some(&key_a)).await;
    let experiment_id = experiments["items"][0]["id"].as_str().unwrap().to_string();
    let job_id = job1["id"].as_str().unwrap().to_string();
    let (_, _, orders) = server
        .get(&format!("/api/v1/runs/{run1}/orders?limit=1"), Some(&key_a))
        .await;
    let order_id = orders["items"][0]["order_id"]
        .as_u64()
        .expect("run has orders");

    // Every read endpoint of A's objects works for A and is 404 for B, byte-identical to the
    // 404 of a random id (SEC-2).
    let random_uuid = uuid::Uuid::new_v4().to_string();
    let random_run = "0123456789abcdef";
    let routes: Vec<(String, String)> = vec![
        (
            format!("/api/v1/experiments/{experiment_id}"),
            format!("/api/v1/experiments/{random_uuid}"),
        ),
        (
            format!("/api/v1/jobs/{job_id}"),
            format!("/api/v1/jobs/{random_uuid}"),
        ),
        (
            format!("/api/v1/runs/{run1}"),
            format!("/api/v1/runs/{random_run}"),
        ),
        (
            format!("/api/v1/runs/{run1}/engine-check"),
            format!("/api/v1/runs/{random_run}/engine-check"),
        ),
        (
            format!("/api/v1/runs/{run1}/orders"),
            format!("/api/v1/runs/{random_run}/orders"),
        ),
        (
            format!("/api/v1/runs/{run1}/orders/{order_id}"),
            format!("/api/v1/runs/{random_run}/orders/{order_id}"),
        ),
        (
            format!("/api/v1/runs/{run1}/fills"),
            format!("/api/v1/runs/{random_run}/fills"),
        ),
        (
            format!("/api/v1/runs/{run1}/equity"),
            format!("/api/v1/runs/{random_run}/equity"),
        ),
        (
            format!("/api/v1/runs/{run1}/tape?limit=50"),
            format!("/api/v1/runs/{random_run}/tape?limit=50"),
        ),
        (
            format!("/api/v1/runs/{run1}/timeline"),
            format!("/api/v1/runs/{random_run}/timeline"),
        ),
        (
            format!("/api/v1/runs/{run1}/analytics"),
            format!("/api/v1/runs/{random_run}/analytics"),
        ),
        (
            format!("/api/v1/comparisons/{comparison_id}"),
            format!("/api/v1/comparisons/{random_uuid}"),
        ),
        (
            format!("/api/v1/comparisons/{comparison_id}/pairs"),
            format!("/api/v1/comparisons/{random_uuid}/pairs"),
        ),
        (
            format!("/api/v1/reports/{report_id}"),
            format!("/api/v1/reports/{random_uuid}"),
        ),
    ];
    for (own, random) in &routes {
        let (status, _, body) = server.get(own, Some(&key_a)).await;
        assert!(status == 200 || status == 409, "A {own}: {status} {body}");
        if status == 200 {
            responses.push((own.clone(), body));
        }
        let (status_b, _, body_b) = server.get(own, Some(&key_b)).await;
        let (status_r, _, body_r) = server.get(random, Some(&key_b)).await;
        assert_eq!(status_b, 404, "B {own}: {body_b}");
        assert_eq!(status_r, 404, "random {random}: {body_r}");
        assert_eq!(strip_request_id(body_b), strip_request_id(body_r), "{own}");
    }
    for path in [
        format!("/api/v1/runs/{run1}/report"),
        format!("/api/v1/reports/{report_id}/content"),
        format!("/api/v1/comparisons/{comparison_id}/report"),
        format!("/api/v1/runs/{run1}/export"),
    ] {
        let r = server
            .http
            .get(server.url(&path))
            .bearer_auth(&key_a)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{path}");
        let r = server
            .http
            .get(server.url(&path))
            .bearer_auth(&key_b)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404, "{path}");
    }
    // Lists with B's key never contain A's objects; ?workspace=<A> is 404 like a random slug.
    for list in [
        "/api/v1/experiments",
        "/api/v1/jobs",
        "/api/v1/runs",
        "/api/v1/comparisons",
        "/api/v1/reports",
    ] {
        let (status, _, body) = server.get(list, Some(&key_b)).await;
        assert_eq!(status, 200);
        assert_eq!(body["items"].as_array().unwrap().len(), 0, "{list}: {body}");
        let (_, _, own) = server.get(list, Some(&key_a)).await;
        responses.push((list.to_string(), own));
    }
    let (s1, _, b1) = server
        .get("/api/v1/experiments?workspace=alpha", Some(&key_b))
        .await;
    let (s2, _, b2) = server
        .get("/api/v1/experiments?workspace=nosuch", Some(&key_b))
        .await;
    assert_eq!((s1, s2), (404, 404));
    assert_eq!(strip_request_id(b1), strip_request_id(b2));
    // B comparing A's runs is 404.
    let (status, _, _) = server
        .post(
            "/api/v1/comparisons",
            Some(&key_b),
            &json!({"run_a": run1, "run_b": run2}),
        )
        .await;
    assert_eq!(status, 404);

    // SEC-1: table-driven over every operation of openapi.json.
    let (_, _, spec) = server.get("/api/v1/openapi.json", None).await;
    let paths = spec["paths"].as_object().expect("paths");
    assert!(paths.len() >= 40, "only {} paths", paths.len());
    let forged = format!("bbk_{}", "A".repeat(43));
    for (path, ops) in paths {
        let concrete = path
            .replace("{run_id}", &run1)
            .replace("{dataset_id}", DATASET)
            .replace("{version}", "1")
            .replace("{order_id}", "1")
            .replace("{id}", &uuid::Uuid::new_v4().to_string());
        let full = concrete;
        for method in ops.as_object().unwrap().keys() {
            let m = reqwest::Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let mutation = m != reqwest::Method::GET;
            if full == "/healthz" {
                continue;
            }
            // Anonymous mutation: 401 with a Bearer challenge, before the (empty) body is read.
            if mutation {
                let r = server
                    .http
                    .request(m.clone(), server.url(&format!("{full}?api_key={key_a}")))
                    .header("content-type", "application/json")
                    .body("{}")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 401, "anonymous {method} {full}");
                assert!(
                    r.headers()["www-authenticate"]
                        .to_str()
                        .unwrap()
                        .starts_with("Bearer"),
                    "{full}"
                );
                // A read key on a mutation: 403.
                let r = server
                    .http
                    .request(m.clone(), server.url(&full))
                    .bearer_auth(&read_a)
                    .header("content-type", "application/json")
                    .body("{}")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 403, "read key {method} {full}");
                // The read-only demo key: 403 too.
                let r = server
                    .http
                    .request(m.clone(), server.url(&full))
                    .bearer_auth(&demo_read)
                    .header("content-type", "application/json")
                    .body("{}")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 403, "demo read key {method} {full}");
            }
            // Malformed, forged and unknown keys: 401 on every route, reads included.
            for bad in ["bbk_not-a-real-key", forged.as_str()] {
                let r = server
                    .http
                    .request(m.clone(), server.url(&full))
                    .bearer_auth(bad)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 401, "bad key {method} {full}");
            }
            // `run` keys never reach operator routes.
            if path.starts_with("/api/v1/system") || path == "/metrics" {
                let r = server
                    .http
                    .request(m.clone(), server.url(&full))
                    .bearer_auth(&key_a)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 403, "run key {method} {full}");
            }
        }
    }
    // A run key writing to the demo workspace: 403.
    let (status, _, _) = server
        .post(
            "/api/v1/experiments?workspace=demo",
            Some(&key_a),
            &json!({"config": config_json(&manifest, 100, 9)}),
        )
        .await;
    assert_eq!(status, 403);
    // `/readyz?probe=1` needs admin.
    let (status, _, _) = server.get("/readyz?probe=1", Some(&key_a)).await;
    assert_eq!(status, 403);

    // SEC-4: CORS and Host.
    let preflight = |origin: &str| {
        server
            .http
            .request(reqwest::Method::OPTIONS, server.url("/api/v1/experiments"))
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .send()
    };
    let r = preflight(ALLOWED_ORIGIN).await.unwrap();
    assert!(r.status().is_success());
    assert_eq!(r.headers()["access-control-allow-origin"], ALLOWED_ORIGIN);
    assert!(
        r.headers()
            .get("access-control-allow-credentials")
            .is_none()
    );
    let r = preflight("https://evil.example").await.unwrap();
    assert!(r.headers().get("access-control-allow-origin").is_none());
    let r = server
        .http
        .get(server.url("/api/v1/status"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = server
        .http
        .get(server.url("/api/v1/status"))
        .header("origin", ALLOWED_ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], ALLOWED_ORIGIN);
    let vary: Vec<_> = r
        .headers()
        .get_all("vary")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(vary.iter().any(|v| v.contains("Origin")), "{vary:?}");
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    let r = server
        .http
        .get(server.url("/api/v1/status"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // SEC-5: request size limits. The server answers 413 from `Content-Length` and closes the
    // connection without reading the body, so a client still streaming 1 MiB can see a broken
    // pipe before the response (observed on Linux CI). Send only the headers and read the
    // status line instead.
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let head = format!(
            "POST /api/v1/experiments HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {key_a}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            server.addr,
            (1 << 20) + 1
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
            .await
            .expect("server answers an oversized declared body without waiting for it")
            .unwrap();
        let status_line = String::from_utf8_lossy(&buf[..n]);
        assert!(
            status_line.starts_with("HTTP/1.1 413"),
            "oversized body: {status_line}"
        );
    }
    let r = server
        .http
        .get(server.url("/api/v1/status"))
        .header("x-padding", "y".repeat(20 * 1024))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 431);
    let r = server
        .http
        .post(server.url("/api/v1/experiments"))
        .bearer_auth(&key_a)
        .header("content-type", "application/json")
        .header("content-encoding", "gzip")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 415);
    // SEC-7: label bounds.
    let (status, _, _) = server
        .post(
            "/api/v1/experiments",
            Some(&key_a),
            &json!({"config": config_json(&manifest, 100, 3), "label": "a\u{1b}b"}),
        )
        .await;
    assert_eq!(status, 422);

    // SEC-10 / SEC-31: forged keys and anonymous public reads do not touch the database.
    for path in [
        "/api/v1/datasets",
        "/api/v1/runs",
        "/api/v1/status",
        "/readyz",
        "/healthz",
    ] {
        let _ = server.get(path, None).await; // warm-up
    }
    let before = bb_app::metrics::Metrics::get(&server.metrics.db_acquires_total);
    for i in 0..1000 {
        let path = [
            "/api/v1/datasets",
            "/api/v1/runs",
            "/api/v1/status",
            "/readyz",
            "/healthz",
        ][i % 5];
        let (status, _, _) = server.get(path, None).await;
        assert_eq!(status, 200, "{path}");
    }
    for _ in 0..1000 {
        let key = format!("bbk_{}", uuid::Uuid::new_v4().simple())
            .chars()
            .chain("abcdefghijk".chars())
            .collect::<String>();
        let (status, _, _) = server.get("/api/v1/whoami", Some(&key)).await;
        assert_eq!(status, 401);
    }
    let after = bb_app::metrics::Metrics::get(&server.metrics.db_acquires_total);
    assert_eq!(
        after, before,
        "public and forged-key requests used the database"
    );

    // Whoami and status.
    let (_, _, who) = server.get("/api/v1/whoami", Some(&key_a)).await;
    assert_eq!(who["workspace"]["slug"], "alpha");
    responses.push(("whoami".into(), who));
    let (_, _, status) = server.get("/api/v1/status", None).await;
    assert_eq!(
        status["git_sha"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    responses.push(("status".into(), status));
    let (_, _, ds) = server
        .get(&format!("/api/v1/datasets/{DATASET}"), None)
        .await;
    responses.push(("dataset".into(), ds));
    let (_, _, demos) = server.get("/api/v1/demos", None).await;
    responses.push(("demos".into(), demos));

    // The wire rules hold on every collected response.
    for (path, body) in &responses {
        assert_wire_safe(body, path);
    }
    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at4_single_flight_and_at5_admission() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let body = json!({"config": config_json(&manifest, 100, 42), "label": "first label"});
    let mut handles = Vec::new();
    for i in 0..8 {
        let http = server.http.clone();
        let url = server.url("/api/v1/experiments");
        let key = key.clone();
        let mut b = body.clone();
        b["label"] = json!(format!("label {i}"));
        if i == 0 {
            b["label"] = json!("first label");
        }
        handles.push(tokio::spawn(async move {
            response(
                http.post(url)
                    .bearer_auth(key)
                    .json(&b)
                    .send()
                    .await
                    .unwrap(),
            )
            .await
        }));
    }
    let mut jobs = std::collections::BTreeSet::new();
    let mut created = 0;
    for h in handles {
        let (status, _, b) = h.await.unwrap();
        assert!(status == 202 || status == 200, "{b}");
        if b["created"] == true {
            created += 1;
        }
        if let Some(id) = b["job"]["id"].as_str() {
            jobs.insert(id.to_string());
        }
    }
    assert_eq!(created, 1, "exactly one submission creates the experiment");
    assert_eq!(
        jobs.len(),
        1,
        "one job for 8 identical submissions: {jobs:?}"
    );
    let job = wait_job(&server, Some(&key), jobs.iter().next().unwrap()).await;
    assert_eq!(job["state"], "completed");
    // A ninth identical submission returns the run, no job.
    let (status, _, b) = server.post("/api/v1/experiments", Some(&key), &body).await;
    assert_eq!(status, 200);
    assert!(b["job"].is_null() && b["run"]["run_id"].is_string(), "{b}");
    assert_eq!(
        bb_app::metrics::Metrics::get(&server.metrics.simulations_started_total),
        1,
        "exactly one simulation"
    );
    server.stop().await;

    // AT-5: under hosted limits an over-budget configuration is refused before queueing.
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            tweak: Box::new(|c| c.limits = bb_core::limits::ExecutionLimits::hosted()),
            ..Options::default()
        },
    )
    .await;
    let mut over = config_json(&manifest, 1, 5);
    // 1 ms interval over 20 s is fine; make the strategy interval tiny relative to limits by
    // using a long dataset instead: register a 300 s dataset under another id.
    let registry = bb_app::registry::DatasetRegistry::new(dir.path());
    bb_app::registry::register_synthetic(
        &server.db,
        &registry,
        &bb_data::synthetic::SyntheticConfig {
            id: "long-synth".into(),
            duration_ns: 300_000_000_000,
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    over["dataset_id"] = json!("long-synth");
    let (status, _, b) = server
        .post("/api/v1/experiments", Some(&key), &json!({"config": over}))
        .await;
    assert_eq!(status, 422, "{b}");
    assert_eq!(b["error"]["code"], "budget_exceeded");
    assert_eq!(b["error"]["details"]["profile"], "local");
    let (_, _, jobs) = server.get("/api/v1/jobs?state=queued", Some(&key)).await;
    assert_eq!(jobs["items"].as_array().unwrap().len(), 0);
    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at6_progress_cancel_lease_drain_and_restart() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    // No worker: jobs stay queued.
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            ..Options::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 30).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let (status, _, b) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config_json(&manifest, 100, 1)}),
        )
        .await;
    assert_eq!(status, 202, "{b}");
    assert_eq!(b["job"]["state"], "queued");
    assert_eq!(b["job"]["queue_position"], 1);
    let queued_job = b["job"]["id"].as_str().unwrap().to_string();
    let (status, _, cancelled) = server
        .post(
            &format!("/api/v1/jobs/{queued_job}/cancel"),
            Some(&key),
            &json!({}),
        )
        .await;
    assert_eq!(status, 202);
    assert_eq!(cancelled["state"], "cancelled");
    let (status, _, again) = server
        .post(
            &format!("/api/v1/jobs/{queued_job}/cancel"),
            Some(&key),
            &json!({}),
        )
        .await;
    assert_eq!((status, again["state"].as_str()), (202, Some("cancelled")));

    // A job claimed by a worker that then crashes: the lease expires and the job is re-queued
    // once (lease_losses = 1) and completed by the next worker.
    let (_, _, b) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config_json(&manifest, 100, 2)}),
        )
        .await;
    let crashed_job = b["job"]["id"].as_str().unwrap().to_string();
    let platform = bb_core::platform::platform_class();
    let claimed = server
        .db
        .claim_fair("crashed-worker", &platform, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id.to_string(), crashed_job);
    server.stop().await;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Long job for progress, running cancel and drain.
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let mut reaped = Value::Null;
    for _ in 0..300 {
        let (_, _, j) = server
            .get(&format!("/api/v1/jobs/{crashed_job}"), Some(&key))
            .await;
        if j["lease_losses"] == 1 {
            reaped = j;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(reaped["lease_losses"], 1, "{reaped}");
    // The reaper backs off 30 s; make it runnable now.
    sqlx::query("UPDATE bb.jobs SET run_after=now() WHERE id=$1::uuid")
        .bind(&crashed_job)
        .execute(server.db.pool())
        .await
        .unwrap();
    server.hub.submitted();
    let done = wait_job(&server, Some(&key), &crashed_job).await;
    assert_eq!(done["state"], "completed", "{done}");
    assert_eq!(done["attempt"], 2);

    let (_, _, b) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config_json(&manifest, 1, 7)}),
        )
        .await;
    let long_job = b["job"]["id"].as_str().unwrap().to_string();
    let mut last = 0u64;
    let mut seen_progress = false;
    for _ in 0..600 {
        let (_, _, job) = server
            .get(&format!("/api/v1/jobs/{long_job}"), Some(&key))
            .await;
        if let Some(done) = job["progress"]["done"].as_u64() {
            assert!(done >= last, "progress went backwards: {last} -> {done}");
            last = done;
            seen_progress = true;
            if done > 0 && job["state"] == "running" {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(seen_progress, "no progress observed");
    let (status, _, c) = server
        .post(
            &format!("/api/v1/jobs/{long_job}/cancel"),
            Some(&key),
            &json!({}),
        )
        .await;
    assert_eq!(status, 202, "{c}");
    assert!(
        matches!(c["state"].as_str(), Some("cancelling" | "cancelled")),
        "{c}"
    );
    let job = wait_job(&server, Some(&key), &long_job).await;
    assert_eq!(job["state"], "cancelled");
    assert!(job["run_id"].is_null());

    // SIGTERM drain: a running job is released (queued, lease_losses unchanged) and the process
    // stops within the grace period.
    let (_, _, b) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config_json(&manifest, 1, 8)}),
        )
        .await;
    let drained_job = b["job"]["id"].as_str().unwrap().to_string();
    wait_job_state(&server, &key, &drained_job, &["running"]).await;
    let started = std::time::Instant::now();
    let db = tdb.pool().await;
    server.stop().await;
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "drain took {:?}",
        started.elapsed()
    );
    let row = db.job(drained_job.parse().unwrap()).await.unwrap();
    assert!(
        matches!(row.state.as_str(), "queued" | "completed"),
        "{}",
        row.state
    );
    assert_eq!(row.lease_losses, 0);

    // Restart: the released job completes, and results survive another restart.
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let job = wait_job(&server, Some(&key), &drained_job).await;
    assert_eq!(job["state"], "completed", "{job}");
    let run_id = job["run_id"].as_str().unwrap().to_string();
    server.stop().await;
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let (status, _, fills) = server
        .get(&format!("/api/v1/runs/{run_id}/fills"), Some(&key))
        .await;
    assert_eq!(status, 200, "{fills}");
    let (status, _, summary) = server
        .get(&format!("/api/v1/runs/{run_id}"), Some(&key))
        .await;
    assert_eq!(status, 200, "{summary}");
    server.stop().await;
    tdb.drop_db().await;
}

async fn wait_job_state(server: &Server, key: &str, job: &str, states: &[&str]) -> Value {
    for _ in 0..1500 {
        let (_, _, j) = server.get(&format!("/api/v1/jobs/{job}"), Some(key)).await;
        if states.contains(&j["state"].as_str().unwrap_or("")) {
            return j;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("job {job} never reached {states:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at3_tampering_is_detected_and_at10_pages_are_bounded() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (_, key) = workspace_key(&server.db, "alpha", &[Scope::Run]).await;
    let config = config_json(&manifest, 100, 11);
    let (run_id, _) = run(&server, &key, config.clone(), "tamper me").await;

    // AT-10: bounded, resumable pages.
    let (status, _, _) = server
        .get(
            &format!("/api/v1/runs/{run_id}/orders?limit=2001"),
            Some(&key),
        )
        .await;
    assert_eq!(status, 400);
    let (_, _, summary) = server
        .get(&format!("/api/v1/runs/{run_id}"), Some(&key))
        .await;
    let tape_len = summary["counts"]["tape_events"].as_u64().unwrap();
    let mut seen = 0u64;
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(c) => format!("/api/v1/runs/{run_id}/tape?limit=100&cursor={c}"),
            None => format!("/api/v1/runs/{run_id}/tape?limit=100"),
        };
        let (status, _, page) = server.get(&path, Some(&key)).await;
        assert_eq!(status, 200, "{page}");
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= 100);
        for (i, e) in items.iter().enumerate() {
            assert_eq!(e["seq"].as_u64().unwrap(), seen + i as u64);
        }
        seen += items.len() as u64;
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(seen, tape_len);

    // The timeline pages its window's events by seq: every event exactly once, then no cursor;
    // the cursor is bound to the window.
    let window = "t0_ns=-1000000000000&t1_ns=1000000000000000";
    let mut seqs: Vec<u64> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first_cursor: Option<String> = None;
    loop {
        let mut path = format!("/api/v1/runs/{run_id}/timeline?{window}&max_events=7");
        if let Some(c) = &cursor {
            path.push_str(&format!("&cursor={c}"));
        }
        let (status, _, page) = server.get(&path, Some(&key)).await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["events_total"].as_u64().unwrap(), tape_len);
        assert!(page["events"].as_array().unwrap().len() <= 7);
        if cursor.is_some() {
            assert_eq!(page["orders"], json!([]), "context on a cursor page");
        }
        seqs.extend(
            page["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["seq"].as_u64().unwrap()),
        );
        match page["next_cursor"].as_str() {
            Some(c) => {
                assert_eq!(page["truncated_by"], "limit");
                first_cursor.get_or_insert_with(|| c.to_string());
                cursor = Some(c.to_string());
            }
            None => break,
        }
    }
    assert_eq!(seqs, (0..tape_len).collect::<Vec<_>>());
    let (status, _, err) = server
        .get(
            &format!(
                "/api/v1/runs/{run_id}/timeline?t0_ns=0&t1_ns=1000000000000000&cursor={}",
                first_cursor.expect("more than one page")
            ),
            Some(&key),
        )
        .await;
    assert_eq!(status, 400, "{err}");
    assert_eq!(err["error"]["details"]["field"], "cursor");

    // AT-3 (summary): a changed metric in the database row is caught by summary_sha256.
    let (run2, _) = run(&server, &key, config_json(&manifest, 200, 12), "row tamper").await;
    sqlx::query("ALTER TABLE bb.runs DISABLE TRIGGER runs_immutable")
        .execute(server.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE bb.runs r SET metrics=jsonb_set(metrics,'{net_pnl}',to_jsonb((metrics->>'net_pnl')::float8+1)) FROM bb.experiment_configs c WHERE c.id=r.experiment_config_id AND c.short_id=$1")
        .bind(&run2)
        .execute(server.db.pool())
        .await
        .unwrap();
    sqlx::query("ALTER TABLE bb.runs ENABLE TRIGGER runs_immutable")
        .execute(server.db.pool())
        .await
        .unwrap();
    let (status, _, b) = server
        .get(&format!("/api/v1/runs/{run2}"), Some(&key))
        .await;
    assert_eq!(status, 500, "{b}");
    assert_eq!(b["error"]["code"], "integrity_failed");
    assert_eq!(b["error"]["details"]["object"], "summary");

    // AT-3 (cache copy): a corrupted local cache copy is refetched, not served.
    let cache_dir = dir.path().join("cache").join("bundles");
    for entry in std::fs::read_dir(&cache_dir).unwrap() {
        let path = entry.unwrap().path();
        let mut bytes = std::fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
    }
    let (status, _, b) = server
        .get(&format!("/api/v1/runs/{run_id}/fills"), Some(&key))
        .await;
    assert_eq!(status, 200, "a corrupted cache copy must be refetched: {b}");
    server.stop().await;

    // AT-3 (stored bundle): flip one byte inside the fills pages of the stored object; a fresh
    // instance (empty cache) detects it on every read endpoint and quarantines the run.
    let objects = dir.path().join("artifacts");
    let trust: Value = {
        let db = tdb.pool().await;
        let row: (Value,) = sqlx::query_as("SELECT a.page_index FROM bb.run_artifacts a JOIN bb.runs r ON r.id=a.run_id JOIN bb.experiment_configs c ON c.id=r.experiment_config_id WHERE c.short_id=$1 AND r.quarantined_at IS NULL")
            .bind(&run_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        row.0
    };
    let page = trust["page_index"]["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["section"] == "fills")
        .or_else(|| trust["page_index"]["pages"].as_array().unwrap().first())
        .unwrap()
        .clone();
    let offset = page["offset"].as_u64().unwrap() + page["len"].as_u64().unwrap() / 2;
    let mut tampered = false;
    for entry in walk(&objects) {
        if entry.extension().is_some_and(|e| e == "bbz")
            && entry.to_string_lossy().contains(&run_id)
        {
            let mut bytes = std::fs::read(&entry).unwrap();
            bytes[offset as usize] ^= 0x01;
            std::fs::write(&entry, bytes).unwrap();
            tampered = true;
        }
    }
    assert!(tampered, "stored object not found");
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let (status, _, b) = server
        .get(&format!("/api/v1/runs/{run_id}/fills"), Some(&key))
        .await;
    assert_eq!(status, 500, "{b}");
    assert_eq!(b["error"]["code"], "integrity_failed");
    for path in ["", "/orders", "/tape", "/equity", "/analytics"] {
        let (status, _, b) = server
            .get(&format!("/api/v1/runs/{run_id}{path}"), Some(&key))
            .await;
        assert_eq!(status, 500, "{path}: {b}");
        assert_eq!(b["error"]["code"], "integrity_failed", "{path}");
    }
    // The next identical submission re-simulates (the quarantined run is never reused).
    let (status, _, b) = server
        .post(
            "/api/v1/experiments",
            Some(&key),
            &json!({"config": config}),
        )
        .await;
    assert_eq!(status, 200, "{b}");
    let job_id = b["job"]["id"].as_str().expect("a new job").to_string();
    let job = wait_job(&server, Some(&key), &job_id).await;
    assert_eq!(job["state"], "completed");
    let (status, _, b) = server
        .get(&format!("/api/v1/runs/{run_id}/fills"), Some(&key))
        .await;
    assert_eq!(status, 200, "{b}");
    server.stop().await;
    tdb.drop_db().await;
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}
