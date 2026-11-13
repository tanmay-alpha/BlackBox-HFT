//! `blackbox`: command-line front end of BlackBox HFT.
//!
//! Every command works on local files under `--data-dir`. The binary never makes an outbound
//! network request; `serve` and `demo` only listen.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use bb_app::{
    DISCLAIMER,
    datasets::{self, DatasetCache, DatasetCtx},
    demos, pipeline, presets,
    server::{self, AppState},
    store::{FsRunStore, RunStore},
};
use bb_core::{
    compare::{Comparison, DecisionOutcome, GridResult, GridSpec, OrderPair},
    config::RunConfig,
    dataset::{DatasetManifest, Quantiles},
    result::{RunResult, RunSummary},
    time::{NS_PER_S, TimeUnit},
};
use bb_data::{synthetic::SyntheticConfig, tardis::TardisIngestConfig};
use clap::{ArgGroup, Args, Parser, Subcommand};

/// Printed by every command that reports simulation output.
const SIMULATED_NOTE: &str = "Simulated results: produced by replaying market data through a \
     simulation model under the stated assumptions. Not real executions.";

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
    /// Serve the HTTP API (and the web UI, if built).
    Serve(ServeArgs),
    /// Create the default synthetic dataset if no dataset exists, then serve.
    Demo(ServeArgs),
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
    /// Address to listen on. Loopback by default: the API has no authentication.
    #[arg(long, default_value = "127.0.0.1:8787")]
    addr: SocketAddr,
    /// Directory of the built web UI. Served when it contains an `index.html`.
    #[arg(long, default_value = "web/dist")]
    web_dir: PathBuf,
    /// Extra `host:port` the server answers to (repeatable). Its own loopback names on the
    /// listening port are always allowed; any other Host header is refused (DNS rebinding).
    #[arg(long = "allow-host")]
    allow_host: Vec<String>,
    /// Extra origin allowed to call the API (repeatable), e.g. `http://localhost:5173` for the
    /// Vite development server. Requests from any other origin are refused.
    #[arg(long = "allow-origin")]
    allow_origin: Vec<String>,
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
    match cli.command {
        Command::Synth {
            id,
            seed,
            duration_s,
        } => cmd_synth(&paths, id, seed, duration_s),
        Command::IngestTardis(args) => cmd_ingest_tardis(&paths, args),
        Command::Datasets => cmd_datasets(&paths),
        Command::Quality { id } => cmd_quality(&paths, &id),
        Command::Presets { dataset } => cmd_presets(&paths, dataset.as_deref()),
        Command::Demos { dataset, run } => cmd_demos(&paths, dataset.as_deref(), run),
        Command::Run(args) => cmd_run(&paths, args),
        Command::Explain { run_id, order_id } => cmd_explain(&paths, &run_id, order_id),
        Command::Compare { run_a, run_b } => cmd_compare(&paths, &run_a, &run_b),
        Command::Grid { spec, threads } => cmd_grid(&paths, &spec, threads),
        Command::Serve(args) => cmd_serve(&paths, args),
        Command::Demo(args) => cmd_demo(&paths, args),
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
                    let comparison =
                        bb_forensics::compare_runs(first, label, &result, &scenario.label);
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
            "{} {} @ tick {} -> {} (filled {})",
            tag(&d.side),
            d.qty,
            d.price_tick,
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
    println!("    A: {}", decision_text(pair.a.as_ref()));
    println!("    B: {}", decision_text(pair.b.as_ref()));
}

fn print_comparison(c: &Comparison) {
    println!("A: run {} ({})", c.run_a, c.label_a);
    println!("B: run {} ({})", c.run_b, c.label_b);
    println!(
        "same dataset: {}, same strategy configuration: {}",
        c.same_dataset, c.same_strategy
    );
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
    let a = &c.alignment;
    println!(
        "Decisions: {} identical, {} same decision with a different outcome, {} different \
         decisions, {} only in A, {} only in B",
        a.identical, a.same_decision_different_outcome, a.different_decision, a.only_a, a.only_b
    );
    match &a.first_divergence {
        Some(pair) => {
            println!("First divergence (everything before it is the same in both runs):");
            print_pair(pair);
        }
        None => println!("The two runs never diverge."),
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
    let comparison = bb_forensics::compare_runs(&a, &label_a, &b, &label_b);
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

// ----- server -----

fn cmd_serve(paths: &Paths, args: ServeArgs) -> Result<()> {
    let store: Arc<dyn RunStore> = Arc::new(FsRunStore::open(&paths.runs)?);
    let state = Arc::new(AppState {
        data_root: paths.datasets.clone(),
        datasets: DatasetCache::new(),
        store,
        grid_threads: default_threads(),
    });
    let web_dir = if args.web_dir.join("index.html").is_file() {
        Some(args.web_dir)
    } else {
        println!(
            "Web UI not found at {} (no index.html); serving the API only.",
            args.web_dir.display()
        );
        None
    };
    let serves_ui = web_dir.is_some();
    let policy =
        bb_app::access::AccessPolicy::for_listener(args.addr, &args.allow_host, &args.allow_origin);
    println!(
        "Accepting requests addressed to: {}",
        policy.hosts().join(", ")
    );
    let app = bb_app::access::guard(server::router(state, web_dir), policy);
    let addr = args.addr;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr}"))?;
        let local = listener.local_addr().context("reading the bound address")?;
        if serves_ui {
            println!("BlackBox HFT is running at http://{local}/");
        }
        println!("API: http://{local}/api/health");
        if !local.ip().is_loopback() {
            println!(
                "Warning: listening on a non-loopback address. The API has no authentication; \
                 anyone who can reach it can run simulations and read or delete stored runs."
            );
        }
        println!("{DISCLAIMER}");
        println!("Press Ctrl-C to stop.");
        server::serve(listener, app).await.context("serving HTTP")?;
        Ok::<(), anyhow::Error>(())
    })
}

fn cmd_demo(paths: &Paths, args: ServeArgs) -> Result<()> {
    if datasets::list_manifests(&paths.datasets)?.is_empty() {
        let manifest = write_synthetic(paths, &SyntheticConfig::default())?;
        println!(
            "No dataset found; generated the default synthetic dataset '{}' ({} rows, {:.0} s). \
             It is generated from a seed, not recorded market data.",
            manifest.id,
            manifest.counts.rows,
            seconds(manifest.end - manifest.start)
        );
    }
    cmd_serve(paths, args)
}
