//! Standalone research report rendering; no simulated result is presented as a live execution.

use bb_core::{
    config::{ExchangeModel, OrderLatencyConfig},
    dataset::{DatasetManifest, LocalTsProvenance},
    result::RunResult,
};

use crate::{AppError, DISCLAIMER, demos};

fn json(value: &impl serde::Serialize) -> Result<String, AppError> {
    Ok(serde_json::to_string_pretty(value)?)
}

pub(crate) fn markdown(run: &RunResult, dataset: &DatasetManifest) -> Result<String, AppError> {
    let m = &run.manifest;
    // Never attach provenance from replacement data that happens to reuse the same id.
    if dataset.events_sha256 != m.dataset.sha256 {
        return Err(AppError::InvalidRequest(
            "dataset bytes no longer match the stored run's dataset hash".into(),
        ));
    }
    let c = &m.config;
    let exchange = match c.execution.exchange_model {
        ExchangeModel::NoPartialFill => {
            "Whole-order fill model: a resting order fills completely once its modeled fill condition is met, irrespective of traded size."
        }
        ExchangeModel::PartialFill => {
            "Partial-fill model: resting fills depend on traded quantity beyond the modeled queue ahead; taking fills use displayed levels without changing replayed depth."
        }
    };
    let latency = match c.execution.order_latency {
        OrderLatencyConfig::Constant {
            entry_ns,
            response_ns,
        } => format!(
            "Constant modeled order latency: entry {entry_ns} ns; response {response_ns} ns."
        ),
        OrderLatencyConfig::Jitter {
            entry_ns,
            response_ns,
            jitter_ns,
        } => format!(
            "BlackBox seeded jitter model: base entry {entry_ns} ns and response {response_ns} ns, each plus a draw from 0 through {jitter_ns} ns. Messages never overtake."
        ),
    };
    let local = match dataset.timestamps.local_ts {
        LocalTsProvenance::Measured => {
            "Receipt timestamps were measured at the vendor's capture host, not the strategy user's location."
        }
        LocalTsProvenance::Synthetic => {
            "Receipt timestamps were generated synthetically; feed latency is not a market measurement."
        }
        LocalTsProvenance::Assumed => {
            "Receipt timestamps were assumed; feed latency is not measured."
        }
    };
    let worst = dataset
        .quality
        .worst()
        .map(|s| serde_json::to_string(&s))
        .transpose()?
        .unwrap_or_else(|| "\"ok\"".into());
    let filename = format!("blackbox-run-{}-config.json", m.run_id);
    Ok(format!(
        "# BlackBox research report: run {id}\n\n{disclaimer}\n\n\
         ## Identity and engine\n\n\
         - Run id: `{id}`\n\
         - Config SHA-256: `{config_hash}`\n\
         - Result SHA-256 (tape, fills, equity): `{result_hash}`\n\
         - Dataset events SHA-256: `{dataset_hash}`\n\
         - Engine: {engine} {version}; features: {features}\n\
         - BlackBox: {blackbox}; contract {contract}; platform {platform}; profile {profile}\n\n\
         ## Dataset provenance\n\n\
         Dataset id: `{dataset_id}`. Kind: {kind}. Worst quality severity: {worst}.\n\n\
         {local}\n\n\
         The following dataset manifest records source input hashes, timestamp provenance and shift, instrument units, and all quality findings. It describes local files; vendor data must not be redistributed.\n\n\
         ```json\n{dataset}\n```\n\n\
         ## Full configuration\n\n```json\n{config}\n```\n\n\
         ## Assumptions in this run\n\n\
         - Historical replay only: simulated orders never move market prices or consume replayed depth.\n\
         - {exchange}\n\
         - Queue model: {queue}. Queue position is modeled, never observed in L2. New resting orders join behind displayed size. Trades reduce the estimate; cancellation allocation follows this selected model. Each own order sees the full trade independently.\n\
         - {latency}\n\
         - Dataset receipt latency is adjusted by {feed_offset} ns; its source is stated above.\n\
         - Strategy: {strategy}, evaluated every {interval} ns on a fixed local-clock grid. Its parameters are preserved in the full configuration. Cancel-and-replace loses queue position; no order modification is modeled.\n\
         - Seed: {seed}; it controls jitter only.\n\
         - Pre-trade limits: absolute position {max_position}, order quantity {max_order}, working orders {max_open}. Drawdown halt: {drawdown} (disabled when null), checked on the strategy grid.\n\
         - Maker fee fraction {maker}; taker fee fraction {taker}; negative fees are rebates. Fees apply to traded value.\n\
         - Linear-contract average-cost accounting, marked at locally known mid; funding, borrowing, margin and liquidation are not modeled.\n\
         - Recorder: equity every {sample_every} strategy ticks; queue-update soft cap {queue_cap} per order. Trade updates are retained, suppressed small depth updates are explicitly marked on tape.\n\
         - Run ends after the last locally received feed event; in-flight requests can remain unresolved.\n\n\
         ## All simulated metrics\n\n```json\n{metrics}\n```\n\n\
         ## Engine cross-check\n\n\
         BlackBox reports its fill-hook ledger, not the engine's local account. Under partial fills the engine's local account is known to under-count; the recorded check and note below are informational.\n\n\
         ```json\n{check}\n```\n\n\
         ## Reproduce\n\n\
         With the same local dataset bytes, BlackBox/engine version and platform, run this POSIX-shell recipe (or save the full configuration JSON above to `{filename}` and invoke the final command). For a nondefault data directory, add `--data-dir <directory>` to `blackbox`.\n\n\
         ```sh\n{reproduce}\n```\n\n\
         ## Standard limitations\n\n{limitations}\n",
        id = m.run_id,
        disclaimer = DISCLAIMER,
        config_hash = m.config_sha256,
        result_hash = m.result_sha256,
        dataset_hash = m.dataset.sha256,
        engine = m.engine.name,
        version = m.engine.version,
        features = m.engine.features.join(", "),
        blackbox = m.blackbox_version,
        contract = m.contract_version,
        platform = m.platform,
        profile = m.profile,
        dataset_id = dataset.id,
        kind = serde_json::to_string(&dataset.kind)?,
        dataset = json(dataset)?,
        config = json(c)?,
        queue = c.execution.queue_model.label(),
        feed_offset = c.execution.feed_latency_offset_ns,
        strategy = c.strategy.kind.label(),
        interval = c.strategy.interval_ns,
        seed = c.seed,
        max_position = c.risk.max_position,
        max_order = c.risk.max_order_qty,
        max_open = c.risk.max_open_orders,
        drawdown = serde_json::to_string(&c.risk.max_drawdown)?,
        maker = c.execution.fees.maker,
        taker = c.execution.fees.taker,
        sample_every = c.record.sample_every_ticks,
        queue_cap = c.record.max_queue_updates_per_order,
        metrics = json(&run.metrics)?,
        check = json(&run.engine_check)?,
        reproduce = demos::reproduce(c, &filename),
        limitations = include_str!("../../../docs/LIMITATIONS.md"),
    ))
}
