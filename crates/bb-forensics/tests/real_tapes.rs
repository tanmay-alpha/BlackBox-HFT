//! Exercise the forensics contract against real recorder output, not hand-authored events.

use std::collections::{BTreeMap, BTreeSet};

use bb_core::{
    Clock, OrderStatus, ReqKind, Source,
    compare::{EligibilityEffect, PairClass},
    config::{
        ExchangeModel, ExecutionConfig, FeeConfig, ImbalanceTakerParams, OrderLatencyConfig,
        QueueModelConfig, RecordConfig, RiskConfig, RunConfig, StrategyConfig, StrategyKind,
        TouchMakerParams,
    },
    dataset::LocalTsProvenance,
    explain::{Question, Verdict},
    flight::{FlightBody, FlightEvent},
    result::{DatasetRef, EngineInfo, OrderOutcome, OrderRecord, RunManifest, RunResult},
};
use bb_data::{
    BuiltDataset,
    synthetic::{SyntheticConfig, generate},
};
use bb_forensics::{
    CompareInput, build_orders, compare_runs, compute_metrics, explain_order, result_sha256,
};
use bb_sim::{SimInput, SimOutput, simulate};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn expected_outcome(events: &[FlightEvent]) -> OrderOutcome {
    if events
        .iter()
        .any(|e| matches!(e.body, FlightBody::RiskBlocked { .. }))
    {
        return OrderOutcome::RiskBlocked;
    }
    if events.iter().any(|e| {
        matches!(
            e.body,
            FlightBody::RequestRefused {
                req: ReqKind::New,
                ..
            }
        )
    }) {
        return OrderOutcome::Refused;
    }
    let status = events
        .iter()
        .filter_map(|e| match e.body {
            FlightBody::ExchResponse {
                status,
                request_rejected: false,
                ..
            } => Some(status),
            _ => None,
        })
        .next_back();
    let has_fills = events
        .iter()
        .any(|e| matches!(e.body, FlightBody::ExchFill { .. }));
    match (has_fills, status) {
        (true, Some(OrderStatus::Filled)) => OrderOutcome::Filled,
        (true, _) => OrderOutcome::PartiallyFilled,
        (false, Some(OrderStatus::Canceled)) => OrderOutcome::Canceled,
        (false, Some(OrderStatus::Expired)) => OrderOutcome::Expired,
        (false, _) if events.iter().any(|e| e.clock == Clock::Exchange) => {
            OrderOutcome::WorkingAtEnd
        }
        _ => OrderOutcome::InFlightAtEnd,
    }
}

fn check_verdict(outcome: OrderOutcome, status: OrderStatus, verdict: Verdict) {
    match outcome {
        OrderOutcome::Filled => assert!(matches!(
            verdict,
            Verdict::FilledQueueExhausted
                | Verdict::FilledPriceCrossed
                | Verdict::FilledTakerOnArrival
                | Verdict::FilledMixed
        )),
        OrderOutcome::PartiallyFilled => match status {
            OrderStatus::Canceled => assert_eq!(verdict, Verdict::NotFilledCanceled),
            OrderStatus::Expired => assert_eq!(verdict, Verdict::NotFilledNoLiquidityOnArrival),
            _ => assert_eq!(verdict, Verdict::NotFilledWorkingAtEnd),
        },
        OrderOutcome::Canceled => assert_eq!(verdict, Verdict::NotFilledCanceled),
        OrderOutcome::Expired => assert!(matches!(
            verdict,
            Verdict::NotFilledPostOnlyWouldCross | Verdict::NotFilledNoLiquidityOnArrival
        )),
        OrderOutcome::WorkingAtEnd => assert!(matches!(
            verdict,
            Verdict::NotFilledQueueNotReached
                | Verdict::NotFilledNoTradeAtPrice
                | Verdict::NotFilledWorkingAtEnd
        )),
        OrderOutcome::InFlightAtEnd => assert_eq!(verdict, Verdict::NotFilledDataEndedInFlight),
        OrderOutcome::RiskBlocked => assert_eq!(verdict, Verdict::NotFilledRiskBlocked),
        OrderOutcome::Refused => assert_eq!(verdict, Verdict::NotFilledRefused),
    }
}

#[test]
fn synthetic_recorder_tapes_support_orders_explanations_metrics_and_self_comparison() {
    let synthetic = SyntheticConfig {
        duration_ns: 20_000_000_000,
        ..SyntheticConfig::default()
    };
    let data = generate(&synthetic).expect("generate short synthetic market");
    let instrument = &data.manifest.instrument;
    let lot = instrument.lot_size;
    let strategies = [
        StrategyKind::TouchMaker(TouchMakerParams {
            order_qty: 5.0 * lot,
            max_position: 40.0 * lot,
            offset_ticks: 0,
            skew_ticks_per_unit: 0.5,
        }),
        StrategyKind::ImbalanceTaker(ImbalanceTakerParams {
            order_qty: 8.0 * lot,
            max_position: 40.0 * lot,
            threshold: 0.4,
            hold_ticks: 12,
            cooldown_ticks: 4,
        }),
    ];
    for kind in strategies {
        for exchange_model in [ExchangeModel::NoPartialFill, ExchangeModel::PartialFill] {
            let config = RunConfig {
                dataset_id: data.manifest.id.clone(),
                strategy: StrategyConfig {
                    interval_ns: 5_000_000,
                    kind: kind.clone(),
                },
                execution: ExecutionConfig {
                    exchange_model,
                    queue_model: QueueModelConfig::RiskAdverse,
                    order_latency: OrderLatencyConfig::Constant {
                        entry_ns: 1_000_000,
                        response_ns: 1_000_000,
                    },
                    think_time_ns: 0,
                    feed_latency_offset_ns: 0,
                    fees: FeeConfig {
                        maker: 0.0002,
                        taker: 0.0005,
                    },
                },
                risk: RiskConfig {
                    max_position: 60.0 * lot,
                    max_order_qty: 20.0 * lot,
                    max_open_orders: 6,
                    max_drawdown: None,
                },
                record: RecordConfig::default(),
                seed: 7,
            };
            let out = simulate(
                &SimInput {
                    events: &data.events,
                    instrument: *instrument,
                    origin_ns: synthetic.origin_ns,
                    config: &config,
                },
                bb_strategies::build(&config.strategy, instrument),
            )
            .expect("simulate generated dataset");
            let context = format!("{} / {exchange_model:?}", kind.label());
            let mut by_order: BTreeMap<u64, Vec<FlightEvent>> = BTreeMap::new();
            for e in &out.tape {
                if let Some(id) = e.order_id {
                    by_order.entry(id).or_default().push(e.clone());
                }
            }
            let orders = build_orders(&out.tape);
            assert!(!orders.is_empty(), "{context}: must exercise order records");
            assert!(!out.fills.is_empty(), "{context}: must exercise fills");
            assert_eq!(
                orders.iter().map(|o| o.order_id).collect::<Vec<_>>(),
                by_order.keys().copied().collect::<Vec<_>>(),
                "{context}"
            );
            for order in &orders {
                let events = &by_order[&order.order_id];
                assert_eq!(
                    order.outcome,
                    expected_outcome(events),
                    "{context}: order {}",
                    order.order_id
                );
                let (mut filled, mut fill_count, mut fees, mut tick_qty) = (0.0, 0, 0.0, 0.0);
                for e in events {
                    if let FlightBody::ExchFill {
                        price_tick,
                        qty,
                        fee,
                        ..
                    } = e.body
                    {
                        filled += qty;
                        fill_count += 1;
                        fees += fee;
                        tick_qty += price_tick as f64 * qty;
                    }
                }
                assert!(close(order.filled_qty, filled), "{context}");
                assert!(close(order.fees, fees), "{context}");
                assert_eq!(order.fill_count, fill_count, "{context}");
                match order.avg_fill_tick {
                    Some(avg) => {
                        assert!(filled > 0.0 && close(avg, tick_qty / filled), "{context}")
                    }
                    None => assert_eq!(filled, 0.0, "{context}"),
                }
                let explanation = explain_order(
                    order,
                    events,
                    instrument,
                    &config,
                    LocalTsProvenance::Synthetic,
                );
                check_verdict(order.outcome, order.final_status, explanation.verdict);
                assert_eq!(
                    explanation.question,
                    match order.outcome {
                        OrderOutcome::Filled => Question::WhyFilled,
                        OrderOutcome::PartiallyFilled => Question::WhyPartiallyFilled,
                        _ => Question::WhyNotFilled,
                    }
                );
                assert!(!explanation.headline.is_empty(), "{context}");
                assert!(!explanation.steps.is_empty(), "{context}");
                assert!(!explanation.caveats.is_empty(), "{context}");
                assert!(
                    explanation.caveats.iter().all(|c| !c.is_empty()),
                    "{context}"
                );
                for step in &explanation.steps {
                    assert!(!step.statement.is_empty(), "{context}");
                    for seq in &step.event_seqs {
                        let event = out
                            .tape
                            .get(*seq as usize)
                            .expect("cited seq exists in tape");
                        assert_eq!(event.seq, *seq);
                        assert!(
                            event.order_id.is_none() || event.order_id == Some(order.order_id),
                            "{context}: wrong order citation"
                        );
                    }
                }
            }
            let metrics = compute_metrics(&out.tape, &orders, &out.fills, &out.equity, instrument);
            assert_eq!(metrics.fill_count, out.fills.len() as u64, "{context}");
            assert!(
                close(metrics.volume, out.fills.iter().map(|f| f.qty).sum()),
                "{context}"
            );
            assert_eq!(
                metrics.fill_count,
                orders.iter().map(|o| u64::from(o.fill_count)).sum::<u64>(),
                "{context}"
            );
            let run = assemble(&data, synthetic.origin_ns, config, out);
            assert_eq!(run.orders, orders, "{context}");
            assert_eq!(run.metrics, metrics, "{context}");
            let comparison = compare_runs(
                CompareInput {
                    run: &run,
                    label: "original",
                    instrument: Some(instrument),
                },
                CompareInput {
                    run: &run,
                    label: "self",
                    instrument: Some(instrument),
                },
            );
            assert!(comparison.eligibility.like_for_like, "{context}");
            let alignment = comparison.alignment.expect("a run aligns with itself");
            assert_eq!(alignment.identical, run.orders.len() as u64, "{context}");
            assert!(alignment.first_divergence.is_none(), "{context}");
            assert!(alignment.pairs.is_empty(), "{context}");
            assert_eq!(alignment.timing.exch_arrival.nonzero, 0, "{context}");
            assert_eq!(alignment.timing.first_fill.nonzero, 0, "{context}");
        }
    }
}

/// A stored-run-shaped result for `out`, with orders and metrics derived by this crate.
fn assemble(data: &BuiltDataset, origin_ns: i64, config: RunConfig, out: SimOutput) -> RunResult {
    let orders = build_orders(&out.tape);
    let metrics = compute_metrics(
        &out.tape,
        &orders,
        &out.fills,
        &out.equity,
        &data.manifest.instrument,
    );
    RunResult {
        manifest: RunManifest {
            contract_version: bb_core::CONTRACT_VERSION,
            run_id: "real-tape-test".into(),
            blackbox_version: env!("CARGO_PKG_VERSION").into(),
            simulation_revision: bb_core::SIMULATION_REVISION,
            engine: EngineInfo {
                name: bb_core::ENGINE_NAME.into(),
                version: bb_core::ENGINE_VERSION.into(),
                features: vec!["backtest".into()],
            },
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            profile: "debug".into(),
            dataset: DatasetRef {
                id: data.manifest.id.clone(),
                sha256: data.manifest.events_sha256.clone(),
                rows: data.events.len() as u64,
                origin_ns: origin_ns.to_string(),
            },
            config,
            config_sha256: String::new(),
            result_sha256: result_sha256(&out.tape, &out.fills, &out.equity),
            created_unix_ms: 0,
            wall_time_ms: 0,
        },
        metrics,
        engine_check: out.engine_check,
        ticks: out.ticks,
        orders,
        fills: out.fills,
        equity: out.equity,
        tape: out.tape,
    }
}

/// The touch maker on `data` with the given queue model and evaluation interval.
fn simulated_maker(
    data: &BuiltDataset,
    origin_ns: i64,
    queue_model: QueueModelConfig,
    interval_ns: i64,
) -> RunResult {
    let lot = data.manifest.instrument.lot_size;
    let config = RunConfig {
        dataset_id: data.manifest.id.clone(),
        strategy: StrategyConfig {
            interval_ns,
            kind: StrategyKind::TouchMaker(TouchMakerParams {
                order_qty: 5.0 * lot,
                max_position: 40.0 * lot,
                offset_ticks: 0,
                skew_ticks_per_unit: 0.5,
            }),
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::NoPartialFill,
            queue_model,
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: 1_000_000,
                response_ns: 1_000_000,
            },
            think_time_ns: 0,
            feed_latency_offset_ns: 0,
            fees: FeeConfig {
                maker: 0.0002,
                taker: 0.0005,
            },
        },
        risk: RiskConfig {
            max_position: 60.0 * lot,
            max_order_qty: 20.0 * lot,
            max_open_orders: 6,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 7,
    };
    let out = simulate(
        &SimInput {
            events: &data.events,
            instrument: data.manifest.instrument,
            origin_ns,
            config: &config,
        },
        bb_strategies::build(&config.strategy, &data.manifest.instrument),
    )
    .expect("simulate generated dataset");
    assemble(data, origin_ns, config, out)
}

fn compare<'a>(
    data: &'a BuiltDataset,
    a: &'a RunResult,
    b: &'a RunResult,
) -> bb_core::compare::Comparison {
    let side = |run: &'a RunResult, label: &'a str| CompareInput {
        run,
        label,
        instrument: Some(&data.manifest.instrument),
    };
    compare_runs(side(a, "A"), side(b, "B"))
}

/// Each order's lifecycle without tape positions: what "the same recorded lifecycle" means.
type Lifecycle = Vec<(i64, Clock, Source, FlightBody)>;

fn lifecycles(run: &RunResult) -> BTreeMap<u64, Lifecycle> {
    let mut out: BTreeMap<u64, Lifecycle> = BTreeMap::new();
    for e in &run.tape {
        if let Some(id) = e.order_id {
            out.entry(id)
                .or_default()
                .push((e.ts, e.clock, e.source, e.body.clone()));
        }
    }
    out
}

/// AT-2 on recorder output: a queue-model switch (BASELINE_AUDIT §5.1). Re-derives, independently
/// of the classifier, that a pair is `identical` exactly when the two order records (apart from
/// tape positions) and lifecycles are equal, and so that every pair before the first divergence
/// really is identical, timing included.
#[test]
fn queue_model_switch_never_hides_a_difference_before_the_first_divergence() {
    let synthetic = SyntheticConfig {
        duration_ns: 20_000_000_000,
        ..SyntheticConfig::default()
    };
    let data = generate(&synthetic).expect("generate short synthetic market");
    let a = simulated_maker(
        &data,
        synthetic.origin_ns,
        QueueModelConfig::PowerProb { n: 3.0 },
        5_000_000,
    );
    let b = simulated_maker(
        &data,
        synthetic.origin_ns,
        QueueModelConfig::RiskAdverse,
        5_000_000,
    );
    let c = compare(&data, &a, &b);
    assert!(c.eligibility.like_for_like);
    let al = c
        .alignment
        .as_ref()
        .expect("same dataset and grid: aligned");
    let first = al
        .first_divergence
        .as_ref()
        .expect("the queue model changes at least one recorded lifecycle");

    let (la, lb) = (lifecycles(&a), lifecycles(&b));
    let strip = |o: &OrderRecord| OrderRecord {
        first_seq: 0,
        last_seq: 0,
        ..o.clone()
    };
    let ids_a: BTreeMap<u64, &OrderRecord> = a.orders.iter().map(|o| (o.order_id, o)).collect();
    let ids_b: BTreeMap<u64, &OrderRecord> = b.orders.iter().map(|o| (o.order_id, o)).collect();
    let listed: BTreeMap<u64, PairClass> = al.pairs.iter().map(|p| (p.order_id, p.class)).collect();
    let horizon = if al.pairs_truncated {
        al.pairs.last().unwrap().order_id
    } else {
        u64::MAX
    };
    let all: BTreeSet<u64> = ids_a.keys().chain(ids_b.keys()).copied().collect();
    let mut checked = 0;
    for id in all.into_iter().take_while(|&id| id <= horizon) {
        let same = match (ids_a.get(&id), ids_b.get(&id)) {
            (Some(x), Some(y)) => strip(x) == strip(y) && la[&id] == lb[&id],
            _ => false,
        };
        assert_eq!(same, !listed.contains_key(&id), "order {id}");
        if id < first.order_id {
            assert!(same, "order {id} precedes the first divergence but differs");
        }
        checked += 1;
    }
    assert!(checked > 0);
    let total = al.identical
        + al.same_fills_different_history
        + al.same_fills_different_fees
        + al.same_outcome_different_timing
        + al.same_decision_different_outcome
        + al.different_decision
        + al.only_a
        + al.only_b;
    assert_eq!(
        total,
        ids_a
            .keys()
            .chain(ids_b.keys())
            .collect::<BTreeSet<_>>()
            .len() as u64
    );
    // Only a fee change could produce this class; the queue model never does.
    assert_eq!(al.same_fills_different_fees, 0);
}

/// AT-1 on recorder output: a 5 ms versus 10 ms evaluation interval is refused alignment.
#[test]
fn different_intervals_on_real_tapes_refuse_alignment() {
    let synthetic = SyntheticConfig {
        duration_ns: 5_000_000_000,
        ..SyntheticConfig::default()
    };
    let data = generate(&synthetic).expect("generate short synthetic market");
    let a = simulated_maker(
        &data,
        synthetic.origin_ns,
        QueueModelConfig::RiskAdverse,
        5_000_000,
    );
    let b = simulated_maker(
        &data,
        synthetic.origin_ns,
        QueueModelConfig::RiskAdverse,
        10_000_000,
    );
    let c = compare(&data, &a, &b);
    assert!(c.alignment.is_none());
    assert_eq!(
        c.eligibility
            .reasons
            .iter()
            .map(|r| (r.field.as_str(), r.effect))
            .collect::<Vec<_>>(),
        [("strategy.interval_ns", EligibilityEffect::NoAlignment)]
    );
    assert!(!c.metrics.is_empty());
}
