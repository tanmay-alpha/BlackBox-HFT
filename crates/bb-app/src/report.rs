//! Standalone research report rendering; no simulated result is presented as a live execution.
//!
//! Every generated report (run, comparison, sweep) follows the evidence structure documented in
//! `docs/research/REPORT_FORMAT.md`: prose uses readable names, and each readable name is
//! followed by its exact machine value in backticks; JSON blocks keep every raw value.

use std::fmt::Write as _;

use bb_core::{
    Basis,
    compare::{Comparison, DeltaStats, EligibilityEffect, PairClass, duration_label},
    config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig, RunConfig, StrategyKind},
    dataset::{DatasetKind, DatasetManifest, LocalTsProvenance, Quantiles, Severity},
    result::{EngineStateCheck, Metrics, RunManifest, RunResult},
};
use serde::Serialize;
use serde_json::Value;

use crate::{AppError, demos};

fn json(value: &impl Serialize) -> Result<String, AppError> {
    Ok(serde_json::to_string_pretty(value)?)
}

/// The serde name of a unit enum value, or the `type` tag of an internally tagged one.
pub(crate) fn serde_code(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Object(o)) => o
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// `value` rounded to six decimals without trailing zeros (exact values stay in the JSON blocks).
pub(crate) fn num(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let s = format!("{value:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// A fee fraction in basis points: `0.0002` -> `2 bp`.
pub(crate) fn fee_bp(fraction: f64) -> String {
    format!("{} bp", num(fraction * 1e4))
}

/// `text` as an inline code span, widening the fence when the text itself holds a backtick.
fn code(text: &str) -> String {
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

/// Stored free text (manifest notes, quality details) quoted verbatim as an indented block
/// quote inside a list item: it is source data, not report prose, so it keeps its own wording.
fn quote(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let _ = writeln!(out, "  > {line}");
    }
    if out.is_empty() {
        out.push_str("  > (empty)\n");
    }
    out
}

/// A nanosecond quantity read as a duration (`1.5 ms`).
fn duration_f(ns: f64) -> String {
    duration_label(ns.round() as i64)
}

/// A readable duration followed by its exact nanoseconds: ``100 ms (`interval_ns` = `100000000`)``.
fn duration_with(ns: i64, field: &str) -> String {
    format!("{} (`{field}` = `{ns}`)", duration_label(ns))
}

pub(crate) fn strategy_name(kind: &StrategyKind) -> &'static str {
    match kind {
        StrategyKind::TouchMaker(_) => "Touch maker",
        StrategyKind::AvellanedaStoikov(_) => "Avellaneda–Stoikov market maker",
        StrategyKind::Glft(_) => "GLFT market maker",
        StrategyKind::ImbalanceTaker(_) => "Order-book imbalance taker",
        StrategyKind::Scripted(_) => "Scripted orders",
    }
}

pub(crate) fn queue_model_name(q: &QueueModelConfig) -> String {
    match q {
        QueueModelConfig::RiskAdverse => {
            "Risk-averse queue (advances only on trades at the order's price)".into()
        }
        QueueModelConfig::LogProb => "Probabilistic queue, logarithmic weights, form 1".into(),
        QueueModelConfig::LogProb2 => "Probabilistic queue, logarithmic weights, form 2".into(),
        QueueModelConfig::PowerProb { n } => {
            format!("Probabilistic queue, power weights with exponent {n}, form 1")
        }
        QueueModelConfig::PowerProb2 { n } => {
            format!("Probabilistic queue, power weights with exponent {n}, form 2")
        }
        QueueModelConfig::PowerProb3 { n } => {
            format!("Probabilistic queue, power weights with exponent {n}, form 3")
        }
    }
}

fn exchange_model_name(m: ExchangeModel) -> &'static str {
    match m {
        ExchangeModel::NoPartialFill => "Whole-order fills",
        ExchangeModel::PartialFill => "Partial fills",
    }
}

fn latency_model_name(l: &OrderLatencyConfig) -> &'static str {
    match l {
        OrderLatencyConfig::Constant { .. } => "Constant latency",
        OrderLatencyConfig::Jitter { .. } => "Seeded uniform jitter",
        OrderLatencyConfig::LogNormal { .. } => "Seeded log-normal latency",
        OrderLatencyConfig::HeavyTail { .. } => "Seeded heavy-tailed latency (ABIDES cubic)",
    }
}

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Info => "information",
        Severity::Warn => "warning",
        Severity::Error => "error",
    }
}

/// What the replayed events are, for prose: never "historical" for generated data.
pub(crate) fn dataset_kind_name(kind: DatasetKind) -> &'static str {
    match kind {
        DatasetKind::Synthetic => "synthetic session generated by BlackBox",
        DatasetKind::Recorded => "recorded market data",
    }
}

/// [`dataset_kind_name`] as a noun phrase: "a synthetic session …" / "recorded market data".
pub(crate) fn dataset_kind_phrase(kind: DatasetKind) -> &'static str {
    match kind {
        DatasetKind::Synthetic => "a synthetic session generated by BlackBox",
        DatasetKind::Recorded => "recorded market data",
    }
}

/// The no-market-impact statement for a replay of this kind of data.
pub(crate) fn replay_sentence(kind: DatasetKind) -> &'static str {
    match kind {
        DatasetKind::Synthetic => {
            "Replay of a synthetic session generated by BlackBox, not recorded exchange data: simulated orders never move the generated prices or consume replayed depth."
        }
        DatasetKind::Recorded => {
            "Replay of recorded market data: simulated orders never move the recorded prices or consume replayed depth."
        }
    }
}

fn disclaimer(kind: DatasetKind) -> String {
    format!(
        "Research only. Every order, fill, queue position and P&L figure is the output of a \
         simulation over {} under stated latency, queue and fee assumptions. Simulated orders \
         never move replayed prices or consume displayed depth (no market impact). Nothing here \
         is a real exchange execution.",
        dataset_kind_phrase(kind)
    )
}

/// Readable name of a sweep objective or ensemble metric key (`ObjectiveMetric::key`).
pub(crate) fn metric_name(key: &str) -> &str {
    match key {
        "net_pnl" => "Net P&L",
        "realized_pnl" => "Realized P&L",
        "fees" => "Fees",
        "spread_capture" => "Spread capture",
        "pnl_per_turnover_bps" => "Net P&L per unit of turnover",
        "max_drawdown" => "Maximum drawdown",
        "fill_count" => "Fill count",
        "fill_ratio_orders" => "Order fill ratio",
        "mean_abs_position" => "Mean absolute position",
        other => other,
    }
}

/// Readable unit of `ObjectiveMetric::unit`.
pub(crate) fn unit_name(unit: &str) -> &str {
    match unit {
        "quote" => "quote currency",
        "bps" => "basis points",
        "count" => "count",
        "ratio" => "ratio",
        "qty" => "base units",
        "ns" => "time",
        other => other,
    }
}

/// Readable name of a swept parameter (`GridAxis::name`, `RandomParam::name`, `seed`).
pub(crate) fn param_name(key: &str) -> &str {
    match key {
        "order_latency_ns" => "Order latency (entry and response)",
        "entry_latency_ns" => "Entry latency",
        "feed_latency_offset_ns" => "Feed latency offset",
        "queue_model" => "Queue model",
        "maker_fee" => "Maker fee",
        "taker_fee" => "Taker fee",
        "seed" => "Seed",
        other => other,
    }
}

fn quantiles_line(q: &Quantiles) -> String {
    format!(
        "median {}, p90 {}, p99 {}, max {} over {} values",
        duration_f(q.p50),
        duration_f(q.p90),
        duration_f(q.p99),
        duration_f(q.max),
        q.n
    )
}

/// The run's Markdown research report. The dataset manifest must be the one the run replayed.
pub(crate) fn markdown(run: &RunResult, dataset: &DatasetManifest) -> Result<String, AppError> {
    markdown_metadata(
        &run.manifest,
        &run.metrics,
        &run.engine_check,
        &bb_artifacts::BundleCounts {
            ticks: run.ticks,
            orders: run.orders.len() as u64,
            fills: run.fills.len() as u64,
            equity: run.equity.len() as u64,
            tape: run.tape.len() as u64,
        },
        dataset,
    )
}

/// Renders the same report from verified trailer metadata; no section rows are needed.
pub(crate) fn markdown_metadata(
    m: &RunManifest,
    met: &Metrics,
    check: &EngineStateCheck,
    counts: &bb_artifacts::BundleCounts,
    dataset: &DatasetManifest,
) -> Result<String, AppError> {
    // Never attach provenance from replacement data that happens to reuse the same id.
    if dataset.events_sha256 != m.dataset.sha256 {
        return Err(AppError::InvalidRequest(
            "dataset bytes no longer match the stored run's dataset hash".into(),
        ));
    }
    let c = &m.config;
    let x = &c.execution;
    let id = &m.run_id;
    let strategy = format!(
        "{} (`{}`)",
        strategy_name(&c.strategy.kind),
        c.strategy.kind.label()
    );
    let queue = format!(
        "{} (`{}`)",
        queue_model_name(&x.queue_model),
        x.queue_model.label()
    );
    let exchange_code = serde_code(&x.exchange_model);
    let latency_code = serde_code(&x.order_latency);
    let local_code = serde_code(&dataset.timestamps.local_ts);
    let level_code = serde_code(&dataset.level);
    let kind_code = serde_code(&dataset.kind);
    let filename = format!("blackbox-run-{id}-config.json");
    let mut md = String::new();
    let w = &mut md;

    let _ = write!(
        w,
        "# BlackBox research report: run {id}\n\n{}\n\n",
        disclaimer(dataset.kind)
    );

    // ----- Research question and hypothesis -----
    let _ = write!(
        w,
        "## Research question\n\n\
         What do the simulated orders of the {strategy} strategy produce when it runs over \
         dataset {title:?} (`{did}`), {kind}, under the execution model stated below?\n\n\
         ## Hypothesis\n\n\
         Descriptive run, no hypothesis: one configuration was simulated and nothing is tested \
         against an alternative.\n\n",
        title = dataset.title,
        did = dataset.id,
        kind = dataset_kind_phrase(dataset.kind),
    );

    // ----- Data source -----
    let _ = write!(
        w,
        "## Data source\n\n\
         - {replay}\n\
         - Kind: {kind} (`{kind_code}`).\n\
         - Vendor or generator `{vendor}`; exchange `{exchange}`; symbol `{symbol}`.\n\
         - Data level: market-by-price depth with trades (`{level_code}`); individual orders are \
         not visible.\n\
         - Exchange timestamp meaning: {semantics}\n\
         - Licence and redistribution: {license}\n\n",
        replay = replay_sentence(dataset.kind),
        kind = dataset_kind_name(dataset.kind),
        vendor = dataset.source.vendor,
        exchange = dataset.source.exchange,
        symbol = dataset.source.symbol,
        semantics = dataset.source.exch_ts_semantics,
        license = dataset.source.license_note,
    );

    // ----- Dataset provenance -----
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
    let _ = write!(
        w,
        "## Dataset provenance\n\n\
         - Dataset id `{did}`, title {title:?}.\n\
         - Dataset events SHA-256 (`events_sha256`): `{sha}`; {bytes} bytes in `{file}`.\n\
         - {rows} engine rows (`counts.rows`): {trades} trades, {depth} depth updates, \
         {snapshots} snapshots of {levels} levels, {clears} depth clears.\n\
         - Exchange-clock window: {span} from the dataset origin (epoch nanoseconds `{origin}`).\n\
         - Receipt-timestamp provenance (`timestamps.local_ts` = `{local_code}`): {local} Local timestamps were shifted by {shift}. Timestamp note, quoted from the manifest:\n\n{note}\n",
        did = dataset.id,
        title = dataset.title,
        sha = dataset.events_sha256,
        bytes = dataset.events_bytes,
        file = dataset.events_file,
        rows = dataset.counts.rows,
        trades = dataset.counts.trades,
        depth = dataset.counts.depth_updates,
        snapshots = dataset.counts.snapshots,
        levels = dataset.counts.snapshot_levels,
        clears = dataset.counts.depth_clears,
        span = duration_label(dataset.end.saturating_sub(dataset.start)),
        origin = dataset.origin_ns,
        shift = duration_with(
            dataset.timestamps.local_ts_shift_ns,
            "timestamps.local_ts_shift_ns"
        ),
        note = quote(&dataset.timestamps.note),
    );
    if dataset.generator.is_some() {
        w.push_str(
            "- The generator configuration that produced the session is the manifest's \
             `generator` field below; the session can be regenerated from it.\n",
        );
    }
    if !dataset.source.inputs.is_empty() {
        w.push_str("\n| Source input | Role | Bytes | Rows | SHA-256 |\n|---|---|---:|---:|---|\n");
        for i in &dataset.source.inputs {
            let _ = writeln!(
                w,
                "| {} | {} | {} | {} | `{}` |",
                code(&i.name),
                code(&i.role),
                i.bytes,
                i.rows,
                i.sha256
            );
        }
    }
    let _ = write!(
        w,
        "\nThe dataset manifest below records source input hashes, timestamp provenance and \
         shift, instrument units, and all quality findings. It describes local files{}.\n\n\
         ```json\n{}\n```\n\n",
        match dataset.kind {
            DatasetKind::Recorded => "; vendor data must not be redistributed",
            DatasetKind::Synthetic => "",
        },
        json(dataset)?
    );

    // ----- Dataset limitations -----
    let q = &dataset.quality;
    let worst = q.worst();
    let _ = write!(
        w,
        "## Dataset limitations\n\n- Worst quality severity: {} (`{}`).\n",
        worst.map_or("no failed check", severity_name),
        worst.map_or_else(|| "ok".to_string(), |s| serde_code(&s)),
    );
    for check in q.checks.iter().filter(|c| !c.passed) {
        let _ = writeln!(
            w,
            "- {} (`{}`): {} (`{}`), {} occurrences. Detail, quoted from the quality report:\n\n{}",
            check.title,
            check.id,
            severity_name(check.severity),
            serde_code(&check.severity),
            check.count,
            quote(&check.detail)
        );
    }
    let _ = writeln!(
        w,
        "- {} of the exchange-clock span had a two-sided, uncrossed book (`quality.book_valid_fraction` = `{}`).",
        format_args!("{}%", num(q.book_valid_fraction * 100.0)),
        q.book_valid_fraction
    );
    if let Some(f) = &q.feed_latency_ns {
        let _ = writeln!(
            w,
            "- Dataset feed latency (`quality.feed_latency_ns`): {}; {}",
            quantiles_line(f),
            match dataset.timestamps.local_ts {
                LocalTsProvenance::Measured => "measured at the vendor's capture host.",
                LocalTsProvenance::Synthetic => "generated, not measured.",
                LocalTsProvenance::Assumed => "assumed, not measured.",
            }
        );
    }
    w.push_str(
        "- Market-by-price data: queue position is never observed; it is estimated by the \
         queue model below.\n",
    );
    w.push_str(match dataset.kind {
        DatasetKind::Synthetic => {
            "- The session was generated by BlackBox for demonstrations and tests; it says nothing about any real market.\n\n"
        }
        DatasetKind::Recorded => {
            "- Recorded data is replayed as captured: the pipeline reports duplicates, gaps and crossed books but does not repair them.\n\n"
        }
    });

    // ----- Model assumptions -----
    let exchange = match x.exchange_model {
        ExchangeModel::NoPartialFill => {
            "a resting order fills completely once its modeled fill condition is met, irrespective of traded size."
        }
        ExchangeModel::PartialFill => {
            "resting fills depend on traded quantity beyond the modeled queue ahead; taking fills use displayed levels without changing replayed depth."
        }
    };
    let mut latency = x.order_latency.describe();
    if x.think_time_ns > 0 {
        let _ = write!(
            latency,
            " Strategy think time {}: each request leaves {} after the decision that made it.",
            duration_with(x.think_time_ns, "execution.think_time_ns"),
            duration_label(x.think_time_ns)
        );
    }
    let _ = write!(
        w,
        "## Model assumptions\n\n\
         - {replay}\n\
         - Fill model: {exchange_name} (`{exchange_code}`): {exchange}\n\
         - Queue model: {queue}. Queue position is modeled, never observed in L2. New resting orders join behind displayed size. Trades reduce the estimate; cancellation allocation follows this selected model. Each own order sees the full trade independently.\n\
         - Order latency: {latency_name} (`{latency_code}`). {latency}\n\
         - Dataset receipt latency is adjusted by {feed_offset}; its source is stated above.\n\
         - Strategy: {strategy}, evaluated every {interval} on a fixed local-clock grid. Its parameters are preserved in the full configuration. Cancel-and-replace loses queue position; no order modification is modeled.\n\
         - Seed `{seed}`; it controls latency draws only.\n\
         - Pre-trade limits: absolute position {max_position}, order quantity {max_order}, working orders {max_open}. Drawdown halt: {drawdown}, checked on the strategy grid.\n\
         - Maker fee {maker} (fraction `{maker_raw}`); taker fee {taker} (fraction `{taker_raw}`); negative fees are rebates. Fees apply to traded value.\n\
         - Linear-contract average-cost accounting, marked at the locally known mid; funding, borrowing, margin and liquidation are not modeled.\n\
         - Recorder: equity every {sample_every} strategy ticks (`record.sample_every_ticks`); queue-update soft cap {queue_cap} per order (`record.max_queue_updates_per_order`). Trade updates are retained; suppressed small depth updates are explicitly marked on the tape.\n\
         - The run ends after the last locally received feed event; in-flight requests can remain unresolved.\n\n",
        replay = replay_sentence(dataset.kind),
        exchange_name = exchange_model_name(x.exchange_model),
        latency_name = latency_model_name(&x.order_latency),
        feed_offset = duration_with(x.feed_latency_offset_ns, "execution.feed_latency_offset_ns"),
        interval = duration_with(c.strategy.interval_ns, "strategy.interval_ns"),
        seed = c.seed,
        max_position = num(c.risk.max_position),
        max_order = num(c.risk.max_order_qty),
        max_open = c.risk.max_open_orders,
        drawdown = c.risk.max_drawdown.map_or_else(
            || "disabled (`risk.max_drawdown` = `null`)".to_string(),
            |d| format!("{} quote currency (`risk.max_drawdown` = `{d}`)", num(d))
        ),
        maker = fee_bp(x.fees.maker),
        maker_raw = x.fees.maker,
        taker = fee_bp(x.fees.taker),
        taker_raw = x.fees.taker,
        sample_every = c.record.sample_every_ticks,
        queue_cap = c.record.max_queue_updates_per_order,
    );

    // ----- Experiment configuration -----
    let _ = write!(
        w,
        "## Experiment configuration\n\n\
         | Setting | Value | Configuration field | Exact value |\n|---|---|---|---|\n\
         | Dataset | {title:?} | `dataset_id` | `{did}` |\n\
         | Strategy | {sname} | `strategy.kind.type` | `{scode}` |\n\
         | Decision interval | {interval} | `strategy.interval_ns` | `{interval_ns}` |\n\
         | Fill model | {exchange_name} | `execution.exchange_model` | `{exchange_code}` |\n\
         | Queue model | {qname} | `execution.queue_model` | `{qcode}` |\n\
         | Order latency | {latency_name} | `execution.order_latency.type` | `{latency_code}` |\n\
         | Feed latency offset | {feed} | `execution.feed_latency_offset_ns` | `{feed_ns}` |\n\
         | Think time | {think} | `execution.think_time_ns` | `{think_ns}` |\n\
         | Maker fee | {maker} | `execution.fees.maker` | `{maker_raw}` |\n\
         | Taker fee | {taker} | `execution.fees.taker` | `{taker_raw}` |\n\
         | Seed | {seed} | `seed` | `{seed}` |\n\n\
         ### Full configuration\n\n```json\n{config}\n```\n\n",
        title = dataset.title,
        did = c.dataset_id,
        sname = strategy_name(&c.strategy.kind),
        scode = c.strategy.kind.label(),
        interval = duration_label(c.strategy.interval_ns),
        interval_ns = c.strategy.interval_ns,
        exchange_name = exchange_model_name(x.exchange_model),
        qname = queue_model_name(&x.queue_model),
        qcode = x.queue_model.label(),
        latency_name = latency_model_name(&x.order_latency),
        feed = duration_label(x.feed_latency_offset_ns),
        feed_ns = x.feed_latency_offset_ns,
        think = duration_label(x.think_time_ns),
        think_ns = x.think_time_ns,
        maker = fee_bp(x.fees.maker),
        maker_raw = x.fees.maker,
        taker = fee_bp(x.fees.taker),
        taker_raw = x.fees.taker,
        seed = c.seed,
        config = json(c)?,
    );

    // ----- Methods -----
    let _ = write!(
        w,
        "## Methods\n\n\
         - The matching engine `{engine}` {version} (features: {features}) processed the \
         dataset's {rows} engine rows (`dataset.rows`) through the fill, queue and latency \
         models above.\n\
         - The strategy was evaluated {ticks} times (`ticks`), every {interval}.\n\
         - BlackBox rebuilt {orders} order lifecycles from the flight tape ({tape} tape events), \
         {fills} fills and {equity} equity samples; every metric below is computed from those \
         records by BlackBox's own fill ledger.\n\
         - The engine's account is compared with that ledger as a cross-check (below); it is not \
         the source of any reported figure.\n\
         - The result SHA-256 covers the tape, fills and equity samples, so equal hashes mean \
         equal records.\n\n",
        engine = m.engine.name,
        version = m.engine.version,
        features = m.engine.features.join(", "),
        rows = m.dataset.rows,
        ticks = counts.ticks,
        interval = duration_label(c.strategy.interval_ns),
        orders = counts.orders,
        tape = counts.tape,
        fills = counts.fills,
        equity = counts.equity,
    );

    // ----- Results -----
    let opt = |v: Option<f64>| v.map_or_else(|| "not defined".to_string(), num);
    let _ = write!(
        w,
        "## Results\n\n\
         Every figure is modeled simulation output. Quote-currency values are rounded to six \
         decimals here; the JSON block keeps them exact.\n\n\
         - Orders: {intended} intended (`orders_intended`), {blocked} blocked by pre-trade \
         limits (`orders_risk_blocked`), {refused} refused (`orders_refused`), {submitted} \
         submitted (`orders_submitted`), {filled} filled (`orders_filled`), {partial} partially \
         filled (`orders_partially_filled`), {canceled} canceled (`orders_canceled`), {working} \
         working at the end (`orders_working_at_end`).\n\
         - Fills: {fill_count} (`fill_count`), {maker} maker (`maker_fill_count`) and {taker} \
         taker (`taker_fill_count`); order fill ratio {ratio} (`fill_ratio_orders`), quantity \
         fill ratio {qty_ratio} (`fill_ratio_qty`).\n\
         - Volume {volume} (`volume`); turnover {turnover} quote currency (`turnover`); fees \
         {fees} quote currency (`fees`).\n\
         - Net P&L {net} quote currency (`net_pnl`) = realized {realized} (`realized_pnl`) + \
         unrealized {unrealized} (`unrealized_pnl`) − fees.\n\
         - Maximum drawdown of sampled equity {dd} quote currency (`max_drawdown`); risk halt: \
         {halted} (`risk_halted` = `{halted_raw}`); {risk_blocks} pre-trade blocks \
         (`risk_blocks`).\n\
         - Inventory: final position {final_pos} (`final_position`), largest long {long} \
         (`max_long`), largest short {short} (`max_short`), mean absolute position {mean_abs} \
         (`mean_abs_position`).\n\
         - Spread capture {capture} quote currency (`spread_capture`), {capture_ticks} ticks per \
         unit (`spread_capture_ticks`).\n",
        intended = met.orders_intended,
        blocked = met.orders_risk_blocked,
        refused = met.orders_refused,
        submitted = met.orders_submitted,
        filled = met.orders_filled,
        partial = met.orders_partially_filled,
        canceled = met.orders_canceled,
        working = met.orders_working_at_end,
        fill_count = met.fill_count,
        maker = met.maker_fill_count,
        taker = met.taker_fill_count,
        ratio = opt(met.fill_ratio_orders),
        qty_ratio = opt(met.fill_ratio_qty),
        volume = num(met.volume),
        turnover = num(met.turnover),
        fees = num(met.fees),
        net = num(met.net_pnl),
        realized = num(met.realized_pnl),
        unrealized = num(met.unrealized_pnl),
        dd = num(met.max_drawdown),
        halted = if met.risk_halted { "yes" } else { "no" },
        halted_raw = met.risk_halted,
        risk_blocks = met.risk_blocks,
        final_pos = num(met.final_position),
        long = num(met.max_long),
        short = num(met.max_short),
        mean_abs = num(met.mean_abs_position),
        capture = num(met.spread_capture),
        capture_ticks = opt(met.spread_capture_ticks),
    );
    if !met.markouts.is_empty() {
        let parts = met
            .markouts
            .iter()
            .map(|k| match k.mean_ticks {
                Some(v) => format!(
                    "after {}: {} ticks over {} fills",
                    duration_label(k.horizon_ns),
                    num(v),
                    k.fills
                ),
                None => format!(
                    "after {}: no fill could be measured",
                    duration_label(k.horizon_ns)
                ),
            })
            .collect::<Vec<_>>()
            .join("; ");
        let _ = writeln!(
            w,
            "- Quantity-weighted markouts (`markouts`; negative means adverse selection): {parts}."
        );
    }
    for (label, field, q) in [
        (
            "Round trip, request to first response",
            "round_trip_ns",
            &met.round_trip_ns,
        ),
        (
            "Time to first fill",
            "time_to_first_fill_ns",
            &met.time_to_first_fill_ns,
        ),
    ] {
        if let Some(q) = q {
            let _ = writeln!(w, "- {label} (`{field}`): {}.", quantiles_line(q));
        }
    }
    let _ = write!(
        w,
        "\n### All simulated metrics\n\n```json\n{metrics}\n```\n\n\
         ### Engine cross-check\n\n\
         The engine's account {agrees} BlackBox's fill ledger (`engine_check.agrees` = \
         `{agrees_raw}`): ledger position {lpos} vs engine {epos}, ledger fills {lfills} vs \
         engine trades {etrades}. BlackBox reports its fill-hook ledger, not the engine's local \
         account. Under partial fills the engine's local account is known to under-count; the \
         recorded check and note below are informational.\n\n\
         ```json\n{check_json}\n```\n\n",
        metrics = json(met)?,
        agrees = if check.agrees {
            "agrees with"
        } else {
            "does not agree with"
        },
        agrees_raw = check.agrees,
        lpos = num(check.ledger_position),
        epos = num(check.engine_position),
        lfills = check.ledger_fills,
        etrades = check.engine_num_trades,
        check_json = json(check)?,
    );

    // ----- Visual evidence -----
    let _ = write!(
        w,
        "## Visual evidence\n\n\
         Paths are relative to the BlackBox console.\n\n\
         - Run overview with orders and fills: `/forensics/{id}`\n\
         - Replay terminal (book, tape and own orders): `/book/runs/{id}`\n\
         - Risk view (position, equity, drawdown): `/risk/{id}`\n\
         - This report in the console: `/reports/{id}`\n\
         - The replayed dataset: `/data/{did}`\n\n",
        did = dataset.id,
    );

    // ----- Sensitivity -----
    let _ = write!(
        w,
        "## Sensitivity\n\n\
         Not assessed: this report covers one configuration. A sweep started from this run \
         (`/sim/sweep?base={id}`) measures how the results above move with an assumption.\n\n"
    );

    // ----- Statistical uncertainty -----
    let _ = write!(
        w,
        "## Statistical uncertainty\n\n\
         - One deterministic simulation: no confidence interval or standard error exists for \
         any figure above.\n\
         - {seed_line}\n\
         - Quantiles above describe this run's own orders and fills, not sampling uncertainty.\n\n",
        seed_line = if x.order_latency.uses_seed() {
            format!(
                "The latency model ({}, `{latency_code}`) draws from seed `{}`; another seed can \
                 give different results. A seed-ensemble sweep measures that spread.",
                latency_model_name(&x.order_latency),
                c.seed
            )
        } else {
            format!(
                "The latency model ({}, `{latency_code}`) draws nothing from seed `{}`; every \
                 seed gives the same result.",
                latency_model_name(&x.order_latency),
                c.seed
            )
        },
    );

    // ----- Reproduction -----
    let _ = write!(
        w,
        "## Reproduction\n\n\
         - Run id: `{id}`\n\
         - Config SHA-256: `{config_hash}`\n\
         - Result SHA-256 (tape, fills, equity): `{result_hash}`\n\
         - Dataset events SHA-256: `{dataset_hash}`\n\
         - Engine: `{engine}` {version}; features: {features}\n\
         - BlackBox: {blackbox}; simulation revision {revision}; contract {contract}; platform \
         `{platform}`; profile `{profile}`\n\n\
         With the same local dataset bytes, BlackBox/engine version and platform, run this \
         POSIX-shell recipe (or save the full configuration JSON above to `{filename}` and invoke \
         the final command). For a nondefault data directory, add `--data-dir <directory>` to \
         `blackbox`.\n\n\
         ```sh\n{reproduce}\n```\n\n",
        config_hash = m.config_sha256,
        result_hash = m.result_sha256,
        dataset_hash = m.dataset.sha256,
        engine = m.engine.name,
        version = m.engine.version,
        features = m.engine.features.join(", "),
        blackbox = m.blackbox_version,
        revision = m.simulation_revision,
        contract = m.contract_version,
        platform = m.platform,
        profile = m.profile,
        reproduce = demos::reproduce(c, &filename),
    );

    // ----- Limitations -----
    let _ = write!(
        w,
        "## Limitations\n\n\
         - No market impact: {no_impact}\n\
         - Queue position is an estimate of the {qname} (`{qcode}`); the market-by-price data \
         (`{level_code}`) does not show it.\n\
         - {fill_limit}\n\
         - Order latency ({latency_name}, `{latency_code}`) is not calibrated to any venue; \
         feed latency is {feed_source} (`{local_code}`).\n\
         - Accounting is linear and marked at mid, which ignores the cost of closing the final \
         position of {final_pos}.\n\
         - {data_limit}\n\
         - The full list of what BlackBox does not model is `docs/LIMITATIONS.md`.\n",
        no_impact = match dataset.kind {
            DatasetKind::Synthetic =>
                "the generated session does what it was generated to do whatever the strategy does.",
            DatasetKind::Recorded =>
                "the recorded market does what it did whatever the strategy does.",
        },
        qname = queue_model_name(&x.queue_model),
        qcode = x.queue_model.label(),
        fill_limit = match x.exchange_model {
            ExchangeModel::NoPartialFill => format!(
                "{} (`{exchange_code}`) are optimistic about size: an order fills completely \
                 whatever quantity traded.",
                exchange_model_name(x.exchange_model)
            ),
            ExchangeModel::PartialFill => format!(
                "{} (`{exchange_code}`): each own order is credited with the full trade; \
                 liquidity is not shared between them.",
                exchange_model_name(x.exchange_model)
            ),
        },
        latency_name = latency_model_name(&x.order_latency),
        feed_source = match dataset.timestamps.local_ts {
            LocalTsProvenance::Measured =>
                "measured at the vendor's capture host, not where a strategy would run",
            LocalTsProvenance::Synthetic => "generated with the session",
            LocalTsProvenance::Assumed => "an assumption",
        },
        final_pos = num(met.final_position),
        data_limit = match dataset.kind {
            DatasetKind::Synthetic => format!(
                "The data is a {} (`{kind_code}`): results demonstrate the mechanics and say nothing about any real market.",
                dataset_kind_name(dataset.kind)
            ),
            DatasetKind::Recorded => format!(
                "The data is {} (`{kind_code}`) from `{}`; results describe one replayed window, not a forecast.",
                dataset_kind_name(dataset.kind),
                dataset.source.vendor
            ),
        },
    );
    Ok(md)
}

// ===================================================================================== //
// Comparison reports
// ===================================================================================== //

/// Identity and assumptions of one compared run, read from database metadata only (the run
/// row, its experiment config and its dataset version); no artefact is opened.
#[derive(Clone, Debug)]
pub(crate) struct RunFacts {
    pub run_id: String,
    pub config_sha256: String,
    pub result_sha256: String,
    pub dataset_id: String,
    pub dataset_version: Option<i32>,
    pub events_sha256: String,
    pub engine: String,
    pub blackbox_version: String,
    pub simulation_revision: i64,
    pub contract_version: i64,
    pub platform: String,
    /// `None` when the stored configuration no longer parses.
    pub config: Option<RunConfig>,
    /// `None` when the dataset version is unavailable or its events hash differs from the run's.
    pub dataset: Option<DatasetManifest>,
}

impl RunFacts {
    pub(crate) fn from_view(view: &bb_db::RunView, dataset: Option<DatasetManifest>) -> Self {
        let events_sha256 = hex::encode(&view.events_sha256);
        Self {
            run_id: view.short_id.clone(),
            config_sha256: hex::encode(&view.fingerprint),
            result_sha256: hex::encode(&view.run.result_sha256),
            dataset_id: view.dataset_id.clone(),
            dataset_version: Some(view.dataset_version_no),
            dataset: dataset.filter(|d| d.events_sha256 == events_sha256),
            events_sha256,
            engine: format!("{} {}", view.engine_name, view.engine_version),
            blackbox_version: view.run.blackbox_version.clone(),
            simulation_revision: i64::from(view.simulation_revision),
            contract_version: i64::from(view.contract_version),
            platform: view.run.platform.clone(),
            config: serde_json::from_value(view.config.clone()).ok(),
        }
    }
}

/// A stored comparison and its identity.
pub(crate) struct ComparisonRecord<'a> {
    pub id: String,
    pub comparator_version: i32,
    /// Stored eligibility value: `aligned` or `metrics_only`.
    pub eligibility: &'a str,
    pub summary_sha256: String,
    pub comparison: &'a Comparison,
}

/// The comparator's stored replay note (bb-forensics). Its wording predates the distinction
/// between synthetic and recorded data, so reports restate it with the datasets' kinds; the
/// stored summary keeps the original text.
const COMPARATOR_REPLAY_NOTE: &str = "Both runs are historical replays without market impact, so neither shows how the market would have reacted to the simulated orders.";

/// Readable name of a stored code value (strategy kinds, queue, fill and latency models).
fn code_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "touch_maker" => "Touch maker",
        "avellaneda_stoikov" => "Avellaneda–Stoikov market maker",
        "glft" => "GLFT market maker",
        "imbalance_taker" => "Order-book imbalance taker",
        "scripted" => "Scripted orders",
        "risk_adverse" => "Risk-averse queue",
        "log_prob" => "Probabilistic queue, logarithmic weights, form 1",
        "log_prob2" => "Probabilistic queue, logarithmic weights, form 2",
        "power_prob" => "Probabilistic queue, power weights, form 1",
        "power_prob2" => "Probabilistic queue, power weights, form 2",
        "power_prob3" => "Probabilistic queue, power weights, form 3",
        "no_partial_fill" => "Whole-order fills",
        "partial_fill" => "Partial fills",
        "constant" => "Constant latency",
        "jitter" => "Seeded uniform jitter",
        "log_normal" => "Seeded log-normal latency",
        "heavy_tail" => "Seeded heavy-tailed latency (ABIDES cubic)",
        "synthetic" => "Synthetic session generated by BlackBox",
        "recorded" => "Recorded market data",
        _ => return None,
    })
}

/// Readable name of a configuration path or eligibility field.
fn path_name(path: &str) -> &'static str {
    match path {
        "dataset_id" => "Dataset",
        "dataset.sha256" => "Dataset events",
        "seed" => "Seed",
        "strategy.kind" | "strategy.kind.type" => "Strategy",
        "strategy.interval_ns" => "Decision interval",
        "execution.exchange_model" => "Fill model",
        "execution.queue_model" | "execution.queue_model.type" => "Queue model",
        "execution.queue_model.n" => "Queue model exponent",
        "execution.order_latency.type" => "Order latency model",
        "execution.feed_latency_offset_ns" => "Feed latency offset",
        "execution.think_time_ns" => "Think time",
        "execution.fees.maker" => "Maker fee",
        "execution.fees.taker" => "Taker fee",
        "risk.max_position" => "Position limit",
        "risk.max_order_qty" => "Order size limit",
        "risk.max_open_orders" => "Working-order limit",
        "risk.max_drawdown" => "Drawdown halt",
        "record.sample_every_ticks" => "Equity sampling interval",
        "record.max_queue_updates_per_order" => "Queue-update retention",
        "instrument" => "Instrument",
        "instrument.tick_size" => "Tick size",
        "instrument.lot_size" => "Lot size",
        "instrument.contract_size" => "Contract size",
        "simulation_revision" => "Simulation revision",
        "engine" => "Matching engine",
        p if p.starts_with("execution.order_latency.") => "Order latency parameter",
        p if p.starts_with("strategy.kind.") => "Strategy parameter",
        _ => "Configuration value",
    }
}

/// A stored JSON value for a table cell: a readable form where one exists, then the exact value.
fn value_cell(path: &str, v: &Value) -> String {
    let raw = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let readable = match v {
        Value::Number(n) if path.ends_with("_ns") => n.as_i64().map(duration_label),
        Value::Number(n) if path.starts_with("execution.fees.") => n.as_f64().map(fee_bp),
        Value::String(s) => code_name(s).map(str::to_string),
        Value::Null => Some("none".into()),
        _ => None,
    };
    match readable {
        Some(r) => format!("{r} (`{raw}`)"),
        None => format!("`{raw}`"),
    }
}

fn metric_cell(unit: &str, v: Option<f64>) -> String {
    match v {
        None => "–".into(),
        Some(v) if unit == "ns" => duration_f(v),
        Some(v) => num(v),
    }
}

fn basis_name(b: Basis) -> &'static str {
    match b {
        Basis::Observed => "Observed in the data",
        Basis::Modeled => "Model output",
        Basis::Derived => "Derived from both runs",
        Basis::Unobservable => "Not knowable from replay",
    }
}

fn effect_name(e: EligibilityEffect) -> &'static str {
    match e {
        EligibilityEffect::NoAlignment => "prevents per-order alignment",
        EligibilityEffect::NotLikeForLike => "makes the comparison not like-for-like",
    }
}

fn class_name(c: PairClass) -> &'static str {
    match c {
        PairClass::Identical => "identical",
        PairClass::SameFillsDifferentHistory => "same fills, different recorded history",
        PairClass::SameFillsDifferentFees => "same fills, different fees",
        PairClass::SameOutcomeDifferentTiming => "same outcome, different timing",
        PairClass::SameDecisionDifferentOutcome => "same decision, different outcome",
        PairClass::DifferentDecision => "different decision",
        PairClass::OnlyA => "decision only in A",
        PairClass::OnlyB => "decision only in B",
    }
}

fn delta_line(label: &str, field: &str, d: &DeltaStats) -> String {
    let opt = |v: Option<i64>| v.map_or_else(|| "–".to_string(), duration_label);
    format!(
        "- {label} (`timing.{field}`): {} of {} pairs differ; B − A min {}, median {}, max {}.\n",
        d.nonzero,
        d.count,
        opt(d.min),
        d.median.map_or_else(|| "–".to_string(), duration_f),
        opt(d.max)
    )
}

/// The dataset kinds of A and B; `None` when either manifest is unavailable.
fn replayed(runs: &[Option<RunFacts>; 2]) -> Option<(DatasetKind, DatasetKind)> {
    let kind = |r: &Option<RunFacts>| r.as_ref()?.dataset.as_ref().map(|d| d.kind);
    Some((kind(&runs[0])?, kind(&runs[1])?))
}

/// The Markdown report of a stored comparison. `runs` are A and B, `None` when a run's metadata
/// is no longer visible.
pub(crate) fn comparison_markdown(
    rec: &ComparisonRecord<'_>,
    runs: &[Option<RunFacts>; 2],
) -> String {
    let c = rec.comparison;
    let name = |label: &str, id: &str| {
        if label.is_empty() {
            format!("run {id}")
        } else {
            label.to_string()
        }
    };
    let (name_a, name_b) = (name(&c.label_a, &c.run_a), name(&c.label_b, &c.run_b));
    let kinds = replayed(runs);
    let mut md = String::new();
    let w = &mut md;
    let _ = write!(
        w,
        "# Comparison report: {name_a} vs {name_b}\n\n\
         Research only. Both runs are simulations over {data} under stated latency, queue and fee \
         assumptions; simulated orders never move replayed prices or consume displayed depth (no \
         market impact). Nothing here is a real exchange execution.\n\n",
        data = match kinds {
            Some((a, b)) if a == b => dataset_kind_phrase(a).to_string(),
            Some((a, b)) => format!(
                "{} (A) and {} (B)",
                dataset_kind_phrase(a),
                dataset_kind_phrase(b)
            ),
            None => "the replayed datasets named below".into(),
        },
    );

    // ----- Research question and hypothesis -----
    let changed = c
        .config_diffs
        .iter()
        .map(|d| format!("{} (`{}`)", path_name(&d.path), d.path))
        .collect::<Vec<_>>();
    let _ = write!(
        w,
        "## Research question\n\n\
         How do the results of B, {name_b} (run `{b}`), differ from those of A, {name_a} (run \
         `{a}`), when {what}?\n\n\
         ## Hypothesis\n\n\
         Descriptive comparison, no hypothesis: the differences are measured, not tested against \
         a stated expectation.\n\n",
        a = c.run_a,
        b = c.run_b,
        what = if changed.is_empty() {
            "no configuration path differs".to_string()
        } else {
            format!(
                "{} configuration path{} differ{}: {}",
                changed.len(),
                if changed.len() == 1 { "" } else { "s" },
                if changed.len() == 1 { "s" } else { "" },
                changed.join(", ")
            )
        },
    );

    // ----- Data source and provenance -----
    w.push_str("## Data source\n\n");
    for (side, r) in [("A", &runs[0]), ("B", &runs[1])] {
        match r.as_ref().and_then(|r| r.dataset.as_ref().map(|d| (r, d))) {
            Some((r, d)) => {
                let _ = writeln!(
                    w,
                    "- {side}: dataset {:?} (`{}`), {} (`{}`); vendor or generator `{}`, \
                     exchange `{}`, symbol `{}`. {}",
                    d.title,
                    r.dataset_id,
                    dataset_kind_name(d.kind),
                    serde_code(&d.kind),
                    d.source.vendor,
                    d.source.exchange,
                    d.source.symbol,
                    replay_sentence(d.kind)
                );
            }
            None => {
                let _ = writeln!(
                    w,
                    "- {side}: dataset manifest unavailable{}; its kind and source are not stated.",
                    r.as_ref()
                        .map(|r| format!(" for `{}`", r.dataset_id))
                        .unwrap_or_default()
                );
            }
        }
    }
    w.push_str(
        "\n## Dataset provenance\n\n\
         | Run | Dataset | Version | Kind | Events SHA-256 |\n|---|---|---:|---|---|\n",
    );
    for (side, id, r) in [("A", &c.run_a, &runs[0]), ("B", &c.run_b, &runs[1])] {
        match r {
            Some(r) => {
                let _ = writeln!(
                    w,
                    "| {side} `{id}` | `{}` | {} | {} | `{}` |",
                    r.dataset_id,
                    r.dataset_version
                        .map_or_else(|| "–".to_string(), |v| v.to_string()),
                    r.dataset.as_ref().map_or_else(
                        || "unknown".to_string(),
                        |d| format!("{} (`{}`)", dataset_kind_name(d.kind), serde_code(&d.kind))
                    ),
                    r.events_sha256
                );
            }
            None => {
                let _ = writeln!(w, "| {side} `{id}` | unavailable | – | unknown | – |");
            }
        }
    }
    let same_data = matches!(runs, [Some(a), Some(b)] if a.events_sha256 == b.events_sha256);
    let _ = writeln!(
        w,
        "\n{}\n",
        if same_data {
            "Both runs replayed the same dataset events (equal events SHA-256)."
        } else {
            "The runs did not replay provably identical dataset events; see the eligibility reasons below."
        }
    );

    // ----- Dataset limitations -----
    w.push_str("## Dataset limitations\n\n");
    let mut seen = Vec::new();
    for r in runs.iter().flatten() {
        let Some(d) = &r.dataset else { continue };
        if seen.contains(&d.events_sha256) {
            continue;
        }
        seen.push(d.events_sha256.clone());
        let worst = d.quality.worst();
        let _ = writeln!(
            w,
            "- `{}`: worst quality severity {} (`{}`); receipt timestamps {} (`{}`); {}% of the \
             span had a two-sided, uncrossed book (`quality.book_valid_fraction` = `{}`).",
            d.id,
            worst.map_or("none failed", severity_name),
            worst.map_or_else(|| "ok".to_string(), |s| serde_code(&s)),
            match d.timestamps.local_ts {
                LocalTsProvenance::Measured => "measured at the vendor's capture host",
                LocalTsProvenance::Synthetic => "generated with the session",
                LocalTsProvenance::Assumed => "assumed",
            },
            serde_code(&d.timestamps.local_ts),
            num(d.quality.book_valid_fraction * 100.0),
            d.quality.book_valid_fraction,
        );
    }
    if seen.is_empty() {
        w.push_str("- No dataset manifest is available, so data quality is not stated.\n");
    }
    w.push_str(
        "- Market-by-price data: queue position is never observed in either run; it is \
         estimated by each run's queue model.\n\n",
    );

    // ----- Model assumptions -----
    w.push_str("## Model assumptions\n\n");
    match runs {
        [
            Some(RunFacts {
                config: Some(a), ..
            }),
            Some(RunFacts {
                config: Some(b), ..
            }),
        ] => {
            let row = |w: &mut String, label: &str, va: String, vb: String| {
                let _ = writeln!(w, "| {label} | {va} | {vb} |");
            };
            w.push_str("| Assumption | A | B |\n|---|---|---|\n");
            let strategy = |c: &RunConfig| {
                format!(
                    "{} (`{}`)",
                    strategy_name(&c.strategy.kind),
                    c.strategy.kind.label()
                )
            };
            let interval = |c: &RunConfig| {
                format!(
                    "{} (`{}`)",
                    duration_label(c.strategy.interval_ns),
                    c.strategy.interval_ns
                )
            };
            let fill = |c: &RunConfig| {
                format!(
                    "{} (`{}`)",
                    exchange_model_name(c.execution.exchange_model),
                    serde_code(&c.execution.exchange_model)
                )
            };
            let queue = |c: &RunConfig| {
                format!(
                    "{} (`{}`)",
                    queue_model_name(&c.execution.queue_model),
                    c.execution.queue_model.label()
                )
            };
            let latency = |c: &RunConfig| c.execution.order_latency.describe();
            let feed = |c: &RunConfig| {
                format!(
                    "{} (`{}`)",
                    duration_label(c.execution.feed_latency_offset_ns),
                    c.execution.feed_latency_offset_ns
                )
            };
            let think = |c: &RunConfig| {
                format!(
                    "{} (`{}`)",
                    duration_label(c.execution.think_time_ns),
                    c.execution.think_time_ns
                )
            };
            let fees = |c: &RunConfig| {
                format!(
                    "maker {} (`{}`), taker {} (`{}`)",
                    fee_bp(c.execution.fees.maker),
                    c.execution.fees.maker,
                    fee_bp(c.execution.fees.taker),
                    c.execution.fees.taker
                )
            };
            let seed = |c: &RunConfig| format!("`{}`", c.seed);
            row(w, "Strategy", strategy(a), strategy(b));
            row(w, "Decision interval", interval(a), interval(b));
            row(w, "Fill model", fill(a), fill(b));
            row(w, "Queue model", queue(a), queue(b));
            row(w, "Order latency", latency(a), latency(b));
            row(w, "Feed latency offset", feed(a), feed(b));
            row(w, "Think time", think(a), think(b));
            row(w, "Fees", fees(a), fees(b));
            row(w, "Seed", seed(a), seed(b));
            w.push('\n');
        }
        _ => w.push_str(
            "The configuration of at least one run is unavailable; the differing paths below \
             come from the stored comparison.\n\n",
        ),
    }

    // ----- Experiment configuration -----
    w.push_str("## Experiment configuration\n\n");
    if c.config_diffs.is_empty() {
        w.push_str("No configuration path differs between A and B.\n\n");
    } else {
        w.push_str("| Setting | Configuration path | A | B |\n|---|---|---|---|\n");
        for d in &c.config_diffs {
            let _ = writeln!(
                w,
                "| {} | `{}` | {} | {} |",
                path_name(&d.path),
                d.path,
                value_cell(&d.path, &d.a),
                value_cell(&d.path, &d.b)
            );
        }
        w.push('\n');
    }

    // ----- Methods -----
    let _ = write!(
        w,
        "## Methods\n\n\
         - Comparator version {ver}: the two stored runs' verified results were compared; every \
         delta below is B − A.\n\
         - Eligibility: {elig} (`{elig_code}`); like-for-like: {like}.\n",
        ver = rec.comparator_version,
        elig = if c.eligibility.aligned {
            "order-aligned, so each order id names the same decision in both runs"
        } else {
            "metrics only, no per-order alignment"
        },
        elig_code = rec.eligibility,
        like = if c.eligibility.like_for_like {
            "yes"
        } else {
            "no"
        },
    );
    for r in &c.eligibility.reasons {
        let _ = writeln!(
            w,
            "- {} (`{}`) {} (`{}`): A {}, B {}. {}",
            path_name(&r.field),
            r.field,
            effect_name(r.effect),
            serde_code(&r.effect),
            value_cell(&r.field, &r.a),
            value_cell(&r.field, &r.b),
            r.explanation
        );
    }
    if c.eligibility.aligned {
        w.push_str(
            "- Aligned pairs are classified by comparing decision, decision time, exchange \
             arrival, first response, every fill, cancel and terminal times, final status and \
             recorded lifecycle events.\n",
        );
    }
    w.push('\n');

    // ----- Results -----
    w.push_str(
        "## Results\n\nAll values are simulation output, rounded to six decimals.\n\n\
         | Metric | Key | Unit | A | B | B − A |\n|---|---|---|---:|---:|---:|\n",
    );
    for m in &c.metrics {
        let _ = writeln!(
            w,
            "| {} | `{}` | {} | {} | {} | {} |",
            m.label,
            m.key,
            unit_name(&m.unit),
            metric_cell(&m.unit, m.a),
            metric_cell(&m.unit, m.b),
            metric_cell(&m.unit, m.delta)
        );
    }
    if let Some(al) = &c.alignment {
        w.push_str("\nAligned order pairs by class:\n\n");
        for (class, n) in [
            (PairClass::Identical, al.identical),
            (
                PairClass::SameFillsDifferentHistory,
                al.same_fills_different_history,
            ),
            (
                PairClass::SameFillsDifferentFees,
                al.same_fills_different_fees,
            ),
            (
                PairClass::SameOutcomeDifferentTiming,
                al.same_outcome_different_timing,
            ),
            (
                PairClass::SameDecisionDifferentOutcome,
                al.same_decision_different_outcome,
            ),
            (PairClass::DifferentDecision, al.different_decision),
            (PairClass::OnlyA, al.only_a),
            (PairClass::OnlyB, al.only_b),
        ] {
            let _ = writeln!(w, "- {} (`{}`): {n}", class_name(class), serde_code(&class));
        }
        match &al.first_divergence {
            Some(p) => {
                let _ = writeln!(
                    w,
                    "\nFirst divergence: order {} at strategy tick {}, {} (`{}`).",
                    p.order_id,
                    p.tick_no,
                    class_name(p.class),
                    serde_code(&p.class)
                );
            }
            None => w.push_str("\nNo aligned pair diverges.\n"),
        }
        let t = &al.timing;
        w.push_str("\nTiming differences of aligned pairs:\n\n");
        w.push_str(&delta_line("Decision time", "decision", &t.decision));
        w.push_str(&delta_line(
            "Exchange arrival",
            "exch_arrival",
            &t.exch_arrival,
        ));
        w.push_str(&delta_line(
            "First response",
            "first_response",
            &t.first_response,
        ));
        w.push_str(&delta_line("First fill", "first_fill", &t.first_fill));
        w.push_str(&delta_line("Completion", "completion", &t.completion));
        if al.pairs_truncated {
            let _ = writeln!(
                w,
                "\nThe stored pairs are capped at {} (`pairs_cap`); the class counts cover every pair.",
                al.pairs_cap
            );
        }
    }
    w.push_str("\n### Interpretation (stored comparator notes)\n\n");
    for n in &c.interpretation {
        let statement = if n.statement == COMPARATOR_REPLAY_NOTE {
            match kinds {
                Some((a, b)) if a == b => format!(
                    "Both runs replay {} without market impact, so neither shows how the market would have reacted to the simulated orders.",
                    dataset_kind_phrase(a)
                ),
                Some((a, b)) => format!(
                    "A replays {} and B replays {}, both without market impact, so neither shows how the market would have reacted to the simulated orders.",
                    dataset_kind_phrase(a),
                    dataset_kind_phrase(b)
                ),
                None => "Both runs are replays without market impact, so neither shows how the market would have reacted to the simulated orders.".into(),
            }
        } else {
            n.statement.clone()
        };
        let _ = writeln!(
            w,
            "- {} (`{}`): {statement}",
            basis_name(n.basis),
            serde_code(&n.basis)
        );
    }

    // ----- Visual evidence -----
    let _ = write!(
        w,
        "\n## Visual evidence\n\n\
         Paths are relative to the BlackBox console.\n\n\
         - Pair comparison: `/compare/pair?a={a}&b={b}`\n\
         - Run A overview: `/forensics/{a}`; run B overview: `/forensics/{b}`\n\
         - Run reports: `/reports/{a}` and `/reports/{b}`\n",
        a = c.run_a,
        b = c.run_b,
    );
    if let Some(p) = c
        .alignment
        .as_ref()
        .and_then(|al| al.first_divergence.as_ref())
    {
        let _ = writeln!(
            w,
            "- First divergence, order {o}: `/forensics/{a}/orders/{o}` and `/forensics/{b}/orders/{o}`",
            o = p.order_id,
            a = c.run_a,
            b = c.run_b,
        );
    }

    // ----- Sensitivity -----
    let _ = write!(
        w,
        "\n## Sensitivity\n\n{}\n\n",
        match c.config_diffs.len() {
            0 => "Not applicable: no configuration path differs.".to_string(),
            1 => "One configuration path differs, so this pair is a one-step sensitivity of the \
                  metrics to that path; a sweep is needed for more than two values."
                .to_string(),
            n => format!(
                "{n} configuration paths differ at once; their individual contributions are not \
                 separated by this pair. Single-parameter comparisons or a sweep are needed."
            ),
        }
    );

    // ----- Statistical uncertainty -----
    w.push_str(
        "## Statistical uncertainty\n\n\
         - Two deterministic simulations: no confidence interval or significance test exists for \
         any delta above.\n",
    );
    for (side, r) in [("A", &runs[0]), ("B", &runs[1])] {
        if let Some(cfg) = r.as_ref().and_then(|r| r.config.as_ref()) {
            let _ = writeln!(
                w,
                "- {side}: seed `{}`; its latency model ({}) {}.",
                cfg.seed,
                latency_model_name(&cfg.execution.order_latency),
                if cfg.execution.order_latency.uses_seed() {
                    "draws from the seed, so another seed can change the result"
                } else {
                    "draws nothing from the seed"
                }
            );
        }
    }
    w.push('\n');

    // ----- Reproduction -----
    let _ = write!(
        w,
        "## Reproduction\n\n\
         - Comparison id: `{id}`\n\
         - Comparator version: {ver}\n\
         - Stored summary SHA-256: `{sha}`\n",
        id = rec.id,
        ver = rec.comparator_version,
        sha = rec.summary_sha256,
    );
    for (side, id, r) in [("A", &c.run_a, &runs[0]), ("B", &c.run_b, &runs[1])] {
        match r {
            Some(r) => {
                let _ = writeln!(
                    w,
                    "- Run {side} `{}`: config SHA-256 `{}`; result SHA-256 `{}`; dataset events \
                     SHA-256 `{}`; engine `{}`; BlackBox {}; simulation revision {}; contract {}; \
                     platform `{}`",
                    r.run_id,
                    r.config_sha256,
                    r.result_sha256,
                    r.events_sha256,
                    r.engine,
                    r.blackbox_version,
                    r.simulation_revision,
                    r.contract_version,
                    r.platform
                );
            }
            None => {
                let _ = writeln!(w, "- Run {side} `{id}`: metadata no longer visible.");
            }
        }
    }
    w.push_str(
        "\nEach run's own report (`/api/v1/runs/<run id>/report`) carries its reproduction \
         recipe; the comparison is recomputed by comparing the two reproduced runs.\n\n",
    );

    // ----- Limitations -----
    let _ = write!(
        w,
        "## Limitations\n\n\
         - Neither run shows how the market would have reacted to its orders (no market impact).\n\
         - Queue positions in both runs are model estimates, not observations.\n\
         {aligned}\
         {like}\
         - A difference is not attributed to one configuration path unless it is arithmetically \
         certain; see the interpretation notes.\n\
         - The comparison rules are listed in `docs/LIMITATIONS.md`.\n",
        aligned = if c
            .alignment
            .as_ref()
            .is_some_and(|al| al.first_divergence.is_some())
        {
            "- Differences after the first divergence may be knock-on effects of it.\n"
        } else {
            ""
        },
        like = if c.eligibility.like_for_like {
            ""
        } else {
            "- The comparison is not like-for-like: the eligibility reasons above can explain part of every delta.\n"
        },
    );
    md
}

#[cfg(test)]
mod tests {
    use bb_data::synthetic::{SyntheticConfig, generate};

    use super::*;
    use crate::{datasets::DatasetCtx, pipeline, presets};

    /// Prose of a Markdown body: fenced blocks and inline code spans removed.
    fn prose(md: &str) -> String {
        let mut out = String::new();
        let mut fenced = false;
        for line in md.lines() {
            if line.starts_with("```") {
                fenced = !fenced;
                continue;
            }
            // Fenced JSON and block-quoted source text are data, not prose.
            if fenced || line.trim_start().starts_with('>') {
                continue;
            }
            for (i, part) in line.split('`').enumerate() {
                if i % 2 == 0 {
                    out.push_str(part);
                }
            }
            out.push('\n');
        }
        out
    }

    /// Raw identifiers that must never appear outside code spans and JSON blocks.
    fn assert_no_raw_names(md: &str) {
        let text = prose(md);
        for raw in [
            "touch_maker",
            "avellaneda_stoikov",
            "imbalance_taker",
            "risk_adverse",
            "log_prob",
            "power_prob",
            "no_partial_fill",
            "partial_fill",
            "metrics_only",
            "l2_mbp_with_trades",
            "net_pnl",
            "interval_ns",
            "NoPartialFill",
            "RiskAdverse",
            "Modeled]",
            "Derived]",
        ] {
            assert!(!text.contains(raw), "raw `{raw}` in prose:\n{text}");
        }
        // No raw nanosecond counts in prose ("100000000 ns").
        for w in text.split_whitespace().collect::<Vec<_>>().windows(2) {
            if w[1].starts_with("ns") {
                assert!(
                    !(w[0].len() > 3 && w[0].chars().all(|c| c.is_ascii_digit())),
                    "raw nanoseconds `{} {}` in prose",
                    w[0],
                    w[1]
                );
            }
        }
    }

    /// A synthetic dataset saved and loaded back the way the application does, so its events
    /// hash is the real one.
    fn context() -> (DatasetCtx, DatasetManifest) {
        let built = generate(&SyntheticConfig {
            id: "report-synth".into(),
            duration_ns: 10_000_000_000,
            ..SyntheticConfig::default()
        })
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        bb_data::store::save(root.path(), &built).unwrap();
        let ctx = DatasetCtx::load(root.path(), "report-synth").unwrap();
        let manifest = ctx.manifest().clone();
        assert_eq!(manifest.events_sha256.len(), 64);
        (ctx, manifest)
    }

    fn base_config(manifest: &DatasetManifest) -> RunConfig {
        presets::find(manifest, "maker_baseline").unwrap().config
    }

    fn sample() -> (RunResult, DatasetManifest) {
        let (ctx, manifest) = context();
        let config = base_config(&manifest);
        (pipeline::execute(&ctx, &config).unwrap(), manifest)
    }

    #[test]
    fn trailer_metadata_renders_identical_synthetic_and_recorded_reports() {
        let (ctx, dataset) = context();
        for preset in presets::presets(&dataset) {
            let run = pipeline::execute(&ctx, &preset.config).unwrap();
            let root = tempfile::tempdir().unwrap();
            let written = bb_artifacts::BundleWriter::new(bb_artifacts::BundleLimits::default())
                .unwrap()
                .write_run(root.path(), &run)
                .unwrap();
            let trailer = bb_artifacts::BundleReader::new(
                &written.trust,
                bb_artifacts::BundleLimits::default(),
            )
            .unwrap()
            .trailer_core(written.path())
            .unwrap();
            for kind in [DatasetKind::Synthetic, DatasetKind::Recorded] {
                let mut dataset = dataset.clone();
                dataset.kind = kind;
                if kind == DatasetKind::Recorded {
                    dataset.timestamps.local_ts = LocalTsProvenance::Measured;
                    dataset.generator = None;
                }
                assert_eq!(
                    markdown(&run, &dataset).unwrap(),
                    markdown_metadata(
                        &trailer.manifest,
                        &trailer.metrics,
                        &trailer.engine_check,
                        &trailer.counts,
                        &dataset,
                    )
                    .unwrap(),
                    "preset {} / {kind:?}",
                    preset.id,
                );
            }
        }
    }

    /// Every evidence section, in order.
    fn assert_sections(md: &str) {
        let mut at = 0;
        for heading in SECTIONS {
            at += md[at..]
                .find(&format!("\n{heading}\n"))
                .unwrap_or_else(|| panic!("{heading} missing or out of order\n{md}"));
        }
    }

    fn facts(run: &RunResult, dataset: &DatasetManifest) -> RunFacts {
        let m = &run.manifest;
        RunFacts {
            run_id: m.run_id.clone(),
            config_sha256: m.config_sha256.clone(),
            result_sha256: m.result_sha256.clone(),
            dataset_id: m.dataset.id.clone(),
            dataset_version: Some(1),
            events_sha256: m.dataset.sha256.clone(),
            engine: format!("{} {}", m.engine.name, m.engine.version),
            blackbox_version: m.blackbox_version.clone(),
            simulation_revision: m.simulation_revision.into(),
            contract_version: m.contract_version.into(),
            platform: m.platform.clone(),
            config: Some(m.config.clone()),
            dataset: Some(dataset.clone()),
        }
    }

    const SECTIONS: [&str; 14] = [
        "## Research question",
        "## Hypothesis",
        "## Data source",
        "## Dataset provenance",
        "## Dataset limitations",
        "## Model assumptions",
        "## Experiment configuration",
        "## Methods",
        "## Results",
        "## Visual evidence",
        "## Sensitivity",
        "## Statistical uncertainty",
        "## Reproduction",
        "## Limitations",
    ];

    #[test]
    fn synthetic_run_report_follows_the_evidence_structure() {
        let (run, dataset) = sample();
        let md = markdown(&run, &dataset).unwrap();
        let m = &run.manifest;
        assert_sections(&md);
        for required in [
            m.run_id.as_str(),
            m.config_sha256.as_str(),
            m.result_sha256.as_str(),
            m.dataset.sha256.as_str(),
            m.engine.version.as_str(),
            m.blackbox_version.as_str(),
            m.platform.as_str(),
            &format!("simulation revision {}", m.simulation_revision),
            &format!("contract {}", m.contract_version),
            "Descriptive run, no hypothesis",
            "Replay of a synthetic session generated by BlackBox, not recorded exchange data",
            "Touch maker (`touch_maker`)",
            "every 100 ms (`strategy.interval_ns` = `100000000`)",
            "| Strategy | Touch maker | `strategy.kind.type` | `touch_maker` |",
            "Kind: synthetic session generated by BlackBox (`synthetic`).",
            "Receipt timestamps were generated synthetically",
            "Worst quality severity:",
            "Nothing here is a real exchange execution.",
            &format!("`/forensics/{}`", m.run_id),
            &format!("`/book/runs/{}`", m.run_id),
            &format!(
                "Net P&L {} quote currency (`net_pnl`)",
                num(run.metrics.net_pnl)
            ),
        ] {
            assert!(md.contains(required), "missing {required}\n{md}");
        }
        for input in &dataset.source.inputs {
            assert!(md.contains(&input.sha256));
        }
        assert!(md.contains(&serde_json::to_string_pretty(&m.config).unwrap()));
        assert!(md.contains(&serde_json::to_string_pretty(&run.metrics).unwrap()));
        assert!(md.contains(&serde_json::to_string_pretty(&run.engine_check).unwrap()));
        assert!(md.contains(&serde_json::to_string_pretty(&dataset).unwrap()));
        assert!(!md.to_lowercase().contains("historical"), "{md}");
        assert!(!md.contains("recorded market data"));
        assert_no_raw_names(&md);
    }

    #[test]
    fn recorded_run_report_names_recorded_data() {
        let (run, mut dataset) = sample();
        dataset.kind = DatasetKind::Recorded;
        dataset.timestamps.local_ts = LocalTsProvenance::Measured;
        dataset.generator = None;
        let md = markdown(&run, &dataset).unwrap();
        for required in [
            "Replay of recorded market data: simulated orders never move the recorded prices",
            "Kind: recorded market data (`recorded`).",
            "Receipt timestamps were measured at the vendor's capture host",
            "vendor data must not be redistributed",
            "simulation over recorded market data under stated",
        ] {
            assert!(md.contains(required), "missing {required}");
        }
        assert!(!md.contains("synthetic session generated by BlackBox"));
        assert_no_raw_names(&md);
    }

    #[test]
    fn replacement_dataset_bytes_are_refused() {
        let (run, mut dataset) = sample();
        dataset.events_sha256 = "0".repeat(64);
        assert!(markdown(&run, &dataset).is_err());
    }

    #[test]
    fn comparison_report_names_assumptions_and_keeps_every_hash() {
        let (ctx, dataset) = context();
        let a_config = base_config(&dataset);
        let mut b_config = a_config.clone();
        b_config.execution.queue_model = QueueModelConfig::LogProb;
        let a = pipeline::execute(&ctx, &a_config).unwrap();
        let b = pipeline::execute(&ctx, &b_config).unwrap();
        let comparison = bb_forensics::compare_runs(
            bb_forensics::CompareInput {
                run: &a,
                label: "",
                instrument: Some(&dataset.instrument),
            },
            bb_forensics::CompareInput {
                run: &b,
                label: "",
                instrument: Some(&dataset.instrument),
            },
        );
        assert!(
            comparison
                .interpretation
                .iter()
                .any(|n| n.statement == COMPARATOR_REPLAY_NOTE),
            "the comparator's replay note changed; update COMPARATOR_REPLAY_NOTE"
        );
        let rec = ComparisonRecord {
            id: "0b5c1d9e-0000-4000-8000-000000000001".into(),
            comparator_version: 2,
            eligibility: "aligned",
            summary_sha256: "ab".repeat(32),
            comparison: &comparison,
        };
        let md = comparison_markdown(
            &rec,
            &[Some(facts(&a, &dataset)), Some(facts(&b, &dataset))],
        );
        assert_sections(&md);
        for run in [&a, &b] {
            let m = &run.manifest;
            for hash in [&m.config_sha256, &m.result_sha256, &m.dataset.sha256] {
                assert!(md.contains(hash.as_str()), "missing {hash}");
            }
            assert!(md.contains(&format!("simulation revision {}", m.simulation_revision)));
        }
        for required in [
            rec.id.as_str(),
            rec.summary_sha256.as_str(),
            "Comparator version: 2",
            "Descriptive comparison, no hypothesis",
            "| Queue model | `execution.queue_model.type` | Probabilistic queue, power weights, form 3 (`power_prob3`) | Probabilistic queue, logarithmic weights, form 1 (`log_prob`) |",
            "Eligibility: order-aligned, so each order id names the same decision in both runs (`aligned`)",
            "Both runs replay a synthetic session generated by BlackBox without market impact",
            "Both runs replayed the same dataset events (equal events SHA-256).",
            &format!(
                "`/compare/pair?a={}&b={}`",
                a.manifest.run_id, b.manifest.run_id
            ),
            "Derived from both runs (`derived`)",
            "| Net P&L | `net_pnl` | quote currency |",
        ] {
            assert!(md.contains(required), "missing {required}\n{md}");
        }
        assert!(!md.to_lowercase().contains("historical"), "{md}");
        assert_no_raw_names(&md);

        // A run that is no longer visible is stated, never invented.
        let md = comparison_markdown(&rec, &[Some(facts(&a, &dataset)), None]);
        assert_sections(&md);
        assert!(md.contains(&format!(
            "- Run B `{}`: metadata no longer visible.",
            b.manifest.run_id
        )));
        assert!(md.contains("simulations over the replayed datasets named below"));
        assert!(!md.to_lowercase().contains("historical"));
        assert_no_raw_names(&md);
    }

    #[test]
    fn sweep_report_names_parameters_and_keeps_codes() {
        use bb_core::{
            compare::GridAxis,
            sweep::{Objective, SweepDesign, SweepSpec},
        };

        use crate::v1::{
            model::{SweepReportCell, SweepReportResponse},
            sweeps::report_markdown,
        };

        let (_, dataset) = context();
        let spec = SweepSpec {
            base: base_config(&dataset),
            design: SweepDesign::Grid {
                axes: vec![
                    GridAxis::QueueModel {
                        values: vec![QueueModelConfig::RiskAdverse, QueueModelConfig::LogProb],
                    },
                    GridAxis::OrderLatencyNs {
                        values: vec![1_000_000, 5_000_000],
                    },
                ],
            },
            partition: None,
            objective: Objective::default(),
            cscv_blocks: None,
        };
        let cells = spec.cells().unwrap();
        let outcomes: Vec<bb_forensics::SweepCellOutcome> = cells
            .iter()
            .map(|c| bb_forensics::SweepCellOutcome {
                cell: c.index,
                in_sample: Some(f64::from(c.index) - 1.5),
                out_of_sample: None,
                blocks: None,
                error: None,
            })
            .collect();
        let analysis = bb_forensics::analyze_sweep(&spec, &outcomes).unwrap();
        let fingerprint = analysis.fingerprint.clone();
        let response = SweepReportResponse {
            sweep_id: "sweep-1".into(),
            label: "Queue model by order latency".into(),
            design: "grid".into(),
            state: "completed".into(),
            objective: spec.objective,
            partition: None,
            cells: cells
                .iter()
                .map(|c| SweepReportCell {
                    index: c.index,
                    params: c.params.clone(),
                    in_sample: Some(f64::from(c.index) - 1.5),
                    out_of_sample: None,
                    in_sample_run_id: Some(format!("{:016x}", c.index + 1)),
                    out_of_sample_run_id: None,
                    error: None,
                })
                .collect(),
            analysis: Some(analysis),
            ensemble: None,
        };
        let md = report_markdown(&response);
        assert_sections(&md);
        for required in [
            fingerprint.as_str(),
            "Queue model (`queue_model`) = Risk-averse queue (advances only on trades at the order's price) (`risk_adverse`)",
            "Order latency (entry and response) (`order_latency_ns`) = 5 ms (`5000000`)",
            "Net P&L (`net_pnl`)",
            "Grid of every combination (`grid`), 4 cells",
            "Selection study, no hypothesis",
            "Selected cell 3 (`selected.cell`)",
            "`/compare/sweeps/sweep-1`",
            "`/forensics/0000000000000004`",
        ] {
            assert!(md.contains(required), "missing {required}\n{md}");
        }
        assert!(!md.to_lowercase().contains("historical"));
        assert_no_raw_names(&md);
    }
}
