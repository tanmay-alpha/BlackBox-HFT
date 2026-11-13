//! `blackbox export-static`: a read-only bundle of precomputed runs for the hosted web viewer.
//!
//! Free static hosting cannot simulate, and recorded market data may not be published: vendor
//! terms forbid redistributing it or offering it through a hosted service, and simulated runs,
//! replay windows and reports are derived from it. The bundle is therefore built from a
//! **synthetic** dataset only; any other dataset is refused before anything is executed or
//! written.
//!
//! Every preset, every run of every canonical demo and one small sensitivity grid are executed
//! through [`pipeline::find_or_execute`] (a run already in the store is read back, so exporting
//! twice simulates nothing twice). The GET responses the web console needs are then written as
//! files holding exactly the JSON the server returns for them, under `<out>/<root>/api/`:
//!
//! | file | server response |
//! | --- | --- |
//! | `health.json` | `GET /api/health` |
//! | `datasets.json`, `datasets/<id>.json` | `GET /api/datasets`, `GET /api/datasets/{id}` |
//! | `datasets/<id>/replay/index.json` | index of the files below, keyed by canonical query |
//! | `datasets/<id>/replay/*.json` | `GET /api/datasets/{id}/replay?...` |
//! | `presets.json`, `demos.json` | `GET /api/presets`, `GET /api/demos` |
//! | `runs.json`, `runs/<run>.json` | `GET /api/runs`, `GET /api/runs/{run}` |
//! | `runs/<run>/{engine-check,fills,equity}.json` | the endpoints of the same name |
//! | `runs/<run>/orders.json` | every `OrderRecord` of the run (the client pages it locally) |
//! | `runs/<run>/orders/<k>.json` | `{ "<order_id>": GET /api/runs/{run}/orders/{order_id} }` for the `k`-th 100 orders by id |
//! | `runs/<run>/orders/index.json` | the first order id of each of those files |
//! | `runs/<run>/timeline/index.json`, `runs/<run>/timeline/*.json` | `GET .../timeline?...` |
//! | `runs/<run>/report.md`, `runs/<run>/export.json` | report and export downloads |
//! | `compare/<a>__<b>.json` | `POST /api/compare` for each demo run against the demo's first run, both ways round |
//! | `grids/default.json` | `POST /api/grids` for the precomputed grid |
//!
//! `<out>/blackbox-static.json` lists what the bundle holds and names its root: the files above
//! live under `<out>/<root>/api/`, where `<root>` is the first 16 hex characters of a SHA-256 over
//! every file's path and content. A changed bundle therefore has new URLs, so hosts may cache the
//! files forever and only the marker must be revalidated. Every API file is gzip-compressed
//! (`.json.gz`; `report.md` is plain): uncompressed, the flight-recorder tape repeated in timeline
//! windows and order details makes the bundle several hundred megabytes. The web client inflates
//! the files and returns the JSON unchanged.
//!
//! Windows: the UI asks for replay and timeline windows of at most 60 s starting anywhere. The
//! bundle holds the exact responses of the Replay page's default and whole-dataset requests, plus
//! tiles of 120 s every 60 s, so any 60 s window lies inside one tile; the client answers such a
//! request with that tile. Tiles keep the server's 60 s depth step and use the server's caps on
//! trades and events, so a tile never holds less than the 60 s request would have returned.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use bb_core::{
    compare::{GridAxis, GridResult, GridSpec},
    config::{OrderLatencyConfig, QueueModelConfig, RunConfig},
    dataset::{DatasetKind, DatasetManifest},
    result::{RunResult, RunSummary},
    time::NS_PER_S,
};
use flate2::{Compression, write::GzEncoder};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    api::{OrderDetail, ReplayQuery, TimelineQuery},
    datasets::{self, DatasetCtx},
    demos,
    error::AppError,
    pipeline, presets, report, server,
    store::{self, RunStore},
};

/// The dataset exported when none is named.
pub const DEFAULT_DATASET: &str = "synthetic-demo";
/// Written last; marks a directory as a bundle. A non-empty directory without it is never
/// overwritten.
pub const MARKER_FILE: &str = "blackbox-static.json";
/// Where an export is written before it is renamed to its content-hash root.
const PARTIAL_DIR: &str = ".partial";
/// Version of the bundle layout described in the module documentation.
pub const FORMAT: u32 = 1;
/// Order details are grouped into files of this many orders, consecutive in order id
/// (`orders/<k>.json`), which keeps the file count of the bundle small.
pub const ORDER_SHARD: usize = 100;
/// Shown by the hosted viewer and recorded in the marker file.
pub const NOTICE: &str = "Hosted viewer: precomputed simulations on synthetic data. Not market \
     data. Run locally to simulate.";

/// The UI's replay window: the Replay page's default length and the Replay Terminal's window.
const UI_WINDOW_NS: i64 = 60 * NS_PER_S;
/// Depth levels and trade cap the Replay page and the Replay Terminal request.
const UI_LEVELS: u32 = 25;
const UI_MAX_TRADES: u32 = 5_000;
/// Tiles are twice the UI window long and start every UI window, so any window of at most
/// `UI_WINDOW_NS` lies entirely inside one tile.
const TILE_SPAN_NS: i64 = 2 * UI_WINDOW_NS;
const TILE_STRIDE_NS: i64 = UI_WINDOW_NS;
/// The server's default depth step for a 60 s window (`span / 300`), kept across the tile.
const TILE_STEP_NS: i64 = UI_WINDOW_NS / 300;
/// The server's caps, so a tile is never cut shorter than a 60 s request would be.
const TILE_MAX_TRADES: u32 = 50_000;
const TILE_MAX_EVENTS: u32 = 50_000;

/// What [`export_static`] executed and wrote.
#[derive(Clone, Debug)]
pub struct ExportSummary {
    pub dataset_id: String,
    /// Every exported run, in execution order: presets, demo runs, grid cells.
    pub runs: Vec<String>,
    pub comparisons: usize,
    pub grid_cells: usize,
    /// Grid cells whose simulation failed; the grid result shows them as missing.
    pub failed_grid_cells: usize,
    pub files: u64,
    /// Size on disk (compressed).
    pub bytes: u64,
    /// Size of the JSON and Markdown before compression.
    pub content_bytes: u64,
}

/// `<out>/blackbox-static.json`: what the bundle holds, read first by the web client.
#[derive(Serialize)]
struct Marker<'a> {
    format: u32,
    notice: &'a str,
    blackbox_version: &'a str,
    dataset_id: &'a str,
    /// Directory under `<out>` holding `api/`.
    root: &'a str,
    runs: &'a [String],
    /// `"<a>__<b>"` for every precomputed comparison of run `a` with run `b`.
    compares: &'a [String],
    /// Files under `api/grids/`, without extension.
    grids: &'a [String],
}

/// Index of the precomputed windows of one replay or timeline endpoint.
#[derive(Serialize)]
struct WindowIndex {
    /// The span tiles cover: the dataset (replay) or the run (timeline).
    start: i64,
    end: i64,
    /// Depth levels of every replay file.
    #[serde(skip_serializing_if = "Option::is_none")]
    levels: Option<u32>,
    /// Canonical query string (keys sorted, absent keys omitted) to bundle path.
    exact: BTreeMap<String, String>,
    tiles: Vec<Tile>,
}

#[derive(Serialize)]
struct Tile {
    t0: i64,
    t1: i64,
    /// The query whose server response the file holds.
    query: String,
    /// Bundle path.
    file: String,
}

/// Refuses every dataset that is not synthetic: the bundle is meant to be published.
pub fn ensure_synthetic(manifest: &DatasetManifest) -> Result<(), AppError> {
    if manifest.kind == DatasetKind::Synthetic {
        return Ok(());
    }
    Err(AppError::InvalidRequest(format!(
        "dataset '{}' is recorded market data ({} / {} / {}); export-static only exports synthetic \
         datasets. The bundle is meant for public hosting, and market-data vendors' terms \
         (including those of the Tardis.dev samples BlackBox ingests) forbid redistributing their \
         data or offering it through a hosted service. Simulated runs, replay windows and \
         reports are derived from that data, so none of them may be published either. Export a \
         synthetic dataset instead (see `blackbox synth`)",
        manifest.id, manifest.source.vendor, manifest.source.exchange, manifest.source.symbol
    )))
}

/// Executes the canonical runs on synthetic dataset `dataset_id` (or reads them back from
/// `store`) and writes the static bundle to `out`.
///
/// Nothing is written before every simulation has finished. An earlier bundle in `out` is
/// replaced; a non-empty `out` that is not a bundle is refused.
pub fn export_static(
    data_root: &Path,
    store: &dyn RunStore,
    dataset_id: &str,
    out: &Path,
    grid_threads: usize,
) -> Result<ExportSummary, AppError> {
    // Checked on the manifest alone, before the events are read or anything runs.
    ensure_synthetic(&datasets::find_manifest(data_root, dataset_id)?)?;
    let ctx = DatasetCtx::load(data_root, dataset_id)?;
    let manifest = ctx.manifest().clone();
    ensure_synthetic(&manifest)?;

    // ----- execute ----------------------------------------------------------------------
    let mut run_ids = Vec::new();
    let catalogue = presets::presets(&manifest);
    for preset in &catalogue {
        let summary = pipeline::find_or_execute(&ctx, &preset.config, store, &preset.id)?;
        push_unique(&mut run_ids, summary.manifest.run_id);
    }
    let demo_list = demos::demos(&manifest);
    let mut pairs: Vec<(String, String)> = Vec::new();
    for demo in &demo_list {
        let mut ids = Vec::new();
        for run in &demo.runs {
            // The label the console's "Run demo" button gives the run.
            let label = format!("{}: {}", demo.title, run.label);
            let summary = pipeline::find_or_execute(&ctx, &run.config, store, &label)?;
            ids.push(summary.manifest.run_id);
        }
        for id in &ids {
            push_unique(&mut run_ids, id.clone());
        }
        // The Latency Lab compares every run of a demo with the demo's first run; the reverse
        // order serves the Compare page's "Swap A and B".
        for other in ids.iter().skip(1).filter(|id| **id != ids[0]) {
            for pair in [
                (ids[0].clone(), other.clone()),
                (other.clone(), ids[0].clone()),
            ] {
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
    }
    let grid = pipeline::run_grid(&ctx, &grid_spec(&manifest)?, store, grid_threads)?;
    for cell in &grid.cells {
        if let Some(id) = &cell.run_id {
            push_unique(&mut run_ids, id.clone());
        }
    }

    let mut summaries = Vec::with_capacity(run_ids.len());
    for id in &run_ids {
        let summary = store
            .summary(id)?
            .ok_or_else(|| AppError::Internal(format!("run {id} vanished from the run store")))?;
        // The invariant that makes the bundle publishable.
        if summary.manifest.dataset.id != manifest.id
            || summary.manifest.dataset.sha256 != manifest.events_sha256
        {
            return Err(AppError::Internal(format!(
                "run {id} was not simulated on synthetic dataset '{}'",
                manifest.id
            )));
        }
        summaries.push(summary);
    }

    // ----- write ------------------------------------------------------------------------
    prepare_out(out)?;
    let partial = out.join(PARTIAL_DIR);
    let mut bundle = Bundle {
        root: partial.clone(),
        files: 0,
        bytes: 0,
        content_bytes: 0,
        digest: Sha256::new(),
    };
    bundle.json("api/health.json", &server::health())?;
    bundle.json("api/datasets.json", &[&manifest])?;
    bundle.json(&format!("api/datasets/{}.json", manifest.id), &manifest)?;
    write_replay(&mut bundle, &ctx)?;
    bundle.json("api/presets.json", &catalogue)?;
    bundle.json("api/demos.json", &demo_list)?;

    let mut listing = summaries.clone();
    store::sort_newest_first(&mut listing);
    bundle.json("api/runs.json", &listing)?;
    for summary in &summaries {
        let run = load(store, &summary.manifest.run_id)?;
        write_run(&mut bundle, data_root, &run, summary)?;
    }

    let mut compares = Vec::with_capacity(pairs.len());
    let label = |id: &str| {
        summaries
            .iter()
            .find(|s| s.manifest.run_id == id)
            .map_or_else(String::new, |s| s.label.clone())
    };
    for (a, b) in &pairs {
        let (run_a, run_b) = (load(store, a)?, load(store, b)?);
        let comparison = bb_forensics::compare_runs(&run_a, &label(a), &run_b, &label(b));
        let key = format!("{a}__{b}");
        bundle.json(&format!("api/compare/{key}.json"), &comparison)?;
        compares.push(key);
    }
    let grids = vec!["default".to_string()];
    bundle.json("api/grids/default.json", &grid)?;

    let digest = bundle.digest.finalize();
    let root: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let root_dir = out.join(&root);
    fs::rename(&partial, &root_dir).map_err(|e| io_error("renaming", &partial, e))?;

    let marker = Marker {
        format: FORMAT,
        notice: NOTICE,
        blackbox_version: env!("CARGO_PKG_VERSION"),
        dataset_id: &manifest.id,
        root: &root,
        runs: &run_ids,
        compares: &compares,
        grids: &grids,
    };
    let marker = serde_json::to_string_pretty(&marker)?;
    let path = out.join(MARKER_FILE);
    fs::write(&path, &marker).map_err(|e| io_error("writing", &path, e))?;
    bundle.files += 1;
    bundle.bytes += marker.len() as u64;
    bundle.content_bytes += marker.len() as u64;

    Ok(ExportSummary {
        dataset_id: manifest.id.clone(),
        comparisons: compares.len(),
        grid_cells: grid.cells.len(),
        failed_grid_cells: failed_cells(&grid),
        runs: run_ids,
        files: bundle.files,
        bytes: bundle.bytes,
        content_bytes: bundle.content_bytes,
    })
}

fn failed_cells(grid: &GridResult) -> usize {
    grid.cells.iter().filter(|c| c.run_id.is_none()).count()
}

fn push_unique(ids: &mut Vec<String>, id: String) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}

fn load(store: &dyn RunStore, run_id: &str) -> Result<std::sync::Arc<RunResult>, AppError> {
    store
        .get(run_id)?
        .ok_or_else(|| AppError::Internal(format!("run {run_id} vanished from the run store")))
}

fn entry_latency(config: &RunConfig) -> i64 {
    match config.execution.order_latency {
        OrderLatencyConfig::Constant { entry_ns, .. }
        | OrderLatencyConfig::Jitter { entry_ns, .. } => entry_ns,
    }
}

/// The precomputed grid: the touch-maker baseline under the two order latencies of the "cost
/// of latency" demo and the three queue models of the "queue position" demo, so four of its six
/// cells are runs the demos already hold.
fn grid_spec(manifest: &DatasetManifest) -> Result<GridSpec, AppError> {
    let preset = |id: &str| {
        presets::find(manifest, id)
            .map(|p| p.config)
            .ok_or_else(|| AppError::Internal(format!("built-in preset {id} is missing")))
    };
    let base = preset("maker_baseline")?;
    let slow = preset("maker_slow")?;
    let mut latencies = vec![entry_latency(&base)];
    if entry_latency(&slow) != latencies[0] {
        latencies.push(entry_latency(&slow));
    }
    let mut queues = vec![QueueModelConfig::RiskAdverse, QueueModelConfig::LogProb];
    if !queues.contains(&base.execution.queue_model) {
        queues.push(base.execution.queue_model);
    }
    Ok(GridSpec {
        base,
        x: GridAxis::OrderLatencyNs { values: latencies },
        y: GridAxis::QueueModel { values: queues },
    })
}

/// Windows `[t0, t1]` of at most `TILE_SPAN_NS`, starting every `TILE_STRIDE_NS` from `start`,
/// the last one ending at `end`.
fn tiles(start: i64, end: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut t0 = start;
    loop {
        let t1 = end.min(t0.saturating_add(TILE_SPAN_NS));
        out.push((t0, t1));
        if t1 >= end {
            return out;
        }
        t0 += TILE_STRIDE_NS;
    }
}

/// `key=value` pairs joined by `&`, absent values skipped. Callers list keys in alphabetical
/// order, which is how the web client canonicalizes a query.
fn query_key(pairs: &[(&str, Option<i64>)]) -> String {
    pairs
        .iter()
        .filter_map(|(key, value)| value.map(|v| format!("{key}={v}")))
        .collect::<Vec<_>>()
        .join("&")
}

fn replay_key(q: &ReplayQuery) -> String {
    query_key(&[
        ("levels", q.levels.map(i64::from)),
        ("max_trades", q.max_trades.map(i64::from)),
        ("step_ns", q.step_ns),
        ("t0", q.t0),
        ("t1", q.t1),
    ])
}

fn timeline_key(q: &TimelineQuery) -> String {
    query_key(&[
        ("max_events", q.max_events.map(i64::from)),
        ("t0", q.t0),
        ("t1", q.t1),
    ])
}

fn write_replay(bundle: &mut Bundle, ctx: &DatasetCtx) -> Result<(), AppError> {
    let m = ctx.manifest();
    let dir = format!("api/datasets/{}/replay", m.id);
    let window = |query: &ReplayQuery| -> Result<_, AppError> {
        let request = server::replay_request(m.start, m.end, query)?;
        Ok(bb_data::book::replay_window(
            ctx.dataset.events.as_slice(),
            &m.instrument,
            ctx.dataset.origin_ns,
            &request,
        ))
    };
    let ui = |t0: i64, t1: i64| ReplayQuery {
        t0: Some(t0),
        t1: Some(t1),
        step_ns: None,
        levels: Some(UI_LEVELS),
        max_trades: Some(UI_MAX_TRADES),
    };
    let mut exact = BTreeMap::new();
    // The Replay page's default window and its "entire dataset" choice.
    for query in [
        ui(m.start, m.end.min(m.start.saturating_add(UI_WINDOW_NS))),
        ui(m.start, m.end),
    ] {
        let key = replay_key(&query);
        if exact.contains_key(&key) {
            continue;
        }
        let file = format!("{dir}/q{}.json.gz", exact.len());
        bundle.json(file.trim_end_matches(".gz"), &window(&query)?)?;
        exact.insert(key, file);
    }
    let mut tile_list = Vec::new();
    for (k, (t0, t1)) in tiles(m.start, m.end).into_iter().enumerate() {
        let query = ReplayQuery {
            t0: Some(t0),
            t1: Some(t1),
            step_ns: Some(TILE_STEP_NS),
            levels: Some(UI_LEVELS),
            max_trades: Some(TILE_MAX_TRADES),
        };
        let file = format!("{dir}/tile{k}.json.gz");
        bundle.json(file.trim_end_matches(".gz"), &window(&query)?)?;
        tile_list.push(Tile {
            t0,
            t1,
            query: replay_key(&query),
            file,
        });
    }
    bundle.json(
        &format!("{dir}/index.json"),
        &WindowIndex {
            start: m.start,
            end: m.end,
            levels: Some(UI_LEVELS),
            exact,
            tiles: tile_list,
        },
    )
}

fn write_run(
    bundle: &mut Bundle,
    data_root: &Path,
    run: &RunResult,
    summary: &RunSummary,
) -> Result<(), AppError> {
    let id = &summary.manifest.run_id;
    let dir = format!("api/runs/{id}");
    // Refuses a dataset whose bytes changed since the run, as the server does.
    let dataset = datasets::manifest_for_run(data_root, &run.manifest)?;

    bundle.json(&format!("{dir}.json"), summary)?;
    bundle.json(&format!("{dir}/engine-check.json"), &run.engine_check)?;
    bundle.json(&format!("{dir}/fills.json"), &run.fills)?;
    bundle.json(&format!("{dir}/equity.json"), &run.equity)?;
    bundle.json(&format!("{dir}/orders.json"), &run.orders)?;

    // Order ids are sparse, so shards hold `ORDER_SHARD` consecutive orders in id order and
    // `orders/index.json` lists the first order id of each shard.
    let mut order_ids: Vec<u64> = run.orders.iter().map(|o| o.order_id).collect();
    order_ids.sort_unstable();
    let mut first_ids = Vec::new();
    for (k, chunk) in order_ids.chunks(ORDER_SHARD).enumerate() {
        let mut shard: BTreeMap<u64, OrderDetail> = BTreeMap::new();
        for &order_id in chunk {
            let detail = pipeline::order_detail(run, &dataset, order_id).ok_or_else(|| {
                AppError::Internal(format!("run {id} has no detail for order {order_id}"))
            })?;
            shard.insert(order_id, detail);
        }
        bundle.json(&format!("{dir}/orders/{k}.json"), &shard)?;
        first_ids.push(chunk[0]);
    }
    bundle.json(&format!("{dir}/orders/index.json"), &first_ids)?;

    bundle.text(
        &format!("{dir}/report.md"),
        &report::markdown(run, &dataset)?,
    )?;
    bundle.json(&format!("{dir}/export.json"), run)?;

    // The Replay Terminal first asks for the run start with a count-only request.
    let first = TimelineQuery {
        t0: None,
        t1: None,
        max_events: Some(0),
    };
    let head = server::timeline(run, &first)?;
    let (start, end) = (head.run_start, head.run_end);
    let mut exact = BTreeMap::new();
    let file = format!("{dir}/timeline/q0.json.gz");
    bundle.json(file.trim_end_matches(".gz"), &head)?;
    exact.insert(timeline_key(&first), file);
    let mut tile_list = Vec::new();
    for (k, (t0, t1)) in tiles(start, end).into_iter().enumerate() {
        let query = TimelineQuery {
            t0: Some(t0),
            t1: Some(t1),
            max_events: Some(TILE_MAX_EVENTS),
        };
        let file = format!("{dir}/timeline/tile{k}.json.gz");
        bundle.json(
            file.trim_end_matches(".gz"),
            &server::timeline(run, &query)?,
        )?;
        tile_list.push(Tile {
            t0,
            t1,
            query: timeline_key(&query),
            file,
        });
    }
    bundle.json(
        &format!("{dir}/timeline/index.json"),
        &WindowIndex {
            start,
            end,
            levels: None,
            exact,
            tiles: tile_list,
        },
    )
}

/// Creates `out`, or empties it when it holds an earlier bundle (or an interrupted export).
fn prepare_out(out: &Path) -> Result<(), AppError> {
    match fs::read_dir(out) {
        Ok(entries) => {
            let entries = entries
                .map(|entry| entry.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| io_error("reading", out, e))?;
            let ours =
                out.join(MARKER_FILE).is_file() || entries.iter().all(|p| p.ends_with(PARTIAL_DIR));
            if !ours {
                return Err(AppError::InvalidRequest(format!(
                    "{} is not empty and holds no {MARKER_FILE}; refusing to write a static \
                     bundle into it",
                    out.display()
                )));
            }
            // Replace the whole earlier bundle so no stale run or window survives.
            for path in entries {
                let removed = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                removed.map_err(|e| io_error("removing", &path, e))?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error("reading", out, e)),
    }
    fs::create_dir_all(out).map_err(|e| io_error("creating", out, e))
}

fn io_error(action: &str, path: &Path, err: io::Error) -> AppError {
    AppError::Internal(format!("{action} {}: {err}", path.display()))
}

/// Writes bundle files, keeps count of them and hashes their paths and contents.
struct Bundle {
    root: PathBuf,
    files: u64,
    bytes: u64,
    content_bytes: u64,
    digest: Sha256,
}

impl Bundle {
    fn create(&self, rel: &str) -> Result<(PathBuf, File), AppError> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error("creating", parent, e))?;
        }
        let file = File::create(&path).map_err(|e| io_error("creating", &path, e))?;
        Ok((path, file))
    }

    /// Writes `value` as the server's compact JSON body, gzip-compressed to `<rel>.gz`.
    fn json<T: Serialize + ?Sized>(&mut self, rel: &str, value: &T) -> Result<(), AppError> {
        let rel = format!("{rel}.gz");
        let (path, file) = self.create(&rel)?;
        self.digest.update(rel.as_bytes());
        self.digest.update([0]);
        let mut writer = Counting {
            inner: BufWriter::with_capacity(1 << 16, GzEncoder::new(file, Compression::default())),
            count: 0,
            digest: &mut self.digest,
        };
        serde_json::to_writer(&mut writer, value)?;
        let content = writer.count;
        writer
            .inner
            .into_inner()
            .map_err(|e| io_error("writing", &path, e.into_error()))?
            .finish()
            .map_err(|e| io_error("writing", &path, e))?;
        self.record(&path, content)
    }

    fn text(&mut self, rel: &str, text: &str) -> Result<(), AppError> {
        let (path, mut file) = self.create(rel)?;
        file.write_all(text.as_bytes())
            .map_err(|e| io_error("writing", &path, e))?;
        self.digest.update(rel.as_bytes());
        self.digest.update([0]);
        self.digest.update(text.as_bytes());
        self.record(&path, text.len() as u64)
    }

    fn record(&mut self, path: &Path, content: u64) -> Result<(), AppError> {
        // The length ends the file's entry, so no two bundles hash the same byte stream.
        self.digest.update(content.to_le_bytes());
        let len = fs::metadata(path)
            .map_err(|e| io_error("reading", path, e))?
            .len();
        self.files += 1;
        self.bytes += len;
        self.content_bytes += content;
        Ok(())
    }
}

/// Counts and hashes the bytes written through it.
struct Counting<'a, W> {
    inner: W,
    count: u64,
    digest: &'a mut Sha256,
}

impl<W: Write> Write for Counting<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.digest.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Read,
        sync::atomic::{AtomicU64, Ordering},
    };

    use bb_core::result::OrderRecord;
    use bb_data::synthetic::SyntheticConfig;
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::{api::RunTimeline, store::FsRunStore};

    /// Long enough for every preset to act (the probe's last cancel is due at 7 s).
    const SECONDS: i64 = 30;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bb-export-static-{}-{}-{name}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_dataset(root: &Path, id: &str) -> DatasetManifest {
        let config = SyntheticConfig {
            id: id.to_string(),
            duration_ns: SECONDS * NS_PER_S,
            ..SyntheticConfig::default()
        };
        let built = bb_data::synthetic::generate(&config).unwrap();
        fs::create_dir_all(root).unwrap();
        bb_data::store::save(root, &built).unwrap()
    }

    fn read_gz<T: DeserializeOwned>(path: &Path) -> T {
        let mut text = String::new();
        flate2::read::GzDecoder::new(File::open(path).unwrap_or_else(|e| {
            panic!("{}: {e}", path.display());
        }))
        .read_to_string(&mut text)
        .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn recorded_datasets_are_refused_before_anything_runs_or_is_written() {
        let dir = TempDir::new("refuse");
        let root = dir.0.join("datasets");
        write_dataset(&root, "it-recorded");
        // Same events, declared as recorded market data.
        let path = root.join("it-recorded").join("dataset.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["kind"] = "recorded".into();
        fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

        let store = FsRunStore::open(dir.0.join("runs")).unwrap();
        let out = dir.0.join("bundle");
        let err = export_static(&root, &store, "it-recorded", &out, 1).unwrap_err();
        assert!(matches!(err, AppError::InvalidRequest(_)), "{err}");
        let message = err.to_string();
        assert!(message.contains("recorded market data"), "{message}");
        assert!(message.contains("redistribut"), "{message}");
        assert!(!out.exists(), "nothing may be written");
        assert!(store.list().unwrap().is_empty(), "nothing may be simulated");
    }

    #[test]
    fn synthetic_export_writes_the_documented_bundle() {
        let dir = TempDir::new("export");
        let root = dir.0.join("datasets");
        let manifest = write_dataset(&root, "it-synth");
        let store = FsRunStore::open(dir.0.join("runs")).unwrap();
        let out = dir.0.join("bundle");

        let summary = export_static(&root, &store, "it-synth", &out, 2).unwrap();
        // 7 presets, 2 demo-only variants, 2 grid-only cells.
        assert_eq!(summary.runs.len(), 11, "{:?}", summary.runs);
        assert_eq!(summary.comparisons, 10);
        assert_eq!(summary.grid_cells, 6);
        assert_eq!(summary.failed_grid_cells, 0);
        assert!(summary.files > 0 && summary.bytes < summary.content_bytes);

        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(MARKER_FILE)).unwrap()).unwrap();
        assert_eq!(marker["dataset_id"], "it-synth");
        assert_eq!(marker["runs"].as_array().unwrap().len(), 11);
        let bundle_root = marker["root"].as_str().unwrap();
        assert_eq!(bundle_root.len(), 16);
        let base = out.join(bundle_root);
        let api = base.join("api");
        assert!(!out.join(PARTIAL_DIR).exists());

        let health: crate::api::Health = read_gz(&api.join("health.json.gz"));
        assert_eq!(health, server::health());
        let listed: Vec<DatasetManifest> = read_gz(&api.join("datasets.json.gz"));
        assert_eq!(listed, vec![manifest.clone()]);
        let runs: Vec<RunSummary> = read_gz(&api.join("runs.json.gz"));
        assert_eq!(runs.len(), 11);
        assert!(runs.iter().all(|r| r.manifest.dataset.id == "it-synth"));
        let _: Vec<crate::api::Preset> = read_gz(&api.join("presets.json.gz"));
        let _: Vec<crate::api::Demo> = read_gz(&api.join("demos.json.gz"));

        for id in &summary.runs {
            let run_dir = api.join("runs").join(id);
            let run = store.get(id).unwrap().unwrap();
            let stored: RunSummary = read_gz(&api.join("runs").join(format!("{id}.json.gz")));
            assert_eq!(stored, store.summary(id).unwrap().unwrap());
            let orders: Vec<OrderRecord> = read_gz(&run_dir.join("orders.json.gz"));
            assert_eq!(orders, run.orders);
            // Every order has exactly the server's detail, in the shard the index names.
            let first_ids: Vec<u64> = read_gz(&run_dir.join("orders/index.json.gz"));
            let mut details = 0;
            for order in &run.orders {
                let k = first_ids.partition_point(|&first| first <= order.order_id) - 1;
                let shard: BTreeMap<String, OrderDetail> =
                    read_gz(&run_dir.join("orders").join(format!("{k}.json.gz")));
                let detail = &shard[&order.order_id.to_string()];
                assert_eq!(
                    Some(detail),
                    pipeline::order_detail(&run, &manifest, order.order_id).as_ref()
                );
                details += 1;
            }
            assert_eq!(details, run.orders.len());
            for file in ["engine-check", "fills", "equity", "export"] {
                assert!(run_dir.join(format!("{file}.json.gz")).is_file(), "{file}");
            }
            let exported: RunResult = read_gz(&run_dir.join("export.json.gz"));
            assert_eq!(exported.manifest, run.manifest);
            assert!(run_dir.join("report.md").is_file());

            let index: serde_json::Value = read_gz(&run_dir.join("timeline/index.json.gz"));
            let head: RunTimeline =
                read_gz(&base.join(index["exact"]["max_events=0"].as_str().unwrap()));
            assert_eq!(
                head,
                server::timeline(
                    &run,
                    &TimelineQuery {
                        max_events: Some(0),
                        ..TimelineQuery::default()
                    }
                )
                .unwrap()
            );
            // A 30 s run fits one tile, which holds every tape event.
            let tiles = index["tiles"].as_array().unwrap();
            assert_eq!(tiles.len(), 1);
            let tile: RunTimeline = read_gz(&base.join(tiles[0]["file"].as_str().unwrap()));
            assert_eq!(tile.events.len(), run.tape.len());
            assert!(!tile.events_truncated);
        }

        let replay: serde_json::Value =
            read_gz(&api.join("datasets/it-synth/replay/index.json.gz"));
        let default_key = format!(
            "levels=25&max_trades=5000&t0={}&t1={}",
            manifest.start, manifest.end
        );
        assert!(replay["exact"][&default_key].is_string(), "{replay}");
        assert_eq!(replay["tiles"].as_array().unwrap().len(), 1);
        for pair in marker["compares"].as_array().unwrap() {
            let pair = pair.as_str().unwrap();
            assert!(
                api.join(format!("compare/{pair}.json.gz")).is_file(),
                "{pair}"
            );
        }
        let grid: GridResult = read_gz(&api.join("grids/default.json.gz"));
        assert_eq!(grid.cells.len(), 6);

        // Exporting again simulates nothing and replaces the bundle.
        let again = export_static(&root, &store, "it-synth", &out, 2).unwrap();
        assert_eq!(again.runs, summary.runs);
        assert_eq!(store.list().unwrap().len(), 11);
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(MARKER_FILE)).unwrap()).unwrap();
        let entries = fs::read_dir(&out).unwrap().count();
        assert_eq!(entries, 2, "only the marker and the new root remain");
        assert!(
            out.join(marker["root"].as_str().unwrap())
                .join("api")
                .is_dir()
        );

        // A directory that is not a bundle is never overwritten.
        let foreign = dir.0.join("foreign");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("keep.txt"), "x").unwrap();
        let err = export_static(&root, &store, "it-synth", &foreign, 1).unwrap_err();
        assert!(matches!(err, AppError::InvalidRequest(_)), "{err}");
        assert!(foreign.join("keep.txt").is_file());
    }
}
