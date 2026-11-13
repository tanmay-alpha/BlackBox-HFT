//! Controlled experiments assembled from the instrument-sized touch-maker presets.

use std::collections::BTreeSet;

use bb_core::{
    config::{FeeConfig, QueueModelConfig, RunConfig},
    dataset::DatasetManifest,
};
use serde_json::Value;

use crate::{
    api::{Demo, DemoRun},
    presets,
};

// Mirrors bb_forensics::compare_runs: object keys recurse, missing keys and non-object
// values (including arrays) are differences at their dotted path.
fn diff_paths(path: &str, a: &Value, b: &Value, out: &mut BTreeSet<String>) {
    if let (Value::Object(a), Value::Object(b)) = (a, b) {
        let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
        for key in keys {
            let child = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            match (a.get(key), b.get(key)) {
                (Some(a), Some(b)) => diff_paths(&child, a, b, out),
                _ => {
                    out.insert(child);
                }
            }
        }
    } else if a != b {
        out.insert(path.to_string());
    }
}

/// A copyable POSIX-shell recipe. It writes the complete config before invoking the CLI;
/// no preset lookup or unrecorded parameter overrides are needed.
pub(crate) fn reproduce(config: &RunConfig, filename: &str) -> String {
    format!(
        "cat > {filename} <<'BLACKBOX_CONFIG'\n{}\nBLACKBOX_CONFIG\nblackbox run --config {filename} --dataset {}",
        serde_json::to_string_pretty(config).expect("RunConfig serializes"),
        config.dataset_id
    )
}

fn experiment(
    manifest: &DatasetManifest,
    id: &str,
    title: &str,
    question: &str,
    how_to_read: &str,
    runs: Vec<DemoRun>,
) -> Demo {
    let values: Vec<Value> = runs
        .iter()
        .map(|r| serde_json::to_value(&r.config).expect("RunConfig serializes"))
        .collect();
    let mut varied = BTreeSet::new();
    for other in &values[1..] {
        diff_paths("", &values[0], other, &mut varied);
    }
    let mut controlled = vec![format!(
        "Dataset id: {}; events SHA-256: {}",
        manifest.id, manifest.events_sha256
    )];
    for (pointer, label) in [
        (
            "/strategy",
            "Strategy kind, parameters and evaluation interval",
        ),
        ("/seed", "Seed"),
        ("/risk", "Risk limits"),
        ("/record", "Recorder settings"),
        ("/execution/exchange_model", "Exchange fill model"),
        ("/execution/queue_model/type", "Queue model"),
        ("/execution/queue_model/n", "Queue exponent"),
        ("/execution/order_latency/type", "Order latency model"),
        (
            "/execution/order_latency/entry_ns",
            "Order entry latency in nanoseconds",
        ),
        (
            "/execution/order_latency/response_ns",
            "Order response latency in nanoseconds",
        ),
        (
            "/execution/feed_latency_offset_ns",
            "Feed latency offset in nanoseconds",
        ),
        (
            "/execution/fees/maker",
            "Maker fee as a fraction of traded value",
        ),
        (
            "/execution/fees/taker",
            "Taker fee as a fraction of traded value",
        ),
    ] {
        if let Some(first) = values[0].pointer(pointer)
            && values[1..]
                .iter()
                .all(|v| v.pointer(pointer) == Some(first))
        {
            controlled.push(format!("{label}: {first}"));
        }
    }
    let reproduce = runs
        .iter()
        .enumerate()
        .map(|(i, r)| reproduce(&r.config, &format!("blackbox-demo-{id}-{}.json", i + 1)))
        .collect();
    Demo {
        id: id.into(),
        title: title.into(),
        question: question.into(),
        varied: varied.into_iter().collect(),
        controlled,
        how_to_read: how_to_read.into(),
        runs,
        reproduce,
    }
}

/// The three canonical demos, sized exactly like the existing presets.
pub fn demos(manifest: &DatasetManifest) -> Vec<Demo> {
    let catalogue = presets::presets(manifest);
    let config = |id: &str| {
        catalogue
            .iter()
            .find(|p| p.id == id)
            .expect("built-in preset exists")
            .config
            .clone()
    };
    let run = |label: &str, config: RunConfig| DemoRun {
        label: label.into(),
        config,
    };
    let baseline = config("maker_baseline");
    let mut log = baseline.clone();
    log.execution.queue_model = QueueModelConfig::LogProb;
    let mut zero = baseline.clone();
    zero.execution.fees = FeeConfig {
        maker: 0.0,
        taker: 0.0,
    };
    vec![
        experiment(
            manifest,
            "cost_of_latency",
            "Cost of latency",
            "How do simulated decisions and fills change between 1 ms and 50 ms order latency each way?",
            "Compare the aligned decisions and first divergence, then inspect simulated fills, inventory and net P&L. Later differences may be knock-on effects; replay does not predict live performance.",
            vec![
                run("1 ms entry and response latency", baseline.clone()),
                run("50 ms entry and response latency", config("maker_slow")),
            ],
        ),
        experiment(
            manifest,
            "queue_position",
            "Queue position uncertainty",
            "How sensitive are simulated touch-maker outcomes to the L2 queue-position model?",
            "Compare the modeled queue trajectories and aligned fill outcomes. L2 data cannot establish actual queue position; differences measure sensitivity to assumptions, not which model is true.",
            vec![
                run("Risk-adverse queue", config("maker_conservative_queue")),
                run("Log-probability queue", log),
                run("PowerProb3 queue, n = 3", baseline.clone()),
            ],
        ),
        experiment(
            manifest,
            "gross_vs_net",
            "Gross versus net",
            "How does the fee schedule change accounting for the same simulated touch-maker fills?",
            "Check fill identity before comparing gross P&L, fees and net P&L. Only when fills match can the net difference be read as pure fee accounting; this is not a forecast of profitability.",
            vec![
                run("2 bp maker / 4 bp taker fees", baseline),
                run("Zero maker and taker fees", zero),
                run("1 bp maker rebate / 4 bp taker fee", config("maker_rebate")),
            ],
        ),
    ]
}
