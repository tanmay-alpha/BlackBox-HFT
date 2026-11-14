//! Generated data only: exercise the real preset -> engine -> tape -> analytics pipeline.
use bb_app::{datasets::DatasetCtx, pipeline, presets};
use bb_core::time::NS_PER_S;
use bb_data::synthetic::SyntheticConfig;

fn dataset(seconds: i64) -> (tempfile::TempDir, DatasetCtx) {
    let directory = tempfile::tempdir().unwrap();
    let config = SyntheticConfig {
        id: "analytics-fixture".into(),
        duration_ns: seconds * NS_PER_S,
        ..SyntheticConfig::default()
    };
    let built = bb_data::synthetic::generate(&config).unwrap();
    bb_data::store::save(directory.path(), &built).unwrap();
    let context = DatasetCtx::load(directory.path(), &config.id).unwrap();
    (directory, context)
}

#[test]
fn preset_pipeline_records_real_fills_and_serializable_risk() {
    let (_directory, context) = dataset(30);
    let config = presets::find(context.manifest(), "maker_baseline")
        .unwrap()
        .config;
    let run = pipeline::execute(&context, &config).unwrap();
    assert!(!run.fills.is_empty());
    let last = run.equity.last().unwrap();
    let report = bb_analytics::risk::compute_risk(
        &run.equity,
        &run.fills,
        &context.manifest().instrument,
        last.equity,
        &bb_analytics::risk::RiskConfig::default(),
    );
    assert!(serde_json::to_value(&report).unwrap().is_object());
    assert!(report.bucket_sharpe.get().is_none());
}

#[test]
fn execution_identities_reconcile_on_generated_preset() {
    let (_directory, context) = dataset(30);
    let config = presets::find(context.manifest(), "maker_baseline")
        .unwrap()
        .config;
    let run = pipeline::execute(&context, &config).unwrap();
    let report = bb_analytics::execution::compute_execution(
        &run,
        &context.bbo,
        &context.manifest().instrument,
        &bb_analytics::execution::ExecutionConfig::default(),
    )
    .unwrap();
    let mut identities = 0;
    for split in [
        &report.spreads.all,
        &report.spreads.maker,
        &report.spreads.taker,
    ] {
        for horizon in &split.horizons {
            if let (Some(es), Some(rs), Some(adverse)) = (
                horizon.matched_effective_half.price.get(),
                horizon.realised_half.price.get(),
                horizon.adverse_selection.price.get(),
            ) {
                // E3 is adverse-selection cost. E1 = E2 - E3 = E2 + favourable drift.
                assert!((es - (rs - adverse)).abs() < 1e-9);
                identities += 1;
            }
        }
    }
    assert!(identities > 0);
    for order in &report.orders {
        for horizon in &order.spreads.all.horizons {
            if let (Some(es), Some(rs), Some(adverse)) = (
                horizon.matched_effective_half.price.get(),
                horizon.realised_half.price.get(),
                horizon.adverse_selection.price.get(),
            ) {
                assert!((es - (rs - adverse)).abs() < 1e-9);
            }
        }
    }
    let maker = report.pnl.maker.net.get().unwrap();
    let taker = report.pnl.taker.net.get().unwrap();
    let net = report.pnl.all.net.get().unwrap();
    assert!((maker + taker - net).abs() < 1e-9);
    let final_mid = report.exchange_end_mid.get().unwrap();
    let ledger = run.equity.last().unwrap();
    // Re-mark the actual ledger at the SAME exchange-clock mark used by E9.
    let remarked_ledger = ledger.cash
        + ledger.position * final_mid * context.manifest().instrument.contract_size
        - ledger.fees;
    assert!(
        (net - remarked_ledger).abs() < 1e-9,
        "{net} != {remarked_ledger}"
    );
    assert!(serde_json::to_value(&report).unwrap().is_object());
}

#[test]
fn streaming_emits_every_order_and_preserves_every_run_summary() {
    let (_directory, context) = dataset(30);
    let config = presets::find(context.manifest(), "maker_baseline")
        .unwrap()
        .config;
    let run = pipeline::execute(&context, &config).unwrap();
    let analytics_config = bb_analytics::execution::ExecutionConfig::default();
    let mut collected = bb_analytics::execution::compute_execution(
        &run,
        &context.bbo,
        &context.manifest().instrument,
        &analytics_config,
    )
    .unwrap();
    let mut emitted = 0;
    let mut streamed = bb_analytics::execution::compute_execution_streaming(
        &run,
        &context.bbo,
        &context.manifest().instrument,
        &analytics_config,
        |order| {
            let expected = &collected.orders[emitted];
            assert_eq!(
                serde_json::to_value(&order).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            emitted += 1;
        },
    )
    .unwrap();
    assert_eq!(emitted, run.orders.len());
    assert_eq!(streamed.order_count as usize, emitted);
    assert!(streamed.order_details_streamed);
    assert!(streamed.orders.is_empty());
    // Only the explicitly advertised detail retention mode differs; all full-population
    // distributions, censor counts, KM curves, fees and P&L must remain byte-identical.
    collected.orders.clear();
    streamed.order_details_streamed = false;
    assert_eq!(
        serde_json::to_value(&collected).unwrap(),
        serde_json::to_value(&streamed).unwrap()
    );
}
