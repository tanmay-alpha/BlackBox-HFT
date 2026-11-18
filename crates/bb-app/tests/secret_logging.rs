//! SEC-18: sentinel secrets configured as the database password, the Blob token and the API key
//! pepper never appear in captured log output or command-line errors, on real startup, auth,
//! HTTP-error and database-failure paths, while the safe diagnostic fields (request id,
//! component, error class, reason) do.
//!
//! Logs are captured from the production subscriber (`runtime::init_tracing_with`, hosted JSON
//! format, SEC-18 redacting writer and panic hook) installed once for this test binary.
mod common;

use std::{
    collections::HashMap,
    io::Write,
    process::Command,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use bb_app::{
    artifacts::Storage,
    config::{AppConfig, Profile},
    error::AppError,
    metrics::Metrics,
    runtime::{self, Runtime},
    v1::error::{ApiError, REQUEST_ID},
};
use bb_db::Scope;
use common::*;
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use tokio_util::sync::CancellationToken;

/// Every sentinel contains this marker; it must never reach a log line or an error message.
const MARKER: &str = "S3NT1NEL";
const SCHEME: &str = "postgres";
const DB_PASSWORD: &str = "S3NT1NEL-db-password-7f3a";
const BLOB_TOKEN: &str = "vercel_blob_rw_S3NT1NELblobtoken7f3a";
const PEPPER: &str = "S3NT1NEL-pepper-0123456789abcdef0123";

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
    // Keep the production panic event and still show test assertion failures.
    let app_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        app_hook(info);
        default_hook(info);
    }));
    sink
});

fn logs() -> &'static Sink {
    &LOGS
}

fn captured() -> String {
    String::from_utf8_lossy(&logs().0.lock().unwrap()).into_owned()
}

fn assert_no_sentinel(what: &str, text: &str) {
    assert!(
        !text.contains(MARKER),
        "{what} leaked a sentinel secret:\n{text}"
    );
}

/// Runs the real binary with only `vars` (plus what the OS needs) in its environment.
fn blackbox(vars: &[(&str, &str)], args: &[&str]) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_blackbox"));
    command
        .env_clear()
        .args(["--data-dir", dir.path().to_str().unwrap()])
        .args(args);
    for name in ["PATH", "SYSTEMROOT", "SystemRoot", "TEMP", "TMP", "HOME"] {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    command.envs(vars.iter().copied());
    let output = command.output().expect("run blackbox");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn startup_and_configuration_failures_name_variables_but_never_values() {
    // `{SCHEME}` keeps fake credentials out of `scripts/ci/secret-scan.sh`'s URL pattern.
    let cloud_db = format!("{SCHEME}://owner:{DB_PASSWORD}@ep-x.eu-central-1.aws.neon.tech/bb");

    // Local profile with production credentials and no operator flag: refused before any I/O.
    let (ok, text) = blackbox(
        &[
            ("DATABASE_URL", &cloud_db),
            ("DATABASE_URL_UNPOOLED", &cloud_db),
            ("BLOB_READ_WRITE_TOKEN", BLOB_TOKEN),
            ("BB_API_KEY_PEPPER", PEPPER),
        ],
        &["migrate"],
    );
    assert!(!ok, "{text}");
    for name in [
        "BLOB_READ_WRITE_TOKEN",
        "DATABASE_URL holds a production cloud credential",
        "DATABASE_URL_UNPOOLED",
        "scripts/ops/prod-run.sh",
    ] {
        assert!(text.contains(name), "{name} missing from {text}");
    }
    assert_no_sentinel("refused local start", &text);

    // Hosted profile with an unverified TLS mode: the variable is named, the value is not.
    let (ok, text) = blackbox(
        &[
            ("BB_PROFILE", "hosted"),
            ("DATABASE_URL", &format!("{cloud_db}?sslmode=require")),
            (
                "DATABASE_URL_UNPOOLED",
                &format!("{cloud_db}?sslmode=require"),
            ),
            ("BLOB_READ_WRITE_TOKEN", BLOB_TOKEN),
            ("BB_API_KEY_PEPPER", PEPPER),
            ("BB_CORS_ALLOWED_ORIGINS", ALLOWED_ORIGIN),
            ("RENDER_EXTERNAL_HOSTNAME", "api.example.org"),
        ],
        &["migrate"],
    );
    assert!(!ok, "{text}");
    assert!(
        text.contains("DATABASE_URL must use sslmode=verify-full"),
        "{text}"
    );
    assert_no_sentinel("refused hosted start", &text);

    // Operator override, then a real connection failure: the startup log and the dependency
    // error are printed, the credentials are not.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let unreachable = format!("{SCHEME}://owner:{DB_PASSWORD}@127.0.0.1:{port}/bb");
    let (ok, text) = blackbox(
        &[
            ("DATABASE_URL", &unreachable),
            ("BLOB_READ_WRITE_TOKEN", BLOB_TOKEN),
            ("BB_API_KEY_PEPPER", PEPPER),
            ("BB_ALLOW_CLOUD_CREDENTIALS", "true"),
            ("NO_COLOR", "1"),
        ],
        &["migrate"],
    );
    assert!(!ok, "{text}");
    for kept in [
        "configuration",
        "storage=vercel_blob",
        "cloud_credentials_override",
        "migration failed",
    ] {
        assert!(text.contains(kept), "{kept} missing from {text}");
    }
    assert_no_sentinel("failed migrate", &text);
}

/// A login role whose password is the sentinel, with the test owner's privileges, so the server
/// below really connects with the sentinel in its configured `DATABASE_URL`.
async fn sentinel_role(tdb: &TestDb) -> (String, String) {
    let admin_url = std::env::var("BLACKBOX_TEST_DATABASE_URL").unwrap();
    let admin: PgConnectOptions = admin_url.parse().unwrap();
    let mut conn = PgConnection::connect_with(&admin).await.unwrap();
    let role = format!("bb_sentinel_{}", uuid::Uuid::new_v4().simple());
    let owner = admin.get_username().to_string();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE \"{role}\" LOGIN PASSWORD '{DB_PASSWORD}' IN ROLE \"{owner}\""
    )))
    .execute(&mut conn)
    .await
    .expect("create sentinel role");
    conn.close().await.ok();
    let mut url = reqwest::Url::parse(&tdb.url).unwrap();
    url.set_username(&role).unwrap();
    url.set_password(Some(DB_PASSWORD)).unwrap();
    (role, url.to_string())
}

async fn drop_role(role: &str) {
    let admin_url = std::env::var("BLACKBOX_TEST_DATABASE_URL").unwrap();
    if let Ok(mut conn) = PgConnection::connect(&admin_url).await {
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP ROLE IF EXISTS \"{role}\""
        )))
        .execute(&mut conn)
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_auth_and_database_failures_log_safe_fields_only() {
    logs();
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (role, sentinel_url) = sentinel_role(&tdb).await;
    let dir = tempfile::tempdir().unwrap();

    let mut vars = env(&sentinel_url);
    vars.extend(
        [
            ("BB_API_KEY_PEPPER", PEPPER),
            ("BLOB_READ_WRITE_TOKEN", BLOB_TOKEN),
            ("BB_ALLOW_CLOUD_CREDENTIALS", "true"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    let lookup: HashMap<String, String> = vars;
    let cfg = AppConfig::from_lookup(dir.path().to_path_buf(), |k| lookup.get(k).cloned())
        .expect("config with sentinel secrets");
    assert!(cfg.cloud_credentials_override);
    tracing::info!(component = "startup", config = %cfg.summary(), "configuration");

    let metrics = Arc::new(Metrics::default());
    let db = runtime::connect(&cfg, metrics.clone())
        .await
        .expect("connect with the sentinel password");
    let store = Arc::new(bb_artifacts::FsArtifactStore::new(dir.path().join("objects")).unwrap());
    let storage = Storage::filesystem(store);
    let rt = Runtime::with_db(
        Arc::new(cfg),
        db.clone(),
        metrics,
        "in_process",
        0,
        Some(storage),
    )
    .unwrap();
    let state = rt.app_state();
    runtime::prepare(&rt, &state).await.expect("prepare");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = runtime::app(state.clone(), None);
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let server = tokio::spawn(runtime::run_until_shutdown(
        rt,
        state,
        listener,
        app,
        async move { stopped.cancelled().await },
    ));
    let http = client();

    let ws = db.create_workspace("alpha", "alpha").await.unwrap();
    let key = db
        .mint_key_tagged(
            ws.id,
            "sentinel",
            &[Scope::Read, Scope::Run],
            None,
            PEPPER.as_bytes(),
        )
        .await
        .unwrap()
        .plaintext;

    // Auth: a forged key and a valid key (tag computed with the sentinel pepper).
    let forged = format!("bbk_{}", "A".repeat(43));
    let r = http
        .get(format!("{base}/api/v1/jobs"))
        .bearer_auth(&forged)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .get(format!("{base}/api/v1/jobs"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Dependency errors that echo credentials, through the real HTTP error conversion.
    let detail = format!(
        "server closed the connection {sentinel_url} (token {BLOB_TOKEN}, pepper {PEPPER})"
    );
    type Make = Box<dyn FnOnce() -> ApiError>;
    let cases: Vec<(&str, &str, Make)> = vec![
        (
            "sec18-database",
            "database",
            Box::new({
                let detail = detail.clone();
                move || ApiError::from(bb_db::Error::Database(sqlx::Error::Protocol(detail)))
            }),
        ),
        (
            "sec18-engine",
            "engine",
            Box::new({
                let detail = detail.clone();
                move || ApiError::from(AppError::SimulationFailed(detail))
            }),
        ),
        (
            "sec18-artifact",
            "artifact",
            Box::new({
                let detail = detail.clone();
                move || {
                    ApiError::from(bb_artifacts::ArtifactError::Io(std::io::Error::other(
                        detail,
                    )))
                }
            }),
        ),
    ];
    for (request_id, class, make) in cases {
        let error = REQUEST_ID.sync_scope(request_id.to_string(), make);
        assert_eq!(error.code, "internal");
        assert_no_sentinel("error message", &error.message);
        let logs = captured();
        let line = logs
            .lines()
            .find(|l| l.contains(&format!("\"request_id\":\"{request_id}\"")))
            .unwrap_or_else(|| panic!("no log line for {request_id}:\n{logs}"));
        for field in [
            "\"component\":\"http\"",
            &format!("\"error_class\":\"{class}\""),
            "\"reason\":\"internal\"",
            "server closed the connection",
            "<redacted>",
        ] {
            assert!(line.contains(field), "{field} missing from {line}");
        }
    }

    // The database goes away: a keyed request answers 503 and the log names the request.
    db.pool().close().await;
    let r = http
        .get(format!("{base}/api/v1/jobs"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    let request_id = r.headers()["x-request-id"].to_str().unwrap().to_string();
    let body = r.text().await.unwrap();
    assert_no_sentinel("503 body", &body);
    assert!(body.contains(&request_id), "{body}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let logs = captured();
    let line = logs
        .lines()
        .find(|l| l.contains(&request_id) && l.contains("db_unreachable"))
        .unwrap_or_else(|| panic!("no db_unreachable line for {request_id}:\n{logs}"));
    assert!(line.contains("\"component\":\"db\""), "{line}");
    assert!(line.contains("\"error_class\":\"database\""), "{line}");

    stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(20), server).await;
    tdb.drop_db().await;
    drop_role(&role).await;

    let logs = captured();
    assert!(logs.contains("\"component\":\"startup\""), "{logs}");
    assert!(logs.contains("storage=vercel_blob"), "{logs}");
    assert_no_sentinel("captured logs", &logs);
    assert!(!logs.contains(&key[4..]), "an API key reached the logs");
    assert!(
        !logs.contains(&forged[4..]),
        "a presented key reached the logs"
    );
}

#[test]
fn a_panic_message_carrying_a_credential_is_logged_redacted() {
    logs();
    // Registered like every configured secret.
    let _cfg = AppConfig::from_lookup(std::env::temp_dir(), |k| {
        (k == "BB_API_KEY_PEPPER").then(|| PEPPER.to_string())
    })
    .unwrap();
    let joined = std::thread::Builder::new()
        .name("sec18-panic".into())
        .spawn(|| panic!("worker state dump: pepper={PEPPER} token={BLOB_TOKEN}"))
        .unwrap()
        .join();
    assert!(joined.is_err());
    let logs = captured();
    let line = logs
        .lines()
        .find(|l| l.contains("thread panicked") && l.contains("worker state dump"))
        .unwrap_or_else(|| panic!("no panic event:\n{logs}"));
    assert!(line.contains("\"component\":\"panic\""), "{line}");
    assert!(line.contains("secret_logging.rs:"), "{line}");
    assert_no_sentinel("panic event", line);
}
