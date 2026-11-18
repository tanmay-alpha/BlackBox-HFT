//! `blackbox study queue-shadow`: shadow orders through the reconstructed L3 queue versus the
//! L2 queue models (docs/research/QUEUE_SHADOW_EXPERIMENT.md).
//!
//! Writes per-cell aggregates only: `study-result.json` (contract `blackbox.queue-shadow` v1,
//! compact, byte-stable) with its `.sha256`, `study-result.md`, `design.json` and a local
//! `run.json` with wall-clock performance. No shadow path, book or L2 row is ever written. Real
//! ITCH input is local-only: its outputs must stay under `data/` and are refused inside
//! `web/public`.
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result, ensure};
use bb_analytics::{
    Value,
    execution::ConfidenceBand,
    shadow_queue::{
        Aggregates, Cell, Design, ENGINE_REVISION, L3_REFERENCE, ModelSpec, SCHEMA, SCHEMA_VERSION,
        ShadowEngine, aggregate,
    },
};
use bb_data::{
    itch::{
        FrameSink,
        decode::{FrameReader, Message, decode},
        run, run_file,
    },
    l3synth::{L3SynthConfig, generate},
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

pub const RESULT_FILE: &str = "study-result.json";
pub const STUDY_ID: &str = "queue-shadow";

const INTERVAL_NOTE: &str = "Intervals are nominal 95%: overlapping horizons and serially dependent shadows are not accounted for, so coverage is not established. Hypothesis decisions are nominal/exploratory and unadjusted for multiple comparisons.";

#[derive(clap::Args, Debug, Clone)]
pub struct QueueShadowArgs {
    /// `synthetic` (the l3-demo generator) or a NASDAQ TotalView-ITCH 5.0 file, plain or gzip.
    #[arg(long)]
    pub source: String,
    /// Symbols to study (repeatable). Default: `SYNTH` for the synthetic source.
    #[arg(long = "symbol")]
    pub symbols: Vec<String>,
    /// Epoch nanoseconds of midnight US Eastern on the session date (recorded; ITCH carries no
    /// date). Required for ITCH files.
    #[arg(long)]
    pub midnight_epoch_ns: Option<i64>,
    /// Synthetic generator seed.
    #[arg(long, default_value_t = 7)]
    pub seed: u64,
    /// Synthetic order-flow messages.
    #[arg(long, default_value_t = 30_000)]
    pub events: u32,
    /// Fresh output directory.
    #[arg(long)]
    pub out: PathBuf,
    /// Refuse to run unless the design hashes to this value (the frozen configuration).
    #[arg(long)]
    pub expect_design_sha256: Option<String>,
    /// Rank symbols by A/F/E/C/X/D/U messages inside the design window instead of running the
    /// study (the held-out symbol rule).
    #[arg(long)]
    pub rank_activity: bool,
}

/// SHA-256 of the design's canonical JSON (fields in declaration order, no whitespace).
pub fn design_sha256(design: &Design) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(design).expect("design serializes"),
    ))
}

/// Decodes frames into the engine until it has seen the window end.
struct EngineSink {
    engine: ShadowEngine,
    frames: u64,
    invalid: u64,
}

impl FrameSink for EngineSink {
    fn limit_reached(&self) -> bool {
        self.engine.ended()
    }

    fn push(&mut self, frame: &[u8]) {
        self.frames += 1;
        match decode(frame) {
            Ok(msg) => self.engine.on_message(&msg),
            Err(_) => self.invalid += 1,
        }
    }
}

/// Counts order-flow messages per locate inside `[start, end)` (exchange clock).
struct ActivitySink {
    start: u64,
    end: u64,
    symbols: BTreeMap<u16, String>,
    counts: BTreeMap<u16, u64>,
    done: bool,
}

impl FrameSink for ActivitySink {
    fn limit_reached(&self) -> bool {
        self.done
    }

    fn push(&mut self, frame: &[u8]) {
        let Ok(msg) = decode(frame) else {
            return;
        };
        let Some(h) = msg.header() else {
            return;
        };
        if h.timestamp_ns >= self.end {
            self.done = true;
            return;
        }
        match msg {
            Message::StockDirectory(m) => {
                self.symbols.insert(h.stock_locate, m.stock.trimmed());
            }
            Message::AddOrder(_)
            | Message::OrderExecuted(_)
            | Message::OrderExecutedWithPrice(_)
            | Message::OrderCancel(_)
            | Message::OrderDelete(_)
            | Message::OrderReplace(_)
                if h.timestamp_ns >= self.start =>
            {
                *self.counts.entry(h.stock_locate).or_default() += 1;
            }
            _ => {}
        }
    }
}

fn inside_web_public(path: &Path) -> bool {
    let text = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    text.contains("web/public")
}

pub fn run_cli(args: QueueShadowArgs) -> Result<()> {
    let design = Design::preregistered();
    let hash = design_sha256(&design);
    if let Some(expected) = &args.expect_design_sha256 {
        ensure!(
            expected.eq_ignore_ascii_case(&hash),
            "design SHA-256 {hash} differs from the frozen {expected}; a changed design is a new \
             in-sample study"
        );
    }
    let synthetic = args.source == "synthetic";
    if !synthetic {
        ensure!(
            args.midnight_epoch_ns.is_some(),
            "--midnight-epoch-ns is required for an ITCH file (compute it per day, with DST)"
        );
        ensure!(
            !args.symbols.is_empty(),
            "--symbol is required for an ITCH file"
        );
        ensure!(
            !inside_web_public(&args.out),
            "real ITCH outputs are local-only and may not be written under web/public"
        );
    }
    ensure!(
        !args.out.exists(),
        "output already exists: {}; choose a fresh directory",
        args.out.display()
    );
    fs::create_dir_all(&args.out).with_context(|| format!("creating {}", args.out.display()))?;
    if args.rank_activity {
        return rank_activity(&args, &design);
    }
    let symbols = if args.symbols.is_empty() {
        vec!["SYNTH".to_string()]
    } else {
        args.symbols.clone()
    };
    let engine = ShadowEngine::new(design.clone(), &symbols).map_err(anyhow::Error::msg)?;
    let mut sink = EngineSink {
        engine,
        frames: 0,
        invalid: 0,
    };
    let started = Instant::now();
    let (source, stop) = if synthetic {
        let config = L3SynthConfig {
            seed: args.seed,
            events: args.events,
            symbol: symbols[0].clone(),
            ..L3SynthConfig::default()
        };
        ensure!(symbols.len() == 1, "the synthetic source has one symbol");
        let bytes = generate(&config).map_err(anyhow::Error::msg)?;
        let mut frames = FrameReader::new(bytes.as_slice());
        let stop = run(&mut frames, &mut sink).expect("reading a slice cannot fail");
        let source = json!({
            "kind": "synthetic",
            "generator": "bb_data::l3synth (the l3-demo generator)",
            "config": config,
            "bytes": bytes.len(),
            "sha256": hex::encode(Sha256::digest(&bytes)),
        });
        (source, stop)
    } else {
        let path = Path::new(&args.source);
        let (file, stop, decoded) = run_file(path, &mut sink)?;
        let source = json!({
            "kind": "nasdaq_totalview_itch_5_0",
            "name": file.name,
            "compression": file.compression,
            "bytes": file.bytes,
            "sha256": file.sha256,
            "decompressed_bytes_read": decoded,
        });
        (source, stop)
    };
    let wall = started.elapsed();
    let frames = sink.frames;
    let invalid = sink.invalid;
    let messages = sink.engine.messages();
    let shadow_run = sink.engine.finish();
    let aggregates = aggregate(&design, &shadow_run);
    let stop = stop.record(frames);
    let command = command_line(&args);
    let result = json!({
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "study_id": STUDY_ID,
        "title": "Shadow orders: reconstructed L3 FIFO queue versus L2 queue models",
        "data_class": if synthetic { "synthetic" } else { "real_local" },
        "labels": labels(synthetic),
        "generator": {
            "command": command,
            "blackbox_version": env!("CARGO_PKG_VERSION"),
            "source_commit": std::env::var("BB_GIT_SHA").ok()
                .filter(|s| (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())),
        },
        "source": source,
        "input": {
            "frames_read": frames,
            "invalid_frames": invalid,
            "stop": stop,
            "messages_applied_before_window_end": messages,
        },
        "clock": clock(&design, args.midnight_epoch_ns, synthetic),
        "design": design,
        "design_sha256": hash,
        "engine": {
            "revision": ENGINE_REVISION,
            "deadline_semantics": "A fill with elapsed <= horizon counts, including every message at the longest deadline; the frozen window end remains exclusive.",
            "checkpoint_semantics": "State after all messages timestamped <= checkpoint, excluding paths already filled by then; cancellation counters likewise include all messages at the horizon, stopping at the L3 fill.",
        },
        "interval_note": INTERVAL_NOTE,
        "interval_methods": {
            "fill_fraction_95": "nominal 95% Wilson score interval (no continuity correction) over eligible shadows",
            "difference_95": "nominal 95% Newcombe (1998) method 10 for paired proportions (model minus L3 on the same shadows): Wilson margins, continuity-corrected phi",
            "km": "Kaplan-Meier product-limit over every placed shadow, right-censored at the horizon or the end of tracking; nominal 95% Greenwood normal interval clipped to [0,1]; median = first time survival <= 0.5",
            "checkpoints": "state after all messages timestamped <= checkpoint; mean over shadows unfilled under both paths by then; no interval",
            "cancelled_volume": "pooled ratio of shares through all messages timestamped <= horizon, stopping at L3 fill; no interval",
        },
        "keys": {
            "cells": ["symbol", "latency_ns", "horizon_ns", "model"],
            "cancelled_volume": ["symbol", "latency_ns", "horizon_ns"],
            "checkpoints": ["symbol", "latency_ns", "checkpoint_ns", "model"],
            "reference_model": L3_REFERENCE,
            "units": {"*_ns": "nanoseconds (exchange clock)", "probability": "fraction in [0,1]", "shares": "shares"},
        },
        "models": models_doc(&design),
        "results": aggregates,
        "hypotheses": hypotheses(&aggregates, &design),
        "caveats": caveats(),
    });
    let bytes = serde_json::to_vec(&result)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    fs::write(args.out.join(RESULT_FILE), &bytes)?;
    fs::write(
        args.out.join(format!("{RESULT_FILE}.sha256")),
        format!("{digest}  {RESULT_FILE}\n"),
    )?;
    fs::write(
        args.out.join("design.json"),
        serde_json::to_string_pretty(&json!({"design": design, "design_sha256": hash}))? + "\n",
    )?;
    fs::write(
        args.out.join("study-result.md"),
        markdown(&result, &aggregates, &design, synthetic),
    )?;
    let secs = wall.as_secs_f64();
    let perf = json!({
        "wall_seconds": secs,
        "frames_read": frames,
        "frames_per_second": if secs > 0.0 { frames as f64 / secs } else { 0.0 },
        "shadows_placed": shadow_run.shadows.len(),
        "note": "single run on a shared workstation; peak memory is measured outside the process",
    });
    fs::write(
        args.out.join("run.json"),
        serde_json::to_string_pretty(&perf)? + "\n",
    )?;
    println!("design_sha256 : {hash}");
    println!(
        "result        : {} ({digest})",
        args.out.join(RESULT_FILE).display()
    );
    println!(
        "frames        : {frames} in {secs:.2} s ({:.0} frames/s), {} shadows placed",
        if secs > 0.0 {
            frames as f64 / secs
        } else {
            0.0
        },
        shadow_run.shadows.len()
    );
    Ok(())
}

fn rank_activity(args: &QueueShadowArgs, design: &Design) -> Result<()> {
    let mut sink = ActivitySink {
        start: design.window_start_ns,
        end: design.window_end_ns,
        symbols: BTreeMap::new(),
        counts: BTreeMap::new(),
        done: false,
    };
    let source = if args.source == "synthetic" {
        let bytes = generate(&L3SynthConfig {
            seed: args.seed,
            events: args.events,
            ..L3SynthConfig::default()
        })
        .map_err(anyhow::Error::msg)?;
        run(&mut FrameReader::new(bytes.as_slice()), &mut sink).expect("slice");
        json!({"kind": "synthetic"})
    } else {
        let (file, _, _) = run_file(Path::new(&args.source), &mut sink)?;
        json!({"name": file.name, "bytes": file.bytes, "sha256": file.sha256})
    };
    let mut ranked: Vec<(String, u64)> = sink
        .counts
        .iter()
        .map(|(l, &c)| (sink.symbols.get(l).cloned().unwrap_or_default(), c))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(25);
    let out = json!({
        "rule": "A/F/E/C/X/D/U messages per symbol with exchange timestamp in [window_start, window_end) of the frozen design, or until the input ends",
        "window_start_ns": design.window_start_ns,
        "window_end_ns": design.window_end_ns,
        "source": source,
        "reached_window_end": sink.done,
        "top": ranked.iter().map(|(s, c)| json!({"symbol": s, "messages": c})).collect::<Vec<_>>(),
    });
    fs::write(
        args.out.join("activity.json"),
        serde_json::to_string_pretty(&out)? + "\n",
    )?;
    for (i, (s, c)) in ranked.iter().enumerate().take(10) {
        println!("{:>2}. {s:<8} {c}", i + 1);
    }
    Ok(())
}

fn command_line(args: &QueueShadowArgs) -> String {
    let mut cmd = String::from("blackbox study queue-shadow --source ");
    if args.source == "synthetic" {
        let _ = write!(
            cmd,
            "synthetic --seed {} --events {}",
            args.seed, args.events
        );
    } else {
        let name = Path::new(&args.source)
            .file_name()
            .map_or_else(|| args.source.clone(), |n| n.to_string_lossy().into_owned());
        cmd.push_str(&name);
    }
    for s in &args.symbols {
        let _ = write!(cmd, " --symbol {s}");
    }
    if let Some(m) = args.midnight_epoch_ns {
        let _ = write!(cmd, " --midnight-epoch-ns {m}");
    }
    cmd.push_str(" --out <dir>");
    cmd
}

fn labels(synthetic: bool) -> Vec<&'static str> {
    let mut out = vec![if synthetic {
        "synthetic: invented ITCH-format messages from the l3-demo generator, not exchange data"
    } else {
        "real_local: NASDAQ ITCH sample data, local private use only; aggregates only"
    }];
    out.extend([
        "L3 reference is reconstructed, shadow fills are counterfactual",
        "shadow orders have no market impact and nobody reacts to them; fills are upper-bound-style model outputs, never recorded executions",
        "latency L is a swept assumption, not calibrated; there is no receipt clock",
        "L2 models apply hftbacktest 0.9.4 queue formulas offline to the L2 projection of the same messages",
    ]);
    out
}

fn caveats() -> Vec<&'static str> {
    vec![
        "A shadow fill assumes the aggressor would have traded the same size against the shadow and that nobody reacted to it; real fill rates may differ in an unknown direction.",
        "The L3 reference follows displayed orders only; hidden and reserve interest is invisible.",
        "A replace is treated as losing priority and file order as time order inside a nanosecond (shared interpretations of the specification).",
        "Fill = first counterfactual execution of any part of the 100-share shadow (first touch).",
        "Differences between models measure dependence on unobservable queue position; they do not say which model is right outside these data.",
        "Shadows have overlapping horizons and serial dependence; nominal 95% Wilson, Newcombe and Greenwood intervals do not account for this dependence, and their coverage is not established.",
        "Hypothesis decisions are nominal/exploratory and unadjusted for multiple comparisons; rejection counts cannot be interpreted as what chance would produce under these dependent comparisons.",
    ]
}

fn clock(design: &Design, midnight: Option<i64>, synthetic: bool) -> serde_json::Value {
    let utc = |ns: u64| {
        midnight.and_then(|m| {
            let at = m.checked_add(i64::try_from(ns).ok()?)?;
            Some(chrono::DateTime::from_timestamp_nanos(at).to_rfc3339())
        })
    };
    json!({
        "basis": if synthetic {
            "nanoseconds since the synthetic session's midnight; frame order"
        } else {
            "ITCH exchange timestamps, nanoseconds since midnight US Eastern; frame order inside a nanosecond"
        },
        "local_clock": "absent: every latency is the swept assumption L",
        "midnight_epoch_ns": midnight,
        "window_start_utc": utc(design.window_start_ns),
        "window_end_utc": utc(design.window_end_ns),
    })
}

fn models_doc(design: &Design) -> Vec<serde_json::Value> {
    let formula = |m: &ModelSpec| match m.family {
        bb_analytics::shadow_queue::Family::RiskAdverse => "front = min(front, new)",
        bb_analytics::shadow_queue::Family::LogProb => {
            "p = ln(1+back) / (ln(1+back) + ln(1+front))"
        }
        bb_analytics::shadow_queue::Family::LogProb2 => "p = ln(1+back) / ln(1+back+front)",
        bb_analytics::shadow_queue::Family::PowerProb => "p = back^n / (back^n + front^n)",
        bb_analytics::shadow_queue::Family::PowerProb2 => "p = back^n / (back+front)^n",
        bb_analytics::shadow_queue::Family::PowerProb3 => "p = 1 - (front / (front+back))^n",
    };
    std::iter::once(json!({
        "id": L3_REFERENCE,
        "family": null,
        "n": null,
        "description": "shares ahead tracked through the reconstructed FIFO queue",
    }))
    .chain(design.models.iter().map(|m| {
        json!({
            "id": m.id(),
            "family": m.family,
            "n": m.n,
            "description": formula(m),
        })
    }))
    .collect()
}

#[derive(Serialize)]
struct Hypothesis {
    symbol: String,
    latency_ns: u64,
    horizon_ns: u64,
    risk_adverse_difference: Option<f64>,
    risk_adverse_difference_95: [Option<f64>; 2],
    /// H-A: RiskAdverse fill fraction below the L3 shadow's (interval entirely below 0).
    h_a_supported: Option<bool>,
    /// H-B (first part): probability models whose |difference| is below RiskAdverse's.
    closer_than_risk_adverse: Vec<String>,
    /// H0 not rejected (interval contains 0).
    h0_not_rejected: Vec<String>,
}

fn find<'a>(agg: &'a Aggregates, symbol: &str, l: u64, h: u64, model: &str) -> Option<&'a Cell> {
    agg.cells
        .iter()
        .find(|c| c.symbol == symbol && c.latency_ns == l && c.horizon_ns == h && c.model == model)
}

fn diff(cell: &Cell) -> (Option<f64>, [Option<f64>; 2]) {
    cell.vs_l3.as_ref().map_or((None, [None, None]), |d| {
        (
            d.difference.get(),
            [d.difference_95.lower.get(), d.difference_95.upper.get()],
        )
    })
}

fn hypotheses(agg: &Aggregates, design: &Design) -> serde_json::Value {
    let mut rows = Vec::new();
    let mut within_all: BTreeMap<(u64, u64), Vec<String>> = BTreeMap::new();
    for &l in &design.latencies_ns {
        for &h in &design.horizons_ns {
            let mut sets: Vec<Vec<String>> = Vec::new();
            for s in &agg.symbols {
                let Some(ra) = find(agg, &s.symbol, l, h, "risk_adverse") else {
                    continue;
                };
                let (ra_d, ra_ci) = diff(ra);
                let mut closer = Vec::new();
                let mut h0 = Vec::new();
                for m in &design.models {
                    let Some(cell) = find(agg, &s.symbol, l, h, &m.id()) else {
                        continue;
                    };
                    let (d, ci) = diff(cell);
                    if let (Some(d), Some(r)) = (d, ra_d)
                        && m.id() != "risk_adverse"
                        && d.abs() < r.abs()
                    {
                        closer.push(m.id());
                    }
                    if let [Some(lo), Some(hi)] = ci
                        && lo <= 0.0
                        && hi >= 0.0
                    {
                        h0.push(m.id());
                    }
                }
                sets.push(h0.clone());
                rows.push(Hypothesis {
                    symbol: s.symbol.clone(),
                    latency_ns: l,
                    horizon_ns: h,
                    risk_adverse_difference: ra_d,
                    risk_adverse_difference_95: ra_ci,
                    h_a_supported: ra_ci[1].map(|hi| hi < 0.0),
                    closer_than_risk_adverse: closer,
                    h0_not_rejected: h0,
                });
            }
            let common = design
                .models
                .iter()
                .map(ModelSpec::id)
                .filter(|id| !sets.is_empty() && sets.iter().all(|set| set.contains(id)))
                .collect();
            within_all.insert((l, h), common);
        }
    }
    json!({
        "definitions": {
            "h_a": "nominal/exploratory support in a cell when the Newcombe nominal 95% interval of RiskAdverse minus L3 lies entirely below 0",
            "h_b_closer": "a probability model is descriptively closer than RiskAdverse in a cell when |its difference| < |RiskAdverse's difference| (point estimates)",
            "h_b_no_single_model": "nominal/exploratory criterion holds for (L, tau) when no model has H0 not rejected on every studied symbol",
            "h0": "per model and cell: nominal/exploratory rejection when the Newcombe nominal 95% interval excludes 0; no multiplicity adjustment",
        },
        "cells": rows,
        "models_within_interval_on_every_symbol": within_all
            .into_iter()
            .map(|((l, h), ids)| json!({"latency_ns": l, "horizon_ns": h, "models": ids}))
            .collect::<Vec<_>>(),
    })
}

fn v(value: &Value, digits: usize) -> String {
    value
        .get()
        .map_or_else(|| "n/a".to_string(), |x| format!("{x:.digits$}"))
}

fn band(b: &ConfidenceBand) -> String {
    match (b.lower.get(), b.upper.get()) {
        (Some(lo), Some(hi)) => format!("[{lo:.3}, {hi:.3}]"),
        _ => "n/a".into(),
    }
}

fn ms(ns: u64) -> String {
    if ns.is_multiple_of(1_000_000) {
        format!("{} ms", ns / 1_000_000)
    } else {
        format!("{} µs", ns as f64 / 1_000.0)
    }
}

fn markdown(
    result: &serde_json::Value,
    agg: &Aggregates,
    design: &Design,
    synthetic: bool,
) -> String {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# Queue shadow study ({})\n",
        if synthetic {
            "synthetic"
        } else {
            "real, local only"
        }
    );
    for label in labels(synthetic) {
        let _ = writeln!(md, "- {label}");
    }
    let _ = writeln!(
        md,
        "\nDesign SHA-256 `{}`. Source SHA-256 `{}`. Tracking ended at {} ns ({}).\n",
        result["design_sha256"].as_str().unwrap_or_default(),
        result["source"]["sha256"].as_str().unwrap_or_default(),
        agg.tracking_end_ns,
        if agg.stream_reached_window_end {
            "window end"
        } else {
            "end of input before the window end"
        }
    );
    let _ = writeln!(md, "{INTERVAL_NOTE}\n");
    for caveat in caveats() {
        let _ = writeln!(md, "- {caveat}");
    }
    md.push_str("## Placement\n\n| Symbol | Scheduled | Placed | Empty side | Locked/crossed | Not arrived |\n|---|---:|---:|---:|---:|---:|\n");
    for s in &agg.symbols {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} |",
            s.symbol,
            s.scheduled,
            s.placed,
            s.skipped_empty_side,
            s.skipped_locked_or_crossed,
            s.not_arrived
        );
    }
    md.push_str("\n## Fill within τ (every cell)\n\nFraction over eligible shadows (arrival + τ inside tracking) with nominal 95% Wilson interval; Δ = model − L3 on the same shadows with nominal 95% Newcombe paired interval; H0 nominally rejected when that interval excludes 0 (exploratory, unadjusted). KM median over every placed shadow. Causes: queue / trade-through / book cross.\n\n| Symbol | L | τ | Model | Eligible | Filled | Fraction [nominal 95%] | Δ vs L3 [nominal 95%] | H0 (nominal/exploratory) | KM median | Causes |\n|---|---|---|---|---:|---:|---|---|---|---:|---|\n");
    for c in &agg.cells {
        let (d, h0) = c.vs_l3.as_ref().map_or(("—".to_string(), "—"), |p| {
            (
                format!("{} {}", v(&p.difference, 3), band(&p.difference_95)),
                match p.excludes_zero {
                    Some(true) => "rejected",
                    Some(false) => "not rejected",
                    None => "n/a",
                },
            )
        });
        let median = c
            .km_median_ns
            .get()
            .map_or_else(|| "not reached".into(), ms_f);
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} {} | {} | {} | {} | {}/{}/{} |",
            c.symbol,
            ms(c.latency_ns),
            ms(c.horizon_ns),
            c.model,
            c.eligible,
            c.filled,
            v(&c.fill_fraction, 3),
            band(&c.fill_fraction_95),
            d,
            h0,
            median,
            c.fill_causes.queue,
            c.fill_causes.trade_through,
            c.fill_causes.book_cross
        );
    }
    md.push_str("\n## Cancelled volume at the shadow's price (L3)\n\nShares cancelled, deleted or replaced away at the shadow's price until min(L3 fill, τ, end of tracking), pooled.\n\n| Symbol | L | τ | Placed | Ahead | Behind | Share ahead |\n|---|---|---|---:|---:|---:|---:|\n");
    for c in &agg.cancelled_volume {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} |",
            c.symbol,
            ms(c.latency_ns),
            ms(c.horizon_ns),
            c.placed,
            c.ahead_shares,
            c.behind_shares,
            v(&c.share_ahead, 3)
        );
    }
    md.push_str("\n## front − ahead_L3 at checkpoints\n\nState after all messages timestamped <= checkpoint, over shadows unfilled under both paths by then (shares).\n\n| Symbol | L | Checkpoint | Model | Pairs | Mean abs | Mean signed | Mean L3 ahead |\n|---|---|---|---|---:|---:|---:|---:|\n");
    for c in &agg.checkpoints {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            c.symbol,
            ms(c.latency_ns),
            ms(c.checkpoint_ns),
            c.model,
            c.pairs,
            v(&c.mean_abs_error_shares, 1),
            v(&c.mean_signed_error_shares, 1),
            v(&c.mean_l3_ahead_shares, 1)
        );
    }
    md.push_str("\n## Hypotheses (nominal/exploratory, unadjusted)\n\n| Symbol | L | τ | RiskAdverse Δ [nominal 95%] | H-A | Closer than RiskAdverse | H0 not rejected |\n|---|---|---|---|---|---|---|\n");
    if let Some(rows) = result["hypotheses"]["cells"].as_array() {
        for r in rows {
            let num =
                |x: &serde_json::Value| x.as_f64().map_or("n/a".into(), |x| format!("{x:.3}"));
            let list = |x: &serde_json::Value| {
                x.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "none".into())
            };
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} [{}, {}] | {} | {} | {} |",
                r["symbol"].as_str().unwrap_or_default(),
                ms(r["latency_ns"].as_u64().unwrap_or_default()),
                ms(r["horizon_ns"].as_u64().unwrap_or_default()),
                num(&r["risk_adverse_difference"]),
                num(&r["risk_adverse_difference_95"][0]),
                num(&r["risk_adverse_difference_95"][1]),
                match r["h_a_supported"].as_bool() {
                    Some(true) => "supported",
                    Some(false) => "not supported",
                    None => "n/a",
                },
                list(&r["closer_than_risk_adverse"]),
                list(&r["h0_not_rejected"]),
            );
        }
    }
    md.push_str("\nModels with H0 not rejected on every studied symbol, per (L, τ):\n\n");
    if let Some(rows) = result["hypotheses"]["models_within_interval_on_every_symbol"].as_array() {
        for r in rows {
            let models: Vec<&str> = r["models"]
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
                .unwrap_or_default();
            let _ = writeln!(
                md,
                "- L {}, τ {}: {}",
                ms(r["latency_ns"].as_u64().unwrap_or_default()),
                ms(r["horizon_ns"].as_u64().unwrap_or_default()),
                if models.is_empty() {
                    "none".to_string()
                } else {
                    models.join(", ")
                }
            );
        }
    }
    let _ = writeln!(
        md,
        "\nDesign: {} models, L ∈ {:?} ns, τ ∈ {:?} ns, checkpoints {:?} ns.",
        design.models.len(),
        design.latencies_ns,
        design.horizons_ns,
        design.checkpoints_ns
    );
    md
}

fn ms_f(ns: f64) -> String {
    format!("{:.1} ms", ns / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(out: PathBuf) -> QueueShadowArgs {
        QueueShadowArgs {
            source: "synthetic".into(),
            symbols: Vec::new(),
            midnight_epoch_ns: None,
            seed: 7,
            events: 30_000,
            out,
            expect_design_sha256: None,
            rank_activity: false,
        }
    }

    #[test]
    fn synthetic_result_is_byte_identical_hashed_and_self_describing() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        run_cli(args(a.clone())).unwrap();
        run_cli(args(b.clone())).unwrap();
        let bytes = fs::read(a.join(RESULT_FILE)).unwrap();
        assert_eq!(bytes, fs::read(b.join(RESULT_FILE)).unwrap());
        assert!(!bytes.contains(&b'\n'));
        let digest = hex::encode(Sha256::digest(&bytes));
        assert_eq!(
            fs::read_to_string(a.join(format!("{RESULT_FILE}.sha256"))).unwrap(),
            format!("{digest}  {RESULT_FILE}\n")
        );
        assert_eq!(
            fs::read(a.join("study-result.md")).unwrap(),
            fs::read(b.join("study-result.md")).unwrap()
        );
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["schema"], SCHEMA);
        assert_eq!(json["schema_version"], SCHEMA_VERSION);
        assert_eq!(json["data_class"], "synthetic");
        assert_eq!(
            json["design_sha256"],
            design_sha256(&Design::preregistered())
        );
        assert_eq!(
            json["design_sha256"],
            "0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502"
        );
        assert_eq!(json["engine"]["revision"], ENGINE_REVISION);
        assert_eq!(json["interval_note"], INTERVAL_NOTE);
        let caveats = json["caveats"].as_array().unwrap();
        assert!(
            caveats
                .iter()
                .any(|c| { c.as_str().is_some_and(|s| s.contains("serial dependence")) })
        );
        assert!(caveats.iter().any(|c| {
            c.as_str()
                .is_some_and(|s| s.contains("multiple comparisons"))
        }));
        let models = 1 + Design::preregistered().models.len();
        let cells = json["results"]["cells"].as_array().unwrap();
        assert_eq!(cells.len(), models * 3 * 3);
        assert!(
            json["labels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l == "L3 reference is reconstructed, shadow fills are counterfactual")
        );
    }

    #[test]
    fn a_changed_design_hash_is_refused_and_real_outputs_stay_out_of_web_public() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(dir.path().join("x"));
        a.expect_design_sha256 = Some("00".repeat(32));
        assert!(
            run_cli(a)
                .unwrap_err()
                .to_string()
                .contains("differs from the frozen")
        );
        let mut real = args(dir.path().join("web").join("public").join("q"));
        real.source = "missing.itch".into();
        real.symbols = vec!["QQQ".into()];
        real.midnight_epoch_ns = Some(0);
        assert!(
            run_cli(real)
                .unwrap_err()
                .to_string()
                .contains("web/public")
        );
    }
}
