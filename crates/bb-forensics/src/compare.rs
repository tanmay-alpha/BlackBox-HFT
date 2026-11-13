//! Deterministic scenario alignment and conservative, arithmetic-only interpretation.

use std::collections::BTreeSet;

use crate::text::{at, num, outcome_label, side_verb, tif_label};
use bb_core::{
    Basis,
    compare::{
        Alignment, Comparison, ConfigDiff, DecisionOutcome, InterpretationNote, MetricRow,
        OrderPair, PairClass,
    },
    result::{Metrics, OrderRecord, RunResult},
};
use serde_json::Value;

const PAIRS_CAP: u64 = 500;

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

fn same_optional(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => close(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn diff_values(path: &str, a: &Value, b: &Value, out: &mut Vec<ConfigDiff>) {
    if let (Value::Object(a), Value::Object(b)) = (a, b) {
        let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
        for key in keys {
            let path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            match (a.get(key), b.get(key)) {
                (Some(a), Some(b)) => diff_values(&path, a, b, out),
                (a, b) => out.push(ConfigDiff {
                    path,
                    a: a.cloned().unwrap_or(Value::Null),
                    b: b.cloned().unwrap_or(Value::Null),
                }),
            }
        }
    } else if a != b {
        out.push(ConfigDiff {
            path: path.into(),
            a: a.clone(),
            b: b.clone(),
        });
    }
}

fn metric_rows(a: &Metrics, b: &Metrics) -> Vec<MetricRow> {
    let mut rows = Vec::new();
    let mut row = |key: &str, label: &str, unit: &str, a: Option<f64>, b: Option<f64>| {
        rows.push(MetricRow {
            key: key.into(),
            label: label.into(),
            unit: unit.into(),
            a,
            b,
            delta: a.zip(b).map(|(a, b)| b - a),
        });
    };
    row(
        "orders_submitted",
        "Orders submitted",
        "count",
        Some(a.orders_submitted as f64),
        Some(b.orders_submitted as f64),
    );
    row(
        "fill_count",
        "Fills",
        "count",
        Some(a.fill_count as f64),
        Some(b.fill_count as f64),
    );
    row(
        "fill_ratio_orders",
        "Order fill ratio",
        "ratio",
        a.fill_ratio_orders,
        b.fill_ratio_orders,
    );
    row(
        "fill_ratio_qty",
        "Quantity fill ratio",
        "ratio",
        a.fill_ratio_qty,
        b.fill_ratio_qty,
    );
    row(
        "order_age_ns",
        "Order age p50",
        "ns",
        a.order_age_ns.as_ref().map(|q| q.p50),
        b.order_age_ns.as_ref().map(|q| q.p50),
    );
    row(
        "time_to_first_fill_ns",
        "Time to first fill p50",
        "ns",
        a.time_to_first_fill_ns.as_ref().map(|q| q.p50),
        b.time_to_first_fill_ns.as_ref().map(|q| q.p50),
    );
    row(
        "entry_latency_ns",
        "Entry latency p50",
        "ns",
        a.entry_latency_ns.as_ref().map(|q| q.p50),
        b.entry_latency_ns.as_ref().map(|q| q.p50),
    );
    row(
        "round_trip_ns",
        "Round trip p50",
        "ns",
        a.round_trip_ns.as_ref().map(|q| q.p50),
        b.round_trip_ns.as_ref().map(|q| q.p50),
    );
    for (key, label, unit, a, b) in [
        ("volume", "Volume", "qty", a.volume, b.volume),
        ("turnover", "Turnover", "quote", a.turnover, b.turnover),
        ("fees", "Fees", "quote", a.fees, b.fees),
        (
            "final_position",
            "Final position",
            "qty",
            a.final_position,
            b.final_position,
        ),
        (
            "max_long",
            "Maximum long position",
            "qty",
            a.max_long,
            b.max_long,
        ),
        (
            "max_short",
            "Maximum short position",
            "qty",
            a.max_short,
            b.max_short,
        ),
        (
            "mean_abs_position",
            "Mean absolute position",
            "qty",
            a.mean_abs_position,
            b.mean_abs_position,
        ),
        (
            "realized_pnl",
            "Realized P&L",
            "quote",
            a.realized_pnl,
            b.realized_pnl,
        ),
        (
            "unrealized_pnl",
            "Unrealized P&L",
            "quote",
            a.unrealized_pnl,
            b.unrealized_pnl,
        ),
        ("net_pnl", "Net P&L", "quote", a.net_pnl, b.net_pnl),
        (
            "max_drawdown",
            "Maximum drawdown",
            "quote",
            a.max_drawdown,
            b.max_drawdown,
        ),
        (
            "spread_capture",
            "Spread capture",
            "quote",
            a.spread_capture,
            b.spread_capture,
        ),
    ] {
        row(key, label, unit, Some(a), Some(b));
    }
    row(
        "spread_capture_ticks",
        "Spread capture",
        "ticks",
        a.spread_capture_ticks,
        b.spread_capture_ticks,
    );
    let horizons: BTreeSet<_> = a
        .markouts
        .iter()
        .chain(&b.markouts)
        .map(|m| m.horizon_ns)
        .collect();
    for horizon in horizons {
        let ms = num(horizon as f64 / 1e6);
        row(
            &format!("markout_{ms}ms"),
            &format!("Markout {ms} ms"),
            "ticks",
            a.markouts
                .iter()
                .find(|m| m.horizon_ns == horizon)
                .and_then(|m| m.mean_ticks),
            b.markouts
                .iter()
                .find(|m| m.horizon_ns == horizon)
                .and_then(|m| m.mean_ticks),
        );
    }
    row(
        "cancels_sent",
        "Cancels sent",
        "count",
        Some(a.cancels_sent as f64),
        Some(b.cancels_sent as f64),
    );
    row(
        "cancels_rejected",
        "Cancels rejected",
        "count",
        Some(a.cancels_rejected as f64),
        Some(b.cancels_rejected as f64),
    );
    row(
        "risk_blocks",
        "Risk blocks",
        "count",
        Some(a.risk_blocks as f64),
        Some(b.risk_blocks as f64),
    );
    rows
}

fn decision(o: &OrderRecord) -> DecisionOutcome {
    DecisionOutcome {
        side: o.side,
        price_tick: o.price_tick,
        qty: o.qty,
        outcome: o.outcome,
        filled_qty: o.filled_qty,
        avg_fill_tick: o.avg_fill_tick,
        queue_ahead_at_entry: o.queue_ahead_at_entry,
        exch_arrival_ts: o.exch_arrival_ts,
        first_fill_ts: o.first_fill_ts,
    }
}

fn pair(a: Option<&OrderRecord>, b: Option<&OrderRecord>) -> OrderPair {
    let o = a.or(b).expect("an aligned pair has at least one order");
    let class = match (a, b) {
        (Some(a), Some(b)) => {
            if a.side != b.side
                || a.price_tick != b.price_tick
                || !close(a.qty, b.qty)
                || a.tif != b.tif
            {
                PairClass::DifferentDecision
            } else if a.outcome != b.outcome
                || !close(a.filled_qty, b.filled_qty)
                || !same_optional(a.avg_fill_tick, b.avg_fill_tick)
            {
                PairClass::SameDecisionDifferentOutcome
            } else {
                PairClass::Identical
            }
        }
        (Some(_), None) => PairClass::OnlyA,
        (None, Some(_)) => PairClass::OnlyB,
        (None, None) => unreachable!(),
    };
    OrderPair {
        order_id: o.order_id,
        tick_no: o.tick_no,
        decision_ts: o.decision_ts,
        class,
        a: a.map(decision),
        b: b.map(decision),
    }
}

fn align(a: &[OrderRecord], b: &[OrderRecord]) -> Alignment {
    let mut out = Alignment {
        identical: 0,
        same_decision_different_outcome: 0,
        different_decision: 0,
        only_a: 0,
        only_b: 0,
        first_divergence: None,
        pairs: Vec::new(),
        pairs_cap: PAIRS_CAP,
        pairs_truncated: false,
    };
    let (mut ai, mut bi) = (0, 0);
    while ai < a.len() || bi < b.len() {
        let p = match (a.get(ai), b.get(bi)) {
            (Some(a), Some(b)) if a.order_id == b.order_id => {
                ai += 1;
                bi += 1;
                pair(Some(a), Some(b))
            }
            (Some(a), b) if b.is_none_or(|b| a.order_id < b.order_id) => {
                ai += 1;
                pair(Some(a), None)
            }
            (_, Some(b)) => {
                bi += 1;
                pair(None, Some(b))
            }
            _ => unreachable!(),
        };
        match p.class {
            PairClass::Identical => {
                out.identical += 1;
                continue;
            }
            PairClass::SameDecisionDifferentOutcome => out.same_decision_different_outcome += 1,
            PairClass::DifferentDecision => out.different_decision += 1,
            PairClass::OnlyA => out.only_a += 1,
            PairClass::OnlyB => out.only_b += 1,
        }
        if out
            .first_divergence
            .as_ref()
            .is_none_or(|first| (p.decision_ts, p.order_id) < (first.decision_ts, first.order_id))
        {
            out.first_divergence = Some(p.clone());
        }
        if out.pairs.len() < PAIRS_CAP as usize {
            out.pairs.push(p);
        } else {
            out.pairs_truncated = true;
        }
    }
    out
}

fn describe(orders: &[OrderRecord], order_id: u64) -> String {
    match orders.binary_search_by_key(&order_id, |o| o.order_id) {
        Err(_) => "no order decision".into(),
        Ok(i) => {
            let o = &orders[i];
            format!(
                "{} {} at price tick {} with {} at {}; {}; filled {} at average fill tick {}",
                side_verb(o.side),
                num(o.qty),
                o.price_tick,
                tif_label(o.tif),
                at(o.decision_ts),
                outcome_label(o.outcome),
                num(o.filled_qty),
                o.avg_fill_tick
                    .map(num)
                    .unwrap_or_else(|| "unavailable".into())
            )
        }
    }
}
/// Compares recorded decisions and measured outcomes, never attributing later P&L to one assumption.
pub fn compare_runs(a: &RunResult, label_a: &str, b: &RunResult, label_b: &str) -> Comparison {
    let mut config_diffs = Vec::new();
    diff_values(
        "",
        &serde_json::to_value(&a.manifest.config).expect("RunConfig serializes"),
        &serde_json::to_value(&b.manifest.config).expect("RunConfig serializes"),
        &mut config_diffs,
    );
    let same_dataset = a.manifest.dataset.sha256 == b.manifest.dataset.sha256;
    let same_strategy = a.manifest.config.strategy == b.manifest.config.strategy;
    let alignment = align(&a.orders, &b.orders);
    let mut interpretation = Vec::new();
    let mut note = |basis, statement| interpretation.push(InterpretationNote { basis, statement });
    if !same_dataset || !same_strategy {
        note(Basis::Derived, "The dataset or strategy differs, so execution assumptions cannot be isolated by this pair.".into());
    }
    note(
        Basis::Derived,
        match config_diffs.len() {
            1 => "1 configuration path differs.".to_string(),
            n => format!("{n} configuration paths differ."),
        },
    );
    if config_diffs.len() > 1 {
        note(Basis::Derived, "The individual contributions of these configuration differences cannot be separated by this pair; single-parameter scenarios are needed.".into());
    }
    if let Some(first) = &alignment.first_divergence {
        note(
            Basis::Derived,
            format!(
                "First divergence at strategy tick {}, order {}, decision time {}: {}: {}; {}: {}. All aligned decisions and outcomes before it were identical, as derived from the tapes.",
                first.tick_no,
                first.order_id,
                at(first.decision_ts),
                label_a,
                describe(&a.orders, first.order_id),
                label_b,
                describe(&b.orders, first.order_id)
            ),
        );
        note(Basis::Derived, "After the first divergence the strategy's inputs differ, so later differences may be knock-on effects and are not separately attributable by this pair.".into());
    } else if !config_diffs.is_empty()
        && config_diffs
            .iter()
            .all(|d| d.path.starts_with("execution.fees."))
    {
        let fees = b.metrics.fees - a.metrics.fees;
        let pnl = b.metrics.net_pnl - a.metrics.net_pnl;
        if close(pnl, -fees) {
            note(
                Basis::Derived,
                format!(
                    "Fills are identical. The net P&L difference (B - A) of {:.6} quote equals the negative fee difference of {:.6} quote (fee difference B - A: {:.6} quote).",
                    pnl, -fees, fees
                ),
            );
        } else {
            note(
                Basis::Derived,
                format!(
                    "Aligned fills are identical, but the net P&L difference (B - A: {:.6} quote) is not equal to the negative fee difference (B - A fees: {:.6} quote); no fee-only P&L attribution is supported.",
                    pnl, fees
                ),
            );
        }
    } else if config_diffs.is_empty() && same_dataset && same_strategy {
        note(Basis::Derived, "The runs are the same experiment: no configuration paths or aligned decisions and outcomes differ.".into());
    }
    note(Basis::Unobservable, "Both runs are historical replays without market impact, so neither shows how the market would have reacted to the simulated orders.".into());
    Comparison {
        run_a: a.manifest.run_id.clone(),
        run_b: b.manifest.run_id.clone(),
        label_a: label_a.into(),
        label_b: label_b.into(),
        same_dataset,
        same_strategy,
        config_diffs,
        metrics: metric_rows(&a.metrics, &b.metrics),
        alignment,
        interpretation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MS, Tape, config, equity_sample, fill_record, run_result};
    use bb_core::{OrderStatus, Side, Tif, config::OrderLatencyConfig};

    fn run() -> RunResult {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(1.0));
        run_result("a", config(), t.0, vec![], vec![])
    }

    #[test]
    fn identical_runs_are_the_same_experiment() {
        let a = run();
        let c = compare_runs(&a, "base", &a, "repeat");
        assert_eq!(
            (&c.run_a, &c.run_b, &c.label_a, &c.label_b),
            (&"a".into(), &"a".into(), &"base".into(), &"repeat".into())
        );
        assert!(c.same_dataset && c.same_strategy);
        assert_eq!(c.alignment.identical, 1);
        assert!(c.alignment.pairs.is_empty());
        assert!(c.alignment.first_divergence.is_none());
        assert!(
            c.interpretation
                .iter()
                .any(|n| n.statement.contains("same experiment"))
        );
        assert_eq!(c.interpretation.last().unwrap().basis, Basis::Unobservable);
        assert_eq!(c.metrics.len(), 28);
        let age = c.metrics.iter().find(|r| r.key == "order_age_ns").unwrap();
        assert_eq!((age.a, age.b, age.delta), (None, None, None));
    }

    #[test]
    fn fee_only_difference_has_checked_exact_arithmetic() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(1.0));
        let seq = t.fill(20 * MS, 256, 20_000, 1.0, true, 1.0, 0.0);
        t.resp(20 * MS, 256, OrderStatus::Filled, 0.0);
        let a = run_result(
            "a",
            config(),
            t.0,
            vec![fill_record(
                seq,
                256,
                20 * MS,
                Side::Buy,
                20_000,
                1.0,
                true,
                1.0,
                1.0,
            )],
            vec![equity_sample(3, -1.0, 1.0)],
        );
        let mut b = a.clone();
        b.manifest.run_id = "b".into();
        b.manifest.config.execution.fees.maker *= 2.0;
        b.metrics.fees = 2.0;
        b.metrics.net_pnl = -2.0;
        let c = compare_runs(&a, "A", &b, "B");
        assert_eq!(c.alignment.identical, 1);
        let n = c
            .interpretation
            .iter()
            .find(|n| n.statement.contains("equals the negative fee difference"))
            .unwrap();
        assert_eq!(n.basis, Basis::Derived);
        assert!(n.statement.contains("of -1.000000 quote"));
        assert!(n.statement.contains("B - A: 1.000000 quote"));
        b.metrics.net_pnl = 0.0;
        assert!(
            !compare_runs(&a, "A", &b, "B")
                .interpretation
                .iter()
                .any(|n| n.statement.contains("equals the negative fee difference"))
        );
    }

    #[test]
    fn latency_outcome_and_intent_differences_align_by_id() {
        let mut a = run();
        for id in [512, 768, 1024, 1280] {
            let mut o = a.orders[0].clone();
            o.order_id = id;
            o.tick_no = id >> 8;
            o.decision_ts = o.tick_no as i64 * 10 * MS;
            a.orders.push(o);
        }
        let mut b = a.clone();
        b.manifest.config.execution.order_latency = OrderLatencyConfig::Constant {
            entry_ns: 2 * MS,
            response_ns: MS,
        };
        b.orders[1].outcome = bb_core::result::OrderOutcome::Expired;
        b.orders[2].tif = Tif::Ioc;
        a.orders.retain(|o| o.order_id != 1280);
        b.orders.retain(|o| o.order_id != 1024);
        let c = compare_runs(&a, "fast", &b, "slow");
        let al = c.alignment;
        assert_eq!(
            (
                al.identical,
                al.same_decision_different_outcome,
                al.different_decision,
                al.only_a,
                al.only_b
            ),
            (1, 1, 1, 1, 1)
        );
        let f = al.first_divergence.unwrap();
        assert_eq!(
            (f.order_id, f.tick_no, f.decision_ts, f.class),
            (512, 2, 20 * MS, PairClass::SameDecisionDifferentOutcome)
        );
        assert_eq!(
            al.pairs.iter().map(|p| p.order_id).collect::<Vec<_>>(),
            [512, 768, 1024, 1280]
        );
        assert!(
            c.interpretation
                .iter()
                .any(|n| n.statement.contains("t=0.020s"))
        );
        assert_eq!(c.config_diffs[0].path, "execution.order_latency.entry_ns");
    }

    #[test]
    fn config_diff_walk_is_sorted_and_keeps_arrays_as_leaves() {
        let mut out = Vec::new();
        diff_values(
            "",
            &serde_json::json!({"z": [1], "a": {"x": 1, "gone": null}}),
            &serde_json::json!({"z": [2], "a": {"x": 2, "new": {"value": 3}}}),
            &mut out,
        );
        assert_eq!(
            out.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["a.gone", "a.new", "a.x", "z"]
        );
        assert_eq!(
            (out[0].a.clone(), out[0].b.clone()),
            (Value::Null, Value::Null)
        );
        assert_eq!(out[1].a, Value::Null);
        let a = run();
        let mut b = a.clone();
        b.manifest.config.risk.max_position = 2.0;
        b.manifest.config.seed += 1;
        let c = compare_runs(&a, "A", &b, "B");
        assert_eq!(
            c.config_diffs
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            ["risk.max_position", "seed"]
        );
        assert!(
            c.interpretation
                .iter()
                .any(|n| n.statement.contains("single-parameter"))
        );
    }

    #[test]
    fn pair_storage_is_capped_but_counts_are_complete() {
        let mut a = run();
        let template = a.orders.pop().unwrap();
        for id in 1..=501 {
            let mut o = template.clone();
            o.order_id = id;
            a.orders.push(o);
        }
        let mut b = a.clone();
        b.orders.clear();
        let c = compare_runs(&a, "A", &b, "B");
        assert_eq!(c.alignment.only_a, 501);
        assert_eq!(c.alignment.pairs.len(), 500);
        assert_eq!(c.alignment.pairs_cap, 500);
        assert!(c.alignment.pairs_truncated);
        a.orders.pop();
        assert!(!compare_runs(&a, "A", &b, "B").alignment.pairs_truncated);
    }

    #[test]
    fn float_comparisons_preserve_missing_values_and_tolerate_roundoff() {
        let a = run();
        let mut b = a.clone();
        b.orders[0].qty += 1e-10;
        assert_eq!(compare_runs(&a, "A", &b, "B").alignment.identical, 1);
        b.orders[0].avg_fill_tick = Some(0.0);
        assert_eq!(
            compare_runs(&a, "A", &b, "B")
                .alignment
                .same_decision_different_outcome,
            1
        );
        b.manifest.dataset.sha256 = "other".into();
        let c = compare_runs(&a, "A", &b, "B");
        assert!(!c.same_dataset);
        assert!(c.interpretation[0].statement.contains("cannot be isolated"));
        assert!(close(1e-12, 1e-12 + 1e-22));
        assert!(!close(0.0, 1e-12));
        assert!(!close(1e-12, 2e-12));
    }
}
