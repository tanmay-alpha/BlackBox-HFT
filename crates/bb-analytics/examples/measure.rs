//! Reproduce the validation workload with `cargo run --release -p bb-analytics --example measure`.
//! This is a measurement harness, not a statistical performance benchmark.
use bb_app::{datasets::DatasetCtx, pipeline, presets};
use bb_core::time::NS_PER_S;
use bb_data::synthetic::SyntheticConfig;
use std::time::Instant;

fn main() {
    let directory = tempfile::tempdir().expect("measurement dataset directory");
    let generated = bb_data::synthetic::generate(&SyntheticConfig {
        id: "analytics-measurement".into(),
        duration_ns: 600 * NS_PER_S,
        ..SyntheticConfig::default()
    })
    .expect("generate 600s dataset");
    bb_data::store::save(directory.path(), &generated).expect("save dataset");
    drop(generated);
    let context =
        DatasetCtx::load(directory.path(), "analytics-measurement").expect("load dataset");
    let config = presets::find(context.manifest(), "maker_baseline")
        .expect("preset")
        .config;
    let run = pipeline::execute(&context, &config).expect("execute preset");
    println!(
        "dataset_seconds=600 rows={} tape_events={} fills={} equity_samples={}",
        context.dataset.events.len(),
        run.tape.len(),
        run.fills.len(),
        run.equity.len()
    );
    let market_config = bb_analytics::market::MarketConfig {
        series_max_points: Some(1_024),
        liquidity_windows: [bb_core::Side::Buy, bb_core::Side::Sell]
            .into_iter()
            .map(|side| bb_analytics::market::LiquidityWindow {
                start_ns: NS_PER_S,
                end_ns: 10 * NS_PER_S,
                side,
                band: bb_analytics::market::LiquidityBand::BasisPoints { bps: 100 },
                trade_match_lag_ns: 0,
                max_matched_trades: 10_000,
            })
            .collect(),
        ..bb_analytics::market::MarketConfig::default()
    };
    let mut measurements_ms = Vec::with_capacity(3);
    for iteration in 1..=3 {
        let start = Instant::now();
        let market = bb_analytics::market::compute_market(
            context.dataset.events.as_slice(),
            &context.manifest().instrument,
            context.dataset.origin_ns,
            &market_config,
        )
        .expect("market analytics");
        let mut emitted_orders = 0usize;
        let execution = bb_analytics::execution::compute_execution_streaming(
            &run,
            &context.bbo,
            &context.manifest().instrument,
            &bb_analytics::execution::ExecutionConfig::default(),
            |order| {
                emitted_orders += 1;
                std::hint::black_box(order);
            },
        )
        .expect("execution analytics");
        assert_eq!(emitted_orders, run.orders.len());
        let risk = bb_analytics::risk::compute_risk(
            &run.equity,
            &run.fills,
            &context.manifest().instrument,
            execution
                .pnl
                .all
                .net
                .get()
                .expect("complete P&L decomposition"),
            &bb_analytics::risk::RiskConfig::default(),
        );
        std::hint::black_box((&market, &execution, &risk));
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        measurements_ms.push(elapsed_ms);
        println!("analytics_iteration={iteration} elapsed_ms={elapsed_ms:.3}");
    }
    println!(
        "analytics_best_of_3_ms={:.3}",
        measurements_ms.into_iter().fold(f64::INFINITY, f64::min)
    );
}
