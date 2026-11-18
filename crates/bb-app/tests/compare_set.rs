//! Phase 6 A1/A2 (docs/rebuild/PHASE6_RELIABILITY.md): `/compare/set?demo=execution_validation`
//! after a clean `demo seed --wait`, against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`; the
//! test skips without it) and an artefact store that counts its reads and can be made as slow as
//! a cold Blob read on the hosted instance.
//!
//! * A1 reproduction: the page's former fan-out (all seven `GET /runs/{id}` at once, each
//!   fetching and verifying its whole bundle under the 4-permit heavy-read limit with a 2 s
//!   wait) answers `503 busy` once the store is slow, and whole-bundle reads never exceed the
//!   four permits (a summary check reads only the header and trailer). Retried after
//!   `Retry-After`, or read two at a time as the page's fallback now does, every run loads;
//!   requests the client abandons give their permits back.
//! * A1 repair: the requests the page now makes for the overview (demo catalogue, run list,
//!   comparison list, stored comparisons) read no bundle, cold, warm and under concurrent page
//!   loads, and warm anonymous ones no database row either.
//! * A2: every run's comparison with the baseline is found and classified exactly as seeded
//!   (three aligned, two metrics-only across datasets, the depleted risk-adverse run compared
//!   with the depleted baseline instead), for anonymous and keyed callers.

mod common;

use std::{
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bb_app::{
    artifacts::{BundleCache, Storage, bundle_limits, open_run},
    metrics::Metrics,
    runtime::{self, Runtime},
};
use bb_artifacts::{ArtifactError, ArtifactHead, ArtifactStore, FsArtifactStore};
use bb_db::Scope;
use bytes::Bytes;
use common::*;
use serde_json::Value;

macro_rules! db_or_skip {
    () => {
        match TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}

/// Reads of at least this many bytes fetch a whole demo bundle (0.8–1.0 MB here); a header read
/// is about 200 bytes and a trailer read (manifest and metrics) about 200–240 kB.
const WHOLE: u64 = 512 << 10;

/// The data directory's file store, counting `get_range` calls, each delayed by `delay_ms`, and
/// the most whole-bundle reads ever in flight at once.
struct SlowStore {
    inner: FsArtifactStore,
    gets: AtomicUsize,
    delay_ms: AtomicU64,
    whole_in_flight: AtomicUsize,
    whole_peak: AtomicUsize,
    /// Whole-bundle reads started, in total.
    wholes: AtomicUsize,
}

/// One whole-bundle read in flight (also when its request is abandoned mid-read).
struct Whole<'a>(&'a AtomicUsize);

impl Drop for Whole<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ArtifactStore for SlowStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<ArtifactHead, ArtifactError> {
        self.inner.put_bundle_from_file(key, path).await
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes, ArtifactError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        let _whole = (range.end - range.start >= WHOLE).then(|| {
            let now = self.whole_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.whole_peak.fetch_max(now, Ordering::SeqCst);
            self.wholes.fetch_add(1, Ordering::SeqCst);
            Whole(&self.whole_in_flight)
        });
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        self.inner.get_range(key, range).await
    }
    async fn head(&self, key: &str) -> Result<ArtifactHead, ArtifactError> {
        self.inner.head(key).await
    }
    async fn delete(&self, key: &str) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }
}

/// `blackbox demo seed --wait` in its own runtime (as the CLI does).
async fn seed(tdb: &TestDb, dir: &Path) {
    let cfg = config(dir, &env(&tdb.url));
    let metrics = Arc::new(Metrics::default());
    let db = runtime::connect(&cfg, metrics.clone())
        .await
        .expect("connect");
    let rt = Runtime::with_db(Arc::new(cfg), db, metrics, "standalone", 1, None).expect("runtime");
    let platform = rt.cfg.result_platform.clone();
    bb_app::seed::seed(&rt, &platform, true)
        .await
        .expect("seed");
    rt.db.pool().close().await;
}

async fn start(tdb: &TestDb, dir: &Path, store: &Arc<SlowStore>) -> Server {
    Server::start(
        tdb,
        dir,
        Options {
            slots: 0,
            storage: Some(Storage::filesystem(store.clone())),
            ..Options::default()
        },
    )
    .await
}

/// Every item of a cursor-paged list, as the console's `collectAll` reads it.
async fn all_pages(server: &Server, path: &str, key: Option<&str>) -> Vec<Value> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = match &cursor {
            Some(c) => format!("{path}{sep}limit=50&cursor={c}"),
            None => format!("{path}{sep}limit=50"),
        };
        let (status, _, page) = server.get(&url, key).await;
        assert_eq!(status, 200, "{url}: {page}");
        items.extend(page["items"].as_array().unwrap().iter().cloned());
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => return items,
        }
    }
}

/// The overview requests of the page for `execution_validation` (keyed: the demo workspace's
/// lists, as `loadRuns` and `readableComparisons` read them). Returns the run ids and, for every
/// run after the baseline, what the server holds about its comparison with the baseline.
async fn overview(server: &Server, key: Option<&str>) -> (Vec<String>, Vec<String>) {
    let (status, _, demos) = server.get("/api/v1/demos", key).await;
    assert_eq!(status, 200, "{demos}");
    let demo = demos
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "execution_validation")
        .expect("the demo is listed")
        .clone();
    let ids: Vec<String> = demo["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["run_id"]
                .as_str()
                .expect("every demo run is stored")
                .to_string()
        })
        .collect();
    assert_eq!(ids.len(), 7);
    let scope = if key.is_some() { "?workspace=demo" } else { "" };
    let runs = all_pages(server, &format!("/api/v1/runs{scope}"), key).await;
    for id in &ids {
        assert!(
            runs.iter().any(|r| r["run_id"] == id.as_str()),
            "{id} is in the verified run list, so the page needs no GET /runs/{id}"
        );
    }
    let list = all_pages(server, &format!("/api/v1/comparisons{scope}"), key).await;
    let base = ids[0].as_str();
    let mut pairs = Vec::new();
    for other in &ids[1..] {
        let other = other.as_str();
        let with = |a: &str, b: &str| list.iter().find(|c| c["run_a"] == a && c["run_b"] == b);
        let state = if let Some(c) = with(base, other) {
            let (status, _, cmp) = server
                .get(
                    &format!("/api/v1/comparisons/{}", c["id"].as_str().unwrap()),
                    key,
                )
                .await;
            assert_eq!(status, 200, "{cmp}");
            assert!(cmp["alignment"].is_object() || cmp["status"] == "metrics_only");
            cmp["status"].as_str().unwrap().to_string()
        } else if with(other, base).is_some() {
            "reversed".to_string()
        } else if let Some(c) = list
            .iter()
            .find(|c| c["run_b"] == other || c["run_a"] == other)
        {
            let partner = if c["run_b"] == other {
                &c["run_a"]
            } else {
                &c["run_b"]
            };
            format!("elsewhere:{}", partner.as_str().unwrap())
        } else {
            "missing".to_string()
        };
        pairs.push(state);
    }
    (ids, pairs)
}

fn expected(ids: &[String]) -> Vec<String> {
    vec![
        "aligned".into(),
        "aligned".into(),
        "aligned".into(),
        "metrics_only".into(),
        format!("elsewhere:{}", ids[4]),
        "metrics_only".into(),
    ]
}

async fn get_status(http: reqwest::Client, url: String) -> (u16, Option<String>, Value) {
    let r = http.get(url).send().await.expect("request");
    let status = r.status().as_u16();
    let retry = r
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body: Value = r.json().await.unwrap_or(Value::Null);
    (status, retry, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compare_set_overview_reads_no_bundle_and_busy_run_reads_recover() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    seed(&tdb, dir.path()).await;
    let store = Arc::new(SlowStore {
        inner: FsArtifactStore::new(dir.path().join("artifacts")).unwrap(),
        gets: AtomicUsize::new(0),
        delay_ms: AtomicU64::new(0),
        whole_in_flight: AtomicUsize::new(0),
        whole_peak: AtomicUsize::new(0),
        wholes: AtomicUsize::new(0),
    });
    let server = start(&tdb, dir.path(), &store).await;

    // Cold: an empty bundle cache, and the overview reads no bundle at all.
    let started = Instant::now();
    let (ids, pairs) = overview(&server, None).await;
    eprintln!("cold anonymous overview: {:?}", started.elapsed());
    assert_eq!(pairs, expected(&ids), "anonymous classification");
    assert_eq!(
        store.gets.load(Ordering::SeqCst),
        0,
        "cold overview read a bundle"
    );

    // Warm: the same requests use neither the store nor the database.
    let acquires = || Metrics::get(&server.metrics.db_acquires_total);
    let before = acquires();
    let (_, warm) = overview(&server, None).await;
    assert_eq!(warm, expected(&ids));
    assert_eq!(
        acquires(),
        before,
        "warm anonymous overview used the database"
    );

    // Eight page loads at once: same answers, still no bundle read.
    let server = Arc::new(server);
    let loads: Vec<_> = (0..8)
        .map(|_| {
            let server = server.clone();
            tokio::spawn(async move { overview(&server, None).await.1 })
        })
        .collect();
    for load in loads {
        assert_eq!(load.await.unwrap(), expected(&ids), "concurrent page load");
    }
    assert_eq!(store.gets.load(Ordering::SeqCst), 0);

    // A keyed visitor reads the demo workspace through the database: the same classification.
    let (_, key) = workspace_key(&server.db, "lab", &[Scope::Read]).await;
    let (keyed_ids, keyed) = overview(&server, Some(&key)).await;
    assert_eq!(keyed_ids, ids);
    assert_eq!(keyed, expected(&ids), "keyed classification");
    assert_eq!(store.gets.load(Ordering::SeqCst), 0);

    // A1 reproduction: the former fan-out, seven run reads at once, each store read held up 4 s
    // (about what one cold bundle read and verification takes on the hosted 0.1 CPU, see
    // PHASE6_RELIABILITY.md). Whole bundles are only ever read under one of the four heavy-read
    // permits (a summary check reads the header and trailer only), so some reads wait more than
    // the 2 s admission limit and are refused `503 busy` with `Retry-After: 2`.
    store.delay_ms.store(4000, Ordering::SeqCst);
    let gets = store.gets.load(Ordering::SeqCst);
    let started = Instant::now();
    let fan_out: Vec<_> = ids
        .iter()
        .map(|id| {
            tokio::spawn(get_status(
                server.http.clone(),
                server.url(&format!("/api/v1/runs/{id}")),
            ))
        })
        .collect();
    let mut refused = Vec::new();
    for (id, call) in ids.iter().zip(fan_out) {
        let (status, retry, body) = call.await.unwrap();
        eprintln!("GET /runs/{id}: {status} after {:?}", started.elapsed());
        match status {
            200 => {}
            503 => {
                assert_eq!(body["error"]["details"]["reason"], "busy", "{body}");
                assert_eq!(retry.as_deref(), Some("2"));
                refused.push(id.clone());
            }
            other => panic!("GET /runs/{id}: {other} {body}"),
        }
    }
    eprintln!(
        "former fan-out: {} of 7 refused busy, {} store reads, at most {} whole-bundle reads at once",
        refused.len(),
        store.gets.load(Ordering::SeqCst) - gets,
        store.whole_peak.load(Ordering::SeqCst)
    );
    assert!(
        !refused.is_empty(),
        "the former fan-out is refused busy once the store is slow"
    );
    assert!(
        store.whole_peak.load(Ordering::SeqCst) <= 4,
        "whole-bundle reads stay under the heavy-read limit"
    );
    // Retried after Retry-After, as the page's bounded retry does, the refused ones load.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let retried: Vec<_> = refused
        .iter()
        .map(|id| {
            tokio::spawn(get_status(
                server.http.clone(),
                server.url(&format!("/api/v1/runs/{id}")),
            ))
        })
        .collect();
    for (id, call) in refused.iter().zip(retried) {
        let (status, _, body) = call.await.unwrap();
        assert_eq!(status, 200, "retried {id}: {body}");
    }
    let server = Arc::into_inner(server).expect("no other holder");
    server.stop().await;

    // A restarted server has a cold cache again. Requests the client abandons after 300 ms give
    // their permits back at once, so four new bundle reads right after them are admitted.
    let server = start(&tdb, dir.path(), &store).await;
    let impatient = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    let abandoned: Vec<_> = ids[..4]
        .iter()
        .map(|id| {
            let http = impatient.clone();
            let url = server.url(&format!("/api/v1/runs/{id}"));
            tokio::spawn(async move { http.get(url).send().await.is_err() })
        })
        .collect();
    for call in abandoned {
        assert!(call.await.unwrap(), "the client gave up");
    }
    let next: Vec<_> = ids[3..]
        .iter()
        .map(|id| {
            tokio::spawn(get_status(
                server.http.clone(),
                server.url(&format!("/api/v1/runs/{id}")),
            ))
        })
        .collect();
    for (id, call) in ids[3..].iter().zip(next) {
        let (status, _, body) = call.await.unwrap();
        assert_eq!(status, 200, "after abandoned reads, {id}: {body}");
    }
    server.stop().await;

    // Cold again: the page's fallback reads runs by id two at a time; none is refused.
    let server = start(&tdb, dir.path(), &store).await;
    let started = Instant::now();
    for chunk in ids.chunks(2) {
        let calls: Vec<_> = chunk
            .iter()
            .map(|id| {
                tokio::spawn(get_status(
                    server.http.clone(),
                    server.url(&format!("/api/v1/runs/{id}")),
                ))
            })
            .collect();
        for call in calls {
            let (status, _, body) = call.await.unwrap();
            assert_eq!(status, 200, "{body}");
        }
    }
    eprintln!(
        "seven cold run reads, two at a time: {:?}",
        started.elapsed()
    );
    server.stop().await;

    // Cold again: three simultaneous reads of the same run download its bundle once (the first
    // fetches, the others wait for it and find it cached).
    store.delay_ms.store(500, Ordering::SeqCst);
    let server = start(&tdb, dir.path(), &store).await;
    let wholes = store.wholes.load(Ordering::SeqCst);
    let same: Vec<_> = (0..3)
        .map(|_| {
            tokio::spawn(get_status(
                server.http.clone(),
                server.url(&format!("/api/v1/runs/{}", ids[0])),
            ))
        })
        .collect();
    for call in same {
        let (status, _, body) = call.await.unwrap();
        assert_eq!(status, 200, "{body}");
    }
    assert!(
        store.wholes.load(Ordering::SeqCst) - wholes == 1,
        "concurrent cold reads of one run fetched its bundle more than once"
    );
    server.stop().await;
    tdb.drop_db().await;
}

/// A cold fetch that cannot finish before the awaiting request is cancelled.
struct BlockedRead {
    inner: FsArtifactStore,
    entered: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl ArtifactStore for BlockedRead {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<ArtifactHead, ArtifactError> {
        self.inner.put_bundle_from_file(key, path).await
    }

    async fn get_range(&self, _key: &str, _range: Range<u64>) -> Result<Bytes, ArtifactError> {
        self.entered.notify_one();
        std::future::pending().await
    }

    async fn head(&self, key: &str) -> Result<ArtifactHead, ArtifactError> {
        self.inner.head(key).await
    }

    async fn delete(&self, key: &str) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_active_and_waiting_cold_opens_cleans_the_single_flight_map() {
    let tdb = db_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&tdb, dir.path(), Options::default()).await;
    let manifest = register_dataset(&server.db, dir.path(), 1).await;
    let (workspace, key) = workspace_key(&server.db, "demo", &[Scope::Admin]).await;
    let (id, _) = run(&server, &key, config_json(&manifest, 100, 1), "cancel").await;
    let view = server
        .db
        .run_view(workspace, &id, &server.state.cfg.result_platform)
        .await
        .unwrap();
    let cold = tempfile::tempdir().unwrap();
    let cache = Arc::new(BundleCache::new(cold.path(), 64 << 20).unwrap());
    let store = Arc::new(BlockedRead {
        inner: FsArtifactStore::new(cold.path()).unwrap(),
        entered: tokio::sync::Notify::new(),
    });
    let storage = Storage::filesystem(store.clone());
    let spawn = || {
        let db = server.db.clone();
        let cache = cache.clone();
        let storage = storage.clone();
        let view = view.clone();
        tokio::spawn(async move {
            open_run(
                &db,
                &storage,
                &cache,
                &Arc::new(Metrics::default()),
                bundle_limits(64 << 20),
                view,
            )
            .await
        })
    };
    // The review's exact one-request reproduction, then cancellation with a waiting reader.
    let active = spawn();
    tokio::time::timeout(Duration::from_secs(5), store.entered.notified())
        .await
        .unwrap();
    assert_eq!(cache.fetching_len(), 1);
    active.abort();
    assert!(matches!(active.await, Err(e) if e.is_cancelled()));
    assert_eq!(cache.fetching_len(), 0);

    let active = spawn();
    tokio::time::timeout(Duration::from_secs(5), store.entered.notified())
        .await
        .unwrap();
    let waiter = spawn();
    // Whether still reading the artifact row or already waiting on the gate, both tasks'
    // cancellation must leave no retained map entry.
    tokio::task::yield_now().await;
    active.abort();
    waiter.abort();
    assert!(matches!(active.await, Err(e) if e.is_cancelled()));
    assert!(matches!(waiter.await, Err(e) if e.is_cancelled()));
    assert_eq!(cache.fetching_len(), 0);
    server.stop().await;
    tdb.drop_db().await;
}
