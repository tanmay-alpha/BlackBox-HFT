//! Defects in the pinned engine (hftbacktest 0.9.4) that BlackBox works around or avoids.
//!
//! Each `upstream_*` test drives the *unwrapped* engine through its public builder and asserts the
//! defective behaviour. They exist so the workarounds rest on evidence, and so that a future
//! engine upgrade which fixes a defect fails here and prompts removal of the workaround. Each is
//! paired with a test showing the adapter's behaviour for the same situation.

mod common;

use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

use bb_core::{Side, Tif, config::ExchangeModel, strategy::order_id};
use common::*;
use hftbacktest::{
    backtest::{
        Backtest, BacktestError, ExchangeKind, L2AssetBuilder,
        assettype::LinearAsset,
        data::{Data, DataSource},
        models::{
            CommonFees, ConstantLatency, FeeModel, RiskAdverseQueueModel, TradingValueFeeModel,
        },
    },
    depth::HashMapMarketDepth,
    types::{Bot, ElapseResult, OrdType, Order, OrderRequest, Side as HSide, Status, TimeInForce},
};

type Depth = HashMapMarketDepth;

/// The stock engine, assembled with its own builder exactly as its documentation shows, with no
/// BlackBox wrapper anywhere.
fn raw_engine_with<F>(m: &Mkt, exchange: ExchangeKind, fee_model: F) -> Backtest<Depth>
where
    F: FeeModel + Clone + 'static,
{
    let asset = L2AssetBuilder::new()
        .data(vec![DataSource::Data(Data::from_data(&m.rows))])
        .latency_model(ConstantLatency::new(ENTRY, RESPONSE))
        .asset_type(LinearAsset::new(INST.contract_size))
        .fee_model(fee_model)
        .exchange(exchange)
        .queue_model(RiskAdverseQueueModel::<Depth>::new())
        .depth(|| Depth::new(INST.tick_size, INST.lot_size))
        .build()
        .expect("asset builds");
    Backtest::builder()
        .add_asset(asset)
        .build()
        .expect("backtest builds")
}

fn stock_fees() -> TradingValueFeeModel<CommonFees> {
    TradingValueFeeModel::new(CommonFees::new(MAKER_FEE, TAKER_FEE))
}

fn raw_engine(m: &Mkt, exchange: ExchangeKind) -> Backtest<Depth> {
    raw_engine_with(m, exchange, stock_fees())
}

/// Runs the raw engine to the end of data, returning the first error if any.
fn drain(hbt: &mut Backtest<Depth>) -> Result<(), BacktestError> {
    loop {
        if hbt.elapse(INTERVAL)? == ElapseResult::EndOfData {
            return Ok(());
        }
    }
}

/// Queue of 5 ahead of a 3-lot buy at 100.0. A trade of 6 fills 1, a trade of 2 fills exactly the
/// remaining 2, then one more trade prints at 100.0.
fn exact_completion_market() -> Mkt {
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 6.0)
        .bid(20 * MS, 100.0, 4.0)
        .trade(30 * MS, Side::Sell, 100.0, 2.0)
        .bid(30 * MS, 100.0, 2.0)
        .trade(40 * MS, Side::Sell, 100.0, 1.0)
        .bid(40 * MS, 100.0, 1.0)
        .keepalive(80 * MS);
    m
}

#[test]
fn upstream_partial_fill_exchange_aborts_after_an_exact_completion() {
    let m = exact_completion_market();
    let mut hbt = raw_engine(&m, ExchangeKind::PartialFillExchange);
    assert_eq!(hbt.elapse(INTERVAL).unwrap(), ElapseResult::Ok);
    hbt.submit_buy_order(0, 1, 100.0, 3.0, TimeInForce::GTC, OrdType::Limit, false)
        .unwrap();
    let err = drain(&mut hbt).expect_err("0.9.4 aborts on the trade after an exact completion");
    assert!(matches!(err, BacktestError::InvalidOrderStatus), "{err:?}");
}

#[test]
fn adapter_survives_an_exact_completion() {
    let m = exact_completion_market();
    let mut cfg = config();
    cfg.execution.exchange_model = ExchangeModel::PartialFill;
    let out = try_run(&m, &cfg, vec![(0, buy(0, 100.0, 3.0, Tif::Gtc))])
        .expect("the adapter's queue wrapper routes the exchange through its removal branch");
    let fills: Vec<(i64, f64)> = out.fills.iter().map(|f| (f.ts, f.qty)).collect();
    // The same two executions; the third trade finds no order left to fill.
    assert_eq!(fills, [(20 * MS, 1.0), (30 * MS, 2.0)]);
    assert_eq!(out.equity.last().unwrap().position, 3.0);
    let ev = of(&out, order_id(0, 0));
    assert_eq!(ev.last().unwrap().body.type_name(), "local_response");
}

/// The workaround must not change what gets executed when a trade overshoots the remainder or
/// falls short of it.
#[test]
fn adapter_completion_workaround_leaves_other_cases_unchanged() {
    let mut cfg = config();
    cfg.execution.exchange_model = ExchangeModel::PartialFill;

    // Overshoot: 5 ahead, trade of 9 -> 4 executable against a 3-lot order -> fills 3.
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 9.0)
        .bid(20 * MS, 100.0, 1.0)
        .keepalive(80 * MS);
    let out = run(&m, &cfg, vec![(0, buy(0, 100.0, 3.0, Tif::Gtc))]);
    assert_eq!(out.fills.iter().map(|f| f.qty).collect::<Vec<_>>(), [3.0]);

    // Shortfall: trade of 7 -> 2 executable -> fills 2, 1 keeps resting.
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 7.0)
        .bid(20 * MS, 100.0, 3.0)
        .keepalive(80 * MS);
    let out = run(&m, &cfg, vec![(0, buy(0, 100.0, 3.0, Tif::Gtc))]);
    assert_eq!(out.fills.iter().map(|f| f.qty).collect::<Vec<_>>(), [2.0]);
    assert_eq!(out.equity.last().unwrap().open_orders, 1);
}

#[test]
fn upstream_submit_order_ignores_the_requested_side() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let mut hbt = raw_engine(&m, ExchangeKind::NoPartialFillExchange);
    hbt.elapse(INTERVAL).unwrap();
    hbt.submit_order(
        0,
        OrderRequest {
            order_id: 7,
            price: 100.0,
            qty: 1.0,
            side: HSide::Buy,
            time_in_force: TimeInForce::GTC,
            order_type: OrdType::Limit,
        },
        false,
    )
    .unwrap();
    // A buy was requested; 0.9.4 records a sell. The adapter never calls this method.
    assert_eq!(hbt.orders(0)[&7].side, HSide::Sell);
}

#[test]
fn upstream_local_account_undercounts_partial_fills() {
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 6.0)
        .bid(20 * MS, 100.0, 4.0)
        .trade(30 * MS, Side::Sell, 100.0, 3.0)
        .bid(30 * MS, 100.0, 1.0)
        .keepalive(80 * MS);
    let mut hbt = raw_engine(&m, ExchangeKind::PartialFillExchange);
    hbt.elapse(INTERVAL).unwrap();
    hbt.submit_buy_order(0, 1, 100.0, 3.0, TimeInForce::GTC, OrdType::Limit, false)
        .unwrap();
    drain(&mut hbt).unwrap();
    // The order is completely filled (1 then 2) ...
    assert_eq!(hbt.orders(0)[&1].status, Status::Filled);
    // ... but the engine's local account only applied the final response's 2.
    assert_eq!(hbt.position(0), 2.0);
    assert_eq!(hbt.state_values(0).num_trades, 1);
}

/// A fee model that notes which order each execution belongs to, in the order the engine reports
/// them.
#[derive(Clone)]
struct NotingFee {
    inner: TradingValueFeeModel<CommonFees>,
    seen: Rc<RefCell<Vec<u64>>>,
}

impl FeeModel for NotingFee {
    fn amount(&self, order: &Order, amount: f64) -> f64 {
        self.seen.borrow_mut().push(order.order_id);
        self.inner.amount(order, amount)
    }
}

/// One market event fills eight resting orders. The unwrapped engine walks a randomly seeded
/// `HashMap` to find them, so the order in which it executes them changes from run to run. This
/// is why the recorder commits each engine step sorted by order id (see `tests/determinism.rs`).
#[test]
fn upstream_simultaneous_fill_order_is_not_reproducible() {
    let mut m = Mkt::standard();
    m.trade(40 * MS, Side::Sell, 90.0, 1.0).keepalive(120 * MS);
    let mut sequences: BTreeSet<Vec<u64>> = BTreeSet::new();
    for _ in 0..40 {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut hbt = raw_engine_with(
            &m,
            ExchangeKind::NoPartialFillExchange,
            NotingFee {
                inner: stock_fees(),
                seen: seen.clone(),
            },
        );
        hbt.elapse(INTERVAL).unwrap();
        for id in 1..=8u64 {
            let px = [100.0, 99.5, 99.0][id as usize % 3];
            hbt.submit_buy_order(0, id, px, 1.0, TimeInForce::GTC, OrdType::Limit, false)
                .unwrap();
        }
        drain(&mut hbt).unwrap();
        // The exchange side reports its eight executions first, in its iteration order.
        let exchange_order: Vec<u64> = seen.borrow()[..8].to_vec();
        let mut sorted = exchange_order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (1..=8).collect::<Vec<u64>>());
        sequences.insert(exchange_order);
    }
    assert!(
        sequences.len() > 1,
        "expected run-to-run variation in execution order, saw only {sequences:?}"
    );
}

/// Defect 5a: `NoPartialFillExchange` fills a market buy at `best_ask_tick()` without checking
/// that an ask exists. On an empty ask side that is the sentinel `i64::MAX`.
#[test]
fn upstream_market_buy_on_empty_ask_fills_at_the_sentinel_tick() {
    let mut m = Mkt::default();
    m.bid(0, 100.0, 5.0).bid(80 * MS, 99.5, 1.0);
    let mut hbt = raw_engine(&m, ExchangeKind::NoPartialFillExchange);
    hbt.elapse(INTERVAL).unwrap();
    hbt.submit_buy_order(0, 1, 0.0, 1.0, TimeInForce::GTC, OrdType::Market, false)
        .unwrap();
    drain(&mut hbt).unwrap();
    let order = &hbt.orders(0)[&1];
    assert_eq!(order.status, Status::Filled);
    assert_eq!(order.exec_price_tick, i64::MAX);
}

/// Defect 5b: `PartialFillExchange` sweeps only `[best_ask, best_ask + 100)` ticks for a market
/// buy. When the order is larger than the best level and the next level is more than 100 ticks
/// above it, the order expires with displayed liquidity left unreached.
#[test]
fn upstream_partial_fill_market_order_scans_only_100_ticks() {
    let mut m = Mkt::default();
    m.bid(0, 100.0, 5.0)
        .ask(0, 101.0, 1.0)
        .ask(0, 160.0, 5.0)
        .keepalive(80 * MS);
    let mut hbt = raw_engine(&m, ExchangeKind::PartialFillExchange);
    hbt.elapse(INTERVAL).unwrap();
    hbt.submit_buy_order(0, 2, 0.0, 3.0, TimeInForce::GTC, OrdType::Market, false)
        .unwrap();
    drain(&mut hbt).unwrap();
    let order = &hbt.orders(0)[&2];
    // Only the 1.0 at 101.0 executes; the 5.0 at 160.0 (118 ticks above) is never reached.
    assert_eq!(order.status, Status::Expired);
    assert_eq!(
        hbt.position(0),
        0.0,
        "the engine's local account ignores the partial execution"
    );
}

#[test]
fn adapter_blocks_market_orders_before_the_engine() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let out = run(
        &m,
        &config(),
        vec![(
            0,
            Act::Market {
                slot: 0,
                side: Side::Buy,
                qty: 1.0,
            },
        )],
    );
    let ev = of(&out, order_id(0, 0));
    assert_eq!(
        ev.iter().map(|e| e.body.type_name()).collect::<Vec<_>>(),
        ["order_intent", "risk_blocked"]
    );
    assert!(out.fills.is_empty());
}

/// Defect 6: `Local::cancel` accepts a cancel for an order the local side already holds as
/// `Filled` (it checks only that no request is pending) and sends a copy of that local order. The
/// exchange no longer holds the order, so `ack_cancel` rejects it by echoing the copy back,
/// still `Filled`. `Local::process_recv_order_` applies every received order whose status is
/// `Filled` to the account before looking at `req`, so the echo books the execution a second
/// time.
#[test]
fn upstream_rejected_cancel_of_a_filled_order_double_counts_the_fill() {
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 1.0)
        .bid(20 * MS, 100.0, 4.0)
        .keepalive(80 * MS);
    let mut hbt = raw_engine(&m, ExchangeKind::NoPartialFillExchange);
    hbt.elapse(INTERVAL).unwrap();
    // Inside the spread: nothing ahead, so the sell print at 100.0 fills it.
    hbt.submit_buy_order(0, 1, 100.5, 1.0, TimeInForce::GTC, OrdType::Limit, false)
        .unwrap();
    hbt.elapse(INTERVAL).unwrap();
    hbt.elapse(INTERVAL).unwrap();
    assert_eq!(hbt.orders(0)[&1].status, Status::Filled);
    assert_eq!(hbt.orders(0)[&1].req, Status::None);
    assert_eq!(hbt.position(0), 1.0);
    assert_eq!(hbt.state_values(0).num_trades, 1);

    // The order is filled, yet the cancel is accepted locally ...
    hbt.cancel(0, 1, false).unwrap();
    drain(&mut hbt).unwrap();
    // ... and its rejection books the one execution again.
    assert_eq!(hbt.orders(0)[&1].status, Status::Filled);
    assert_eq!(hbt.position(0), 2.0);
    assert_eq!(hbt.state_values(0).num_trades, 2);
    assert_eq!(hbt.state_values(0).balance, -201.0);
}

/// The adapter's ledger is fed by the exchange-side fill hook and counts the execution once. It
/// can only reach the defect when a strategy cancels an order it already knows is filled, which
/// requires the engine to still hold the order: clearing is deferred while an earlier cancel is in
/// flight. The bundled strategies never do this (they cancel only `cancellable()` orders); a
/// custom script can, and then the engine cross-check reports the engine's doubled figure.
#[test]
fn adapter_ledger_counts_the_fill_once_when_a_filled_order_is_cancelled() {
    let mut m = Mkt::standard();
    m.trade(35 * MS, Side::Sell, 100.0, 1.0)
        .bid(35 * MS, 100.0, 4.0)
        .keepalive(120 * MS);
    let mut cfg = config();
    // Long entry latency, instant responses: the first cancel is still in flight when the fill
    // response arrives, so the filled order is kept and the second cancel reaches the engine.
    cfg.execution.order_latency = bb_core::config::OrderLatencyConfig::Constant {
        entry_ns: 15 * MS,
        response_ns: 0,
    };
    let id = order_id(0, 0);
    let out = run(
        &m,
        &cfg,
        vec![
            (0, buy(0, 100.5, 1.0, Tif::Gtc)),
            (2, Act::Cancel { order_id: id }),
            (3, Act::Cancel { order_id: id }),
        ],
    );
    assert_eq!(out.fills.len(), 1);
    let last = out.equity.last().unwrap();
    assert_eq!((last.position, last.known_position), (1.0, 1.0));
    // The echo is on the tape: a rejected cancel answered with status `filled`.
    let echoes = of(&out, id)
        .into_iter()
        .filter(|e| {
            matches!(
                e.body,
                bb_core::flight::FlightBody::LocalResponse {
                    status: bb_core::OrderStatus::Filled,
                    request_rejected: true,
                    ..
                }
            )
        })
        .count();
    assert_eq!(echoes, 1);
    let check = &out.engine_check;
    assert_eq!(check.ledger_position, 1.0);
    assert_eq!(check.engine_position, 2.0);
    assert_eq!((check.ledger_fills, check.engine_num_trades), (1, 2));
    assert!(!check.agrees);
}
