//! End-to-end tests of the application layer on a small synthetic dataset: the run pipeline, the
//! run store, sensitivity grids, the preset catalogue and the HTTP API.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use bb_app::{
    AppError,
    api::{
        CompareRequest, ErrorBody, Health, OrderDetail, OrdersPage, Preset, RunRequest, TapePage,
    },
    datasets::{DatasetCache, DatasetCtx},
    pipeline, presets,
    server::{self, AppState},
    store::{self, FsRunStore, RunStore},
};
use bb_core::{
    CONTRACT_VERSION, ENGINE_NAME, ENGINE_VERSION, Side,
    compare::{Comparison, GridAxis, GridResult, GridSpec},
    config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig, RunConfig},
    dataset::DatasetManifest,
    market::ReplayWindow,
    result::{EquitySample, FillRecord, RunSummary},
    time::NS_PER_S,
};
use bb_data::synthetic::SyntheticConfig;
use serde::{Serialize, de::DeserializeOwned};
use tower::ServiceExt;

const DATASET_ID: &str = "it-synth";
/// Long enough for every preset to act (the probe's last cancel is due at tick 70 of 100 ms).
const DATASET_SECONDS: i64 = 30;

/// A unique scratch directory, removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bb-app-it-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }

    fn datasets(&self) -> PathBuf {
        self.0.join("datasets")
    }

    fn runs(&self) -> PathBuf {
        self.0.join("runs")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Generates the synthetic dataset into the scratch directory and returns its manifest.
fn write_dataset(dir: &TempDir) -> DatasetManifest {
    let config = SyntheticConfig {
        id: DATASET_ID.to_string(),
        duration_ns: DATASET_SECONDS * NS_PER_S,
        ..SyntheticConfig::default()
    };
    let built = bb_data::synthetic::generate(&config).expect("generate synthetic dataset");
    let root = dir.datasets();
    std::fs::create_dir_all(&root).expect("create dataset root");
    bb_data::store::save(&root, &built).expect("save synthetic dataset")
}

/// Generates the dataset, then loads it back from disk the way the application does.
fn make_dataset(dir: &TempDir) -> DatasetCtx {
    write_dataset(dir);
    match DatasetCtx::load(&dir.datasets(), DATASET_ID) {
        Ok(ctx) => ctx,
        Err(err) => panic!("loading the synthetic dataset failed: {err}"),
    }
}

fn preset_config(manifest: &DatasetManifest, id: &str) -> RunConfig {
    presets::find(manifest, id)
        .unwrap_or_else(|| panic!("preset {id} missing"))
        .config
}

// ----- (1) determinism -----

#[test]
fn execution_is_deterministic_and_identity_tracks_every_assumption() {
    let dir = TempDir::new("determinism");
    let ctx = make_dataset(&dir);
    let config = preset_config(ctx.manifest(), "maker_baseline");

    let first = pipeline::execute(&ctx, &config).expect("first run");
    let second = pipeline::execute(&ctx, &config).expect("second run");
    assert_eq!(first.manifest.run_id, second.manifest.run_id);
    assert_eq!(first.manifest.config_sha256, second.manifest.config_sha256);
    assert_eq!(first.manifest.result_sha256, second.manifest.result_sha256);
    assert_eq!(first.tape, second.tape);
    assert_eq!(first.orders, second.orders);
    assert_eq!(first.fills, second.fills);
    assert_eq!(first.equity, second.equity);
    assert_eq!(first.metrics, second.metrics);

    // The manifest describes exactly what was run.
    let identity = pipeline::run_identity(ctx.manifest(), &config);
    assert_eq!(first.manifest.config_sha256, identity.config_sha256);
    assert_eq!(first.manifest.run_id, identity.run_id);
    assert_eq!(first.manifest.run_id, first.manifest.config_sha256[..16]);
    assert!(store::is_valid_run_id(&first.manifest.run_id));
    assert_eq!(first.manifest.result_sha256.len(), 64);
    assert_eq!(first.manifest.contract_version, CONTRACT_VERSION);
    assert_eq!(first.manifest.engine.name, ENGINE_NAME);
    assert_eq!(first.manifest.engine.version, ENGINE_VERSION);
    assert_eq!(first.manifest.engine.features, ["backtest"]);
    assert_eq!(first.manifest.dataset.id, DATASET_ID);
    assert_eq!(first.manifest.dataset.sha256, ctx.manifest().events_sha256);
    assert_eq!(first.manifest.dataset.origin_ns, ctx.manifest().origin_ns);
    assert_eq!(first.manifest.config, config);
    assert!(first.ticks > 0, "the strategy was never evaluated");
    assert!(!first.tape.is_empty(), "a touch maker must leave a tape");
    for (i, event) in first.tape.iter().enumerate() {
        assert_eq!(event.seq, i as u64, "tape seq must be contiguous from 0");
    }

    // The seed and every execution field are part of the identity.
    let variants: Vec<(&str, RunConfig)> = vec![
        ("seed", {
            let mut c = config.clone();
            c.seed += 1;
            c
        }),
        ("exchange_model", {
            let mut c = config.clone();
            c.execution.exchange_model = ExchangeModel::PartialFill;
            c
        }),
        ("queue_model", {
            let mut c = config.clone();
            c.execution.queue_model = QueueModelConfig::LogProb;
            c
        }),
        ("order_latency", {
            let mut c = config.clone();
            c.execution.order_latency = OrderLatencyConfig::Constant {
                entry_ns: 2_000_000,
                response_ns: 1_000_000,
            };
            c
        }),
        ("feed_latency_offset_ns", {
            let mut c = config.clone();
            c.execution.feed_latency_offset_ns = 1_000;
            c
        }),
        ("fees.maker", {
            let mut c = config.clone();
            c.execution.fees.maker = 0.0001;
            c
        }),
        ("fees.taker", {
            let mut c = config.clone();
            c.execution.fees.taker = 0.0005;
            c
        }),
    ];
    let mut seen = BTreeSet::from([identity.config_sha256.clone()]);
    for (field, variant) in &variants {
        let other = pipeline::run_identity(ctx.manifest(), variant);
        assert_ne!(
            other.config_sha256, identity.config_sha256,
            "changing {field} did not change config_sha256"
        );
        assert_ne!(other.run_id, identity.run_id, "{field}");
        assert!(seen.insert(other.config_sha256), "{field} collides");
    }

    // So is the dataset: the same configuration on other events is another experiment.
    let mut other_dataset = ctx.manifest().clone();
    other_dataset.events_sha256 = "0".repeat(64);
    assert_ne!(
        pipeline::run_identity(&other_dataset, &config).config_sha256,
        identity.config_sha256
    );

    // A configuration for another dataset, or an invalid one, is refused with every problem.
    let mut wrong_dataset = config.clone();
    wrong_dataset.dataset_id = "some-other-dataset".to_string();
    assert!(matches!(
        pipeline::execute(&ctx, &wrong_dataset),
        Err(AppError::InvalidConfig(_))
    ));
    let mut invalid = config.clone();
    invalid.strategy.interval_ns = 0;
    invalid.risk.max_open_orders = 0;
    match pipeline::execute(&ctx, &invalid) {
        Err(AppError::InvalidConfig(problems)) => assert_eq!(problems.len(), 2, "{problems:?}"),
        Err(other) => panic!("wrong error: {other}"),
        Ok(_) => panic!("an invalid configuration was simulated"),
    }
}

// ----- (2) run store -----

#[test]
fn fs_run_store_round_trips_and_rejects_malformed_ids() {
    let dir = TempDir::new("store");
    let ctx = make_dataset(&dir);
    let baseline = pipeline::execute(&ctx, &preset_config(ctx.manifest(), "maker_baseline"))
        .expect("baseline run");
    let slow =
        pipeline::execute(&ctx, &preset_config(ctx.manifest(), "maker_slow")).expect("slow run");
    let id_a = baseline.manifest.run_id.clone();
    let id_b = slow.manifest.run_id.clone();
    assert_ne!(id_a, id_b);

    let store = FsRunStore::open(dir.runs()).expect("open store");
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&id_a).unwrap().is_none());

    let summary_a = store.put(&baseline, "baseline").unwrap();
    let summary_b = store.put(&slow, "slow").unwrap();
    assert_eq!(summary_a.label, "baseline");
    assert_eq!(summary_a.manifest, baseline.manifest);
    assert_eq!(summary_a.metrics, baseline.metrics);
    assert_eq!(summary_a.ticks, baseline.ticks);
    assert_eq!(summary_a.order_count, baseline.orders.len() as u64);
    assert_eq!(summary_a.fill_count, baseline.fills.len() as u64);
    assert_eq!(summary_a.tape_len, baseline.tape.len() as u64);

    let cached = store.get(&id_a).unwrap().expect("stored run");
    assert_eq!(*cached, baseline);
    assert_eq!(store.summary(&id_a).unwrap(), Some(summary_a.clone()));

    // A second store on the same directory has no cache, so this is the on-disk copy. It must
    // be the same run bit for bit: recomputing the result hash from it gives the recorded hash.
    let reopened = FsRunStore::open(dir.runs()).expect("reopen store");
    let from_disk = reopened.get(&id_a).unwrap().expect("run on disk");
    assert_eq!(*from_disk, baseline);
    assert_eq!(
        bb_forensics::result_sha256(&from_disk.tape, &from_disk.fills, &from_disk.equity),
        baseline.manifest.result_sha256
    );
    assert_eq!(reopened.summary(&id_b).unwrap(), Some(summary_b.clone()));

    // Newest first, ties broken by run id.
    let listed: Vec<String> = reopened
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.manifest.run_id)
        .collect();
    let mut expected = vec![summary_a.clone(), summary_b.clone()];
    expected.sort_by(|x, y| {
        y.manifest
            .created_unix_ms
            .cmp(&x.manifest.created_unix_ms)
            .then_with(|| x.manifest.run_id.cmp(&y.manifest.run_id))
    });
    let expected: Vec<String> = expected.into_iter().map(|s| s.manifest.run_id).collect();
    assert_eq!(listed, expected);

    let mut older = summary_a.clone();
    older.manifest.created_unix_ms = 1_000;
    let mut newer = summary_b.clone();
    newer.manifest.created_unix_ms = 2_000;
    let mut ordered = vec![older.clone(), newer.clone()];
    store::sort_newest_first(&mut ordered);
    assert_eq!(ordered, vec![newer, older]);

    // Storing the same run again replaces it instead of duplicating it.
    let relabelled = store.put(&baseline, "baseline, relabelled").unwrap();
    assert_eq!(relabelled.label, "baseline, relabelled");
    assert_eq!(store.list().unwrap().len(), 2);
    assert_eq!(
        store.summary(&id_a).unwrap().map(|s| s.label),
        Some("baseline, relabelled".to_string())
    );

    // A directory holding a run file without its summary is an interrupted write, not a run.
    let interrupted = "00000000000000aa";
    std::fs::create_dir_all(dir.runs().join(interrupted)).unwrap();
    std::fs::write(dir.runs().join(interrupted).join("run.json"), b"{\"manif").unwrap();
    assert!(store.get(interrupted).unwrap().is_none());
    assert!(store.summary(interrupted).unwrap().is_none());
    assert_eq!(store.list().unwrap().len(), 2);

    // Malformed ids are rejected before any path is built from them.
    for bad in ["../outside", "ABCDEF0123456789", "0123", "0123456789abcdef/.."] {
        assert!(
            matches!(store.get(bad), Err(AppError::InvalidRequest(_))),
            "get({bad:?})"
        );
        assert!(
            matches!(store.summary(bad), Err(AppError::InvalidRequest(_))),
            "summary({bad:?})"
        );
        assert!(
            matches!(store.delete(bad), Err(AppError::InvalidRequest(_))),
            "delete({bad:?})"
        );
    }
    let mut forged = baseline.clone();
    forged.manifest.run_id = "../forged-run-id".to_string();
    assert!(matches!(
        store.put(&forged, "forged"),
        Err(AppError::InvalidRequest(_))
    ));
    assert!(!dir.0.join("forged-run-id").exists());

    assert!(store.delete(&id_a).unwrap());
    assert!(store.get(&id_a).unwrap().is_none());
    assert!(store.summary(&id_a).unwrap().is_none());
    assert!(!dir.runs().join(&id_a).exists());
    assert!(!store.delete(&id_a).unwrap(), "second delete finds nothing");
    let remaining: Vec<String> = store
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.manifest.run_id)
        .collect();
    assert_eq!(remaining, vec![id_b]);
}

// ----- (3) grids -----

#[test]
fn grid_runs_every_cell_as_a_stored_run() {
    let dir = TempDir::new("grid");
    let ctx = make_dataset(&dir);
    let store = FsRunStore::open(dir.runs()).expect("open store");
    let base = preset_config(ctx.manifest(), "maker_baseline");
    let spec = GridSpec {
        base: base.clone(),
        x: GridAxis::OrderLatencyNs {
            values: vec![1_000_000, 20_000_000],
        },
        y: GridAxis::MakerFee {
            values: vec![0.0002, -0.0001],
        },
    };

    let grid = pipeline::run_grid(&ctx, &spec, &store, 2).expect("grid");
    assert_eq!(grid.spec, spec);
    assert_eq!(grid.x_labels, ["1000000", "20000000"]);
    assert_eq!(grid.y_labels, ["0.0002", "-0.0001"]);
    assert_eq!(grid.cells.len(), 4);

    let mut run_ids = BTreeSet::new();
    for (index, cell) in grid.cells.iter().enumerate() {
        // Row-major: cells[yi * x_len + xi].
        let (xi, yi) = (index % 2, index / 2);
        assert_eq!((cell.xi as usize, cell.yi as usize), (xi, yi));
        assert_eq!(cell.error, None, "cell ({xi},{yi})");
        let run_id = cell.run_id.as_deref().expect("completed cell has a run id");
        let stored = store
            .get(run_id)
            .unwrap()
            .unwrap_or_else(|| panic!("cell ({xi},{yi}) run {run_id} is not in the store"));
        assert_eq!(cell.metrics.as_ref(), Some(&stored.metrics));
        assert_eq!(
            cell.result_sha256.as_deref(),
            Some(stored.manifest.result_sha256.as_str())
        );
        assert_eq!(stored.manifest.config, spec.y.apply(&spec.x.apply(&spec.base, xi), yi));
        let summary = store.summary(run_id).unwrap().expect("summary");
        assert_eq!(
            summary.label,
            format!("grid x={} y={}", grid.x_labels[xi], grid.y_labels[yi])
        );
        run_ids.insert(run_id.to_string());
    }
    assert_eq!(run_ids.len(), 4, "every cell is a distinct experiment");
    assert_eq!(store.list().unwrap().len(), 4);

    // Cell (0,0) applies the base's own values, so it is the base experiment itself.
    let base_id = pipeline::run_identity(ctx.manifest(), &base).run_id;
    assert_eq!(grid.cells[0].run_id.as_deref(), Some(base_id.as_str()));

    // Running the grid again reuses the stored runs instead of simulating them again.
    let created: Vec<u64> = store
        .list()
        .unwrap()
        .iter()
        .map(|s| s.manifest.created_unix_ms)
        .collect();
    let again = pipeline::run_grid(&ctx, &spec, &store, 1).expect("grid again");
    assert_eq!(again.cells, grid.cells);
    let created_again: Vec<u64> = store
        .list()
        .unwrap()
        .iter()
        .map(|s| s.manifest.created_unix_ms)
        .collect();
    assert_eq!(created_again, created);
    assert_eq!(store.list().unwrap().len(), 4);

    // Invalid grids are refused before anything runs.
    let same_axis = GridSpec {
        y: spec.x.clone(),
        ..spec.clone()
    };
    assert!(matches!(
        pipeline::run_grid(&ctx, &same_axis, &store, 2),
        Err(AppError::InvalidConfig(_))
    ));
    let mut other_dataset = spec.clone();
    other_dataset.base.dataset_id = "some-other-dataset".to_string();
    assert!(matches!(
        pipeline::run_grid(&ctx, &other_dataset, &store, 2),
        Err(AppError::InvalidConfig(_))
    ));
}

// ----- (5) presets -----

#[test]
fn every_preset_validates_and_executes() {
    let dir = TempDir::new("presets");
    let ctx = make_dataset(&dir);
    let catalogue = presets::presets(ctx.manifest());
    assert_eq!(catalogue.len(), 7);
    let ids: BTreeSet<&str> = catalogue.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids.len(), catalogue.len(), "preset ids must be unique");

    let mut run_ids = BTreeSet::new();
    for preset in &catalogue {
        assert_eq!(
            preset.config.validate(),
            Vec::<String>::new(),
            "preset {}",
            preset.id
        );
        assert_eq!(preset.config.dataset_id, DATASET_ID);
        assert!(!preset.demonstrates.is_empty());
        let result = match pipeline::execute(&ctx, &preset.config) {
            Ok(result) => result,
            Err(err) => panic!("preset {} failed: {err}", preset.id),
        };
        assert!(result.ticks > 0, "preset {}", preset.id);
        assert_eq!(result.manifest.config, preset.config);
        assert_eq!(result.manifest.dataset.id, DATASET_ID);
        assert!(
            run_ids.insert(result.manifest.run_id.clone()),
            "preset {} repeats another preset's experiment",
            preset.id
        );
        if preset.id == "forensics_probe" {
            assert_eq!(result.orders.len(), 5, "one order per scripted probe");
            for order in &result.orders {
                assert!(order.reason.starts_with("probe "), "{}", order.reason);
            }
        }
    }
}

// ----- (4) HTTP -----

fn test_app(dir: &TempDir, web_dir: Option<PathBuf>) -> Router {
    let store: Arc<dyn RunStore> = Arc::new(FsRunStore::open(dir.runs()).expect("open store"));
    let state = Arc::new(AppState {
        data_root: dir.datasets(),
        datasets: DatasetCache::new(),
        store,
        grid_threads: 2,
    });
    server::router(state, web_dir)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("valid request")
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .expect("valid request")
}

fn post_raw(uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("valid request")
}

fn post<T: Serialize>(uri: &str, body: &T) -> Request<Body> {
    post_raw(uri, serde_json::to_vec(body).expect("serializable body"))
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = match app.clone().oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable body");
    (status, bytes.to_vec())
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> T {
    match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(err) => panic!(
            "undecodable body ({err}): {}",
            String::from_utf8_lossy(bytes)
        ),
    }
}

/// Sends a request that must succeed with 200 and decodes its body.
async fn ok<T: DeserializeOwned>(app: &Router, request: Request<Body>) -> T {
    let (status, bytes) = send(app, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    decode(&bytes)
}

/// Sends a request that must fail with the documented error body.
async fn expect_error(app: &Router, request: Request<Body>, status: StatusCode, code: &str) -> String {
    let what = format!("{} {}", request.method(), request.uri());
    let (got, bytes) = send(app, request).await;
    assert_eq!(got, status, "{what}: {}", String::from_utf8_lossy(&bytes));
    let body: ErrorBody = decode(&bytes);
    assert_eq!(body.error.code, code, "{what}");
    assert!(!body.error.message.is_empty(), "{what}");
    body.error.message
}

#[tokio::test]
async fn http_api_end_to_end() {
    let dir = TempDir::new("http");
    write_dataset(&dir);
    let app = test_app(&dir, None);

    // Health, with CORS for the web UI's development origin.
    let request = Request::builder()
        .uri("/api/health")
        .header(header::ORIGIN, "http://localhost:5173")
        .body(Body::empty())
        .expect("valid request");
    let response = match app.clone().oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    let health: Health = ok(&app, get("/api/health")).await;
    assert_eq!(health.status, "ok");
    assert_eq!(health.contract_version, CONTRACT_VERSION);
    assert_eq!(health.engine.name, ENGINE_NAME);
    assert_eq!(health.engine.version, ENGINE_VERSION);
    assert!(!health.disclaimer.is_empty());

    // Datasets.
    let listed: Vec<DatasetManifest> = ok(&app, get("/api/datasets")).await;
    let on_disk = bb_app::datasets::list_manifests(&dir.datasets()).expect("list datasets");
    assert_eq!(listed, on_disk);
    assert_eq!(listed.len(), 1);
    let manifest = listed[0].clone();
    assert_eq!(manifest.id, DATASET_ID);
    let one: DatasetManifest = ok(&app, get(&format!("/api/datasets/{DATASET_ID}"))).await;
    assert_eq!(one, manifest);
    expect_error(
        &app,
        get("/api/datasets/nope"),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    // Replay: defaults, then a rejected window and a rejected step.
    let replay_uri = format!("/api/datasets/{DATASET_ID}/replay");
    let window: ReplayWindow = ok(&app, get(&replay_uri)).await;
    assert_eq!(window.t0, manifest.start);
    assert_eq!(window.t1, manifest.end.min(manifest.start + 60 * NS_PER_S));
    assert_eq!(window.levels, 25);
    assert!(!window.depth.is_empty());
    assert!(!window.bbo.is_empty());
    let narrow: ReplayWindow = ok(
        &app,
        get(&format!("{replay_uri}?t0=1000000000&t1=2000000000&step_ns=100000000&levels=3")),
    )
    .await;
    assert_eq!((narrow.t0, narrow.t1, narrow.step_ns, narrow.levels), (1_000_000_000, 2_000_000_000, 100_000_000, 3));
    assert!(narrow.depth.iter().all(|f| f.bids.len() <= 3 && f.asks.len() <= 3));
    for query in ["?t0=10&t1=5", "?t0=7&t1=7", "?step_ns=0", "?step_ns=-5", "?t0=abc"] {
        expect_error(
            &app,
            get(&format!("{replay_uri}{query}")),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
    expect_error(
        &app,
        get("/api/datasets/nope/replay"),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    // Presets, defaulting to the first dataset.
    let catalogue: Vec<Preset> = ok(&app, get("/api/presets")).await;
    assert_eq!(catalogue, presets::presets(&manifest));
    let explicit: Vec<Preset> = ok(&app, get(&format!("/api/presets?dataset_id={DATASET_ID}"))).await;
    assert_eq!(explicit, catalogue);
    expect_error(
        &app,
        get("/api/presets?dataset_id=nope"),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    // No runs yet.
    let runs: Vec<RunSummary> = ok(&app, get("/api/runs")).await;
    assert!(runs.is_empty());

    // POST a run; POSTing the same configuration again returns the stored run.
    let probe_request = RunRequest {
        config: preset_config(&manifest, "forensics_probe"),
        label: Some("probe".to_string()),
    };
    let probe: RunSummary = ok(&app, post("/api/runs", &probe_request)).await;
    assert_eq!(probe.label, "probe");
    assert_eq!(probe.manifest.config, probe_request.config);
    assert_eq!(
        probe.manifest.run_id,
        pipeline::run_identity(&manifest, &probe_request.config).run_id
    );
    assert_eq!(probe.order_count, 5);
    let repeated: RunSummary = ok(
        &app,
        post(
            "/api/runs",
            &RunRequest {
                config: probe_request.config.clone(),
                label: Some("another label".to_string()),
            },
        ),
    )
    .await;
    assert_eq!(repeated, probe, "the endpoint is idempotent");
    let runs: Vec<RunSummary> = ok(&app, get("/api/runs")).await;
    assert_eq!(runs, vec![probe.clone()]);
    let run_uri = format!("/api/runs/{}", probe.manifest.run_id);
    let fetched: RunSummary = ok(&app, get(&run_uri)).await;
    assert_eq!(fetched, probe);

    // Orders: paging and filtering.
    let all: OrdersPage = ok(&app, get(&format!("{run_uri}/orders"))).await;
    assert_eq!((all.total, all.offset), (5, 0));
    assert_eq!(all.orders.len(), 5);
    assert!(all.orders.windows(2).all(|w| w[0].order_id < w[1].order_id));
    let page: OrdersPage = ok(&app, get(&format!("{run_uri}/orders?offset=1&limit=2"))).await;
    assert_eq!((page.total, page.offset), (5, 1));
    assert_eq!(page.orders, all.orders[1..3]);
    let sells: OrdersPage = ok(&app, get(&format!("{run_uri}/orders?side=sell"))).await;
    assert_eq!(sells.total, 1);
    assert!(sells.orders.iter().all(|o| o.side == Side::Sell));
    let first_outcome = serde_json::to_value(all.orders[0].outcome).expect("outcome tag");
    let by_outcome: OrdersPage = ok(
        &app,
        get(&format!(
            "{run_uri}/orders?outcome={}",
            first_outcome.as_str().expect("outcome serializes as a string")
        )),
    )
    .await;
    assert!(by_outcome.total >= 1);
    assert!(by_outcome.orders.iter().all(|o| o.outcome == all.orders[0].outcome));
    expect_error(
        &app,
        get(&format!("{run_uri}/orders?outcome=bogus")),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;

    // Order detail: every order has its events and a rule-based explanation.
    for order in &all.orders {
        let detail: OrderDetail =
            ok(&app, get(&format!("{run_uri}/orders/{}", order.order_id))).await;
        assert_eq!(&detail.order, order);
        assert_eq!(detail.instrument, manifest.instrument);
        assert!(!detail.events.is_empty());
        assert!(detail.events.iter().all(|e| e.order_id == Some(order.order_id)));
        assert!(detail.events.windows(2).all(|w| w[0].seq < w[1].seq));
        assert!(detail.fills.iter().all(|f| f.order_id == order.order_id));
        assert_eq!(detail.fills.len() as u32, order.fill_count);
        assert_eq!(detail.explanation.order_id, order.order_id);
        assert!(!detail.explanation.headline.is_empty());
        assert!(!detail.explanation.steps.is_empty());
        assert!(!detail.explanation.caveats.is_empty());

        let tape: TapePage = ok(
            &app,
            get(&format!("{run_uri}/tape?order_id={}&limit=5000", order.order_id)),
        )
        .await;
        assert_eq!(tape.total, detail.events.len() as u64);
        // The page is capped at 5000 events; a long-resting order can have more.
        let shown = detail.events.len().min(5000);
        assert_eq!(tape.events, detail.events[..shown]);
    }
    expect_error(
        &app,
        get(&format!("{run_uri}/orders/123456789")),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;
    expect_error(
        &app,
        get(&format!("{run_uri}/orders/not-a-number")),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;

    // Tape paging.
    let head: TapePage = ok(&app, get(&format!("{run_uri}/tape?limit=5"))).await;
    assert_eq!(head.total, probe.tape_len);
    assert!(probe.tape_len > 5, "five probes leave more than five events");
    let seqs: Vec<u64> = head.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [0, 1, 2, 3, 4]);
    let middle: TapePage = ok(&app, get(&format!("{run_uri}/tape?from_seq=3&limit=2"))).await;
    let seqs: Vec<u64> = middle.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [3, 4]);
    let default_page: TapePage = ok(&app, get(&format!("{run_uri}/tape"))).await;
    assert_eq!(default_page.events.len() as u64, probe.tape_len.min(500));
    let past_end: TapePage = ok(
        &app,
        get(&format!("{run_uri}/tape?from_seq={}", probe.tape_len)),
    )
    .await;
    assert!(past_end.events.is_empty());
    assert_eq!(past_end.total, probe.tape_len);

    // Fills and equity.
    let fills: Vec<FillRecord> = ok(&app, get(&format!("{run_uri}/fills"))).await;
    assert_eq!(fills.len() as u64, probe.fill_count);
    let equity: Vec<EquitySample> = ok(&app, get(&format!("{run_uri}/equity"))).await;
    assert!(!equity.is_empty());

    // Two scenarios of one strategy, compared.
    let baseline: RunSummary = ok(
        &app,
        post(
            "/api/runs",
            &RunRequest {
                config: preset_config(&manifest, "maker_baseline"),
                label: Some("baseline".to_string()),
            },
        ),
    )
    .await;
    let slow: RunSummary = ok(
        &app,
        post(
            "/api/runs",
            &RunRequest {
                config: preset_config(&manifest, "maker_slow"),
                label: None,
            },
        ),
    )
    .await;
    assert_eq!(slow.label, "touch_maker", "an absent label defaults to the strategy kind");
    let comparison: Comparison = ok(
        &app,
        post(
            "/api/compare",
            &CompareRequest {
                a: baseline.manifest.run_id.clone(),
                b: slow.manifest.run_id.clone(),
            },
        ),
    )
    .await;
    assert_eq!(comparison.run_a, baseline.manifest.run_id);
    assert_eq!(comparison.run_b, slow.manifest.run_id);
    assert_eq!(comparison.label_a, "baseline");
    assert_eq!(comparison.label_b, "touch_maker");
    assert!(comparison.same_dataset);
    assert!(comparison.same_strategy);
    assert!(!comparison.config_diffs.is_empty());
    assert!(!comparison.metrics.is_empty());
    expect_error(
        &app,
        post(
            "/api/compare",
            &CompareRequest {
                a: baseline.manifest.run_id.clone(),
                b: "0123456789abcdef".to_string(),
            },
        ),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    // A grid over the baseline: both cells become ordinary runs.
    let spec = GridSpec {
        base: preset_config(&manifest, "maker_baseline"),
        x: GridAxis::OrderLatencyNs {
            values: vec![1_000_000, 5_000_000],
        },
        y: GridAxis::TakerFee {
            values: vec![0.0004],
        },
    };
    let grid: GridResult = ok(&app, post("/api/grids", &spec)).await;
    assert_eq!(grid.cells.len(), 2);
    assert_eq!(
        grid.cells[0].run_id.as_deref(),
        Some(baseline.manifest.run_id.as_str()),
        "the cell equal to the baseline reuses the stored baseline run"
    );
    for cell in &grid.cells {
        let run_id = cell.run_id.as_deref().expect("completed cell");
        let summary: RunSummary = ok(&app, get(&format!("/api/runs/{run_id}"))).await;
        assert_eq!(cell.metrics.as_ref(), Some(&summary.metrics));
    }
    let bad_grid = GridSpec {
        y: spec.x.clone(),
        ..spec.clone()
    };
    expect_error(
        &app,
        post("/api/grids", &bad_grid),
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_config",
    )
    .await;

    // 422: the configuration parses but cannot be simulated; every problem is listed.
    let mut invalid = preset_config(&manifest, "maker_baseline");
    invalid.strategy.interval_ns = 0;
    invalid.risk.max_open_orders = 0;
    let message = expect_error(
        &app,
        post(
            "/api/runs",
            &RunRequest {
                config: invalid,
                label: None,
            },
        ),
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_config",
    )
    .await;
    assert!(message.contains("interval_ns"), "{message}");
    assert!(message.contains("max_open_orders"), "{message}");

    // 404: unknown dataset in a valid configuration, unknown run, unknown endpoint.
    let mut elsewhere = preset_config(&manifest, "maker_baseline");
    elsewhere.dataset_id = "nope".to_string();
    expect_error(
        &app,
        post(
            "/api/runs",
            &RunRequest {
                config: elsewhere,
                label: None,
            },
        ),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;
    for uri in [
        "/api/runs/0123456789abcdef",
        "/api/runs/0123456789abcdef/orders",
        "/api/runs/0123456789abcdef/orders/1",
        "/api/runs/0123456789abcdef/fills",
        "/api/runs/0123456789abcdef/equity",
        "/api/runs/0123456789abcdef/tape",
        "/api/no-such-endpoint",
        "/somewhere-else",
    ] {
        expect_error(&app, get(uri), StatusCode::NOT_FOUND, "not_found").await;
    }

    // 400: malformed run id, malformed JSON, JSON of the wrong shape.
    expect_error(
        &app,
        get("/api/runs/not-a-run-id"),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;
    expect_error(
        &app,
        post_raw("/api/runs", b"{\"config\": ".to_vec()),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;
    expect_error(
        &app,
        post_raw("/api/runs", b"{\"config\": 5}".to_vec()),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;

    // DELETE removes the run; a second DELETE finds nothing.
    let (status, body) = send(&app, delete(&run_uri)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());
    expect_error(&app, get(&run_uri), StatusCode::NOT_FOUND, "not_found").await;
    expect_error(&app, delete(&run_uri), StatusCode::NOT_FOUND, "not_found").await;
    let runs: Vec<RunSummary> = ok(&app, get("/api/runs")).await;
    assert!(runs.iter().all(|r| r.manifest.run_id != probe.manifest.run_id));
    assert_eq!(runs.len(), 3, "baseline, slow and the second grid cell remain");
}

#[tokio::test]
async fn web_ui_is_served_with_single_page_app_fallback() {
    let dir = TempDir::new("web");
    let web = dir.0.join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html><title>BlackBox</title>").unwrap();
    std::fs::write(web.join("assets").join("app.js"), "console.log('ui');").unwrap();
    let app = test_app(&dir, Some(web));

    let (status, body) = send(&app, get("/")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("BlackBox"));

    let (status, body) = send(&app, get("/assets/app.js")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"console.log('ui');");

    // A client-side route is not a file; the app shell is returned so the UI can route it.
    let (status, body) = send(&app, get("/runs/0123456789abcdef/orders")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("BlackBox"));

    // API paths never fall through to the UI.
    let health: Health = ok(&app, get("/api/health")).await;
    assert_eq!(health.status, "ok");
    expect_error(
        &app,
        get("/api/no-such-endpoint"),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;
    // With no dataset at all there is nothing to size presets for.
    let catalogue: Vec<Preset> = ok(&app, get("/api/presets")).await;
    assert!(catalogue.is_empty());
    let datasets: Vec<DatasetManifest> = ok(&app, get("/api/datasets")).await;
    assert!(datasets.is_empty());
}
