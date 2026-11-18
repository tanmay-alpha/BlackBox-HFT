//! SECURITY_REVIEW §15 / SEC-25 audit matrix against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`,
//! else skip): every audited action writes rows with the right actor, workspace, key id, request
//! id, action, outcome, timestamp and safe metadata (never a raw key); keyed `auth.denied` rows
//! are throttled to one per key per minute; anonymous and forged-key traffic writes nothing (and
//! a forged key touches no database at all).

mod common;

use std::collections::BTreeSet;

use bb_app::artifacts::{ReadBudget, Storage};
use bb_db::{AuditEvent, Database, Scope};
use chrono::{DateTime, Utc};
use common::*;
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

macro_rules! db_or_skip {
    () => {
        match TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}

async fn send(
    server: &Server,
    method: Method,
    path: &str,
    key: Option<&str>,
    if_match: Option<&str>,
    body: Option<Value>,
) -> (u16, reqwest::header::HeaderMap, Value) {
    let mut req = server.http.request(method, server.url(path));
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    if let Some(tag) = if_match {
        req = req.header("If-Match", tag);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    response(req.send().await.unwrap()).await
}

fn request_id(headers: &reqwest::header::HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("x-request-id")
        .to_string()
}

async fn key_id(db: &Database, plaintext: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM bb.api_keys WHERE display_prefix=$1")
        .bind(&plaintext[..8])
        .fetch_one(db.pool())
        .await
        .unwrap()
}

const COLUMNS: &str = "SELECT id,at,workspace_id,api_key_id,action,outcome,target_type,target_id,request_id,host(client_ip) AS client_ip,detail FROM bb.audit_events";

async fn rows(db: &Database, action: &str, outcome: &str) -> Vec<AuditEvent> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COLUMNS} WHERE action=$1 AND outcome=$2 ORDER BY id"
    )))
    .bind(action)
    .bind(outcome)
    .fetch_all(db.pool())
    .await
    .unwrap()
}

async fn all_rows(db: &Database) -> Vec<AuditEvent> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLUMNS} ORDER BY id")))
        .fetch_all(db.pool())
        .await
        .unwrap()
}

async fn audit_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM bb.audit_events")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

async fn db_now(db: &Database) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// The expected attribution of one audit row.
struct Expect<'a> {
    workspace: Option<Uuid>,
    key: Option<Uuid>,
    request: Option<&'a str>,
    target_type: &'a str,
    target: &'a str,
}

/// Exactly one row of `action`/`outcome` for `target`, attributed as expected; returns its detail.
async fn one(db: &Database, action: &str, outcome: &str, e: Expect<'_>) -> Value {
    let found: Vec<AuditEvent> = rows(db, action, outcome)
        .await
        .into_iter()
        .filter(|r| r.target_id.as_deref() == Some(e.target))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{action}/{outcome} for {}: {found:?}",
        e.target
    );
    let r = &found[0];
    assert_eq!(r.workspace_id, e.workspace, "{action} workspace");
    assert_eq!(r.api_key_id, e.key, "{action} key");
    assert_eq!(r.request_id.as_deref(), e.request, "{action} request id");
    assert_eq!(
        r.target_type.as_deref(),
        Some(e.target_type),
        "{action} target type"
    );
    assert!(r.detail.is_object(), "{action} detail {}", r.detail);
    r.detail.clone()
}

/// Every §15 action with its attribution: CLI dataset registration (ok and licence denial), key
/// mint and revoke, throttled `auth.denied` for revoked, expired and under-scoped keys, every
/// v1 mutation, `demo.write`, a sweep's derived dataset registrations (attributed to the key and
/// request), `quota.exceeded` and `storage.budget_exceeded`. No row carries a raw key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_section15_action_writes_attributed_rows_without_secrets() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let setup = tdb.pool().await;
    let started = db_now(&setup).await;

    // ----- phase 1: no worker, so jobs and sweeps stay cancellable -----
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            ..Options::default()
        },
    )
    .await;
    let db = server.db.clone();

    // dataset.create (CLI): one row for the inserted version, none for an identical repeat.
    let manifest = register_dataset(&db, dir.path(), 20).await;
    register_dataset(&db, dir.path(), 20).await;
    let detail = one(
        &db,
        "dataset.create",
        "ok",
        Expect {
            workspace: None,
            key: None,
            request: None,
            target_type: "dataset",
            target: DATASET,
        },
    )
    .await;
    assert_eq!(detail["actor"], "cli");
    assert_eq!(detail["version_no"], 1);
    assert_eq!(detail["events_sha256"], manifest.events_sha256);
    // dataset.create denied (licence gate): a recorded source in the hosted profile.
    let mut recorded = manifest.clone();
    recorded.id = "recorded-matrix".into();
    recorded.kind = bb_core::dataset::DatasetKind::Recorded;
    bb_app::registry::register(&db, &recorded, "fs", true)
        .await
        .unwrap_err();
    let detail = one(
        &db,
        "dataset.create",
        "denied",
        Expect {
            workspace: None,
            key: None,
            request: None,
            target_type: "dataset",
            target: "recorded-matrix",
        },
    )
    .await;
    assert_eq!(
        detail,
        json!({"actor": "cli", "reason": "source is not hostable"})
    );

    // key.mint (CLI): one row per key in its workspace, with no actor key or request.
    let (alpha, key) = workspace_key(&db, "alpha", &[Scope::Run]).await;
    let (_, reader) = workspace_key(&db, "alpha", &[Scope::Read]).await;
    let (_, runner) = workspace_key(&db, "alpha", &[Scope::Run]).await;
    let (_, gone) = workspace_key(&db, "alpha", &[Scope::Run]).await;
    let (_, stale) = workspace_key(&db, "alpha", &[Scope::Run]).await;
    let (ops, admin) = workspace_key(&db, "ops", &[Scope::Admin]).await;
    let (demo, demo_admin) = workspace_key(&db, "demo", &[Scope::Admin]).await;
    let plaintexts = [&key, &reader, &runner, &gone, &stale, &admin, &demo_admin];
    let (kid, reader_id, runner_id, gone_id, stale_id, admin_id, demo_admin_id) = (
        key_id(&db, &key).await,
        key_id(&db, &reader).await,
        key_id(&db, &runner).await,
        key_id(&db, &gone).await,
        key_id(&db, &stale).await,
        key_id(&db, &admin).await,
        key_id(&db, &demo_admin).await,
    );
    for (id, ws) in [
        (kid, alpha),
        (reader_id, alpha),
        (runner_id, alpha),
        (gone_id, alpha),
        (stale_id, alpha),
        (admin_id, ops),
        (demo_admin_id, demo),
    ] {
        let detail = one(
            &db,
            "key.mint",
            "ok",
            Expect {
                workspace: Some(ws),
                key: None,
                request: None,
                target_type: "api_key",
                target: &id.to_string(),
            },
        )
        .await;
        assert!(detail["display_prefix"].is_string());
    }

    // key.revoke (admin API): the revoked key's workspace, the admin's key and request.
    let (status, headers, _) = send(
        &server,
        Method::POST,
        &format!("/api/v1/system/api-keys/{gone_id}/revoke"),
        Some(&admin),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200);
    one(
        &db,
        "key.revoke",
        "ok",
        Expect {
            workspace: Some(alpha),
            key: Some(admin_id),
            request: Some(&request_id(&headers)),
            target_type: "api_key",
            target: &gone_id.to_string(),
        },
    )
    .await;

    // auth.denied: revoked and expired keys (401), a read key mutating and a run key on an
    // admin route or a demo write (403); one row per key, repeats within the minute add none.
    sqlx::query("UPDATE bb.api_keys SET created_at=now()-interval '2 minutes',expires_at=now()-interval '1 minute' WHERE id=$1")
        .bind(stale_id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut denials = Vec::new();
    for (plaintext, method, path, status) in [
        (&gone, Method::GET, "/api/v1/runs", 401),
        (&stale, Method::GET, "/api/v1/experiments", 401),
        (&reader, Method::POST, "/api/v1/experiments", 403),
        (&runner, Method::GET, "/api/v1/system/queue", 403),
    ] {
        let body = (method == Method::POST).then(|| json!({}));
        let (got, headers, b) =
            send(&server, method.clone(), path, Some(plaintext), None, body).await;
        assert_eq!(got, status, "{path}: {b}");
        denials.push(request_id(&headers));
        // Repeats (another route for the run key) are throttled.
        for _ in 0..3 {
            let (got, _, _) = send(
                &server,
                Method::POST,
                "/api/v1/experiments?workspace=demo",
                Some(plaintext),
                None,
                Some(json!({"config": config_json(&manifest, 100, 1)})),
            )
            .await;
            assert!(got == 401 || got == 403, "{got}");
        }
    }
    for ((id, reason, scope, method, path), request) in [
        (gone_id, "revoked", None, "GET", "/api/v1/runs"),
        (stale_id, "expired", None, "GET", "/api/v1/experiments"),
        (
            reader_id,
            "scope",
            Some("run"),
            "POST",
            "/api/v1/experiments",
        ),
        (
            runner_id,
            "scope",
            Some("admin"),
            "GET",
            "/api/v1/system/queue",
        ),
    ]
    .into_iter()
    .zip(&denials)
    {
        let detail = one(
            &db,
            "auth.denied",
            "denied",
            Expect {
                workspace: Some(alpha),
                key: Some(id),
                request: Some(request),
                target_type: "api_key",
                target: &id.to_string(),
            },
        )
        .await;
        let mut expected = json!({"reason": reason, "method": method, "path": path});
        if let Some(scope) = scope {
            expected["required_scope"] = scope.into();
        }
        assert_eq!(detail, expected);
    }
    assert!(
        rows(&db, "auth.denied", "denied")
            .await
            .iter()
            .all(|r| r.client_ip.is_some())
    );

    // Experiment create, rename, cancel, delete.
    let (status, headers, submitted) = send(
        &server,
        Method::POST,
        "/api/v1/experiments",
        Some(&key),
        None,
        Some(json!({"config": config_json(&manifest, 100, 9), "label": "queued"})),
    )
    .await;
    assert_eq!(status, 202, "{submitted}");
    let experiment = submitted["experiment"]["id"].as_str().unwrap().to_string();
    let job = submitted["job"]["id"].as_str().unwrap().to_string();
    // Rows of the run key's own mutations in alpha, under the response's request id.
    let mine = attributed(move |request| Expect {
        workspace: Some(alpha),
        key: Some(kid),
        request: Some(request),
        target_type: "",
        target: "",
    });
    one(
        &db,
        "experiment.create",
        "ok",
        Expect {
            target_type: "experiment",
            target: &experiment,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let path = format!("/api/v1/experiments/{experiment}");
    let version = submitted["experiment"]["version"].as_i64().unwrap();
    let (status, headers, _) = send(
        &server,
        Method::PATCH,
        &path,
        Some(&key),
        Some(&format!("\"v{version}\"")),
        Some(json!({"label": "renamed"})),
    )
    .await;
    assert_eq!(status, 200);
    one(
        &db,
        "experiment.rename",
        "ok",
        Expect {
            target_type: "experiment",
            target: &experiment,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, _) = send(
        &server,
        Method::POST,
        &format!("/api/v1/jobs/{job}/cancel"),
        Some(&key),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 202);
    one(
        &db,
        "job.cancel",
        "ok",
        Expect {
            target_type: "job",
            target: &job,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, _) = send(&server, Method::DELETE, &path, Some(&key), None, None).await;
    assert_eq!(status, 204);
    one(
        &db,
        "experiment.delete",
        "ok",
        Expect {
            target_type: "experiment",
            target: &experiment,
            ..mine(&request_id(&headers))
        },
    )
    .await;

    // demo.write: a write to the demo workspace (admin scope, a demo key) writes its action row
    // and a demo.write row in the same transaction, both in the demo workspace.
    let (status, headers, b) = send(
        &server,
        Method::POST,
        "/api/v1/experiments?workspace=demo",
        Some(&demo_admin),
        None,
        Some(json!({"config": config_json(&manifest, 400, 2), "label": "demo"})),
    )
    .await;
    assert_eq!(status, 202, "{b}");
    let demo_experiment = b["experiment"]["id"].as_str().unwrap().to_string();
    let demo_request = request_id(&headers);
    for action in ["experiment.create", "demo.write"] {
        let detail = one(
            &db,
            action,
            "ok",
            Expect {
                workspace: Some(demo),
                key: Some(demo_admin_id),
                request: Some(&demo_request),
                target_type: "experiment",
                target: &demo_experiment,
            },
        )
        .await;
        if action == "demo.write" {
            assert_eq!(detail, json!({"action": "experiment.create"}));
        }
    }
    assert_eq!(rows(&db, "demo.write", "ok").await.len(), 1);

    // Sweeps: create (its window slices are registered as datasets attributed to the key and
    // the request), rename, cancel, delete.
    let s = manifest.start;
    let split = s + 12_000_000_000;
    let spec = json!({
        "base": config_json(&manifest, 250, 5),
        "design": {"type": "grid", "axes": [
            {"type": "maker_fee", "values": [0.0, 0.0002]}
        ]},
        "partition": {
            "in_sample": {"start": s.to_string(), "end": split.to_string()},
            "out_of_sample": {"start": split.to_string(), "end": (manifest.end + 1).to_string()}
        }
    });
    let (status, headers, created) = send(
        &server,
        Method::POST,
        "/api/v1/sweeps",
        Some(&key),
        None,
        Some(json!({"spec": spec})),
    )
    .await;
    assert_eq!(status, 202, "{created}");
    let sweep_request = request_id(&headers);
    let sweep = created["sweep"]["id"].as_str().unwrap().to_string();
    one(
        &db,
        "sweep.create",
        "ok",
        Expect {
            target_type: "sweep",
            target: &sweep,
            ..mine(&sweep_request)
        },
    )
    .await;
    let slices: Vec<AuditEvent> = rows(&db, "dataset.create", "ok")
        .await
        .into_iter()
        .filter(|r| r.target_id.as_deref() != Some(DATASET))
        .collect();
    assert_eq!(slices.len(), 2, "{slices:?}");
    for r in &slices {
        assert_eq!(r.workspace_id, None);
        assert_eq!(r.api_key_id, Some(kid));
        assert_eq!(r.request_id.as_deref(), Some(sweep_request.as_str()));
        assert_eq!(r.detail["actor"], "api");
        assert_eq!(r.detail["kind"], "synthetic");
    }
    let spath = format!("/api/v1/sweeps/{sweep}");
    let version = created["sweep"]["version"].as_i64().unwrap();
    let (status, headers, _) = send(
        &server,
        Method::PATCH,
        &spath,
        Some(&key),
        Some(&format!("\"v{version}\"")),
        Some(json!({"label": "ensemble"})),
    )
    .await;
    assert_eq!(status, 200);
    one(
        &db,
        "sweep.rename",
        "ok",
        Expect {
            target_type: "sweep",
            target: &sweep,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, _) = send(
        &server,
        Method::POST,
        &format!("{spath}/cancel"),
        Some(&key),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 202);
    one(
        &db,
        "sweep.cancel",
        "ok",
        Expect {
            target_type: "sweep",
            target: &sweep,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, _) = send(&server, Method::DELETE, &spath, Some(&key), None, None).await;
    assert_eq!(status, 204);
    one(
        &db,
        "sweep.delete",
        "ok",
        Expect {
            target_type: "sweep",
            target: &sweep,
            ..mine(&request_id(&headers))
        },
    )
    .await;

    // quota.exceeded: the daily experiment quota spent.
    sqlx::query("UPDATE bb.workspaces SET quota_experiments_per_day=0 WHERE id=$1")
        .bind(alpha)
        .execute(db.pool())
        .await
        .unwrap();
    let (status, headers, b) = send(
        &server,
        Method::POST,
        "/api/v1/experiments",
        Some(&key),
        None,
        Some(json!({"config": config_json(&manifest, 500, 3)})),
    )
    .await;
    assert_eq!(status, 429, "{b}");
    let detail = one(
        &db,
        "quota.exceeded",
        "denied",
        Expect {
            target_type: "workspace",
            target: "alpha",
            ..mine(&request_id(&headers))
        },
    )
    .await;
    assert_eq!(detail, json!({"scope": "experiments_per_day"}));
    sqlx::query("UPDATE bb.workspaces SET quota_experiments_per_day=100 WHERE id=$1")
        .bind(alpha)
        .execute(db.pool())
        .await
        .unwrap();
    server.stop().await;

    // ----- phase 2: a worker and a metered (Blob-like) store -----
    let objects = bb_artifacts::FsArtifactStore::new(dir.path().join("blob")).unwrap();
    let storage = Storage::metered(
        std::sync::Arc::new(objects),
        "vercel_blob",
        ReadBudget::new(setup.clone(), 1_000_000, i64::MAX),
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
    let db = server.db.clone();
    let (run1, _) = run(&server, &key, config_json(&manifest, 100, 1), "one").await;
    let (run2, _) = run(&server, &key, config_json(&manifest, 200, 1), "two").await;
    let (status, headers, comparison) = send(
        &server,
        Method::POST,
        "/api/v1/comparisons",
        Some(&key),
        None,
        Some(json!({"run_a": run1, "run_b": run2})),
    )
    .await;
    assert_eq!(status, 201, "{comparison}");
    let comparison = comparison["id"].as_str().unwrap().to_string();
    one(
        &db,
        "comparison.create",
        "ok",
        Expect {
            target_type: "comparison",
            target: &comparison,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let cpath = format!("/api/v1/comparisons/{comparison}");
    let (status, headers, _) = send(
        &server,
        Method::PATCH,
        &cpath,
        Some(&key),
        Some("\"v1\""),
        Some(json!({"label": "pair"})),
    )
    .await;
    assert_eq!(status, 200);
    one(
        &db,
        "comparison.rename",
        "ok",
        Expect {
            target_type: "comparison",
            target: &comparison,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, report) = send(
        &server,
        Method::POST,
        "/api/v1/reports",
        Some(&key),
        None,
        Some(json!({"kind": "run", "subject_id": run1, "title": "One"})),
    )
    .await;
    assert_eq!(status, 201, "{report}");
    let report = report["id"].as_str().unwrap().to_string();
    one(
        &db,
        "report.create",
        "ok",
        Expect {
            target_type: "report",
            target: &report,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, _) = send(
        &server,
        Method::DELETE,
        &format!("/api/v1/reports/{report}"),
        Some(&key),
        None,
        None,
    )
    .await;
    assert_eq!(status, 204);
    one(
        &db,
        "report.delete",
        "ok",
        Expect {
            target_type: "report",
            target: &report,
            ..mine(&request_id(&headers))
        },
    )
    .await;
    let (status, headers, _) = send(&server, Method::DELETE, &cpath, Some(&key), None, None).await;
    assert_eq!(status, 204);
    one(
        &db,
        "comparison.delete",
        "ok",
        Expect {
            target_type: "comparison",
            target: &comparison,
            ..mine(&request_id(&headers))
        },
    )
    .await;

    // storage.budget_exceeded: the workspace's monthly upload share spent.
    sqlx::query("UPDATE bb.workspaces SET quota_uploads_per_month=0 WHERE id=$1")
        .bind(alpha)
        .execute(db.pool())
        .await
        .unwrap();
    let (status, headers, b) = send(
        &server,
        Method::POST,
        "/api/v1/experiments",
        Some(&key),
        None,
        Some(json!({"config": config_json(&manifest, 600, 3)})),
    )
    .await;
    assert_eq!(status, 429, "{b}");
    let detail = one(
        &db,
        "storage.budget_exceeded",
        "denied",
        Expect {
            target_type: "workspace",
            target: "alpha",
            ..mine(&request_id(&headers))
        },
    )
    .await;
    assert_eq!(detail, json!({"scope": "storage"}));
    server.stop().await;

    // The whole table: exactly the §15 actions exercised above, every row timestamped within
    // the test, and no raw key (or its secret part) anywhere in any row.
    let finished = db_now(&setup).await;
    let all = all_rows(&setup).await;
    let seen: BTreeSet<(String, String)> = all
        .iter()
        .map(|r| (r.action.clone(), r.outcome.clone()))
        .collect();
    let expected: BTreeSet<(String, String)> = [
        ("auth.denied", "denied"),
        ("comparison.create", "ok"),
        ("comparison.delete", "ok"),
        ("comparison.rename", "ok"),
        ("dataset.create", "denied"),
        ("dataset.create", "ok"),
        ("demo.write", "ok"),
        ("experiment.create", "ok"),
        ("experiment.delete", "ok"),
        ("experiment.rename", "ok"),
        ("job.cancel", "ok"),
        ("key.mint", "ok"),
        ("key.revoke", "ok"),
        ("quota.exceeded", "denied"),
        ("report.create", "ok"),
        ("report.delete", "ok"),
        ("storage.budget_exceeded", "denied"),
        ("sweep.cancel", "ok"),
        ("sweep.create", "ok"),
        ("sweep.delete", "ok"),
        ("sweep.rename", "ok"),
    ]
    .into_iter()
    .map(|(a, o)| (a.to_string(), o.to_string()))
    .collect();
    assert_eq!(seen, expected);
    for r in &all {
        assert!(r.at >= started && r.at <= finished, "{r:?}");
        let text = serde_json::to_string(r).unwrap();
        for plaintext in plaintexts {
            assert!(!text.contains(plaintext.as_str()), "raw key in {text}");
            assert!(!text.contains(&plaintext[8..]), "key secret in {text}");
        }
    }
    setup.pool().close().await;
    tdb.drop_db().await;
}

/// Lets a closure return an [`Expect`] borrowing its request id argument.
fn attributed<F: for<'a> Fn(&'a str) -> Expect<'a>>(f: F) -> F {
    f
}

/// 1,000 anonymous requests (reads, mutations, admin routes) and 1,000 forged-key requests
/// write no audit row; the forged keys are refused (401) without a single database access.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_and_forged_key_traffic_writes_no_audit_rows() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 0,
            tweak: Box::new(|c| {
                c.rate_limits.anonymous_per_min = 1_000_000;
                c.rate_limits.anonymous_burst = 1_000_000;
                c.anonymous_global_per_min = 4_000_000;
            }),
            storage: None,
        },
    )
    .await;
    let db = server.db.clone();
    let manifest = register_dataset(&db, dir.path(), 20).await;
    let baseline = audit_count(&db).await;

    let anonymous = |i: usize| -> (Method, String, Option<Value>, u16) {
        match i % 5 {
            0 => (
                Method::GET,
                format!("/api/v1/runs?limit={}", 1 + i % 50),
                None,
                200,
            ),
            1 => (
                Method::POST,
                "/api/v1/experiments".into(),
                Some(json!({"config": config_json(&manifest, 100, i as u64)})),
                401,
            ),
            2 => (Method::GET, "/api/v1/system/queue".into(), None, 401),
            3 => (
                Method::DELETE,
                format!("/api/v1/experiments/{}", Uuid::new_v4()),
                None,
                401,
            ),
            _ => (Method::GET, format!("/api/v1/datasets?n={i}"), None, 200),
        }
    };
    for i in 0..1000 {
        let (method, path, body, status) = anonymous(i);
        let (got, _, b) = send(&server, method, &path, None, None, body).await;
        assert_eq!(got, status, "{path}: {b}");
    }
    assert_eq!(
        audit_count(&db).await,
        baseline,
        "anonymous traffic wrote audit rows"
    );

    // Forged keys: well-formed `bbk_` keys whose tag does not verify.
    let forged = |i: usize| {
        use base64::Engine;
        let mut raw = [0u8; 32];
        raw[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        raw[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        raw[0] = i as u8;
        format!(
            "bbk_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
        )
    };
    let acquires = || bb_app::metrics::Metrics::get(&server.metrics.db_acquires_total);
    // Settle background work started by the requests above.
    let mut settled = false;
    for _ in 0..20 {
        let before = acquires();
        for i in 0..20 {
            send(
                &server,
                Method::GET,
                "/api/v1/runs",
                Some(&forged(i)),
                None,
                None,
            )
            .await;
        }
        if acquires() == before {
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
    assert!(settled, "forged keys kept using the database");
    let before = acquires();
    for i in 0..1000 {
        let key = forged(i);
        assert_eq!(key.len(), 47);
        let (method, path, body) = match i % 4 {
            0 => (Method::GET, "/api/v1/runs".to_string(), None),
            1 => (
                Method::POST,
                "/api/v1/experiments".to_string(),
                Some(json!({"config": config_json(&manifest, 100, 1)})),
            ),
            2 => (Method::GET, "/api/v1/system/queue".to_string(), None),
            _ => (
                Method::DELETE,
                format!("/api/v1/experiments/{}", Uuid::new_v4()),
                None,
            ),
        };
        let (status, _, b) = send(&server, method, &path, Some(&key), None, body).await;
        assert_eq!(status, 401, "{path}: {b}");
    }
    assert_eq!(acquires(), before, "a forged key reached the database");
    assert_eq!(
        audit_count(&db).await,
        baseline,
        "forged keys wrote audit rows"
    );
    server.stop().await;
    tdb.drop_db().await;
}
