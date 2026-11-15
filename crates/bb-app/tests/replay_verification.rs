//! Replay verification against real storage, HTTP middleware and separate server processes.
//! Set BLACKBOX_TEST_DATABASE_URL to a local Postgres; no production requests are made.

mod common;

use std::{
    path::Path,
    process::{Child, Command},
    time::Duration,
};

#[cfg(windows)]
use std::{io::Read, process::Stdio, time::Instant};

use bb_db::Scope;
use common::*;
use serde_json::Value;

const MIB: usize = 1 << 20;

struct ProcessServer {
    child: Child,
    base: String,
    http: reqwest::Client,
}

impl ProcessServer {
    async fn start(tdb: &TestDb, dir: &Path, binary: &Path, pepper: Option<&str>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let log = std::fs::File::create(dir.join("replay-serve.log")).unwrap();
        let mut command = Command::new(binary);
        command
            .args(["--data-dir", dir.to_str().unwrap(), "serve", "--addr"])
            .arg(addr.to_string())
            .envs(env(&tdb.url))
            .env("BB_WORKER_SLOTS", "0")
            // Read the fixture's recorded class even when this executable links another libm.
            .env("BB_RESULT_PLATFORM", bb_core::platform::platform_class())
            .env("BB_MAX_RESPONSE_BYTES", (4 * MIB).to_string())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        if let Some(pepper) = pepper {
            command.env("BB_API_KEY_PEPPER", pepper);
        } else {
            command.env_remove("BB_API_KEY_PEPPER");
        }
        let mut server = Self {
            child: command.spawn().expect("start blackbox serve"),
            base: format!("http://{addr}"),
            http: client(),
        };
        for _ in 0..300 {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "server exited; see replay-serve.log"
            );
            if server
                .http
                .get(format!("{}/healthz", server.base))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return server;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("server did not become healthy");
    }

    async fn get(&self, path: &str, key: Option<&str>) -> (u16, Vec<u8>, Value) {
        let mut req = self.http.get(format!("{}{path}", self.base));
        if let Some(key) = key {
            req = req.bearer_auth(key);
        }
        let response = req.send().await.expect("replay request");
        let status = response.status().as_u16();
        let bytes = response.bytes().await.unwrap().to_vec();
        let body = serde_json::from_slice(&bytes).unwrap();
        (status, bytes, body)
    }
}

impl Drop for ProcessServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn fixture(tdb: &TestDb, dir: &Path, seconds: i64) -> (String, String, Value) {
    let server = Server::start(tdb, dir, Options::default()).await;
    let manifest = register_dataset(&server.db, dir, seconds).await;
    let (_, key) = workspace_key(&server.db, "replay", &[Scope::Run]).await;
    let (run_id, _) = run(
        &server,
        &key,
        config_json(&manifest, 10, 711),
        "replay verification",
    )
    .await;
    let (status, _, summary) = server
        .get(&format!("/api/v1/runs/{run_id}"), Some(&key))
        .await;
    assert_eq!(status, 200, "{summary}");
    assert_eq!(summary["counts"]["ticks"], seconds * 100);
    // Publish only this test-generated run in the test database's public workspace.
    sqlx::query("INSERT INTO bb.experiments (workspace_id,experiment_config_id,dataset_id,strategy_kind,platform,label) SELECT w.id,e.experiment_config_id,e.dataset_id,e.strategy_kind,e.platform,'replay verification' FROM bb.experiments e CROSS JOIN bb.workspaces w WHERE w.slug='demo' AND e.label='replay verification'")
        .execute(server.db.pool()).await.unwrap();
    server.stop().await;
    (run_id, key, summary)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_process_restart_preserves_cursor_and_enforces_caps_and_budget() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (run_id, key, summary) = fixture(&tdb, dir.path(), 20).await;
    let binary = Path::new(env!("CARGO_BIN_EXE_blackbox"));
    let server = ProcessServer::start(&tdb, dir.path(), binary, None).await;
    let (_, _, status) = server.get("/api/v1/status", None).await;
    assert_eq!(
        status["result_platform"], summary["platform"],
        "stored run and server platform"
    );
    let window = "t0_ns=0&t1_ns=10000000000";
    let first_path = format!("/api/v1/runs/{run_id}/timeline?{window}&max_events=100");
    let (status, _, first) = server.get(&first_path, Some(&key)).await;
    assert_eq!(status, 200, "{first}");
    assert!(first["events_total"].as_u64().unwrap() > 5_000);
    let cursor = first["next_cursor"].as_str().unwrap();
    let path = format!("/api/v1/runs/{run_id}/timeline?{window}&max_events=50000&cursor={cursor}");
    let (status, keyed_bytes, keyed) = server.get(&path, Some(&key)).await;
    assert_eq!(status, 200, "{keyed}");
    assert!(keyed_bytes.len() <= 4 * MIB);
    let (status, anonymous_bytes, anonymous) = server.get(&path, None).await;
    assert_eq!(status, 200, "{anonymous}");
    assert!(anonymous_bytes.len() <= MIB);
    assert!(
        anonymous["events"].as_array().unwrap().len() < keyed["events"].as_array().unwrap().len()
    );
    assert_eq!(anonymous["truncated_by"], "bytes");
    let (status, retry_bytes, _) = server.get(&path, Some(&key)).await;
    assert_eq!(status, 200);
    assert_eq!(
        retry_bytes, keyed_bytes,
        "identical cursor requests are byte-idempotent"
    );
    drop(server); // Real process death, not a fresh router sharing the static HMAC key.
    let server = ProcessServer::start(&tdb, dir.path(), binary, None).await;
    let (status, restarted_bytes, _) = server.get(&path, Some(&key)).await;
    assert_eq!(status, 200);
    assert_eq!(
        restarted_bytes, keyed_bytes,
        "same pepper preserves the cursor across process restarts"
    );
    let (status, _, huge) = server
        .get(
            &format!("/api/v1/runs/{run_id}/timeline?t0_ns=0&t1_ns={}", i64::MAX),
            None,
        )
        .await;
    assert_eq!(status, 400, "{huge}");
    assert_eq!(huge["error"]["details"]["reason"], "window_too_large");
    drop(server);
    let server = ProcessServer::start(
        &tdb,
        dir.path(),
        binary,
        Some("different-test-pepper-at-least-32-bytes"),
    )
    .await;
    let (status, _, changed) = server.get(&path, None).await;
    assert_eq!(status, 400, "{changed}");
    assert_eq!(changed["error"]["details"]["field"], "cursor");
    drop(server);
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            tweak: Box::new(|cfg| {
                cfg.egress_anonymous_daily_bytes = 0;
                cfg.egress_keyed_daily_bytes = 0;
            }),
            ..Options::default()
        },
    )
    .await;
    for key in [None, Some(key.as_str())] {
        let (status, _, error) = server.get(&path, key).await;
        assert_eq!(status, 503, "{error}");
        assert_eq!(error["error"]["code"], "unavailable");
        assert_eq!(error["error"]["details"]["reason"], "busy");
        assert_eq!(error["error"]["details"]["budget"], "egress");
        let (status, _, error) = server
            .get(&format!("{first_path}&cursor=malformed"), key)
            .await;
        assert_eq!(
            status, 400,
            "small errors remain available after budget exhaustion: {error}"
        );
    }
    server.stop().await;
    tdb.drop_db().await;
}

/// Explicit, opt-in local measurement. The child must be a release binary, not the test runner.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "local release measurement; set BLACKBOX_REPLAY_BINARY"]
async fn replay_release_measurement() {
    let binary = std::env::var_os("BLACKBOX_REPLAY_BINARY")
        .expect("set BLACKBOX_REPLAY_BINARY to release/blackbox.exe");
    let Some(tdb) = TestDb::create().await else {
        panic!("local test database required")
    };
    let dir = tempfile::tempdir().unwrap();
    let (run_id, key, summary) = fixture(&tdb, dir.path(), 120).await;
    let server = ProcessServer::start(&tdb, dir.path(), Path::new(&binary), None).await;
    let mut sampler = Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!(
            "$peak=0L; $samples=0; [Console]::Out.Write('R'); [Console]::Out.Flush(); while ($p=Get-Process -Id {} -ErrorAction SilentlyContinue) {{ $peak=[Math]::Max($peak,$p.WorkingSet64); $samples++; Start-Sleep -Milliseconds 10 }}; Write-Output \"$peak $samples\"", server.child.id()
        )])
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut ready = [0];
    sampler
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    assert_eq!(ready, [b'R'], "RSS sampler must start before paging");
    let mut latencies = Vec::new();
    let mut all_seqs = Vec::new();
    let mut response_bytes = Vec::new();
    // Non-overlapping inclusive windows cover the entire run, without loading its context at once.
    for index in 0..12_i64 {
        let t0 = index * 10_000_000_000;
        let t1 = if index == 11 {
            i64::MAX
        } else {
            (index + 1) * 10_000_000_000 - 1
        };
        let mut cursor = None;
        let mut window_count = 0;
        loop {
            let mut path = format!("/api/v1/runs/{run_id}/timeline?t0_ns={t0}&t1_ns={t1}");
            if let Some(c) = &cursor {
                path.push_str(&format!("&cursor={c}"));
            }
            let started = Instant::now();
            let (status, bytes, page) = server.get(&path, Some(&key)).await;
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(status, 200, "{page}");
            assert!(bytes.len() <= 4 * MIB);
            response_bytes.push(bytes.len());
            let events = page["events"].as_array().unwrap();
            all_seqs.extend(events.iter().map(|e| e["seq"].as_u64().unwrap()));
            window_count += events.len() as u64;
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                assert_eq!(window_count, page["events_total"].as_u64().unwrap());
                break;
            }
        }
    }
    let tape_events = summary["counts"]["tape_events"].as_u64().unwrap();
    // Tape sequence and timestamp order differ across clocks; windows reorder the former.
    all_seqs.sort_unstable();
    assert_eq!(all_seqs, (0..tape_events).collect::<Vec<_>>());
    drop(server);
    let sampled = sampler.wait_with_output().unwrap();
    assert!(sampled.status.success());
    latencies.sort_by(f64::total_cmp);
    let quantile =
        |q: f64| latencies[((latencies.len() as f64 * q).ceil() as usize).saturating_sub(1)];
    println!(
        "REPLAY_MEASUREMENT run={run_id} ticks=12000 tape_events={tape_events} pages={} p50_ms={:.3} p95_ms={:.3} rss_peak_bytes_samples={} response_bytes={response_bytes:?}",
        response_bytes.len(),
        quantile(0.50),
        quantile(0.95),
        String::from_utf8(sampled.stdout).unwrap().trim()
    );
    tdb.drop_db().await;
}
