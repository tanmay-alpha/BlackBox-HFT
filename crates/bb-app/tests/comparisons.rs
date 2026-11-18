//! `POST /comparisons` against a real Postgres (`BLACKBOX_TEST_DATABASE_URL`; skipped without
//! it), FINAL_AUDIT §4 item 2: an ineligible pair is answered from the verified bundle header and
//! trailer alone (no page of either bundle is read), and every stored result equals the full
//! comparison of the decoded runs. `audit_sized_comparison_wall_time` is a measurement, not a
//! gate: `cargo test --release -p bb-app --test comparisons -- --ignored --nocapture`.

mod common;

use std::{
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bb_app::{
    artifacts::{Storage, open_run, trust_of, verified_trailer},
    v1::{AppState, comparisons::COMPARATOR_VERSION, runs::verified_summary},
};
use bb_db::{RunView, Scope};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

/// A filesystem store that records every ranged read.
struct CountingStore {
    inner: bb_artifacts::FsArtifactStore,
    reads: Mutex<Vec<(String, Range<u64>)>>,
}

#[async_trait::async_trait]
impl bb_artifacts::ArtifactStore for CountingStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &std::path::Path,
    ) -> Result<bb_artifacts::ArtifactHead, bb_artifacts::ArtifactError> {
        self.inner.put_bundle_from_file(key, path).await
    }
    async fn get_range(
        &self,
        key: &str,
        range: Range<u64>,
    ) -> Result<bytes::Bytes, bb_artifacts::ArtifactError> {
        self.reads
            .lock()
            .unwrap()
            .push((key.to_string(), range.clone()));
        self.inner.get_range(key, range).await
    }
    async fn head(
        &self,
        key: &str,
    ) -> Result<bb_artifacts::ArtifactHead, bb_artifacts::ArtifactError> {
        self.inner.head(key).await
    }
    async fn delete(&self, key: &str) -> Result<(), bb_artifacts::ArtifactError> {
        self.inner.delete(key).await
    }
}

fn counting(root: &std::path::Path) -> (Arc<CountingStore>, Storage) {
    let store = Arc::new(CountingStore {
        inner: bb_artifacts::FsArtifactStore::new(root).unwrap(),
        reads: Mutex::new(Vec::new()),
    });
    (store.clone(), Storage::filesystem(store))
}

/// `maker_baseline` on `DATASET` with another interval and maker fee, every tick recorded.
fn config(manifest: &bb_core::dataset::DatasetManifest, interval_ms: i64, maker_fee: f64) -> Value {
    let mut config = bb_app::presets::find(manifest, "maker_baseline")
        .expect("preset")
        .config;
    config.strategy.interval_ns = interval_ms * 1_000_000;
    config.record.sample_every_ticks = 1;
    config.record.max_queue_updates_per_order = 2_000;
    config.execution.fees.maker = maker_fee;
    let mut v = serde_json::to_value(config).unwrap();
    bb_app::wire::to_wire(&mut v).unwrap();
    v
}

async fn view(state: &AppState, ws: Uuid, id: &str) -> RunView {
    state
        .db
        .run_view(ws, id, &state.cfg.result_platform)
        .await
        .unwrap()
}

async fn open(state: &AppState, v: &RunView) -> bb_app::artifacts::OpenRun {
    open_run(
        &state.db,
        &state.storage,
        &state.cache,
        &state.metrics,
        state.bundle_limits(),
        v.clone(),
    )
    .await
    .unwrap()
}

/// The comparison of the fully decoded runs, as `compare_and_store` computes it for an
/// eligible pair (and always did before eligibility was decided from metadata).
async fn full_comparison(state: &Arc<AppState>, a: &RunView, b: &RunView) -> Value {
    let label = |v: &RunView| {
        let (state, v) = (state.clone(), v.clone());
        async move {
            verified_summary(&state, &v, false)
                .await
                .unwrap()
                .label
                .unwrap_or_default()
        }
    };
    let instrument = |v: &RunView| {
        let (db, v) = (state.db.clone(), v.clone());
        async move {
            db.dataset_version(&v.dataset_id, v.dataset_version_no)
                .await
                .ok()
                .and_then(|ver| bb_app::registry::manifest_of(&ver).ok())
                .map(|m| m.instrument)
        }
    };
    let (la, lb) = (label(a).await, label(b).await);
    let (ia, ib) = (instrument(a).await, instrument(b).await);
    let ra = open(state, a).await.full().await.unwrap();
    let rb = open(state, b).await.full().await.unwrap();
    serde_json::to_value(bb_forensics::compare_runs(
        bb_forensics::CompareInput {
            run: &ra,
            label: &la,
            instrument: ia.as_ref(),
        },
        bb_forensics::CompareInput {
            run: &rb,
            label: &lb,
            instrument: ib.as_ref(),
        },
    ))
    .unwrap()
}

async fn compare(server: &Server, key: &str, a: &str, b: &str) -> (Duration, Value) {
    let started = Instant::now();
    let (status, _, body) = server
        .post(
            "/api/v1/comparisons",
            Some(key),
            &json!({"run_a": a, "run_b": b, "label": "timed"}),
        )
        .await;
    let elapsed = started.elapsed();
    assert_eq!(status, 201, "{body}");
    (elapsed, body)
}

async fn stored(server: &Server, ws: Uuid, a: &RunView, b: &RunView) -> Value {
    server
        .db
        .find_comparison(ws, a.run.id, b.run.id, COMPARATOR_VERSION)
        .await
        .unwrap()
        .expect("stored comparison")
        .summary
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ineligible_pairs_read_no_page_and_every_result_equals_the_full_comparison() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    let (_, storage) = counting(&objects);
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 3,
            storage: Some(storage),
            ..Default::default()
        },
    )
    .await;
    let manifest = register_dataset(&server.db, dir.path(), 20).await;
    let (ws, key) = workspace_key(&server.db, "lab", &[Scope::Run]).await;
    let (a, _) = run(&server, &key, config(&manifest, 100, 0.0002), "a").await;
    let (b, _) = run(&server, &key, config(&manifest, 100, -0.0001), "b").await;
    let (c, _) = run(&server, &key, config(&manifest, 200, 0.0002), "c").await;
    server.stop().await;

    // A fresh process: empty bundle cache, every read goes to the (counting) store.
    let (store, storage) = counting(&objects);
    let server = Server::start(
        &tdb,
        dir.path(),
        Options {
            storage: Some(storage),
            ..Default::default()
        },
    )
    .await;
    let state = server.state.clone();
    let va = view(&state, ws, &a).await;
    let vb = view(&state, ws, &b).await;
    let vc = view(&state, ws, &c).await;

    let (_, body) = compare(&server, &key, &a, &c).await;
    assert_eq!(body["status"], "metrics_only", "{body}");
    let mut allowed = Vec::new();
    for v in [&va, &vc] {
        let artifact = state.db.artifact(v.run.id).await.unwrap();
        let trust = trust_of(v, &artifact).unwrap();
        assert!(!trust.page_index.is_empty());
        for member in [&trust.header, &trust.trailer] {
            allowed.push((artifact.storage_key.clone(), member.range().unwrap()));
        }
    }
    let mut reads = store.reads.lock().unwrap().clone();
    reads.sort_by_key(|(k, r)| (k.clone(), r.start));
    allowed.sort_by_key(|(k, r)| (k.clone(), r.start));
    assert_eq!(reads, allowed, "only the header and trailer of each bundle");

    // Unchanged results: the metadata-only result and the aligned result both equal the
    // comparison of the fully decoded runs.
    assert_eq!(
        stored(&server, ws, &va, &vc).await,
        full_comparison(&state, &va, &vc).await
    );
    let (_, body) = compare(&server, &key, &a, &b).await;
    assert_eq!(body["status"], "aligned", "{body}");
    assert_eq!(
        stored(&server, ws, &va, &vb).await,
        full_comparison(&state, &va, &vb).await
    );
    server.stop().await;
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "timing measurement; run by hand in release"]
async fn audit_sized_comparison_wall_time() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start(
        &tdb,
        dir.path(),
        Options {
            slots: 3,
            ..Default::default()
        },
    )
    .await;
    // FINAL_AUDIT R13: 12,000 strategy ticks at 10 ms, every tick recorded.
    let manifest = register_dataset(&server.db, dir.path(), 121).await;
    let (ws, key) = workspace_key(&server.db, "lab", &[Scope::Run]).await;
    let (large_a, _) = run(&server, &key, config(&manifest, 10, 0.0002), "a").await;
    let (large_b, _) = run(&server, &key, config(&manifest, 10, -0.0001), "b").await;
    let (demo, _) = run(&server, &key, config(&manifest, 100, 0.0002), "demo").await;
    let large = view(&server.state, ws, &large_a).await;
    let artifact = server.db.artifact(large.run.id).await.unwrap();
    eprintln!(
        "audit-sized run: {} ticks, {} orders, {} fills, {} tape events, {} equity samples, bundle {} bytes",
        large.run.ticks,
        large.run.order_count,
        large.run.fill_count,
        large.run.tape_len,
        large.run.equity_len,
        artifact.size_bytes
    );

    // Eligibility input of one run on a cold cache: the old path (fetch and verify the whole
    // bundle, then read its trailer) against the new one (header and trailer only).
    server.stop().await;
    server = Server::start(&tdb, dir.path(), Options::default()).await;
    let state = server.state.clone();
    let started = Instant::now();
    verified_trailer(
        &state.db,
        &state.storage,
        &state.cache,
        &state.metrics,
        state.bundle_limits(),
        &large,
    )
    .await
    .unwrap();
    eprintln!(
        "cold trailer, header+trailer reads: {:?}",
        started.elapsed()
    );
    let started = Instant::now();
    let opened = open(&state, &large).await;
    opened.trailer().await.unwrap();
    eprintln!(
        "cold trailer, whole-bundle fetch+verify: {:?}",
        started.elapsed()
    );

    for cache in ["cold", "warm"] {
        if cache == "cold" {
            server.stop().await;
            server = Server::start(&tdb, dir.path(), Options::default()).await;
        }
        let (t, body) = compare(&server, &key, &large_a, &demo).await;
        eprintln!(
            "{cache} cache, audit-sized vs demo-sized ({}): {t:?}",
            body["status"]
        );
        let (t2, body2) = compare(&server, &key, &large_a, &large_b).await;
        eprintln!(
            "{cache} cache, audit-sized vs audit-sized ({}): {t2:?}",
            body2["status"]
        );
        for id in [&body["id"], &body2["id"]] {
            let r = server
                .http
                .delete(server.url(&format!("/api/v1/comparisons/{}", id.as_str().unwrap())))
                .bearer_auth(&key)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), 204);
        }
    }

    // Steps of the eligible path on a warm cache.
    let state = server.state.clone();
    let vb = view(&state, ws, &large_b).await;
    let started = Instant::now();
    let (oa, ob) = (open(&state, &large).await, open(&state, &vb).await);
    let (ra, rb) = (oa.full().await.unwrap(), ob.full().await.unwrap());
    eprintln!("warm decode of both full runs: {:?}", started.elapsed());
    let started = Instant::now();
    let c = tokio::task::spawn_blocking(move || {
        let side = |run| bb_forensics::CompareInput {
            run,
            label: "",
            instrument: None,
        };
        bb_forensics::compare_runs(side(&ra), side(&rb))
    })
    .await
    .unwrap();
    eprintln!(
        "alignment (aligned={}): {:?}",
        c.eligibility.aligned,
        started.elapsed()
    );
    server.stop().await;
    tdb.drop_db().await;
}
