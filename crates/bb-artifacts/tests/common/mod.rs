#![allow(dead_code)]
use bb_core::{
    Clock, Source,
    flight::{FlightBody, FlightEvent},
    result::*,
};
use serde_json::json;

pub fn event(seq: u64) -> FlightEvent {
    FlightEvent {
        seq,
        ts: seq as i64 * 10 - 50,
        clock: if seq.is_multiple_of(2) {
            Clock::Local
        } else {
            Clock::Exchange
        },
        source: Source::Recorder,
        order_id: None,
        body: FlightBody::QueueTruncated { dropped: seq },
    }
}
pub fn equity(i: u64) -> EquitySample {
    EquitySample {
        ts: i as i64 * 10,
        tick_no: i,
        mid_tick: Some(1.5),
        position: 0.0,
        known_position: 0.0,
        cash: 0.0,
        fees: 0.0,
        realized_pnl: 0.0,
        unrealized_pnl: 0.0,
        equity: 0.0,
        open_orders: 0,
    }
}
pub fn fill(i: u64) -> FillRecord {
    FillRecord {
        seq: i,
        order_id: i,
        ts: i as i64 * 10,
        side: bb_core::Side::Buy,
        price_tick: 100,
        qty: 0.1,
        maker: true,
        fee: 0.0001,
        value: 10.0,
        position_after: 0.1,
        realized_pnl: 0.0,
        mid_tick: Some(100.5),
        markout_ticks: vec![None],
    }
}
pub fn order(i: u64) -> OrderRecord {
    OrderRecord {
        order_id: i,
        tick_no: i,
        slot: 0,
        side: bb_core::Side::Buy,
        price_tick: 100,
        qty: 0.1,
        tif: bb_core::Tif::Gtc,
        ord_type: bb_core::OrdType::Limit,
        reason: "test".into(),
        outcome: OrderOutcome::Filled,
        decision_ts: i as i64,
        exch_arrival_ts: Some(i as i64 + 1),
        first_response_ts: None,
        queue_ahead_at_entry: None,
        min_queue_ahead: None,
        traded_at_price_qty: 0.1,
        filled_qty: 0.1,
        maker_qty: 0.1,
        taker_qty: 0.0,
        avg_fill_tick: Some(100.0),
        fees: 0.0001,
        fill_count: 1,
        first_fill_ts: None,
        last_fill_ts: None,
        cancel_sent_ts: None,
        cancel_reason: None,
        cancel_rejected: false,
        terminal_ts: None,
        final_status: bb_core::OrderStatus::Filled,
        first_seq: i,
        last_seq: i,
    }
}
pub fn run(n: u64) -> RunResult {
    let tape: Vec<_> = (0..n).map(event).collect();
    let fills: Vec<_> = (0..3).map(fill).collect();
    let equity: Vec<_> = (0..7).map(equity).collect();
    let mut metrics = serde_json::Map::new();
    for name in [
        "orders_intended",
        "orders_risk_blocked",
        "orders_refused",
        "orders_submitted",
        "orders_filled",
        "orders_partially_filled",
        "orders_canceled",
        "orders_expired",
        "orders_working_at_end",
        "cancels_sent",
        "cancels_rejected",
        "fill_count",
        "maker_fill_count",
        "taker_fill_count",
        "risk_blocks",
        "volume",
        "turnover",
        "fees",
        "final_position",
        "max_long",
        "max_short",
        "mean_abs_position",
        "realized_pnl",
        "unrealized_pnl",
        "net_pnl",
        "max_drawdown",
        "spread_capture",
    ] {
        metrics.insert(name.into(), json!(0));
    }
    for name in [
        "fill_ratio_orders",
        "fill_ratio_qty",
        "order_age_ns",
        "time_to_first_fill_ns",
        "entry_latency_ns",
        "response_latency_ns",
        "round_trip_ns",
        "spread_capture_ticks",
    ] {
        metrics.insert(name.into(), json!(null));
    }
    metrics.insert("markouts".into(), json!([]));
    metrics.insert("risk_halted".into(), json!(false));
    let config = serde_json::from_value(json!({
        "dataset_id":"test", "strategy":{"interval_ns":1000,"kind":{"type":"scripted","orders":[]}},
        "execution":{"exchange_model":"no_partial_fill","queue_model":{"type":"risk_adverse"},
            "order_latency":{"type":"constant","entry_ns":1,"response_ns":1},"feed_latency_offset_ns":0,"fees":{"maker":0.0,"taker":0.0}},
        "risk":{"max_position":1.0,"max_order_qty":1.0,"max_open_orders":10,"max_drawdown":null},
        "record":{"sample_every_ticks":1,"max_queue_updates_per_order":2000},"seed":1
    })).unwrap();
    RunResult {
        manifest: RunManifest {
            contract_version: 1,
            run_id: "a".repeat(16),
            blackbox_version: "test".into(),
            simulation_revision: 2,
            engine: EngineInfo {
                name: "test".into(),
                version: "1".into(),
                features: vec![],
            },
            platform: "test-x86_64".into(),
            profile: "debug".into(),
            dataset: DatasetRef {
                id: "test".into(),
                sha256: "b".repeat(64),
                rows: 0,
                origin_ns: "0".into(),
            },
            config,
            config_sha256: "a".repeat(64),
            result_sha256: bb_core::hash::sha256_json(&(&tape, &fills, &equity)),
            created_unix_ms: 0,
            wall_time_ms: 0,
        },
        metrics: serde_json::from_value(metrics.into()).unwrap(),
        engine_check: EngineStateCheck {
            agrees: true,
            ledger_position: 0.0,
            engine_position: 0.0,
            ledger_cash: 0.0,
            engine_balance: 0.0,
            ledger_fees: 0.0,
            engine_fee: 0.0,
            ledger_fills: 0,
            engine_num_trades: 0,
            note: String::new(),
        },
        ticks: 7,
        orders: (0..4).map(order).collect(),
        fills,
        equity,
        tape,
    }
}
