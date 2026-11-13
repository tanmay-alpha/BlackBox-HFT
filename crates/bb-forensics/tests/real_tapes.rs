//! Exercise the forensics contract against real recorder output, not hand-authored events.

use std::collections::BTreeMap;

use bb_core::{
    config::{ExchangeModel, ExecutionConfig, FeeConfig, ImbalanceTakerParams, OrderLatencyConfig,
        QueueModelConfig, RecordConfig, RiskConfig, RunConfig, StrategyConfig, StrategyKind, TouchMakerParams},
    dataset::LocalTsProvenance,
    explain::{Question, Verdict},
    flight::{FlightBody, FlightEvent},
    result::{DatasetRef, EngineInfo, OrderOutcome, RunManifest, RunResult},
    Clock, OrderStatus, ReqKind,
};
use bb_data::synthetic::{generate, SyntheticConfig};
use bb_forensics::{build_orders, compare_runs, compute_metrics, explain_order, result_sha256};
use bb_sim::{simulate, SimInput};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn expected_outcome(events: &[FlightEvent]) -> OrderOutcome {
    if events.iter().any(|e| matches!(e.body, FlightBody::RiskBlocked { .. })) {
        return OrderOutcome::RiskBlocked;
    }
    if events.iter().any(|e| matches!(e.body, FlightBody::RequestRefused { req: ReqKind::New, .. })) {
        return OrderOutcome::Refused;
    }
    let status = events.iter().filter_map(|e| match e.body {
        FlightBody::ExchResponse { status, request_rejected: false, .. } => Some(status),
        _ => None,
    }).last();
    let has_fills = events.iter().any(|e| matches!(e.body, FlightBody::ExchFill { .. }));
    match (has_fills, status) {
        (true, Some(OrderStatus::Filled)) => OrderOutcome::Filled,
        (true, _) => OrderOutcome::PartiallyFilled,
        (false, Some(OrderStatus::Canceled)) => OrderOutcome::Canceled,
        (false, Some(OrderStatus::Expired)) => OrderOutcome::Expired,
        (false, _) if events.iter().any(|e| e.clock == Clock::Exchange) => OrderOutcome::WorkingAtEnd,
        _ => OrderOutcome::InFlightAtEnd,
    }
}

fn check_verdict(outcome: OrderOutcome, status: OrderStatus, verdict: Verdict) {
    match outcome {
        OrderOutcome::Filled => assert!(matches!(verdict, Verdict::FilledQueueExhausted | Verdict::FilledPriceCrossed | Verdict::FilledTakerOnArrival | Verdict::FilledMixed)),
        OrderOutcome::PartiallyFilled => match status {
            OrderStatus::Canceled => assert_eq!(verdict, Verdict::NotFilledCanceled),
            OrderStatus::Expired => assert_eq!(verdict, Verdict::NotFilledNoLiquidityOnArrival),
            _ => assert_eq!(verdict, Verdict::NotFilledWorkingAtEnd),
        },
        OrderOutcome::Canceled => assert_eq!(verdict, Verdict::NotFilledCanceled),
        OrderOutcome::Expired => assert!(matches!(verdict, Verdict::NotFilledPostOnlyWouldCross | Verdict::NotFilledNoLiquidityOnArrival)),
        OrderOutcome::WorkingAtEnd => assert!(matches!(verdict, Verdict::NotFilledQueueNotReached | Verdict::NotFilledNoTradeAtPrice | Verdict::NotFilledWorkingAtEnd)),
        OrderOutcome::InFlightAtEnd => assert_eq!(verdict, Verdict::NotFilledDataEndedInFlight),
        OrderOutcome::RiskBlocked => assert_eq!(verdict, Verdict::NotFilledRiskBlocked),
        OrderOutcome::Refused => assert_eq!(verdict, Verdict::NotFilledRefused),
    }
}

#[test]
fn synthetic_recorder_tapes_support_orders_explanations_metrics_and_self_comparison() {
    let synthetic = SyntheticConfig { duration_ns: 20_000_000_000, ..SyntheticConfig::default() };
    let data = generate(&synthetic).expect("generate short synthetic market");
    let instrument = &data.manifest.instrument;
    let lot = instrument.lot_size;
    let strategies = [
        StrategyKind::TouchMaker(TouchMakerParams {
            order_qty: 5.0 * lot, max_position: 40.0 * lot,
            offset_ticks: 0, skew_ticks_per_unit: 0.5,
        }),
        StrategyKind::ImbalanceTaker(ImbalanceTakerParams {
            order_qty: 8.0 * lot, max_position: 40.0 * lot,
            threshold: 0.4, hold_ticks: 12, cooldown_ticks: 4,
        }),
    ];
    for kind in strategies {
        for exchange_model in [ExchangeModel::NoPartialFill, ExchangeModel::PartialFill] {
            let config = RunConfig {
                dataset_id: data.manifest.id.clone(),
                strategy: StrategyConfig { interval_ns: 5_000_000, kind: kind.clone() },
                execution: ExecutionConfig {
                    exchange_model, queue_model: QueueModelConfig::RiskAdverse,
                    order_latency: OrderLatencyConfig::Constant { entry_ns: 1_000_000, response_ns: 1_000_000 },
                    feed_latency_offset_ns: 0, fees: FeeConfig { maker: 0.0002, taker: 0.0005 },
                },
                risk: RiskConfig { max_position: 60.0 * lot, max_order_qty: 20.0 * lot, max_open_orders: 6, max_drawdown: None },
                record: RecordConfig::default(), seed: 7,
            };
            let out = simulate(&SimInput {
                events: &data.events, instrument: *instrument, origin_ns: synthetic.origin_ns, config: &config,
            }, bb_strategies::build(&config.strategy, instrument)).expect("simulate generated dataset");
            let context = format!("{} / {exchange_model:?}", kind.label());
            let mut by_order: BTreeMap<u64, Vec<FlightEvent>> = BTreeMap::new();
            for e in &out.tape {
                if let Some(id) = e.order_id { by_order.entry(id).or_default().push(e.clone()); }
            }
            let orders = build_orders(&out.tape);
            assert!(!orders.is_empty(), "{context}: must exercise order records");
            assert!(!out.fills.is_empty(), "{context}: must exercise fills");
            assert_eq!(orders.iter().map(|o| o.order_id).collect::<Vec<_>>(), by_order.keys().copied().collect::<Vec<_>>(), "{context}");
            for order in &orders {
                let events = &by_order[&order.order_id];
                assert_eq!(order.outcome, expected_outcome(events), "{context}: order {}", order.order_id);
                let (mut filled, mut fill_count, mut fees, mut tick_qty) = (0.0, 0, 0.0, 0.0);
                for e in events {
                    if let FlightBody::ExchFill { price_tick, qty, fee, .. } = e.body {
                        filled += qty; fill_count += 1; fees += fee; tick_qty += price_tick as f64 * qty;
                    }
                }
                assert!(close(order.filled_qty, filled), "{context}");
                assert!(close(order.fees, fees), "{context}");
                assert_eq!(order.fill_count, fill_count, "{context}");
                match order.avg_fill_tick {
                    Some(avg) => assert!(filled > 0.0 && close(avg, tick_qty / filled), "{context}"),
                    None => assert_eq!(filled, 0.0, "{context}"),
                }
                let explanation = explain_order(order, events, instrument, &config, LocalTsProvenance::Synthetic);
                check_verdict(order.outcome, order.final_status, explanation.verdict);
                assert_eq!(explanation.question, match order.outcome {
                    OrderOutcome::Filled => Question::WhyFilled,
                    OrderOutcome::PartiallyFilled => Question::WhyPartiallyFilled,
                    _ => Question::WhyNotFilled,
                });
                assert!(!explanation.headline.is_empty(), "{context}");
                assert!(!explanation.steps.is_empty(), "{context}");
                assert!(!explanation.caveats.is_empty(), "{context}");
                assert!(explanation.caveats.iter().all(|c| !c.is_empty()), "{context}");
                for step in &explanation.steps {
                    assert!(!step.statement.is_empty(), "{context}");
                    for seq in &step.event_seqs {
                        let event = out.tape.get(*seq as usize).expect("cited seq exists in tape");
                        assert_eq!(event.seq, *seq);
                        assert!(event.order_id.is_none() || event.order_id == Some(order.order_id), "{context}: wrong order citation");
                    }
                }
            }
            let metrics = compute_metrics(&out.tape, &orders, &out.fills, &out.equity, instrument);
            assert_eq!(metrics.fill_count, out.fills.len() as u64, "{context}");
            assert!(close(metrics.volume, out.fills.iter().map(|f| f.qty).sum()), "{context}");
            assert_eq!(metrics.fill_count, orders.iter().map(|o| u64::from(o.fill_count)).sum::<u64>(), "{context}");
            let run = RunResult {
                manifest: RunManifest {
                    contract_version: bb_core::CONTRACT_VERSION,
                    run_id: "real-tape-test".into(), blackbox_version: env!("CARGO_PKG_VERSION").into(),
                    engine: EngineInfo { name: bb_core::ENGINE_NAME.into(), version: bb_core::ENGINE_VERSION.into(), features: vec!["backtest".into()] },
                    platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH), profile: "debug".into(),
                    dataset: DatasetRef { id: data.manifest.id.clone(), sha256: data.manifest.events_sha256.clone(), rows: data.events.len() as u64, origin_ns: synthetic.origin_ns.to_string() },
                    config, config_sha256: String::new(), result_sha256: result_sha256(&out.tape, &out.fills, &out.equity), created_unix_ms: 0, wall_time_ms: 0,
                },
                metrics, engine_check: out.engine_check, ticks: out.ticks, orders,
                fills: out.fills, equity: out.equity, tape: out.tape,
            };
            let comparison = compare_runs(&run, "original", &run, "self");
            assert_eq!(comparison.alignment.identical, run.orders.len() as u64, "{context}");
            assert_eq!((comparison.alignment.same_decision_different_outcome, comparison.alignment.different_decision, comparison.alignment.only_a, comparison.alignment.only_b), (0, 0, 0, 0), "{context}");
            assert!(comparison.alignment.first_divergence.is_none(), "{context}");
            assert!(comparison.alignment.pairs.is_empty(), "{context}");
        }
    }
}
