//! `blackbox`: command-line front end of BlackBox HFT.
//!
//! Offline commands (`synth`, `run`, `explain`, `compare`, `grid`, …) work on local files under
//! `--data-dir` and need no database. `serve`, `worker`, `migrate`, `keys`, `register-dataset`
//! and `demo seed` use Postgres (`DATABASE_URL`, see docs/engineering/DEPLOYMENT.md §5).

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use bb_app::{
    DISCLAIMER,
    config::{AppConfig, Profile},
    datasets::{self, DatasetCache, DatasetCtx},
    demos, pipeline, presets, runtime,
    server::{self, AppState},
    store::{FsRunStore, RunStore},
};
use bb_core::{
    compare::{Comparison, DecisionOutcome, DeltaStats, GridResult, GridSpec, OrderPair},
    config::RunConfig,
    dataset::{DatasetManifest, Quantiles},
    result::{RunResult, RunSummary},
    time::{NS_PER_S, TimeUnit},
};
use bb_data::{synthetic::SyntheticConfig, tardis::TardisIngestConfig};
use bb_db::Scope;
use clap::{ArgGroup, Args, Parser, Subcommand};

/// Printed by every command that reports simulation output.
const SIMULATED_NOTE: &str = "Simulated results: produced by replaying market data through a \
     simulation model under the stated assumptions. Simulated orders never move replayed prices \
     (no market impact). Not real executions.";

#[derive(Parser)]
#[command(
    name = "blackbox",
    version,
    about = "BlackBox HFT: research-only execution forensics over replayed Level-2 market data"
)]
struct Cli {
    /// Directory holding `datasets/` and `runs/`.
    #[arg(long, global = true, default_value = "data")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a deterministic synthetic dataset and print its quality summary.
    Synth {
        /// Dataset id (`[a-z0-9_-]+`). Default: the generator's default id.
        #[arg(long)]
        id: Option<String>,
        /// Generator seed. Default: the generator's default seed.
        #[arg(long)]
        seed: Option<u64>,
        /// Length of the dataset in seconds. Default: the generator's default duration.
        #[arg(long)]
        duration_s: Option<u64>,
    },
    /// Convert Tardis.dev CSV files (trades + incremental_book_L2) into a dataset.
    IngestTardis(IngestArgs),
    /// Rebuild the order book of a NASDAQ TotalView-ITCH 5.0 file and print its validation
    /// manifest. Local use only: never commit or upload ITCH files or anything derived from them.
    ItchReconstruct(ItchArgs),
    /// List datasets.
    Datasets,
    /// Print every data-quality check of a dataset.
    Quality {
        /// Dataset id.
        id: String,
    },
    /// List the curated presets, sized for a dataset.
    Presets {
        /// Dataset id. Default: the first dataset by id.
        #[arg(long)]
        dataset: Option<String>,
    },
    /// List controlled experiments, optionally executing every run idempotently.
    Demos {
        /// Dataset id. Default: the first dataset by id.
        #[arg(long)]
        dataset: Option<String>,
        /// Execute all configurations and compare each alternative against its baseline.
        #[arg(long)]
        run: bool,
    },
    /// Simulate one configuration and store the run.
    Run(RunArgs),
    /// Explain why one order of a stored run did or did not fill.
    Explain { run_id: String, order_id: u64 },
    /// Compare two stored runs.
    Compare { run_a: String, run_b: String },
    /// Run a bounded two-parameter sensitivity grid.
    Grid {
        /// JSON file holding a `GridSpec`.
        #[arg(long)]
        spec: PathBuf,
        /// Worker threads. Default: the number of available CPUs.
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Serve the HTTP API `/api/v1` with the in-process job worker (needs DATABASE_URL).
    Serve(ServeArgs),
    /// Run the job worker only (standalone; polls with backoff while idle).
    Worker,
    /// Apply the database migrations over DATABASE_URL_UNPOOLED.
    Migrate,
    /// Mint, revoke and list API keys.
    #[command(subcommand)]
    Keys(KeysCommand),
    /// Write the OpenAPI document of /api/v1 (no database or network needed).
    Openapi {
        /// Output file. Default: standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Register a dataset under `<data-dir>/datasets` as a dataset version in the database.
    RegisterDataset {
        /// Dataset id.
        id: String,
    },
    /// Demo workspace commands (`demo seed`).
    #[command(subcommand)]
    Demo(DemoCommand),
}

#[derive(Args)]
struct IngestArgs {
    /// Tardis `trades` CSV (`.csv` or `.csv.gz`).
    #[arg(long)]
    trades: PathBuf,
    /// Tardis `incremental_book_L2` CSV (`.csv` or `.csv.gz`).
    #[arg(long)]
    book: PathBuf,
    /// Dataset id (`[a-z0-9_-]+`).
    #[arg(long)]
    id: String,
    /// Display title. Default: "<exchange> <symbol>".
    #[arg(long)]
    title: Option<String>,
    /// Exchange name exactly as it appears in the files, e.g. `binance-futures`.
    #[arg(long)]
    exchange: String,
    /// Symbol exactly as it appears in the files, e.g. `BTCUSDT`.
    #[arg(long)]
    symbol: String,
    /// Minimum price increment of the instrument.
    #[arg(long)]
    tick_size: f64,
    /// Minimum quantity increment of the instrument.
    #[arg(long)]
    lot_size: f64,
    /// Keep only this many seconds from the start of the book file.
    #[arg(long)]
    max_duration_s: Option<u64>,
}

#[derive(Args)]
struct ItchArgs {
    /// ITCH 5.0 file in the 2-byte length-prefixed format, plain or gzip-compressed.
    file: PathBuf,
    /// Stop after this many messages.
    #[arg(long)]
    max_messages: Option<u64>,
    /// Stock to normalize into Level-2 rows (repeatable), as in the stock directory, e.g. `AAPL`.
    #[arg(long = "symbol")]
    symbols: Vec<String>,
    /// Epoch nanoseconds of the session's midnight (US Eastern), added to every ITCH timestamp.
    #[arg(long, default_value_t = 0)]
    midnight_epoch_ns: i64,
    /// Write the manifest as JSON to this file.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Only frame and decode, without any book: a quick scan, or a timing of the decoder alone.
    #[arg(long, conflicts_with_all = ["symbols", "out"])]
    decode_only: bool,
}

#[derive(Args)]
#[command(group(ArgGroup::new("source").required(true).args(["preset", "config"])))]
struct RunArgs {
    /// Preset id (see `blackbox presets`).
    #[arg(long)]
    preset: Option<String>,
    /// JSON file holding a `RunConfig`.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Dataset id. With --preset: the dataset to size the preset for (default: the first).
    /// With --config: overrides the file's `dataset_id`.
    #[arg(long)]
    dataset: Option<String>,
    /// Label stored with the run.
    #[arg(long)]
    label: Option<String>,
}

#[derive(Args)]
struct ServeArgs {
    /// Address to listen on. Precedence: --addr > BB_ADDR > 0.0.0.0:$PORT > 127.0.0.1:8787.
    #[arg(long)]
    addr: Option<SocketAddr>,
    /// Directory of the built web UI. Served when it contains an `index.html`.
    #[arg(long, default_value = "web/dist")]
    web_dir: PathBuf,
}

#[derive(Subcommand)]
enum DemoCommand {
    /// Register the synthetic demo dataset, queue the canonical demo experiments in the public
    /// demo workspace and, once their runs are completed, store the curated comparisons and
    /// saved reports there (idempotent; needs DATABASE_URL). Prints what it created and what
    /// is still pending.
    Seed {
        /// Full platform class the results are recorded for, e.g. `linux-x86_64+libm-53154a4e`
        /// (default BB_RESULT_PLATFORM or this build's class). Only a worker of exactly that
        /// class computes the queued jobs; hosted results are produced by the Linux worker, so
        /// seeding hosted data from another machine passes the Linux worker's class.
        #[arg(long)]
        platform: Option<String>,
        /// Compute queued demo runs in this process (this machine's platform only) before
        /// curating, instead of leaving them to `serve`/`worker`.
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Mint an API key and print it once (only its sha256 is stored).
    Mint {
        /// Workspace slug; a research workspace with this slug is created if missing.
        #[arg(long)]
        workspace: String,
        /// Comma-separated scopes: read, run, admin.
        #[arg(long, value_delimiter = ',', default_value = "read,run")]
        scopes: Vec<String>,
        #[arg(long)]
        label: String,
        /// Lifetime such as `30d` or `12h`. Admin keys: default 30d, at most 90d.
        #[arg(long)]
        expires_in: Option<String>,
    },
    /// Revoke a key by id (effective on the next request).
    Revoke { id: String },
    /// List keys (never their secrets).
    List {
        #[arg(long)]
        workspace: Option<String>,
    },
}

/// Where things live under `--data-dir`.
struct Paths {
    datasets: PathBuf,
    runs: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths {
        datasets: cli.data_dir.join("datasets"),
        runs: cli.data_dir.join("runs"),
    };
    let data_dir = cli.data_dir.as_path();
    match cli.command {
        Command::Synth {
            id,
            seed,
            duration_s,
        } => cmd_synth(&paths, id, seed, duration_s),
        Command::IngestTardis(args) => cmd_ingest_tardis(&paths, args),
        Command::ItchReconstruct(args) => cmd_itch_reconstruct(args),
        Command::Datasets => cmd_datasets(&paths),
        Command::Quality { id } => cmd_quality(&paths, &id),
        Command::Presets { dataset } => cmd_presets(&paths, dataset.as_deref()),
        Command::Demos { dataset, run } => cmd_demos(&paths, dataset.as_deref(), run),
        Command::Run(args) => cmd_run(&paths, args),
        Command::Explain { run_id, order_id } => cmd_explain(&paths, &run_id, order_id),
        Command::Compare { run_a, run_b } => cmd_compare(&paths, &run_a, &run_b),
        Command::Grid { spec, threads } => cmd_grid(&paths, &spec, threads),
        Command::Serve(args) => cmd_serve(&paths, data_dir, args),
        Command::Worker => cmd_worker(data_dir),
        Command::Migrate => cmd_migrate(data_dir),
        Command::Keys(command) => cmd_keys(data_dir, command),
        Command::Openapi { out } => cmd_openapi(out),
        Command::RegisterDataset { id } => cmd_register_dataset(&paths, data_dir, &id),
        Command::Demo(DemoCommand::Seed { platform, wait }) => {
            cmd_demo_seed(data_dir, platform, wait)
        }
    }
}

// ----- helpers -----

/// The serialized (snake_case) name of a contract enum, so the CLI and the API use one
/// vocabulary.
fn tag<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(err) => format!("<unprintable: {err}>"),
    }
}

fn opt(value: Option<f64>, decimals: usize) -> String {
    match value {
        Some(v) => format!("{v:.decimals$}"),
        None => "-".to_string(),
    }
}

/// A quantity without binary floating-point noise (`0.09999999999999999` prints as `0.1`).
/// Nine decimals is far below any lot size, so no real digit is lost.
fn qty(value: f64) -> String {
    let s = format!("{value:.9}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn opt_ms(ns: Option<i64>) -> String {
    match ns {
        Some(ns) => format!("{:.3} ms", ns as f64 / 1e6),
        None => "-".to_string(),
    }
}

fn seconds(ns: i64) -> f64 {
    ns as f64 / NS_PER_S as f64
}

fn seconds_to_ns(seconds: u64, flag: &str) -> Result<i64> {
    i64::try_from(seconds)
        .ok()
        .and_then(|s| s.checked_mul(NS_PER_S))
        .ok_or_else(|| anyhow!("{flag} {seconds} is too large"))
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

/// The dataset named by `id`, or the first dataset by id when none is named.
fn resolve_dataset(paths: &Paths, id: Option<&str>) -> Result<DatasetManifest> {
    match id {
        Some(id) => Ok(datasets::find_manifest(&paths.datasets, id)?),
        None => datasets::list_manifests(&paths.datasets)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                anyhow!(
                    "no datasets under {}; create one with `blackbox synth`",
                    paths.datasets.display()
                )
            }),
    }
}

fn load_run(store: &FsRunStore, run_id: &str) -> Result<Arc<RunResult>> {
    store
        .get(run_id)?
        .ok_or_else(|| anyhow!("run '{run_id}' not found in {}", store.root().display()))
}

fn load_summary(store: &FsRunStore, run_id: &str) -> Result<RunSummary> {
    store
        .summary(run_id)?
        .ok_or_else(|| anyhow!("run '{run_id}' not found in {}", store.root().display()))
}

fn worst_quality(manifest: &DatasetManifest) -> String {
    match manifest.quality.worst() {
        Some(severity) => tag(&severity),
        None => "ok".to_string(),
    }
}

fn print_quantiles(name: &str, unit: &str, q: Option<&Quantiles>) {
    match q {
        Some(q) => println!(
            "  {name:<22} n={} min={:.3} p50={:.3} p90={:.3} p99={:.3} max={:.3} mean={:.3} {unit}",
            q.n, q.min, q.p50, q.p90, q.p99, q.max, q.mean
        ),
        None => println!("  {name:<22} not available"),
    }
}

fn print_dataset_summary(manifest: &DatasetManifest) {
    println!("dataset   : {} ({})", manifest.id, manifest.title);
    println!(
        "kind      : {} / {}",
        tag(&manifest.kind),
        tag(&manifest.level)
    );
    println!(
        "source    : {} {} {}",
        manifest.source.vendor, manifest.source.exchange, manifest.source.symbol
    );
    println!(
        "instrument: tick size {}, lot size {}, contract size {}",
        manifest.instrument.tick_size,
        manifest.instrument.lot_size,
        manifest.instrument.contract_size
    );
    println!(
        "events    : {} rows ({} trades, {} depth updates, {} snapshot levels), {:.3} s",
        manifest.counts.rows,
        manifest.counts.trades,
        manifest.counts.depth_updates,
        manifest.counts.snapshot_levels,
        seconds(manifest.end - manifest.start)
    );
    println!(
        "file      : {} ({} bytes, sha256 {})",
        manifest.events_file, manifest.events_bytes, manifest.events_sha256
    );
    println!(
        "local time: {} ({})",
        tag(&manifest.timestamps.local_ts),
        manifest.timestamps.note
    );
}

fn print_quality_summary(manifest: &DatasetManifest) {
    let quality = &manifest.quality;
    let failed: Vec<_> = quality.checks.iter().filter(|c| !c.passed).collect();
    println!(
        "quality   : {} checks, {} with findings, worst severity {}",
        quality.checks.len(),
        failed.len(),
        worst_quality(manifest)
    );
    for check in failed {
        println!(
            "  [{}] {} (count {}): {}",
            tag(&check.severity),
            check.id,
            check.count,
            check.detail
        );
    }
    print_quantiles(
        "feed latency",
        "ms",
        scaled(quality.feed_latency_ns, 1e-6).as_ref(),
    );
    print_quantiles("spread", "ticks", quality.spread_ticks.as_ref());
    println!(
        "  {:<22} {:.4}",
        "book valid fraction", quality.book_valid_fraction
    );
}

/// Quantiles multiplied by `factor` (for unit conversion in output).
fn scaled(q: Option<Quantiles>, factor: f64) -> Option<Quantiles> {
    q.map(|q| Quantiles {
        n: q.n,
        min: q.min * factor,
        p50: q.p50 * factor,
        p90: q.p90 * factor,
        p99: q.p99 * factor,
        max: q.max * factor,
        mean: q.mean * factor,
    })
}

// ----- datasets -----

fn write_synthetic(paths: &Paths, config: &SyntheticConfig) -> Result<DatasetManifest> {
    let built = bb_data::synthetic::generate(config)?;
    std::fs::create_dir_all(&paths.datasets)
        .with_context(|| format!("creating {}", paths.datasets.display()))?;
    Ok(bb_data::store::save(&paths.datasets, &built)?)
}

fn cmd_synth(
    paths: &Paths,
    id: Option<String>,
    seed: Option<u64>,
    duration_s: Option<u64>,
) -> Result<()> {
    let mut config = SyntheticConfig::default();
    if let Some(id) = id {
        config.id = id;
    }
    if let Some(seed) = seed {
        config.seed = seed;
    }
    if let Some(duration_s) = duration_s {
        config.duration_ns = seconds_to_ns(duration_s, "--duration-s")?;
    }
    let manifest = write_synthetic(paths, &config)?;
    println!(
        "Wrote synthetic dataset to {}",
        paths.datasets.join(&manifest.id).display()
    );
    print_dataset_summary(&manifest);
    print_quality_summary(&manifest);
    println!(
        "This dataset is synthetic: generated from seed {}, not recorded market data.",
        config.seed
    );
    Ok(())
}

fn cmd_ingest_tardis(paths: &Paths, args: IngestArgs) -> Result<()> {
    let max_duration_ns = args
        .max_duration_s
        .map(|s| seconds_to_ns(s, "--max-duration-s"))
        .transpose()?;
    let config = TardisIngestConfig {
        id: args.id,
        title: args
            .title
            .unwrap_or_else(|| format!("{} {}", args.exchange, args.symbol)),
        trades_path: args.trades,
        book_path: args.book,
        exchange: args.exchange,
        symbol: args.symbol,
        tick_size: args.tick_size,
        lot_size: args.lot_size,
        contract_size: 1.0,
        // Tardis CSV timestamps are microseconds since the epoch.
        time_unit: TimeUnit::Micros,
        max_duration_ns,
        base_latency_ns: 0,
        gap_warn_ns: bb_data::tardis::DEFAULT_GAP_WARN_NS,
    };
    let built = bb_data::tardis::ingest(&config)?;
    std::fs::create_dir_all(&paths.datasets)
        .with_context(|| format!("creating {}", paths.datasets.display()))?;
    let manifest = bb_data::store::save(&paths.datasets, &built)?;
    println!(
        "Wrote dataset to {}",
        paths.datasets.join(&manifest.id).display()
    );
    print_dataset_summary(&manifest);
    print_quality_summary(&manifest);
    println!("license   : {}", manifest.source.license_note);
    Ok(())
}

fn cmd_itch_reconstruct(args: ItchArgs) -> Result<()> {
    use bb_data::itch::{DecodeScan, ItchOptions, reconstruct_file, run_file};

    let started = std::time::Instant::now();
    let (source, frames, decoded, stop) = if args.decode_only {
        let mut scan = DecodeScan::new(args.max_messages);
        let (source, stop, decoded) = run_file(&args.file, &mut scan)?;
        println!("frames    : {} ({} invalid)", scan.frames, scan.invalid);
        for (kind, count) in scan.by_type() {
            println!("  type {kind:<5} {count}");
        }
        (source, scan.frames, decoded, stop.record(scan.frames))
    } else {
        let options = ItchOptions {
            max_messages: args.max_messages,
            symbols: args.symbols,
            midnight_epoch_ns: args.midnight_epoch_ns,
        };
        let r = reconstruct_file(&args.file, options)?;
        let m = r.manifest;
        let b = &m.book;
        println!("frames    : {}", m.frames);
        println!(
            "book      : {} stocks with orders, {} orders added, {} live at the end, {} \
             executions, {} quarantined",
            b.stocks_with_orders,
            b.orders_added,
            b.live_orders_at_end,
            b.executions,
            b.quarantined_total
        );
        for s in &m.normalization.streams {
            println!(
                "l2        : {} (locate {}): {} rows ({} depth, {} trades)",
                s.symbol, s.locate, s.rows, s.depth_rows, s.trade_rows
            );
        }
        let failed: Vec<_> = m.checks.iter().filter(|c| !c.passed).collect();
        println!(
            "checks    : {} checks, {} with findings, worst severity {}",
            m.checks.len(),
            failed.len(),
            m.worst().map_or_else(|| "ok".to_string(), |s| tag(&s))
        );
        for check in failed {
            println!(
                "  [{}] {} (count {}): {}",
                tag(&check.severity),
                check.id,
                check.count,
                check.examples.first().map_or("", String::as_str)
            );
        }
        if let Some(out) = &args.out {
            std::fs::write(out, serde_json::to_vec_pretty(&m)?)
                .with_context(|| format!("writing {}", out.display()))?;
            println!("manifest  : {}", out.display());
        }
        (m.source, m.frames, m.decoded_bytes, m.stop)
    };
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "source    : {} ({}, {} bytes, sha256 {})",
        source.name, source.compression, source.bytes, source.sha256
    );
    println!(
        "stop      : {}{}",
        stop.reason,
        stop.detail.map(|d| format!(" ({d})")).unwrap_or_default()
    );
    println!(
        "time      : {elapsed:.3} s wall, {:.0} messages/s, {:.1} MB/s decompressed ({} build; \
         includes reading, decompressing and hashing the whole file)",
        frames as f64 / elapsed,
        decoded as f64 / elapsed / 1e6,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!("license   : {}", bb_data::itch::LICENSE_NOTE);
    Ok(())
}

fn cmd_datasets(paths: &Paths) -> Result<()> {
    let manifests = datasets::list_manifests(&paths.datasets)?;
    if manifests.is_empty() {
        println!(
            "No datasets under {}. Create one with `blackbox synth`.",
            paths.datasets.display()
        );
        return Ok(());
    }
    let width = manifests
        .iter()
        .map(|m| m.id.len())
        .max()
        .unwrap_or(0)
        .max("ID".len());
    println!(
        "{:<width$}  {:<9}  {:>12}  {:>10}  QUALITY",
        "ID", "KIND", "EVENTS", "SPAN_S"
    );
    for m in &manifests {
        println!(
            "{:<width$}  {:<9}  {:>12}  {:>10.3}  {}",
            m.id,
            tag(&m.kind),
            m.counts.rows,
            seconds(m.end - m.start),
            worst_quality(m)
        );
    }
    Ok(())
}

fn cmd_quality(paths: &Paths, id: &str) -> Result<()> {
    let manifest = datasets::find_manifest(&paths.datasets, id)?;
    print_dataset_summary(&manifest);
    let quality = &manifest.quality;
    println!("checks    : {}", quality.checks.len());
    for check in &quality.checks {
        println!(
            "  [{}] {:<5} {} - {} (count {})",
            if check.passed { "pass" } else { "FAIL" },
            tag(&check.severity),
            check.id,
            check.title,
            check.count
        );
        if !check.detail.is_empty() {
            println!("         {}", check.detail);
        }
        for example in &check.examples {
            println!("         e.g. {example}");
        }
    }
    print_quantiles(
        "feed latency",
        "ms",
        scaled(quality.feed_latency_ns, 1e-6).as_ref(),
    );
    print_quantiles("spread", "ticks", quality.spread_ticks.as_ref());
    print_quantiles(
        "inter-event gap",
        "ms",
        scaled(quality.inter_event_gap_ns, 1e-6).as_ref(),
    );
    println!(
        "  {:<22} {:.4}",
        "book valid fraction", quality.book_valid_fraction
    );
    println!(
        "worst severity among findings: {}",
        worst_quality(&manifest)
    );
    Ok(())
}

// ----- presets and runs -----

fn cmd_presets(paths: &Paths, dataset: Option<&str>) -> Result<()> {
    let manifest = resolve_dataset(paths, dataset)?;
    println!("Presets sized for dataset '{}':", manifest.id);
    for preset in presets::presets(&manifest) {
        println!();
        println!("{}  -  {}", preset.id, preset.title);
        println!("  demonstrates: {}", preset.demonstrates);
        println!("  {}", preset.description);
    }
    Ok(())
}

fn cmd_demos(paths: &Paths, dataset: Option<&str>, execute: bool) -> Result<()> {
    let manifest = resolve_dataset(paths, dataset)?;
    let ctx = if execute {
        Some(DatasetCtx::load(&paths.datasets, &manifest.id)?)
    } else {
        None
    };
    let store = if execute {
        Some(FsRunStore::open(&paths.runs)?)
    } else {
        None
    };
    for demo in demos::demos(&manifest) {
        println!("\n{} - {}", demo.id, demo.title);
        println!("  {}", demo.question);
        println!("  Varied: {}", demo.varied.join(", "));
        for controlled in &demo.controlled {
            println!("  Controlled: {controlled}");
        }
        println!("  Read: {}", demo.how_to_read);
        let mut baseline: Option<(Arc<RunResult>, String)> = None;
        for (scenario, recipe) in demo.runs.iter().zip(&demo.reproduce) {
            println!("\n  {}", scenario.label);
            println!("{recipe}");
            if let (Some(ctx), Some(store)) = (&ctx, &store) {
                let summary =
                    pipeline::find_or_execute(ctx, &scenario.config, store, &scenario.label)?;
                println!("  Stored simulated run: {}", summary.manifest.run_id);
                let result = load_run(store, &summary.manifest.run_id)?;
                if let Some((first, label)) = &baseline {
                    let comparison = bb_forensics::compare_runs(
                        bb_forensics::CompareInput {
                            run: first,
                            label,
                            instrument: Some(&manifest.instrument),
                        },
                        bb_forensics::CompareInput {
                            run: &result,
                            label: &scenario.label,
                            instrument: Some(&manifest.instrument),
                        },
                    );
                    print_comparison(&comparison);
                } else {
                    baseline = Some((result, scenario.label.clone()));
                }
            }
        }
    }
    println!("\n{SIMULATED_NOTE}");
    Ok(())
}

fn print_run(summary: &RunSummary, run: &RunResult) {
    let manifest = &summary.manifest;
    let m = &summary.metrics;
    println!("run id        : {}", manifest.run_id);
    println!("label         : {}", summary.label);
    println!(
        "dataset       : {} (events sha256 {})",
        manifest.dataset.id, manifest.dataset.sha256
    );
    println!("strategy      : {}", manifest.config.strategy.kind.label());
    println!(
        "execution     : {} exchange model, {} queue model, order latency {}, feed offset {} ns, \
         fees maker {} / taker {}, seed {}",
        tag(&manifest.config.execution.exchange_model),
        manifest.config.execution.queue_model.label(),
        serde_json::to_string(&manifest.config.execution.order_latency)
            .unwrap_or_else(|err| format!("<unprintable: {err}>")),
        manifest.config.execution.feed_latency_offset_ns,
        manifest.config.execution.fees.maker,
        manifest.config.execution.fees.taker,
        manifest.config.seed
    );
    println!("config sha256 : {}", manifest.config_sha256);
    println!("result sha256 : {}", manifest.result_sha256);
    println!(
        "engine        : {} {} [{}] on {} ({} build), blackbox {}",
        manifest.engine.name,
        manifest.engine.version,
        manifest.engine.features.join(", "),
        manifest.platform,
        manifest.profile,
        manifest.blackbox_version
    );
    let check = &run.engine_check;
    println!(
        "engine check  : {}",
        if check.agrees {
            "ledger agrees with the engine's account state"
        } else {
            "LEDGER DISAGREES with the engine's account state"
        }
    );
    println!(
        "                position {} vs {}, cash {:.6} vs balance {:.6}, fees {:.6} vs {:.6}, \
         fills {} vs trades {}",
        qty(check.ledger_position),
        qty(check.engine_position),
        check.ledger_cash,
        check.engine_balance,
        check.ledger_fees,
        check.engine_fee,
        check.ledger_fills,
        check.engine_num_trades
    );
    if !check.note.is_empty() {
        println!("                note: {}", check.note);
    }
    println!(
        "size          : {} ticks, {} orders, {} fills, {} tape events, {} ms wall time",
        summary.ticks,
        summary.order_count,
        summary.fill_count,
        summary.tape_len,
        manifest.wall_time_ms
    );
    println!(
        "orders        : intended {}, risk-blocked {}, refused {}, submitted {}",
        m.orders_intended, m.orders_risk_blocked, m.orders_refused, m.orders_submitted
    );
    println!(
        "outcomes      : filled {}, partially filled {}, canceled {}, expired {}, working at end {}",
        m.orders_filled,
        m.orders_partially_filled,
        m.orders_canceled,
        m.orders_expired,
        m.orders_working_at_end
    );
    println!(
        "fills         : {} ({} maker, {} taker), fill ratio by orders {}, by quantity {}",
        m.fill_count,
        m.maker_fill_count,
        m.taker_fill_count,
        opt(m.fill_ratio_orders, 4),
        opt(m.fill_ratio_qty, 4)
    );
    println!(
        "activity      : volume {}, turnover {:.6}, fees {:.6}",
        qty(m.volume),
        m.turnover,
        m.fees
    );
    println!(
        "inventory     : final {}, max long {}, max short {}, mean |position| {:.6}",
        qty(m.final_position),
        qty(m.max_long),
        qty(m.max_short),
        m.mean_abs_position
    );
    println!(
        "p&l           : net {:.6} (realized {:.6}, unrealized {:.6}, fees {:.6}), max drawdown {:.6}",
        m.net_pnl, m.realized_pnl, m.unrealized_pnl, m.fees, m.max_drawdown
    );
    println!(
        "spread capture: {:.6} quote at the moment of each passive fill ({} ticks per unit)",
        m.spread_capture,
        opt(m.spread_capture_ticks, 4)
    );
    for markout in &m.markouts {
        println!(
            "markout {:>6.1} s: {} ticks over {} fills",
            seconds(markout.horizon_ns),
            opt(markout.mean_ticks, 4),
            markout.fills
        );
    }
    println!(
        "risk          : {} blocks, halted: {}",
        m.risk_blocks, m.risk_halted
    );
}

fn cmd_run(paths: &Paths, args: RunArgs) -> Result<()> {
    let (config, default_label) = match (&args.preset, &args.config) {
        (Some(preset_id), None) => {
            let manifest = resolve_dataset(paths, args.dataset.as_deref())?;
            let preset = presets::find(&manifest, preset_id).ok_or_else(|| {
                anyhow!("unknown preset '{preset_id}'; list them with `blackbox presets`")
            })?;
            (preset.config, preset.id)
        }
        (None, Some(path)) => {
            let mut config: RunConfig = read_json(path)?;
            if let Some(dataset) = &args.dataset {
                config.dataset_id = dataset.clone();
            }
            let label = config.strategy.kind.label().to_string();
            (config, label)
        }
        _ => bail!("give exactly one of --preset and --config"),
    };
    let label = args.label.unwrap_or(default_label);

    pipeline::check_config(&config)?;
    let ctx = DatasetCtx::load(&paths.datasets, &config.dataset_id)?;
    let store = FsRunStore::open(&paths.runs)?;
    let identity = pipeline::run_identity(ctx.manifest(), &config);
    let already_stored = store.summary(&identity.run_id)?.is_some();
    let summary = pipeline::find_or_execute(&ctx, &config, &store, &label)?;
    let run = load_run(&store, &summary.manifest.run_id)?;

    if already_stored {
        println!(
            "This configuration was already simulated on this dataset; showing the stored run."
        );
    }
    print_run(&summary, &run);
    println!("{SIMULATED_NOTE}");
    Ok(())
}

fn cmd_explain(paths: &Paths, run_id: &str, order_id: u64) -> Result<()> {
    let store = FsRunStore::open(&paths.runs)?;
    let run = load_run(&store, run_id)?;
    let dataset = datasets::manifest_for_run(&paths.datasets, &run.manifest)?;
    let detail = pipeline::order_detail(&run, &dataset, order_id)
        .ok_or_else(|| anyhow!("run '{run_id}' has no order {order_id}"))?;
    let order = &detail.order;
    let explanation = &detail.explanation;

    println!(
        "order {} of run {}: {} {} @ {} (tick {}), {} {}",
        order.order_id,
        run_id,
        tag(&order.side),
        order.qty,
        detail.instrument.price(order.price_tick),
        order.price_tick,
        tag(&order.tif),
        tag(&order.ord_type)
    );
    println!("strategy reason: {}", order.reason);
    println!(
        "outcome: {} (filled {} of {}, {} fills)",
        tag(&order.outcome),
        order.filled_qty,
        order.qty,
        order.fill_count
    );
    println!();
    println!(
        "{} -> {}",
        tag(&explanation.question),
        tag(&explanation.verdict)
    );
    println!("{}", explanation.headline);
    println!();
    println!("Reasoning (each step tagged with its basis):");
    for (i, step) in explanation.steps.iter().enumerate() {
        let seqs = if step.event_seqs.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = step.event_seqs.iter().map(|s| format!("#{s}")).collect();
            format!("  (tape {})", list.join(", "))
        };
        println!(
            "  {:>2}. [{}] {}{}",
            i + 1,
            tag(&step.basis),
            step.statement,
            seqs
        );
    }
    let latency = &explanation.latency;
    println!();
    println!("Timing:");
    println!(
        "  feed staleness at decision : {}",
        opt_ms(latency.feed_staleness_ns)
    );
    println!(
        "  feed latency of that event : {}",
        opt_ms(latency.feed_latency_ns)
    );
    println!(
        "  order entry latency        : {}",
        opt_ms(latency.entry_latency_ns)
    );
    println!(
        "  response latency           : {}",
        opt_ms(latency.response_latency_ns)
    );
    println!(
        "  arrival to first fill      : {}",
        opt_ms(latency.time_to_first_fill_ns)
    );
    println!(
        "  resting time               : {}",
        opt_ms(latency.resting_time_ns)
    );
    println!(
        "  cancel entry latency       : {}",
        opt_ms(latency.cancel_entry_latency_ns)
    );
    println!();
    println!("Caveats:");
    for caveat in &explanation.caveats {
        println!("  - {caveat}");
    }
    println!();
    println!("{SIMULATED_NOTE}");
    Ok(())
}

fn metric_value(value: Option<f64>, unit: &str) -> String {
    let decimals = match unit {
        "count" | "ns" => 0,
        "ratio" => 4,
        "ticks" => 3,
        _ => 6,
    };
    opt(value, decimals)
}

fn decision_text(outcome: Option<&DecisionOutcome>) -> String {
    match outcome {
        Some(d) => format!(
            "{} {} @ tick {} decided at {:.6} s, arrival {}, first fill {} -> {} (filled {})",
            tag(&d.side),
            d.qty,
            d.price_tick,
            seconds(d.decision_ts),
            d.exch_arrival_ts
                .map_or_else(|| "-".to_string(), |t| format!("{:.6} s", seconds(t))),
            d.first_fill_ts
                .map_or_else(|| "-".to_string(), |t| format!("{:.6} s", seconds(t))),
            tag(&d.outcome),
            d.filled_qty
        ),
        None => "no such decision".to_string(),
    }
}

fn print_pair(pair: &OrderPair) {
    println!(
        "  order {} (tick {}, decided at {:.3} s): {}",
        pair.order_id,
        pair.tick_no,
        seconds(pair.decision_ts),
        tag(&pair.class)
    );
    if !pair.differences.is_empty() {
        println!("    differs in: {}", pair.differences.join(", "));
    }
    println!("    A: {}", decision_text(pair.a.as_ref()));
    println!("    B: {}", decision_text(pair.b.as_ref()));
}

fn print_delta_stats(name: &str, s: &DeltaStats) {
    let ns = |v: Option<i64>| v.map_or_else(|| "-".to_string(), |v| v.to_string());
    println!(
        "  {name:<15} pairs {:>7}, nonzero {:>7}, min {:>12} ns, median {:>14} ns, max {:>12} ns",
        s.count,
        s.nonzero,
        ns(s.min),
        s.median
            .map_or_else(|| "-".to_string(), |v| format!("{v:.1}")),
        ns(s.max)
    );
}

fn print_comparison(c: &Comparison) {
    println!("A: run {} ({})", c.run_a, c.label_a);
    println!("B: run {} ({})", c.run_b, c.label_b);
    println!(
        "per-order alignment: {}, like-for-like: {}",
        if c.eligibility.aligned { "yes" } else { "no" },
        if c.eligibility.like_for_like {
            "yes"
        } else {
            "no"
        }
    );
    for r in &c.eligibility.reasons {
        println!(
            "  {} [{}]: A = {}, B = {}. {}",
            r.field,
            tag(&r.effect),
            r.a,
            r.b,
            r.explanation
        );
    }
    println!();
    if c.config_diffs.is_empty() {
        println!("Configuration: identical.");
    } else {
        println!("Configuration differences:");
        for diff in &c.config_diffs {
            println!("  {}: A = {}, B = {}", diff.path, diff.a, diff.b);
        }
    }
    println!();
    let width = c
        .metrics
        .iter()
        .map(|row| row.label.len() + row.unit.len() + 3)
        .max()
        .unwrap_or(0)
        .max("METRIC".len());
    if !c.eligibility.like_for_like {
        println!("Metric deltas are not like-for-like (see the reasons above).");
    }
    println!(
        "{:<width$}  {:>18}  {:>18}  {:>18}",
        "METRIC", "A", "B", "B - A"
    );
    for row in &c.metrics {
        let name = format!("{} ({})", row.label, row.unit);
        println!(
            "{:<width$}  {:>18}  {:>18}  {:>18}",
            name,
            metric_value(row.a, &row.unit),
            metric_value(row.b, &row.unit),
            metric_value(row.delta, &row.unit)
        );
    }
    println!();
    match &c.alignment {
        None => println!("Decisions: not aligned; order ids do not name the same decisions."),
        Some(a) => {
            println!(
                "Decisions: {} identical, {} same fills with a different recorded history, {} same \
                 fills with different fees, {} same outcome with different timing, {} same \
                 decision with a different outcome, {} different decisions, {} only in A, {} only \
                 in B",
                a.identical,
                a.same_fills_different_history,
                a.same_fills_different_fees,
                a.same_outcome_different_timing,
                a.same_decision_different_outcome,
                a.different_decision,
                a.only_a,
                a.only_b
            );
            println!("Timing deltas (B - A) over pairs with the same decision:");
            print_delta_stats("decision", &a.timing.decision);
            print_delta_stats("exch arrival", &a.timing.exch_arrival);
            print_delta_stats("first response", &a.timing.first_response);
            print_delta_stats("first fill", &a.timing.first_fill);
            print_delta_stats("completion", &a.timing.completion);
            match &a.first_divergence {
                Some(pair) => {
                    println!(
                        "First divergence (every pair before it is identical, timing included):"
                    );
                    print_pair(pair);
                }
                None => println!("The two runs never diverge."),
            }
            if let Some(pair) = &a.first_outcome_divergence
                && a.first_divergence.as_ref() != Some(pair)
            {
                println!("First difference in a decision or its fills:");
                print_pair(pair);
            }
        }
    }
    println!();
    println!("Interpretation (each statement tagged with its basis):");
    for note in &c.interpretation {
        println!("  [{}] {}", tag(&note.basis), note.statement);
    }
}

fn cmd_compare(paths: &Paths, run_a: &str, run_b: &str) -> Result<()> {
    let store = FsRunStore::open(&paths.runs)?;
    let label_a = load_summary(&store, run_a)?.label;
    let label_b = load_summary(&store, run_b)?.label;
    let a = load_run(&store, run_a)?;
    let b = load_run(&store, run_b)?;
    let instrument = |run: &RunResult| {
        datasets::manifest_for_run(&paths.datasets, &run.manifest)
            .ok()
            .map(|m| m.instrument)
    };
    let (inst_a, inst_b) = (instrument(&a), instrument(&b));
    let comparison = bb_forensics::compare_runs(
        bb_forensics::CompareInput {
            run: &a,
            label: &label_a,
            instrument: inst_a.as_ref(),
        },
        bb_forensics::CompareInput {
            run: &b,
            label: &label_b,
            instrument: inst_b.as_ref(),
        },
    );
    print_comparison(&comparison);
    println!();
    println!("{SIMULATED_NOTE}");
    Ok(())
}

fn print_grid(grid: &GridResult) {
    let x_len = grid.x_labels.len();
    println!(
        "Grid: x = {} ({} values), y = {} ({} values), {} ms wall time",
        grid.spec.x.name(),
        x_len,
        grid.spec.y.name(),
        grid.y_labels.len(),
        grid.wall_time_ms
    );
    println!();
    println!(
        "Net P&L per cell (rows: y, columns: x; `failed` marks a cell that did not complete):"
    );
    let y_width = grid
        .y_labels
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max(grid.spec.y.name().len());
    let x_width = grid
        .x_labels
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max(14);
    let mut header = format!("{:<y_width$}", grid.spec.y.name());
    for label in &grid.x_labels {
        header.push_str(&format!("  {label:>x_width$}"));
    }
    println!("{header}");
    for (yi, y_label) in grid.y_labels.iter().enumerate() {
        let mut line = format!("{y_label:<y_width$}");
        for xi in 0..x_len {
            let text = match grid
                .cells
                .get(yi * x_len + xi)
                .and_then(|c| c.metrics.as_ref())
            {
                Some(metrics) => format!("{:.6}", metrics.net_pnl),
                None => "failed".to_string(),
            };
            line.push_str(&format!("  {text:>x_width$}"));
        }
        println!("{line}");
    }
    println!();
    println!("Cells:");
    for cell in &grid.cells {
        let x_label = grid
            .x_labels
            .get(cell.xi as usize)
            .map_or("?", String::as_str);
        let y_label = grid
            .y_labels
            .get(cell.yi as usize)
            .map_or("?", String::as_str);
        match (&cell.run_id, &cell.metrics, &cell.error) {
            (Some(run_id), Some(metrics), _) => println!(
                "  x={x_label} y={y_label}: run {run_id}, {} fills, net p&l {:.6}, fees {:.6}",
                metrics.fill_count, metrics.net_pnl, metrics.fees
            ),
            (_, _, Some(error)) => println!("  x={x_label} y={y_label}: FAILED: {error}"),
            _ => println!("  x={x_label} y={y_label}: no result recorded"),
        }
    }
}

fn cmd_grid(paths: &Paths, spec_path: &Path, threads: Option<usize>) -> Result<()> {
    let spec: GridSpec = read_json(spec_path)?;
    pipeline::check_grid(&spec)?;
    let ctx = DatasetCtx::load(&paths.datasets, &spec.base.dataset_id)?;
    let store = FsRunStore::open(&paths.runs)?;
    let grid = pipeline::run_grid(&ctx, &spec, &store, threads.unwrap_or_else(default_threads))?;
    print_grid(&grid);
    println!();
    println!("{SIMULATED_NOTE}");
    Ok(())
}

// ----- server, worker, database commands -----

fn tokio_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")
}

fn app_config(cli_data_dir: &Path) -> Result<AppConfig> {
    let cfg = AppConfig::from_env(cli_data_dir.to_path_buf())?;
    runtime::init_tracing(cfg.profile);
    tracing::info!(config = %cfg.summary(), "configuration");
    Ok(cfg)
}

/// The legacy file-store router (deprecated `/api`) and the web UI, mounted as the local
/// profile's `serve` fallback.
fn legacy_router(paths: &Paths, args: &ServeArgs) -> Result<axum::Router> {
    let store: Arc<dyn RunStore> = Arc::new(FsRunStore::open(&paths.runs)?);
    let state = Arc::new(AppState {
        data_root: paths.datasets.clone(),
        datasets: DatasetCache::new(),
        store,
        grid_threads: default_threads(),
    });
    let web_dir = if args.web_dir.join("index.html").is_file() {
        Some(args.web_dir.clone())
    } else {
        println!(
            "Web UI not found at {} (no index.html); serving the API only.",
            args.web_dir.display()
        );
        None
    };
    Ok(server::router(state, web_dir))
}

fn cmd_serve(paths: &Paths, data_dir: &Path, args: ServeArgs) -> Result<()> {
    let mut cfg = app_config(data_dir)?;
    if let Some(addr) = args.addr {
        cfg.addr = addr;
    }
    let addr = cfg.addr;
    // The deprecated `/api` router and the web UI stay available locally until the UI uses
    // `/api/v1`; the hosted profile never serves them (TARGET_ARCHITECTURE §11).
    let legacy = match cfg.profile {
        Profile::Local => Some(legacy_router(paths, &args)?),
        Profile::Hosted => None,
    };
    let slots = cfg.worker_slots;
    tokio_runtime()?.block_on(async move {
        let rt = runtime::Runtime::build(cfg, "in_process", slots).await?;
        println!("{DISCLAIMER}");
        runtime::serve(
            rt,
            runtime::ServeOptions { addr, legacy },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    })
}

fn cmd_worker(data_dir: &Path) -> Result<()> {
    let cfg = app_config(data_dir)?;
    let slots = cfg.worker_slots.max(1);
    tokio_runtime()?.block_on(async move {
        let rt = runtime::Runtime::build(cfg, "standalone", slots).await?;
        runtime::worker_only(rt).await
    })
}

fn cmd_migrate(data_dir: &Path) -> Result<()> {
    let cfg = app_config(data_dir)?;
    tokio_runtime()?.block_on(async move {
        runtime::migrate(&cfg).await?;
        println!("Migrations are up to date.");
        Ok(())
    })
}

fn parse_lifetime(text: &str) -> Result<chrono::Duration> {
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let n: i64 = number
        .parse()
        .map_err(|_| anyhow!("--expires-in must look like 30d or 12h"))?;
    match unit {
        "d" => Ok(chrono::Duration::days(n)),
        "h" => Ok(chrono::Duration::hours(n)),
        _ => bail!("--expires-in must end in d or h"),
    }
}

fn cmd_keys(data_dir: &Path, command: KeysCommand) -> Result<()> {
    let cfg = app_config(data_dir)?;
    tokio_runtime()?.block_on(async move {
        let metrics = Arc::new(bb_app::metrics::Metrics::default());
        let db = runtime::connect(&cfg, metrics).await?;
        match command {
            KeysCommand::Mint {
                workspace,
                scopes,
                label,
                expires_in,
            } => {
                let scopes: Vec<Scope> = scopes
                    .iter()
                    .map(|s| match s.trim() {
                        "read" => Ok(Scope::Read),
                        "run" => Ok(Scope::Run),
                        "admin" => Ok(Scope::Admin),
                        other => Err(anyhow!("unknown scope `{other}` (read, run, admin)")),
                    })
                    .collect::<Result<_>>()?;
                let admin = scopes.contains(&Scope::Admin);
                let lifetime = match (expires_in.as_deref(), admin) {
                    (Some(text), _) => Some(parse_lifetime(text)?),
                    (None, true) => Some(chrono::Duration::days(30)),
                    (None, false) => None,
                };
                if admin && lifetime.is_some_and(|l| l > chrono::Duration::days(90)) {
                    bail!("admin keys may live at most 90 days");
                }
                let ws = match db.workspace(&workspace).await {
                    Ok(ws) => ws,
                    Err(bb_db::Error::NotFound) => {
                        println!("Creating research workspace '{workspace}'.");
                        db.create_workspace(&workspace, &workspace)
                            .await
                            .map_err(|e| anyhow!("creating workspace: {e}"))?
                    }
                    Err(e) => bail!("workspace lookup: {e}"),
                };
                let minted = db
                    .mint_key_tagged(
                        ws.id,
                        &label,
                        &scopes,
                        lifetime.map(|l| chrono::Utc::now() + l),
                        cfg.api_key_pepper.expose().as_bytes(),
                    )
                    .await
                    .map_err(|e| anyhow!("minting the key: {e}"))?;
                println!("Key id     : {}", minted.key.id);
                println!("Workspace  : {}", ws.slug);
                println!("Scopes     : {}", minted.key.scopes.join(","));
                println!(
                    "Expires    : {}",
                    minted
                        .key
                        .expires_at
                        .map_or_else(|| "never".to_string(), |t| t.to_rfc3339())
                );
                println!("API key (shown once, store it now; only its sha256 is kept):");
                println!("{}", minted.plaintext);
            }
            KeysCommand::Revoke { id } => {
                let id: uuid::Uuid = id.parse().map_err(|_| anyhow!("key id must be a UUID"))?;
                let key = db
                    .revoke_key_any(id, &bb_db::Audit::default())
                    .await
                    .map_err(|e| anyhow!("revoking the key: {e}"))?;
                println!("Revoked key {} ({})", key.id, key.display_prefix);
            }
            KeysCommand::List { workspace } => {
                let keys = match &workspace {
                    Some(slug) => {
                        let ws = db
                            .workspace(slug)
                            .await
                            .map_err(|e| anyhow!("workspace: {e}"))?;
                        db.api_keys(ws.id).await.map_err(|e| anyhow!("{e}"))?
                    }
                    None => db.all_api_keys().await.map_err(|e| anyhow!("{e}"))?,
                };
                println!(
                    "{:<36}  {:<8}  {:<15}  {:<10}  LABEL",
                    "ID", "PREFIX", "SCOPES", "STATE"
                );
                for k in keys {
                    let state = if k.revoked_at.is_some() {
                        "revoked"
                    } else if k.expires_at.is_some_and(|t| t <= chrono::Utc::now()) {
                        "expired"
                    } else {
                        "active"
                    };
                    println!(
                        "{:<36}  {:<8}  {:<15}  {:<10}  {}",
                        k.id,
                        k.display_prefix,
                        k.scopes.join(","),
                        state,
                        k.label
                    );
                }
            }
        }
        db.pool().close().await;
        Ok(())
    })
}

fn cmd_openapi(out: Option<PathBuf>) -> Result<()> {
    let doc = bb_app::v1::OPENAPI_JSON.clone();
    match out {
        Some(path) => {
            std::fs::write(&path, format!("{doc}\n"))
                .with_context(|| format!("writing {}", path.display()))?;
            println!("Wrote {}", path.display());
        }
        None => println!("{doc}"),
    }
    Ok(())
}

fn cmd_register_dataset(paths: &Paths, data_dir: &Path, id: &str) -> Result<()> {
    let cfg = app_config(data_dir)?;
    let manifest = datasets::find_manifest(&paths.datasets, id)?;
    let hosted = cfg.profile == Profile::Hosted;
    tokio_runtime()?.block_on(async move {
        let db = runtime::connect(&cfg, Arc::new(bb_app::metrics::Metrics::default())).await?;
        let storage = if matches!(manifest.kind, bb_core::dataset::DatasetKind::Synthetic)
            && manifest.generator.is_some()
        {
            "regenerate"
        } else {
            "fs"
        };
        let version = bb_app::registry::register(&db, &manifest, storage, hosted)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        println!(
            "Registered dataset '{}' version {} (events sha256 {}, storage {storage}).",
            version.dataset_id,
            version.version_no,
            hex::encode(&version.events_sha256)
        );
        Ok(())
    })
}

fn cmd_demo_seed(data_dir: &Path, platform: Option<String>, wait: bool) -> Result<()> {
    let cfg = app_config(data_dir)?;
    let platform = platform.unwrap_or_else(|| cfg.result_platform.clone());
    if !bb_app::config::is_platform(&platform) {
        bail!(
            "`{platform}` is not a platform class like {}",
            bb_core::platform::platform_class()
        );
    }
    tokio_runtime()?.block_on(async move {
        // With --wait this process runs a standalone worker loop (heartbeat mode `standalone`).
        let (mode, slots) = if wait { ("standalone", 1) } else { ("seed", 0) };
        let rt = runtime::Runtime::build(cfg, mode, slots).await?;
        let report = bb_app::seed::seed(&rt, &platform, wait).await?;
        println!(
            "Demo dataset '{}' version {} (events sha256 {}).",
            report.dataset_id, report.version, report.events_sha256
        );
        println!("Demo experiments in workspace 'demo' (platform {platform}):");
        for (label, run_id, state) in &report.experiments {
            println!(
                "  {run_id}  {:<10}  {label}",
                state.as_deref().unwrap_or("-")
            );
        }
        println!("Curated comparisons and saved reports in workspace 'demo':");
        for c in &report.curated {
            println!(
                "  {:<10}  {:<8}  {}  ({})",
                c.kind, c.status, c.title, c.detail
            );
        }
        if report.curated.iter().any(|c| c.status == "pending") {
            println!(
                "Pending items need completed runs: a worker of platform {platform} computes the \
                 queued ones (`blackbox serve` or `blackbox worker`); then run `blackbox demo \
                 seed` again, or use `blackbox demo seed --wait`."
            );
        }
        rt.db.pool().close().await;
        Ok(())
    })
}
