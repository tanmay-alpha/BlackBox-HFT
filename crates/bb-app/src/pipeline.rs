//! The run pipeline: configuration in, complete forensics-ready [`RunResult`] out.
//!
//! `execute` is a pure function of the dataset and the configuration apart from the wall-clock
//! fields of the manifest, which are informational and excluded from both hashes.

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use bb_core::{
    CONTRACT_VERSION, ENGINE_NAME, ENGINE_VERSION,
    compare::{GridCell, GridResult, GridSpec},
    config::RunConfig,
    dataset::DatasetManifest,
    result::{DatasetRef, EngineInfo, RunManifest, RunResult, RunSummary},
};
use bb_sim::SimInput;

use crate::{
    api::OrderDetail,
    datasets::DatasetCtx,
    error::AppError,
    store::{RUN_ID_LEN, RunStore},
};

/// Build profile of this binary, recorded in run manifests as provenance.
const PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

/// The pinned engine as recorded in manifests and reported by the health endpoint. Only the
/// engine's `backtest` feature is compiled in; exchange connectivity is not.
pub fn engine_info() -> EngineInfo {
    EngineInfo {
        name: ENGINE_NAME.to_string(),
        version: ENGINE_VERSION.to_string(),
        features: vec!["backtest".to_string()],
    }
}

/// What identifies an experiment: the same dataset bytes, configuration, contract version and
/// engine version always give the same identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunIdentity {
    pub config_sha256: String,
    /// First 16 hex characters of `config_sha256`.
    pub run_id: String,
}

/// Computes the identity `config` has on the dataset described by `manifest`, without running it.
pub fn run_identity(manifest: &DatasetManifest, config: &RunConfig) -> RunIdentity {
    let config_sha256 = bb_core::hash::sha256_json(&(
        CONTRACT_VERSION,
        ENGINE_NAME,
        ENGINE_VERSION,
        &manifest.events_sha256,
        config,
    ));
    // A hex SHA-256 is 64 ASCII characters, so the prefix is always on a character boundary.
    let run_id = config_sha256[..RUN_ID_LEN].to_string();
    RunIdentity {
        config_sha256,
        run_id,
    }
}

/// Rejects a configuration with any validation problem, reporting all of them.
pub fn check_config(config: &RunConfig) -> Result<(), AppError> {
    let problems = config.validate();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(AppError::InvalidConfig(problems))
    }
}

/// Rejects a grid with any validation problem, reporting all of them.
pub fn check_grid(spec: &GridSpec) -> Result<(), AppError> {
    let problems = spec.validate();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(AppError::InvalidConfig(problems))
    }
}

fn check_dataset(manifest: &DatasetManifest, config: &RunConfig) -> Result<(), AppError> {
    if config.dataset_id == manifest.id {
        Ok(())
    } else {
        Err(AppError::InvalidConfig(vec![format!(
            "config.dataset_id is '{}' but the dataset supplied is '{}'",
            config.dataset_id, manifest.id
        )]))
    }
}

/// Simulates `config` on the dataset and assembles the complete result.
pub fn execute(ctx: &DatasetCtx, config: &RunConfig) -> Result<RunResult, AppError> {
    check_config(config)?;
    let dataset = ctx.manifest();
    check_dataset(dataset, config)?;

    // Wall-clock facts are informational only; neither hash covers them.
    let started = Instant::now();
    let created_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let instrument = dataset.instrument;
    let strategy = bb_strategies::build(&config.strategy, &instrument);
    let input = SimInput {
        events: ctx.dataset.events.as_slice(),
        instrument,
        origin_ns: ctx.dataset.origin_ns,
        config,
    };
    let out = bb_sim::simulate(&input, strategy)?;

    let tape = out.tape;
    let mut fills = out.fills;
    let equity = out.equity;
    let orders = bb_forensics::build_orders(&tape);
    bb_forensics::enrich_fills(&mut fills, &ctx.bbo);
    let metrics = bb_forensics::compute_metrics(&tape, &orders, &fills, &equity, &instrument);

    let identity = run_identity(dataset, config);
    let result_sha256 = bb_forensics::result_sha256(&tape, &fills, &equity);
    let manifest = RunManifest {
        contract_version: CONTRACT_VERSION,
        run_id: identity.run_id,
        blackbox_version: env!("CARGO_PKG_VERSION").to_string(),
        engine: engine_info(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        profile: PROFILE.to_string(),
        dataset: DatasetRef {
            id: dataset.id.clone(),
            sha256: dataset.events_sha256.clone(),
            rows: dataset.counts.rows,
            origin_ns: dataset.origin_ns.clone(),
        },
        config: config.clone(),
        config_sha256: identity.config_sha256,
        result_sha256,
        created_unix_ms,
        wall_time_ms: started.elapsed().as_millis() as u64,
    };

    Ok(RunResult {
        manifest,
        metrics,
        engine_check: out.engine_check,
        ticks: out.ticks,
        orders,
        fills,
        equity,
        tape,
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "no panic message".to_string()
    }
}

/// Returns the stored run for `config` if this exact experiment was already simulated, otherwise
/// simulates it and stores it under `label`.
///
/// A panic inside the simulation is reported as `simulation_failed` instead of unwinding into
/// the caller: one broken scenario must not take down a grid or the server's worker.
pub fn find_or_execute(
    ctx: &DatasetCtx,
    config: &RunConfig,
    store: &dyn RunStore,
    label: &str,
) -> Result<RunSummary, AppError> {
    check_config(config)?;
    check_dataset(ctx.manifest(), config)?;
    let identity = run_identity(ctx.manifest(), config);
    if let Some(existing) = store.summary(&identity.run_id)?
        // The id is a 64-bit prefix; only the full hash proves it is the same experiment.
        && existing.manifest.config_sha256 == identity.config_sha256
    {
        return Ok(existing);
    }
    // `execute` only reads shared state and owns everything it builds, so nothing observable is
    // left half-modified if it unwinds.
    let result = match catch_unwind(AssertUnwindSafe(|| execute(ctx, config))) {
        Ok(result) => result?,
        Err(payload) => {
            return Err(AppError::SimulationFailed(format!(
                "the simulation panicked: {}",
                panic_message(&*payload)
            )));
        }
    };
    store.put(&result, label)
}

fn run_cell(ctx: &DatasetCtx, spec: &GridSpec, store: &dyn RunStore, xi: usize, yi: usize) -> GridCell {
    let config = spec.y.apply(&spec.x.apply(&spec.base, xi), yi);
    let label = format!(
        "grid x={} y={}",
        spec.x.value_label(xi),
        spec.y.value_label(yi)
    );
    match find_or_execute(ctx, &config, store, &label) {
        Ok(summary) => GridCell {
            xi: xi as u32,
            yi: yi as u32,
            run_id: Some(summary.manifest.run_id),
            result_sha256: Some(summary.manifest.result_sha256),
            metrics: Some(summary.metrics),
            error: None,
        },
        // A failed cell stays visibly missing; the rest of the grid is still worth having.
        Err(err) => GridCell {
            xi: xi as u32,
            yi: yi as u32,
            run_id: None,
            result_sha256: None,
            metrics: None,
            error: Some(err.to_string()),
        },
    }
}

/// Runs every cell of a sensitivity grid as a real simulation and stores each as a normal run.
///
/// Cells are independent: each simulation is single-threaded and builds its own engine over the
/// shared, read-only dataset. `threads` workers pull cell indices from a shared counter, so the
/// result does not depend on scheduling.
pub fn run_grid(
    ctx: &DatasetCtx,
    spec: &GridSpec,
    store: &dyn RunStore,
    threads: usize,
) -> Result<GridResult, AppError> {
    check_grid(spec)?;
    check_dataset(ctx.manifest(), &spec.base)?;

    let started = Instant::now();
    let x_len = spec.x.len();
    let total = spec.cells();
    let workers = threads.clamp(1, total.max(1));
    let next = AtomicUsize::new(0);

    let finished: Vec<Vec<(usize, GridCell)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= total {
                            break;
                        }
                        // Row-major: index = yi * x_len + xi.
                        let cell = run_cell(ctx, spec, store, index % x_len, index / x_len);
                        done.push((index, cell));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().map_err(|payload| {
                    AppError::Internal(format!(
                        "a grid worker panicked: {}",
                        panic_message(&*payload)
                    ))
                })
            })
            .collect::<Result<Vec<_>, AppError>>()
    })?;

    let mut slots: Vec<Option<GridCell>> = vec![None; total];
    for (index, cell) in finished.into_iter().flatten() {
        slots[index] = Some(cell);
    }
    let cells = slots
        .into_iter()
        .enumerate()
        .map(|(index, cell)| {
            cell.ok_or_else(|| AppError::Internal(format!("grid cell {index} was never run")))
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(GridResult {
        spec: spec.clone(),
        x_labels: (0..x_len).map(|i| spec.x.value_label(i)).collect(),
        y_labels: (0..spec.y.len()).map(|i| spec.y.value_label(i)).collect(),
        cells,
        wall_time_ms: started.elapsed().as_millis() as u64,
    })
}

/// Everything the forensics view shows for one order: its lifecycle record, its tape events, its
/// fills and the rule-based explanation. `None` when the run has no such order.
///
/// `dataset` must be the manifest of the dataset the run was simulated on; it supplies the
/// instrument and the provenance of the feed's local timestamps, which decides whether feed
/// latency may be described as measured.
pub fn order_detail(run: &RunResult, dataset: &DatasetManifest, order_id: u64) -> Option<OrderDetail> {
    let order = run.orders.iter().find(|o| o.order_id == order_id)?.clone();
    let events: Vec<_> = run
        .tape
        .iter()
        .filter(|e| e.order_id == Some(order_id))
        .cloned()
        .collect();
    let fills = run
        .fills
        .iter()
        .filter(|f| f.order_id == order_id)
        .cloned()
        .collect();
    let explanation = bb_forensics::explain_order(
        &order,
        &events,
        &dataset.instrument,
        &run.manifest.config,
        dataset.timestamps.local_ts,
    );
    Some(OrderDetail {
        order,
        events,
        fills,
        explanation,
        instrument: dataset.instrument,
    })
}
