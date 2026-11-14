//! In-process `/api/v1` test harness: a throwaway Postgres database (from
//! `BLACKBOX_TEST_DATABASE_URL`), the real migrations, a `FsArtifactStore` under a temp dir and
//! the real server bound to `127.0.0.1:0` with its in-process worker.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Once},
    time::Duration,
};

use bb_app::{
    artifacts::Storage,
    config::AppConfig,
    metrics::Metrics,
    runtime::{self, Runtime},
};
use bb_db::{Database, Scope};
use serde_json::{Value, json};
use sqlx::{
    Connection, PgConnection,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const ALLOWED_ORIGIN: &str = "https://app.example.org";
pub const DATASET: &str = "test-synth";

/// A throwaway database, dropped when the test finishes.
pub struct TestDb {
    pub url: String,
    pub name: String,
    admin: PgConnectOptions,
}

impl TestDb {
    pub async fn create() -> Option<Self> {
        let Ok(url) = std::env::var("BLACKBOX_TEST_DATABASE_URL") else {
            eprintln!("SKIP: BLACKBOX_TEST_DATABASE_URL is unset");
            return None;
        };
        let admin = PgConnectOptions::from_str(&url).expect("valid test URL");
        let mut conn = PgConnection::connect_with(&admin).await.expect("connect");
        let name = format!("bb_api_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&mut conn)
            .await
            .expect("CREATEDB");
        conn.close().await.ok();
        let mut db_url = reqwest::Url::parse(&url).expect("url");
        db_url.set_path(&format!("/{name}"));
        bb_db::migrate(db_url.as_str()).await.expect("migrate");
        Some(Self {
            url: db_url.to_string(),
            name,
            admin,
        })
    }

    pub async fn pool(&self) -> Database {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&self.url)
            .await
            .expect("pool");
        Database::new(pool)
    }

    pub async fn drop_db(self) {
        if let Ok(mut conn) = PgConnection::connect_with(&self.admin).await {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE \"{}\" WITH (FORCE)",
                self.name
            )))
            .execute(&mut conn)
            .await;
        }
    }
}

/// Environment of a test server (local profile, fast heartbeats).
pub fn env(db_url: &str) -> HashMap<String, String> {
    [
        ("BB_PROFILE", "local"),
        ("DATABASE_URL", db_url),
        ("BB_CORS_ALLOWED_ORIGINS", ALLOWED_ORIGIN),
        ("BB_HEARTBEAT_INTERVAL_MS", "200"),
        ("BB_LEASE_SECONDS", "2"),
        ("BB_SHUTDOWN_GRACE_MS", "8000"),
        ("BB_GIT_SHA", "0123456789abcdef0123456789abcdef01234567"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

pub fn config(data_dir: &Path, vars: &HashMap<String, String>) -> AppConfig {
    AppConfig::from_lookup(data_dir.to_path_buf(), |k| vars.get(k).cloned()).expect("config")
}

static PROVIDER: Once = Once::new();

pub fn client() -> reqwest::Client {
    PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client")
}

/// A running server.
pub struct Server {
    pub addr: SocketAddr,
    pub base: String,
    pub db: Database,
    pub metrics: Arc<Metrics>,
    pub data_dir: PathBuf,
    pub hub: Arc<bb_app::worker::WorkerHub>,
    stop: CancellationToken,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    pub http: reqwest::Client,
}

pub struct Options {
    pub slots: usize,
    pub storage: Option<Storage>,
    pub tweak: Box<dyn FnOnce(&mut AppConfig) + Send>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            slots: 1,
            storage: None,
            tweak: Box::new(|_| {}),
        }
    }
}

impl Server {
    pub async fn start(db: &TestDb, data_dir: &Path, options: Options) -> Self {
        let vars = env(&db.url);
        let mut cfg = config(data_dir, &vars);
        (options.tweak)(&mut cfg);
        let metrics = Arc::new(Metrics::default());
        let database = runtime::connect(&cfg, metrics.clone())
            .await
            .expect("connect");
        let rt = Runtime::with_db(
            Arc::new(cfg),
            database.clone(),
            metrics.clone(),
            "in_process",
            options.slots,
            options.storage,
        )
        .expect("runtime");
        let state = rt.app_state();
        runtime::prepare(&rt, &state).await.expect("prepare");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().unwrap();
        let app = runtime::app(state.clone(), None);
        let stop = CancellationToken::new();
        let stop_signal = stop.clone();
        let hub = rt.hub.clone();
        let handle = tokio::spawn(runtime::run_until_shutdown(
            rt,
            state,
            listener,
            app,
            async move { stop_signal.cancelled().await },
        ));
        Self {
            addr,
            base: format!("http://{addr}"),
            db: database,
            metrics,
            data_dir: data_dir.to_path_buf(),
            hub,
            stop,
            handle,
            http: client(),
        }
    }

    /// Graceful stop (SIGTERM path).
    pub async fn stop(self) {
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(20), self.handle).await;
    }

    /// Abrupt stop (crash): no drain, leases stay held until they expire.
    pub async fn kill(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn get(
        &self,
        path: &str,
        key: Option<&str>,
    ) -> (u16, reqwest::header::HeaderMap, Value) {
        let mut req = self.http.get(self.url(path));
        if let Some(key) = key {
            req = req.bearer_auth(key);
        }
        response(req.send().await.expect("request")).await
    }

    pub async fn post(
        &self,
        path: &str,
        key: Option<&str>,
        body: &Value,
    ) -> (u16, reqwest::header::HeaderMap, Value) {
        let mut req = self.http.post(self.url(path)).json(body);
        if let Some(key) = key {
            req = req.bearer_auth(key);
        }
        response(req.send().await.expect("request")).await
    }
}

pub async fn response(r: reqwest::Response) -> (u16, reqwest::header::HeaderMap, Value) {
    let status = r.status().as_u16();
    let headers = r.headers().clone();
    let bytes = r.bytes().await.unwrap_or_default();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

/// Registers a small synthetic dataset (`DATASET`, `seconds` long).
pub async fn register_dataset(
    db: &Database,
    data_dir: &Path,
    seconds: i64,
) -> bb_core::dataset::DatasetManifest {
    let registry = bb_app::registry::DatasetRegistry::new(data_dir);
    let config = bb_data::synthetic::SyntheticConfig {
        id: DATASET.into(),
        duration_ns: seconds * 1_000_000_000,
        ..Default::default()
    };
    let (_, ctx) = bb_app::registry::register_synthetic(db, &registry, &config, false)
        .await
        .expect("register dataset");
    ctx.manifest().clone()
}

/// A research workspace with a key of `scopes`.
pub async fn workspace_key(db: &Database, slug: &str, scopes: &[Scope]) -> (Uuid, String) {
    let ws = match db.workspace(slug).await {
        Ok(ws) => ws,
        Err(_) => db.create_workspace(slug, slug).await.expect("workspace"),
    };
    let key = db
        .mint_key_tagged(
            ws.id,
            "test",
            scopes,
            None,
            bb_app::config::LOCAL_DEV_PEPPER.as_bytes(),
        )
        .await
        .expect("mint");
    (ws.id, key.plaintext)
}

/// The `maker_baseline` preset on `DATASET` with another interval and seed, wire-encoded.
pub fn config_json(
    manifest: &bb_core::dataset::DatasetManifest,
    interval_ms: i64,
    seed: u64,
) -> Value {
    let mut config = bb_app::presets::find(manifest, "maker_baseline")
        .expect("preset")
        .config;
    config.strategy.interval_ns = interval_ms * 1_000_000;
    config.seed = seed;
    let mut v = serde_json::to_value(config).unwrap();
    bb_app::wire::to_wire(&mut v).unwrap();
    v
}

/// Polls a job until it reaches a terminal state.
pub async fn wait_job(server: &Server, key: Option<&str>, job_id: &str) -> Value {
    for _ in 0..1200 {
        let (status, _, job) = server.get(&format!("/api/v1/jobs/{job_id}"), key).await;
        assert_eq!(status, 200, "{job}");
        if matches!(
            job["state"].as_str(),
            Some("completed" | "failed" | "cancelled" | "expired")
        ) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("job {job_id} did not finish");
}

/// Submits a configuration and waits for its run; returns (run_id, job).
pub async fn run(server: &Server, key: &str, config: Value, label: &str) -> (String, Value) {
    let (status, _, body) = server
        .post(
            "/api/v1/experiments",
            Some(key),
            &json!({"config": config, "label": label}),
        )
        .await;
    assert!(status == 202 || status == 200, "{status} {body}");
    if let Some(run) = body["run"].as_object() {
        return (
            run["run_id"].as_str().unwrap().to_string(),
            body["job"].clone(),
        );
    }
    let job_id = body["job"]["id"].as_str().unwrap().to_string();
    let job = wait_job(server, Some(key), &job_id).await;
    assert_eq!(job["state"], "completed", "{job}");
    (job["run_id"].as_str().unwrap().to_string(), job)
}

/// Asserts the wire rules on a response body: no ns field as a JSON number, no integer above
/// 2^53 - 1 anywhere.
pub fn assert_wire_safe(value: &Value, path: &str) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let p = format!("{path}.{k}");
                let ns_key = k == "ts" || k.ends_with("_ns") || k.ends_with("_ts");
                if ns_key {
                    let numeric = match v {
                        Value::Number(n) => n.is_i64() || n.is_u64(),
                        Value::Array(items) => items
                            .iter()
                            .any(|i| i.as_i64().is_some() || i.as_u64().is_some()),
                        _ => false,
                    };
                    assert!(!numeric, "{p} is an integer on the wire: {v}");
                }
                assert_wire_safe(v, &p);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                assert_wire_safe(v, &format!("{path}[{i}]"));
            }
        }
        Value::Number(n) => {
            if let Some(v) = n.as_u64() {
                assert!(v < (1u64 << 53), "{path} = {v} exceeds 2^53 - 1");
            }
            if let Some(v) = n.as_i64() {
                assert!(
                    v.unsigned_abs() < (1u64 << 53),
                    "{path} = {v} exceeds 2^53 - 1"
                );
            }
        }
        _ => {}
    }
}
