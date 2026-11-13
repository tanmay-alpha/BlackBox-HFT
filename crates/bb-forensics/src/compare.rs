//! Deterministic scenario comparison: an eligibility gate, timing-aware per-order alignment and
//! conservative, arithmetic-only interpretation.

use std::collections::BTreeSet;

use crate::text::{at, num, outcome_label, side_verb, tif_label};
use bb_core::{
    Basis, RelNs,
    compare::{
        Alignment, Comparison, ConfigDiff, DecisionOutcome, DeltaStats, Eligibility,
        EligibilityEffect, EligibilityReason, InterpretationNote, MetricRow, OrderPair, PairClass,
        PairTiming, TimingSummary,
    },
    config::RunConfig,
    dataset::InstrumentMeta,
    flight::{FlightBody, FlightEvent},
    result::{Metrics, OrderRecord, RunResult},
};
use serde_json::{Value, json};

const PAIRS_CAP: u64 = 500;

/// One side of a comparison.
#[derive(Copy, Clone, Debug)]
pub struct CompareInput<'a> {
    pub run: &'a RunResult,
    pub label: &'a str,
    /// Instrument of the run's dataset. `None` when the dataset manifest is unavailable or its
    /// events no longer match the run; tick, lot and contract sizes are then reported as
    /// unverified rather than assumed equal.
    pub instrument: Option<&'a InstrumentMeta>,
}

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

/// Every dotted path at which two serialized configurations differ, sorted by path. Objects
/// recurse; a key missing on one side and any differing non-object value (arrays included, as
/// whole values) is one difference at its path.
pub fn config_diffs(a: &RunConfig, b: &RunConfig) -> Vec<ConfigDiff> {
    let mut out = Vec::new();
    diff_values(
        "",
        &serde_json::to_value(a).expect("RunConfig serializes"),
        &serde_json::to_value(b).expect("RunConfig serializes"),
        &mut out,
    );
    out
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

fn reason(
    out: &mut Vec<EligibilityReason>,
    field: &str,
    a: Value,
    b: Value,
    effect: EligibilityEffect,
    explanation: &str,
) {
    if a != b {
        out.push(EligibilityReason {
            field: field.into(),
            a,
            b,
            effect,
            explanation: explanation.into(),
        });
    }
}

/// The identity checks made before any per-order alignment.
fn eligibility(a: &CompareInput<'_>, b: &CompareInput<'_>) -> Eligibility {
    use EligibilityEffect::{NoAlignment, NotLikeForLike};
    let (ma, mb) = (&a.run.manifest, &b.run.manifest);
    let mut out = Vec::new();
    reason(
        &mut out,
        "dataset.sha256",
        json!(ma.dataset.sha256),
        json!(mb.dataset.sha256),
        NoAlignment,
        "The runs replayed different market data (the dataset events SHA-256 differs), so the same order id is a decision about a different market.",
    );
    match (a.instrument, b.instrument) {
        (Some(ia), Some(ib)) => {
            reason(
                &mut out,
                "instrument.tick_size",
                json!(ia.tick_size),
                json!(ib.tick_size),
                NoAlignment,
                "Under different tick sizes an equal price tick is a different price.",
            );
            reason(
                &mut out,
                "instrument.lot_size",
                json!(ia.lot_size),
                json!(ib.lot_size),
                NoAlignment,
                "Under different lot sizes order quantities are rounded differently, so equal decisions cannot be identified.",
            );
            reason(
                &mut out,
                "instrument.contract_size",
                json!(ia.contract_size),
                json!(ib.contract_size),
                NotLikeForLike,
                "Quote-currency values (turnover, fees, P&L) scale with the contract size.",
            );
        }
        (ia, ib) => out.push(EligibilityReason {
            field: "instrument".into(),
            a: ia.map_or(Value::Null, |i| json!(i)),
            b: ib.map_or(Value::Null, |i| json!(i)),
            effect: NotLikeForLike,
            explanation: "The instrument of at least one run could not be verified: its dataset manifest is unavailable or its events changed after the run. Tick, lot and contract sizes were not compared.".into(),
        }),
    }
    let kind = |r: &RunResult| {
        serde_json::to_value(&r.manifest.config.strategy.kind).expect("StrategyKind serializes")
            ["type"]
            .clone()
    };
    reason(
        &mut out,
        "strategy.kind",
        kind(a.run),
        kind(b.run),
        NoAlignment,
        "Different strategies: the same tick and slot holds unrelated decisions.",
    );
    reason(
        &mut out,
        "strategy.interval_ns",
        json!(ma.config.strategy.interval_ns),
        json!(mb.config.strategy.interval_ns),
        NoAlignment,
        "The strategies were evaluated on different decision grids: tick n falls at a different time in each run, so the same order id names decisions made at different times.",
    );
    reason(
        &mut out,
        "simulation_revision",
        json!(ma.simulation_revision),
        json!(mb.simulation_revision),
        NoAlignment,
        "Different simulator revisions may decide, sequence or record differently, so the same order id need not name the same decision and differences cannot be attributed to the configuration.",
    );
    let engine = |r: &RunResult| {
        json!(format!(
            "{} {}",
            r.manifest.engine.name, r.manifest.engine.version
        ))
    };
    reason(
        &mut out,
        "engine",
        engine(a.run),
        engine(b.run),
        NotLikeForLike,
        "A different matching-engine version can fill differently, so differences may come from the engine rather than the configuration.",
    );
    reason(
        &mut out,
        "record.sample_every_ticks",
        json!(ma.config.record.sample_every_ticks),
        json!(mb.config.record.sample_every_ticks),
        NotLikeForLike,
        "Equity is marked at different strategy ticks, so metrics computed from the samples (maximum drawdown, mean absolute position) are measured at different times.",
    );
    reason(
        &mut out,
        "record.max_queue_updates_per_order",
        json!(ma.config.record.max_queue_updates_per_order),
        json!(mb.config.record.max_queue_updates_per_order),
        NotLikeForLike,
        "The recorder kept a different number of depth-driven queue updates per order, so recorded lifecycles can differ by retention alone; same_fills_different_history pairs may reflect only that.",
    );
    Eligibility {
        aligned: out.iter().all(|r| r.effect != NoAlignment),
        like_for_like: out.is_empty(),
        reasons: out,
    }
}

/// Each order's tape events, grouped once. The tape is in `seq` order, so each group is too.
struct Lifecycles<'t> {
    tape: &'t [FlightEvent],
    start: Vec<usize>,
    index: Vec<usize>,
}

impl<'t> Lifecycles<'t> {
    fn new(tape: &'t [FlightEvent], orders: &[OrderRecord]) -> Self {
        let slot = |e: &FlightEvent| {
            e.order_id
                .and_then(|id| orders.binary_search_by_key(&id, |o| o.order_id).ok())
        };
        let mut start = vec![0usize; orders.len() + 1];
        for e in tape {
            if let Some(i) = slot(e) {
                start[i + 1] += 1;
            }
        }
        for i in 0..orders.len() {
            start[i + 1] += start[i];
        }
        let mut next = start.clone();
        let mut index = vec![0usize; start[orders.len()]];
        for (k, e) in tape.iter().enumerate() {
            if let Some(i) = slot(e) {
                index[next[i]] = k;
                next[i] += 1;
            }
        }
        Self { tape, start, index }
    }

    fn of(&self, order: usize) -> Vec<&'t FlightEvent> {
        self.index[self.start[order]..self.start[order + 1]]
            .iter()
            .map(|&k| &self.tape[k])
            .collect()
    }
}

/// One order with its recorded lifecycle.
struct Leg<'r> {
    order: &'r OrderRecord,
    events: Vec<&'r FlightEvent>,
}

struct Fill {
    ts: RelNs,
    price_tick: i64,
    qty: f64,
    maker: bool,
    fee: f64,
}

fn fills(events: &[&FlightEvent]) -> Vec<Fill> {
    events
        .iter()
        .filter_map(|e| match e.body {
            FlightBody::ExchFill {
                price_tick,
                qty,
                maker,
                fee,
                ..
            } => Some(Fill {
                ts: e.ts,
                price_tick,
                qty,
                maker,
                fee,
            }),
            _ => None,
        })
        .collect()
}

/// Equal lifecycles: the same events in the same order with the same timestamps, clocks,
/// sources and contents. Only the tape position (`seq`) may differ.
fn same_events(a: &[&FlightEvent], b: &[&FlightEvent]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.ts == y.ts && x.clock == y.clock && x.source == y.source && x.body == y.body
        })
}

/// The class of a pair present in both runs and every field that differs, most significant group
/// first. The first non-empty group decides the class.
fn classify(a: &Leg<'_>, b: &Leg<'_>) -> (PairClass, Vec<String>) {
    fn flag(group: &mut Vec<&'static str>, differs: bool, field: &'static str) {
        if differs {
            group.push(field);
        }
    }
    let (oa, ob) = (a.order, b.order);
    let (fa, fb) = (fills(&a.events), fills(&b.events));
    let same_count = fa.len() == fb.len();
    let any_fill = |differs: &dyn Fn(&Fill, &Fill) -> bool| {
        same_count && fa.iter().zip(&fb).any(|(x, y)| differs(x, y))
    };

    let mut decision = Vec::new();
    flag(&mut decision, oa.side != ob.side, "side");
    flag(&mut decision, oa.price_tick != ob.price_tick, "price_tick");
    flag(&mut decision, !close(oa.qty, ob.qty), "qty");
    flag(&mut decision, oa.tif != ob.tif, "tif");
    flag(&mut decision, oa.ord_type != ob.ord_type, "ord_type");

    let mut outcome = Vec::new();
    flag(&mut outcome, oa.outcome != ob.outcome, "outcome");
    flag(
        &mut outcome,
        oa.final_status != ob.final_status,
        "final_status",
    );
    flag(
        &mut outcome,
        !close(oa.filled_qty, ob.filled_qty),
        "filled_qty",
    );
    flag(
        &mut outcome,
        !same_optional(oa.avg_fill_tick, ob.avg_fill_tick),
        "avg_fill_tick",
    );
    flag(&mut outcome, !same_count, "fill_count");
    flag(
        &mut outcome,
        any_fill(&|x, y| {
            x.price_tick != y.price_tick || !close(x.qty, y.qty) || x.maker != y.maker
        }),
        "fills",
    );

    let mut timing = Vec::new();
    flag(&mut timing, oa.decision_ts != ob.decision_ts, "decision_ts");
    flag(
        &mut timing,
        oa.exch_arrival_ts != ob.exch_arrival_ts,
        "exch_arrival_ts",
    );
    flag(
        &mut timing,
        oa.first_response_ts != ob.first_response_ts,
        "first_response_ts",
    );
    flag(&mut timing, any_fill(&|x, y| x.ts != y.ts), "fill_ts");
    flag(
        &mut timing,
        oa.cancel_sent_ts != ob.cancel_sent_ts,
        "cancel_sent_ts",
    );
    flag(&mut timing, oa.terminal_ts != ob.terminal_ts, "terminal_ts");

    let mut fees = Vec::new();
    flag(&mut fees, any_fill(&|x, y| !close(x.fee, y.fee)), "fees");

    let mut history = Vec::new();
    flag(
        &mut history,
        !same_optional(oa.queue_ahead_at_entry, ob.queue_ahead_at_entry),
        "queue_ahead_at_entry",
    );
    flag(
        &mut history,
        oa.cancel_rejected != ob.cancel_rejected,
        "cancel_rejected",
    );
    flag(
        &mut history,
        !same_events(&a.events, &b.events),
        "lifecycle_events",
    );

    let class = [
        (&decision, PairClass::DifferentDecision),
        (&outcome, PairClass::SameDecisionDifferentOutcome),
        (&timing, PairClass::SameOutcomeDifferentTiming),
        (&fees, PairClass::SameFillsDifferentFees),
        (&history, PairClass::SameFillsDifferentHistory),
    ]
    .into_iter()
    .find(|(group, _)| !group.is_empty())
    .map_or(PairClass::Identical, |(_, class)| class);
    let differences = [decision, outcome, timing, fees, history]
        .concat()
        .into_iter()
        .map(String::from)
        .collect();
    (class, differences)
}

fn decision(o: &OrderRecord) -> DecisionOutcome {
    DecisionOutcome {
        side: o.side,
        price_tick: o.price_tick,
        qty: o.qty,
        tif: o.tif,
        ord_type: o.ord_type,
        decision_ts: o.decision_ts,
        outcome: o.outcome,
        final_status: o.final_status,
        filled_qty: o.filled_qty,
        avg_fill_tick: o.avg_fill_tick,
        maker_qty: o.maker_qty,
        taker_qty: o.taker_qty,
        fees: o.fees,
        fill_count: o.fill_count,
        queue_ahead_at_entry: o.queue_ahead_at_entry,
        exch_arrival_ts: o.exch_arrival_ts,
        first_response_ts: o.first_response_ts,
        first_fill_ts: o.first_fill_ts,
        last_fill_ts: o.last_fill_ts,
        cancel_sent_ts: o.cancel_sent_ts,
        terminal_ts: o.terminal_ts,
    }
}

fn delta(a: Option<RelNs>, b: Option<RelNs>) -> Option<i64> {
    Some(b? - a?)
}

fn pair(a: Option<&Leg<'_>>, b: Option<&Leg<'_>>) -> OrderPair {
    let o = a
        .or(b)
        .expect("an aligned pair has at least one order")
        .order;
    let (class, differences, timing) = match (a, b) {
        (Some(a), Some(b)) => {
            let (class, differences) = classify(a, b);
            let (oa, ob) = (a.order, b.order);
            let timing = PairTiming {
                decision_ns: Some(ob.decision_ts - oa.decision_ts),
                exch_arrival_ns: delta(oa.exch_arrival_ts, ob.exch_arrival_ts),
                first_response_ns: delta(oa.first_response_ts, ob.first_response_ts),
                first_fill_ns: delta(oa.first_fill_ts, ob.first_fill_ts),
                completion_ns: delta(oa.terminal_ts, ob.terminal_ts),
            };
            (class, differences, Some(timing))
        }
        (Some(_), None) => (PairClass::OnlyA, Vec::new(), None),
        (None, Some(_)) => (PairClass::OnlyB, Vec::new(), None),
        (None, None) => unreachable!(),
    };
    OrderPair {
        order_id: o.order_id,
        tick_no: o.tick_no,
        decision_ts: o.decision_ts,
        class,
        differences,
        timing,
        a: a.map(|l| decision(l.order)),
        b: b.map(|l| decision(l.order)),
    }
}

fn stats(mut deltas: Vec<i64>) -> DeltaStats {
    deltas.sort_unstable();
    let n = deltas.len();
    DeltaStats {
        count: n as u64,
        nonzero: deltas.iter().filter(|&&d| d != 0).count() as u64,
        min: deltas.first().copied(),
        median: (n > 0).then(|| {
            if n % 2 == 1 {
                deltas[n / 2] as f64
            } else {
                (deltas[n / 2 - 1] as f64 + deltas[n / 2] as f64) / 2.0
            }
        }),
        max: deltas.last().copied(),
    }
}

/// Non-identical pairs that differ only in timing, fees or recorded history, counted before the
/// first outcome divergence (over all pairs when there is none).
#[derive(Default)]
struct Benign {
    timing: u64,
    fees: u64,
    history: u64,
}

fn leg<'r>(run: &'r RunResult, lifecycles: &Lifecycles<'r>, i: usize) -> Option<Leg<'r>> {
    run.orders.get(i).map(|order| Leg {
        order,
        events: lifecycles.of(i),
    })
}

/// Merges both runs' orders by id (decision order: tick, then slot) and classifies every pair.
fn align(a: &RunResult, b: &RunResult) -> (Alignment, Benign) {
    let (la, lb) = (
        Lifecycles::new(&a.tape, &a.orders),
        Lifecycles::new(&b.tape, &b.orders),
    );
    let mut out = Alignment {
        identical: 0,
        same_fills_different_history: 0,
        same_fills_different_fees: 0,
        same_outcome_different_timing: 0,
        same_decision_different_outcome: 0,
        different_decision: 0,
        only_a: 0,
        only_b: 0,
        first_divergence: None,
        first_outcome_divergence: None,
        timing: TimingSummary {
            decision: stats(Vec::new()),
            exch_arrival: stats(Vec::new()),
            first_response: stats(Vec::new()),
            first_fill: stats(Vec::new()),
            completion: stats(Vec::new()),
        },
        pairs: Vec::new(),
        pairs_cap: PAIRS_CAP,
        pairs_truncated: false,
    };
    let mut before = Benign::default();
    let mut deltas: [Vec<i64>; 5] = Default::default();
    let (mut ai, mut bi) = (0, 0);
    while ai < a.orders.len() || bi < b.orders.len() {
        let (x, y) = match (a.orders.get(ai), b.orders.get(bi)) {
            (Some(x), Some(y)) if x.order_id == y.order_id => {
                ai += 1;
                bi += 1;
                (leg(a, &la, ai - 1), leg(b, &lb, bi - 1))
            }
            (Some(x), y) if y.is_none_or(|y| x.order_id < y.order_id) => {
                ai += 1;
                (leg(a, &la, ai - 1), None)
            }
            (_, Some(_)) => {
                bi += 1;
                (None, leg(b, &lb, bi - 1))
            }
            _ => unreachable!(),
        };
        let p = pair(x.as_ref(), y.as_ref());
        let counter = match p.class {
            PairClass::Identical => &mut out.identical,
            PairClass::SameFillsDifferentHistory => &mut out.same_fills_different_history,
            PairClass::SameFillsDifferentFees => &mut out.same_fills_different_fees,
            PairClass::SameOutcomeDifferentTiming => &mut out.same_outcome_different_timing,
            PairClass::SameDecisionDifferentOutcome => &mut out.same_decision_different_outcome,
            PairClass::DifferentDecision => &mut out.different_decision,
            PairClass::OnlyA => &mut out.only_a,
            PairClass::OnlyB => &mut out.only_b,
        };
        *counter += 1;
        if let Some(t) = p.timing
            && p.class != PairClass::DifferentDecision
        {
            for (values, d) in deltas.iter_mut().zip([
                t.decision_ns,
                t.exch_arrival_ns,
                t.first_response_ns,
                t.first_fill_ns,
                t.completion_ns,
            ]) {
                values.extend(d);
            }
        }
        if p.class == PairClass::Identical {
            continue;
        }
        if out.first_outcome_divergence.is_none() {
            match p.class {
                PairClass::SameOutcomeDifferentTiming => before.timing += 1,
                PairClass::SameFillsDifferentFees => before.fees += 1,
                PairClass::SameFillsDifferentHistory => before.history += 1,
                _ => out.first_outcome_divergence = Some(p.clone()),
            }
        }
        if out.first_divergence.is_none() {
            out.first_divergence = Some(p.clone());
        }
        if out.pairs.len() < PAIRS_CAP as usize {
            out.pairs.push(p);
        } else {
            out.pairs_truncated = true;
        }
    }
    let [
        decision,
        exch_arrival,
        first_response,
        first_fill,
        completion,
    ] = deltas.map(stats);
    out.timing = TimingSummary {
        decision,
        exch_arrival,
        first_response,
        first_fill,
        completion,
    };
    (out, before)
}

fn describe(orders: &[OrderRecord], order_id: u64) -> String {
    match orders.binary_search_by_key(&order_id, |o| o.order_id) {
        Err(_) => "no order decision".into(),
        Ok(i) => {
            let o = &orders[i];
            let arrival = o.exch_arrival_ts.map_or_else(
                || "never reached the exchange".to_string(),
                |t| format!("arrived at the exchange at {}", at(t)),
            );
            let fill_times = match (o.first_fill_ts, o.last_fill_ts) {
                (Some(first), Some(last)) if first != last => {
                    format!("; first fill at {}, last at {}", at(first), at(last))
                }
                (Some(first), _) => format!("; filled at {}", at(first)),
                _ => String::new(),
            };
            format!(
                "{} {} at price tick {} with {} decided at {}; {}; {}; filled {} at average fill tick {}{}",
                side_verb(o.side),
                num(o.qty),
                o.price_tick,
                tif_label(o.tif),
                at(o.decision_ts),
                arrival,
                outcome_label(o.outcome),
                num(o.filled_qty),
                o.avg_fill_tick
                    .map(num)
                    .unwrap_or_else(|| "unavailable".into()),
                fill_times
            )
        }
    }
}

fn class_label(class: PairClass) -> &'static str {
    match class {
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

/// Compares two runs: identity checks first, then (only when order ids name the same decisions)
/// a timing-aware per-order alignment. Never attributes later P&L to one assumption.
pub fn compare_runs(a: CompareInput<'_>, b: CompareInput<'_>) -> Comparison {
    let (ra, rb) = (a.run, b.run);
    let config_diffs = config_diffs(&ra.manifest.config, &rb.manifest.config);
    let eligibility = eligibility(&a, &b);
    let aligned = eligibility.aligned.then(|| align(ra, rb));
    let mut interpretation = Vec::new();
    let mut note =
        |basis, statement: String| interpretation.push(InterpretationNote { basis, statement });
    let fields = |effect| {
        eligibility
            .reasons
            .iter()
            .filter(|r| r.effect == effect)
            .map(|r| r.field.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !eligibility.aligned {
        note(
            Basis::Derived,
            format!(
                "No per-order alignment: {} differ, so the same order id does not name the same decision in both runs. The metric deltas are not like-for-like.",
                fields(EligibilityEffect::NoAlignment)
            ),
        );
    } else if !eligibility.like_for_like {
        note(
            Basis::Derived,
            format!(
                "Not like-for-like: {} differ or could not be verified, so metric deltas and pair differences may also reflect them, not only the configuration paths that differ.",
                fields(EligibilityEffect::NotLikeForLike)
            ),
        );
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
    if let Some((al, before)) = &aligned {
        if let Some(first) = &al.first_divergence {
            note(
                Basis::Derived,
                format!(
                    "First divergence in decision order at strategy tick {}, order {} ({}): {}: {}; {}: {}. Every aligned pair before it is identical: same decision and decision time, exchange arrival, first response, every fill (time, price, quantity, maker/taker, fee), cancel and terminal times, final status and recorded lifecycle events.",
                    first.tick_no,
                    first.order_id,
                    class_label(first.class),
                    a.label,
                    describe(&ra.orders, first.order_id),
                    b.label,
                    describe(&rb.orders, first.order_id)
                ),
            );
            match &al.first_outcome_divergence {
                Some(o) if o.order_id != first.order_id => note(
                    Basis::Derived,
                    format!(
                        "The first difference in a decision or its fills comes later, at strategy tick {}, order {} ({}). The {} pairs before it that are not identical differ only in timing ({}), fees ({}) or recorded history ({}); their decisions and fills (price, quantity, maker/taker) agree.",
                        o.tick_no,
                        o.order_id,
                        class_label(o.class),
                        before.timing + before.fees + before.history,
                        before.timing,
                        before.fees,
                        before.history
                    ),
                ),
                Some(_) => {}
                None => note(
                    Basis::Derived,
                    format!(
                        "No aligned decision or fill differs in price, quantity or maker/taker; {} pairs differ only in timing, {} only in fees and {} only in recorded history.",
                        al.same_outcome_different_timing,
                        al.same_fills_different_fees,
                        al.same_fills_different_history
                    ),
                ),
            }
            note(Basis::Derived, "After the first divergence the strategy's inputs may differ, so later differences may be knock-on effects and are not separately attributable by this pair.".into());
        }
        let fee_only = !config_diffs.is_empty()
            && config_diffs
                .iter()
                .all(|d| d.path.starts_with("execution.fees."));
        if fee_only
            && eligibility.like_for_like
            && al.first_outcome_divergence.is_none()
            && al.same_outcome_different_timing == 0
            && al.same_fills_different_history == 0
        {
            let fees = rb.metrics.fees - ra.metrics.fees;
            let pnl = rb.metrics.net_pnl - ra.metrics.net_pnl;
            if close(pnl, -fees) {
                note(
                    Basis::Derived,
                    format!(
                        "Fills are identical apart from their fees (time, price, quantity and maker/taker all agree). The net P&L difference (B - A) of {:.6} quote equals the negative fee difference of {:.6} quote (fee difference B - A: {:.6} quote).",
                        pnl, -fees, fees
                    ),
                );
            } else {
                note(
                    Basis::Derived,
                    format!(
                        "Aligned fills are identical apart from their fees, but the net P&L difference (B - A: {:.6} quote) is not equal to the negative fee difference (B - A fees: {:.6} quote); no fee-only P&L attribution is supported.",
                        pnl, fees
                    ),
                );
            }
        } else if config_diffs.is_empty()
            && eligibility.like_for_like
            && al.first_divergence.is_none()
        {
            note(Basis::Derived, "The runs are the same experiment: no configuration path, identity field, aligned decision, timing, fill or recorded lifecycle differs.".into());
        }
    }
    note(Basis::Unobservable, "Both runs are historical replays without market impact, so neither shows how the market would have reacted to the simulated orders.".into());
    Comparison {
        run_a: ra.manifest.run_id.clone(),
        run_b: rb.manifest.run_id.clone(),
        label_a: a.label.into(),
        label_b: b.label.into(),
        eligibility,
        config_diffs,
        metrics: metric_rows(&ra.metrics, &rb.metrics),
        alignment: aligned.map(|(al, _)| al),
        interpretation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{INSTRUMENT, MS, Tape, config, equity_sample, fill_record, run_result};
    use EligibilityEffect::{NoAlignment, NotLikeForLike};
    use bb_core::{
        OrderStatus, ReqKind, Side, Tif,
        config::{OrderLatencyConfig, StrategyKind, TouchMakerParams},
        result::OrderOutcome,
    };

    fn input<'a>(run: &'a RunResult, label: &'a str) -> CompareInput<'a> {
        CompareInput {
            run,
            label,
            instrument: Some(&INSTRUMENT),
        }
    }

    fn cmp(a: &RunResult, b: &RunResult) -> Comparison {
        compare_runs(input(a, "A"), input(b, "B"))
    }

    fn aligned(c: &Comparison) -> &Alignment {
        c.alignment.as_ref().expect("the runs are aligned")
    }

    fn reasons(c: &Comparison) -> Vec<(&str, EligibilityEffect)> {
        c.eligibility
            .reasons
            .iter()
            .map(|r| (r.field.as_str(), r.effect))
            .collect()
    }

    fn counts(al: &Alignment) -> [u64; 8] {
        [
            al.identical,
            al.same_fills_different_history,
            al.same_fills_different_fees,
            al.same_outcome_different_timing,
            al.same_decision_different_outcome,
            al.different_decision,
            al.only_a,
            al.only_b,
        ]
    }

    fn tape(build: impl FnOnce(&mut Tape)) -> RunResult {
        let mut t = Tape::new();
        build(&mut t);
        run_result("a", config(), t.0, vec![], vec![])
    }

    fn run() -> RunResult {
        tape(|t| {
            t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(1.0));
        })
    }

    /// A resting buy of 1 at tick 20 000 decided at `decided`, arriving `entry` later with `ahead`
    /// queued in front of it.
    fn open(t: &mut Tape, id: u64, decided: RelNs, entry: i64, ahead: f64) {
        t.intent(decided, id, Side::Buy, 20_000, 1.0, Tif::Gtc);
        t.sent(decided, id, ReqKind::New, entry);
        t.queued(decided + entry, id, Some(ahead), ahead);
        t.resp(decided + entry, id, OrderStatus::New, 1.0);
        t.local(
            decided + entry + MS,
            id,
            OrderStatus::New,
            0.0,
            1.0,
            decided + entry,
        );
    }

    /// Fills the whole order at `ts` with a fee of 1.
    fn fill_all(t: &mut Tape, id: u64, ts: RelNs, maker: bool) {
        t.fill(ts, id, 20_000, 1.0, maker, 1.0, 0.0);
        t.resp(ts, id, OrderStatus::Filled, 0.0);
        t.local(ts + MS, id, OrderStatus::Filled, 1.0, 0.0, ts);
    }

    fn filled(t: &mut Tape, id: u64, decided: RelNs, entry: i64, fill: RelNs) {
        open(t, id, decided, entry, 1.0);
        fill_all(t, id, fill, true);
    }

    #[test]
    fn identical_runs_are_the_same_experiment() {
        let a = run();
        let c = compare_runs(input(&a, "base"), input(&a, "repeat"));
        assert_eq!(
            (&c.run_a, &c.run_b, &c.label_a, &c.label_b),
            (&"a".into(), &"a".into(), &"base".into(), &"repeat".into())
        );
        assert!(c.eligibility.aligned && c.eligibility.like_for_like);
        assert!(c.eligibility.reasons.is_empty());
        let al = aligned(&c);
        assert_eq!(counts(al), [1, 0, 0, 0, 0, 0, 0, 0]);
        assert!(al.pairs.is_empty());
        assert!(al.first_divergence.is_none() && al.first_outcome_divergence.is_none());
        assert!(
            c.interpretation
                .iter()
                .any(|n| n.statement.contains("same experiment"))
        );
        assert_eq!(c.interpretation.last().unwrap().basis, Basis::Unobservable);
        assert_eq!(c.metrics.len(), 28);
        let age = c.metrics.iter().find(|r| r.key == "order_age_ns").unwrap();
        assert_eq!((age.a, age.b, age.delta), (None, None, None));

        // A filled order compared with itself: every timing delta is zero and counted.
        let a = tape(|t| filled(t, 256, 10 * MS, MS, 30 * MS));
        let al = cmp(&a, &a).alignment.unwrap();
        assert_eq!(counts(&al), [1, 0, 0, 0, 0, 0, 0, 0]);
        let zero = DeltaStats {
            count: 1,
            nonzero: 0,
            min: Some(0),
            median: Some(0.0),
            max: Some(0),
        };
        assert_eq!(
            [
                al.timing.decision,
                al.timing.exch_arrival,
                al.timing.first_response,
                al.timing.first_fill,
                al.timing.completion
            ],
            [zero; 5]
        );
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
        for e in &mut b.tape {
            if let FlightBody::ExchFill { fee, .. } = &mut e.body {
                *fee = 2.0;
            }
        }
        b.orders = crate::build_orders(&b.tape);
        b.metrics.fees = 2.0;
        b.metrics.net_pnl = -2.0;
        let c = cmp(&a, &b);
        let al = aligned(&c);
        // A fee change is never "identical": it is its own class, and only fees differ.
        assert_eq!(counts(al), [0, 0, 1, 0, 0, 0, 0, 0]);
        let p = al.first_divergence.as_ref().unwrap();
        assert_eq!(p.class, PairClass::SameFillsDifferentFees);
        assert_eq!(p.differences, ["fees", "lifecycle_events"]);
        assert!(al.first_outcome_divergence.is_none());
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
            !cmp(&a, &b)
                .interpretation
                .iter()
                .any(|n| n.statement.contains("equals the negative fee difference"))
        );
    }

    #[test]
    fn different_evaluation_intervals_refuse_alignment() {
        let a = run();
        let mut b = a.clone();
        b.manifest.config.strategy.interval_ns = 20 * MS;
        let c = cmp(&a, &b);
        assert!(c.alignment.is_none());
        assert!(!c.eligibility.aligned && !c.eligibility.like_for_like);
        assert_eq!(reasons(&c), [("strategy.interval_ns", NoAlignment)]);
        let r = &c.eligibility.reasons[0];
        assert_eq!((r.a.clone(), r.b.clone()), (json!(10 * MS), json!(20 * MS)));
        assert!(r.explanation.contains("different decision grids"));
        // Metric deltas are still returned, labelled not like-for-like; no pair is claimed.
        assert_eq!(c.metrics.len(), 28);
        assert!(
            c.interpretation[0]
                .statement
                .starts_with("No per-order alignment: strategy.interval_ns differ")
        );
        assert!(c.interpretation[0].statement.contains("not like-for-like"));
        assert!(
            !c.interpretation
                .iter()
                .any(|n| n.statement.contains("divergence")
                    || n.statement.contains("same experiment"))
        );
        assert_eq!(c.config_diffs[0].path, "strategy.interval_ns");
    }

    #[test]
    fn every_identity_difference_is_named_with_its_effect() {
        let a = run();
        let with = |b: &RunResult, instrument: Option<&InstrumentMeta>| {
            compare_runs(
                input(&a, "A"),
                CompareInput {
                    run: b,
                    label: "B",
                    instrument,
                },
            )
        };
        let check = |c: Comparison, field: &str, effect: EligibilityEffect| {
            assert_eq!(reasons(&c), [(field, effect)], "{field}");
            assert_eq!(c.eligibility.aligned, effect == NotLikeForLike, "{field}");
            assert_eq!(c.alignment.is_some(), effect == NotLikeForLike, "{field}");
            assert!(!c.eligibility.like_for_like, "{field}");
            assert!(c.interpretation[0].statement.contains(field), "{field}");
            c
        };

        let mut b = a.clone();
        b.manifest.dataset.sha256 = "e".repeat(64);
        check(with(&b, Some(&INSTRUMENT)), "dataset.sha256", NoAlignment);

        for (instrument, field, effect) in [
            (
                InstrumentMeta {
                    tick_size: 1.0,
                    ..INSTRUMENT
                },
                "instrument.tick_size",
                NoAlignment,
            ),
            (
                InstrumentMeta {
                    lot_size: 0.01,
                    ..INSTRUMENT
                },
                "instrument.lot_size",
                NoAlignment,
            ),
            (
                InstrumentMeta {
                    contract_size: 10.0,
                    ..INSTRUMENT
                },
                "instrument.contract_size",
                NotLikeForLike,
            ),
        ] {
            check(with(&a, Some(&instrument)), field, effect);
        }
        let c = check(with(&a, None), "instrument", NotLikeForLike);
        assert_eq!(c.eligibility.reasons[0].b, Value::Null);
        assert_eq!(c.eligibility.reasons[0].a, json!(INSTRUMENT));

        let mut b = a.clone();
        b.manifest.config.strategy.kind = StrategyKind::TouchMaker(TouchMakerParams {
            order_qty: 1.0,
            max_position: 2.0,
            offset_ticks: 0,
            skew_ticks_per_unit: 0.0,
        });
        let c = check(with(&b, Some(&INSTRUMENT)), "strategy.kind", NoAlignment);
        let r = &c.eligibility.reasons[0];
        assert_eq!(
            (r.a.clone(), r.b.clone()),
            (json!("scripted"), json!("touch_maker"))
        );

        let mut b = a.clone();
        b.manifest.simulation_revision += 1;
        check(
            with(&b, Some(&INSTRUMENT)),
            "simulation_revision",
            NoAlignment,
        );

        let mut b = a.clone();
        b.manifest.engine.version = "0.0.0".into();
        check(with(&b, Some(&INSTRUMENT)), "engine", NotLikeForLike);

        let mut b = a.clone();
        b.manifest.config.record.sample_every_ticks += 1;
        check(
            with(&b, Some(&INSTRUMENT)),
            "record.sample_every_ticks",
            NotLikeForLike,
        );

        let mut b = a.clone();
        b.manifest.config.record.max_queue_updates_per_order += 1;
        check(
            with(&b, Some(&INSTRUMENT)),
            "record.max_queue_updates_per_order",
            NotLikeForLike,
        );

        // Several refusals are all named.
        let mut b = a.clone();
        b.manifest.dataset.sha256 = "e".repeat(64);
        b.manifest.config.strategy.interval_ns = 20 * MS;
        let c = with(&b, None);
        assert_eq!(
            reasons(&c),
            [
                ("dataset.sha256", NoAlignment),
                ("instrument", NotLikeForLike),
                ("strategy.interval_ns", NoAlignment)
            ]
        );
        assert!(
            c.interpretation[0]
                .statement
                .contains("dataset.sha256, strategy.interval_ns differ")
        );
    }

    #[test]
    fn changed_decision_times_are_timing_differences() {
        let a = tape(|t| filled(t, 256, 10 * MS, MS, 30 * MS));
        let b = tape(|t| filled(t, 256, 11 * MS, MS, 30 * MS));
        let c = cmp(&a, &b);
        let al = aligned(&c);
        assert_eq!(counts(al), [0, 0, 0, 1, 0, 0, 0, 0]);
        let p = al.first_divergence.as_ref().unwrap();
        assert_eq!(p.class, PairClass::SameOutcomeDifferentTiming);
        assert_eq!(p.differences[0], "decision_ts");
        assert!(!p.differences.contains(&"fill_ts".to_string()));
        let timing = p.timing.unwrap();
        assert_eq!(
            (
                timing.decision_ns,
                timing.first_fill_ns,
                timing.completion_ns
            ),
            (Some(MS), Some(0), Some(0))
        );
        assert_eq!(
            (
                p.decision_ts,
                p.a.as_ref().unwrap().decision_ts,
                p.b.as_ref().unwrap().decision_ts
            ),
            (10 * MS, 10 * MS, 11 * MS)
        );
        assert!(al.first_outcome_divergence.is_none());
        assert!(c.interpretation.iter().any(|n| n.statement.contains(
            "No aligned decision or fill differs in price, quantity or maker/taker; 1 pairs differ only in timing"
        )));
    }

    #[test]
    fn changed_exchange_arrival_times_are_timing_differences() {
        let a = tape(|t| filled(t, 256, 10 * MS, MS, 30 * MS));
        let b = tape(|t| filled(t, 256, 10 * MS, 3 * MS, 30 * MS));
        let c = cmp(&a, &b);
        let al = aligned(&c);
        assert_eq!(counts(al), [0, 0, 0, 1, 0, 0, 0, 0]);
        let p = &al.pairs[0];
        assert_eq!(p.class, PairClass::SameOutcomeDifferentTiming);
        assert!(p.differences.contains(&"exch_arrival_ts".to_string()));
        assert!(!p.differences.contains(&"decision_ts".to_string()));
        assert_eq!(p.timing.unwrap().exch_arrival_ns, Some(2 * MS));
        assert_eq!(
            al.timing.exch_arrival,
            DeltaStats {
                count: 1,
                nonzero: 1,
                min: Some(2 * MS),
                median: Some((2 * MS) as f64),
                max: Some(2 * MS),
            }
        );
        assert_eq!(al.timing.decision.nonzero, 0);
    }

    #[test]
    fn missing_orders_are_only_a_or_only_b_and_can_be_the_first_divergence() {
        let both = tape(|t| {
            filled(t, 256, 10 * MS, MS, 30 * MS);
            filled(t, 512, 20 * MS, MS, 40 * MS);
        });
        let second = tape(|t| filled(t, 512, 20 * MS, MS, 40 * MS));
        for (a, b, class) in [
            (&both, &second, PairClass::OnlyA),
            (&second, &both, PairClass::OnlyB),
        ] {
            let c = cmp(a, b);
            let al = aligned(&c);
            assert_eq!(al.identical, 1);
            let p = al.first_divergence.as_ref().unwrap();
            assert_eq!((p.order_id, p.class), (256, class));
            assert_eq!(al.first_outcome_divergence.as_ref(), Some(p));
            assert!(p.timing.is_none() && p.differences.is_empty());
            assert_eq!(p.a.is_some(), class == PairClass::OnlyA);
            assert!(
                c.interpretation
                    .iter()
                    .any(|n| n.statement.contains("no order decision"))
            );
        }
    }

    #[test]
    fn record_level_outcome_and_intent_differences_align_by_id() {
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
        b.orders[1].outcome = OrderOutcome::Expired;
        b.orders[2].tif = Tif::Ioc;
        a.orders.retain(|o| o.order_id != 1280);
        b.orders.retain(|o| o.order_id != 1024);
        let c = cmp(&a, &b);
        let al = aligned(&c);
        assert_eq!(counts(al), [1, 0, 0, 0, 1, 1, 1, 1]);
        let f = al.first_divergence.as_ref().unwrap();
        assert_eq!(
            (f.order_id, f.tick_no, f.decision_ts, f.class),
            (512, 2, 20 * MS, PairClass::SameDecisionDifferentOutcome)
        );
        assert_eq!(f.differences, ["outcome"]);
        assert_eq!(al.first_outcome_divergence.as_ref(), Some(f));
        assert_eq!(al.pairs[1].differences, ["tif"]);
        assert_eq!(
            al.pairs.iter().map(|p| p.order_id).collect::<Vec<_>>(),
            [512, 768, 1024, 1280]
        );
        assert!(
            c.interpretation
                .iter()
                .any(|n| n.statement.contains("decided at t=0.020s"))
        );
        assert_eq!(c.config_diffs[0].path, "execution.order_latency.entry_ns");
    }

    #[test]
    fn shifted_order_ids_never_pair_as_identical() {
        // The same two decisions, but B numbers them one tick later (as a shifted grid would).
        let a = tape(|t| {
            filled(t, 256, 10 * MS, MS, 30 * MS);
            filled(t, 512, 20 * MS, MS, 40 * MS);
        });
        let b = tape(|t| {
            filled(t, 512, 10 * MS, MS, 30 * MS);
            filled(t, 768, 20 * MS, MS, 40 * MS);
        });
        let c = cmp(&a, &b);
        let al = aligned(&c);
        assert_eq!(counts(al), [0, 0, 0, 1, 0, 0, 1, 1]);
        let shifted = &al.pairs[1];
        assert_eq!(shifted.order_id, 512);
        assert_eq!(shifted.timing.unwrap().decision_ns, Some(-10 * MS));
        assert_eq!(
            al.first_divergence.as_ref().map(|p| (p.order_id, p.class)),
            Some((256, PairClass::OnlyA))
        );
    }

    #[test]
    fn divergent_fills_are_outcome_differences() {
        let maker = tape(|t| filled(t, 256, 10 * MS, MS, 30 * MS));
        let taker = tape(|t| {
            open(t, 256, 10 * MS, MS, 1.0);
            fill_all(t, 256, 30 * MS, false);
        });
        let c = cmp(&maker, &taker);
        let p = &aligned(&c).pairs[0];
        assert_eq!(p.class, PairClass::SameDecisionDifferentOutcome);
        assert!(p.differences.contains(&"fills".to_string()));

        // Same quantity, price, average and times, but split into two fills.
        let split = tape(|t| {
            open(t, 256, 10 * MS, MS, 1.0);
            t.fill(30 * MS, 256, 20_000, 0.5, true, 0.5, 0.5);
            t.fill(30 * MS, 256, 20_000, 0.5, true, 0.5, 0.0);
            t.resp(30 * MS, 256, OrderStatus::Filled, 0.0);
            t.local(31 * MS, 256, OrderStatus::Filled, 1.0, 0.0, 30 * MS);
        });
        let c = cmp(&maker, &split);
        let p = &aligned(&c).pairs[0];
        assert_eq!(p.class, PairClass::SameDecisionDifferentOutcome);
        assert_eq!(p.differences[0], "fill_count");
        let (a, b) = (p.a.as_ref().unwrap(), p.b.as_ref().unwrap());
        assert_eq!(
            (a.filled_qty, a.avg_fill_tick),
            (b.filled_qty, b.avg_fill_tick)
        );
    }

    #[test]
    fn identical_fills_under_distinct_causal_histories() {
        let a = tape(|t| filled(t, 256, 10 * MS, MS, 30 * MS));
        let deeper = tape(|t| {
            open(t, 256, 10 * MS, MS, 3.0);
            fill_all(t, 256, 30 * MS, true);
        });
        let c = cmp(&a, &deeper);
        let al = aligned(&c);
        assert_eq!(counts(al), [0, 1, 0, 0, 0, 0, 0, 0]);
        let p = al.first_divergence.as_ref().unwrap();
        assert_eq!(p.class, PairClass::SameFillsDifferentHistory);
        assert_eq!(p.differences, ["queue_ahead_at_entry", "lifecycle_events"]);
        assert_eq!(al.timing.first_fill.nonzero, 0);

        // Same queue at entry, but the modeled queue moved differently before the same fill.
        let moved = tape(|t| {
            open(t, 256, 10 * MS, MS, 1.0);
            t.trade(20 * MS, 256, 0.4, 1.0, 0.6);
            fill_all(t, 256, 30 * MS, true);
        });
        let c = cmp(&a, &moved);
        let p = &aligned(&c).pairs[0];
        assert_eq!(p.class, PairClass::SameFillsDifferentHistory);
        assert_eq!(p.differences, ["lifecycle_events"]);
        assert!(c.interpretation.iter().any(|n| {
            n.statement
                .contains("(same fills, different recorded history)")
        }));
    }

    /// BASELINE_AUDIT §5.1: switching the queue model moved order 9472's fill from 3.831 s to
    /// 3.860 s at the same price, before the reported first divergence at strategy tick 671.
    #[test]
    fn audit_queue_model_case_reports_the_earlier_fill_timing_difference() {
        const A_FILL: RelNs = 3_831_084_401;
        const B_FILL: RelNs = 3_859_654_055;
        let later = 671 << 8;
        let build = |fill: RelNs, later_fills: bool| {
            tape(|t| {
                filled(t, 256, 100 * MS, MS, 200 * MS);
                open(t, 9472, 3_700 * MS, MS, 0.959);
                fill_all(t, 9472, fill, true);
                open(t, later, 67_200 * MS, MS, 2.0);
                if later_fills {
                    fill_all(t, later, 67_300 * MS, true);
                }
            })
        };
        let (a, b) = (build(A_FILL, true), build(B_FILL, false));
        let c = cmp(&a, &b);
        let al = aligned(&c);
        assert_eq!(counts(al), [1, 0, 0, 1, 1, 0, 0, 0]);

        let first = al.first_divergence.as_ref().unwrap();
        assert_eq!(
            (first.order_id, first.class),
            (9472, PairClass::SameOutcomeDifferentTiming)
        );
        assert!(first.differences.contains(&"fill_ts".to_string()));
        assert!(first.differences.contains(&"terminal_ts".to_string()));
        let (pa, pb) = (first.a.as_ref().unwrap(), first.b.as_ref().unwrap());
        assert_eq!(
            (pa.price_tick, pa.avg_fill_tick, pa.outcome),
            (pb.price_tick, pb.avg_fill_tick, pb.outcome)
        );
        assert_eq!(first.timing.unwrap().first_fill_ns, Some(B_FILL - A_FILL));

        let outcome = al.first_outcome_divergence.as_ref().unwrap();
        assert_eq!(
            (outcome.order_id, outcome.tick_no, outcome.class),
            (later, 671, PairClass::SameDecisionDifferentOutcome)
        );
        // Order 256 (identical) and 9472 have both fills; `later` has none in B.
        assert_eq!(
            al.timing.first_fill,
            DeltaStats {
                count: 2,
                nonzero: 1,
                min: Some(0),
                median: Some((B_FILL - A_FILL) as f64 / 2.0),
                max: Some(B_FILL - A_FILL),
            }
        );

        let statements: Vec<_> = c.interpretation.iter().map(|n| &n.statement).collect();
        let first_note = statements
            .iter()
            .find(|s| s.starts_with("First divergence"))
            .unwrap();
        assert!(
            first_note.contains("strategy tick 37, order 9472 (same outcome, different timing)")
        );
        assert!(first_note.contains("filled at t=3.831084401s"));
        assert!(first_note.contains("filled at t=3.859654055s"));
        assert!(first_note.contains("Every aligned pair before it is identical"));
        assert!(statements.iter().any(|s| s.contains(&format!(
            "comes later, at strategy tick 671, order {later} (same decision, different outcome). The 1 pairs before it that are not identical differ only in timing (1)"
        ))));
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
        let c = cmp(&a, &b);
        assert_eq!(
            c.config_diffs
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            ["risk.max_position", "seed"]
        );
        assert_eq!(
            config_diffs(&a.manifest.config, &b.manifest.config),
            c.config_diffs
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
        let c = cmp(&a, &b);
        let al = aligned(&c);
        assert_eq!(al.only_a, 501);
        assert_eq!(al.pairs.len(), 500);
        assert_eq!(al.pairs_cap, 500);
        assert!(al.pairs_truncated);
        a.orders.pop();
        assert!(!aligned(&cmp(&a, &b)).pairs_truncated);
    }

    #[test]
    fn float_comparisons_preserve_missing_values_and_tolerate_roundoff() {
        let a = run();
        let mut b = a.clone();
        b.orders[0].qty += 1e-10;
        assert_eq!(aligned(&cmp(&a, &b)).identical, 1);
        b.orders[0].avg_fill_tick = Some(0.0);
        assert_eq!(aligned(&cmp(&a, &b)).same_decision_different_outcome, 1);
        assert!(close(1e-12, 1e-12 + 1e-22));
        assert!(!close(0.0, 1e-12));
        assert!(!close(1e-12, 2e-12));
    }
}
