//! Behaviour of the engine adapter and flight recorder on hand-built markets, where every expected
//! value can be worked out on paper.

mod common;

use bb_core::{
    config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig},
    flight::{FlightBody, QueueCause, RiskRule},
    strategy::order_id,
    Clock, OrderStatus, ReqKind, Side, Source, Tif,
};
use common::*;

/// A resting buy joins 5.0 displayed at 100.0. Trades of 3 and 3 at its price exhaust the modeled
/// queue (risk-adverse model: only trades advance it) and the second trade fills it.
#[test]
fn maker_fill_when_trades_exhaust_the_modeled_queue() {
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 3.0)
        .bid(20 * MS, 100.0, 2.0)
        // Others join behind: displayed quantity grows, the estimate ahead must not.
        .bid(25 * MS, 100.0, 6.0)
        .trade(30 * MS, Side::Sell, 100.0, 3.0)
        .bid(30 * MS, 100.0, 3.0)
        .keepalive(80 * MS);
    let out = run(&m, &config(), vec![(0, buy(0, 100.0, 1.0, Tif::Gtc))]);
    let id = order_id(0, 0);
    let ev = of(&out, id);

    assert_eq!(
        types(&ev),
        [
            "order_intent",
            "request_sent",
            "exch_queued",
            "exch_response",
            "local_response",
            "queue_update",
            "queue_update",
            "queue_update",
            "queue_update",
            "exch_fill",
            "exch_response",
            "local_response",
        ]
    );

    // Decision at the first tick, on the local clock, from the strategy.
    assert_eq!((ev[0].ts, ev[0].clock, ev[0].source), (tick_ts(0), Clock::Local, Source::Strategy));
    let FlightBody::OrderIntent { market, known_position, .. } = &ev[0].body else { panic!() };
    assert_eq!(market.best_bid_tick, Some(tk(100.0)));
    assert_eq!(market.best_ask_tick, Some(tk(101.0)));
    assert_eq!(market.feed_exch_ts, Some(0));
    assert_eq!(market.feed_local_ts, Some(FEED));
    assert_eq!(*known_position, 0.0);

    // Request leaves locally with the configured entry latency.
    assert_eq!(
        ev[1].body,
        FlightBody::RequestSent {
            req: ReqKind::New,
            entry_latency_ns: ENTRY,
            exch_arrival_ts: Some(tick_ts(0) + ENTRY),
        }
    );
    assert_eq!(ev[1].source, Source::Engine);

    // Arrival: queued behind everything displayed at 100.0.
    assert_eq!((ev[2].ts, ev[2].clock), (tick_ts(0) + ENTRY, Clock::Exchange));
    assert_eq!(
        ev[2].body,
        FlightBody::ExchQueued {
            queue_ahead: Some(5.0),
            level_qty: 5.0,
            best_bid_tick: Some(tk(100.0)),
            best_ask_tick: Some(tk(101.0)),
        }
    );
    assert_eq!(ev[4].ts, tick_ts(0) + ENTRY + RESPONSE);

    // Queue path: 5 -> 2 (trade 3) -> 2 (level shrinks to 2) -> 2 (level grows) -> -1 (trade 3).
    let expect = [
        (QueueCause::Trade { qty: 3.0 }, 5.0, 2.0),
        (QueueCause::Depth { prev_qty: 5.0, new_qty: 2.0 }, 2.0, 2.0),
        (QueueCause::Depth { prev_qty: 2.0, new_qty: 6.0 }, 2.0, 2.0),
        (QueueCause::Trade { qty: 3.0 }, 2.0, -1.0),
    ];
    for (e, (cause, before, after)) in ev[5..9].iter().zip(expect) {
        assert_eq!(
            e.body,
            FlightBody::QueueUpdate {
                cause,
                ahead_before: Some(before),
                ahead_after: Some(after),
            }
        );
        assert_eq!(e.clock, Clock::Exchange);
    }

    // The fill immediately follows the trade that exhausted the queue, at the same instant.
    assert!(is_trade_update(ev[8]));
    assert_eq!(ev[9].seq, ev[8].seq + 1);
    assert_eq!(ev[9].ts, 30 * MS);
    let value = 100.0 * 1.0;
    assert_eq!(
        ev[9].body,
        FlightBody::ExchFill {
            price_tick: tk(100.0),
            qty: 1.0,
            maker: true,
            fee: MAKER_FEE * value,
            value,
            leaves_qty: 0.0,
        }
    );
    assert_eq!(
        ev[11].body,
        FlightBody::LocalResponse {
            status: OrderStatus::Filled,
            request_rejected: false,
            exec_qty: 1.0,
            exec_price_tick: tk(100.0),
            leaves_qty: 0.0,
            exch_ts: 30 * MS,
        }
    );
    assert_eq!(ev[11].ts, 30 * MS + RESPONSE);

    assert_eq!(out.fills.len(), 1);
    let f = &out.fills[0];
    assert_eq!((f.seq, f.order_id, f.ts, f.position_after), (ev[9].seq, id, 30 * MS, 1.0));
    assert!(out.engine_check.agrees, "{:?}", out.engine_check);
    assert_eq!(out.engine_check.engine_position, 1.0);

    // Tape sequence numbers are contiguous from zero.
    for (i, e) in out.tape.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }
}

/// A sell trade prints *below* a resting buy's price. The exchange model fills the order without
/// consulting the queue, so there is no queue update in front of the fill.
#[test]
fn maker_fill_when_the_market_trades_through_the_price() {
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 99.5, 1.0).keepalive(80 * MS);
    let out = run(&m, &config(), vec![(0, buy(0, 100.0, 1.0, Tif::Gtc))]);
    let ev = of(&out, order_id(0, 0));
    assert_eq!(
        types(&ev),
        [
            "order_intent",
            "request_sent",
            "exch_queued",
            "exch_response",
            "local_response",
            "exch_fill",
            "exch_response",
            "local_response",
        ]
    );
    let FlightBody::ExchFill { price_tick, maker, .. } = ev[5].body else { panic!() };
    // Filled at its own limit price, as a maker.
    assert_eq!((price_tick, maker), (tk(100.0), true));
    assert!(out.engine_check.agrees);
}

/// A post-only buy priced at the best ask would cross, so the exchange model expires it.
#[test]
fn post_only_order_that_would_cross_is_expired() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let out = run(&m, &config(), vec![(0, buy(0, 101.0, 1.0, Tif::Gtx))]);
    let ev = of(&out, order_id(0, 0));
    assert_eq!(
        types(&ev),
        ["order_intent", "request_sent", "exch_response", "local_response"]
    );
    let FlightBody::ExchResponse { status, request_rejected, .. } = ev[2].body else { panic!() };
    assert_eq!((status, request_rejected), (OrderStatus::Expired, false));
    assert!(out.fills.is_empty());
}

/// An IOC buy at the ask takes liquidity on arrival and pays the taker fee.
#[test]
fn marketable_ioc_fills_as_taker_on_arrival() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let out = run(&m, &config(), vec![(0, buy(0, 101.0, 2.0, Tif::Ioc))]);
    let ev = of(&out, order_id(0, 0));
    assert_eq!(
        types(&ev),
        [
            "order_intent",
            "request_sent",
            "exch_fill",
            "exch_response",
            "local_response",
        ]
    );
    let value = 101.0 * 2.0;
    assert_eq!(
        ev[2].body,
        FlightBody::ExchFill {
            price_tick: tk(101.0),
            qty: 2.0,
            maker: false,
            fee: TAKER_FEE * value,
            value,
            leaves_qty: 0.0,
        }
    );
    assert_eq!(ev[2].ts, tick_ts(0) + ENTRY);
    assert!(out.engine_check.agrees);
    let last = out.equity.last().unwrap();
    assert_eq!((last.position, last.known_position), (2.0, 2.0));
    assert_eq!(last.cash, -value);
    assert_eq!(last.fees, TAKER_FEE * value);
    // Bought at the ask, marked at mid 100.5: unrealized is the half spread lost.
    assert!((last.unrealized_pnl - 2.0 * (100.5 - 101.0)).abs() < 1e-9);
    assert!((last.equity - (last.unrealized_pnl - last.fees)).abs() < 1e-12);
}

/// A resting order is cancelled before anything trades at its price.
#[test]
fn cancel_of_a_resting_order() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let id = order_id(0, 0);
    let out = run(
        &m,
        &config(),
        vec![(0, buy(0, 100.0, 1.0, Tif::Gtc)), (2, Act::Cancel { order_id: id })],
    );
    let ev = of(&out, id);
    assert_eq!(
        types(&ev),
        [
            "order_intent",
            "request_sent",
            "exch_queued",
            "exch_response",
            "local_response",
            "cancel_intent",
            "request_sent",
            "exch_response",
            "local_response",
        ]
    );
    assert_eq!(
        ev[6].body,
        FlightBody::RequestSent {
            req: ReqKind::Cancel,
            entry_latency_ns: ENTRY,
            exch_arrival_ts: Some(tick_ts(2) + ENTRY),
        }
    );
    let FlightBody::ExchResponse { status, request_rejected, .. } = ev[7].body else { panic!() };
    assert_eq!((status, request_rejected), (OrderStatus::Canceled, false));
    assert_eq!(ev[7].ts, tick_ts(2) + ENTRY);
}

/// The cancel is sent, but the order fills while the cancel is still on its way. The exchange
/// rejects the cancel; the fill stands.
#[test]
fn cancel_that_loses_the_race_against_a_fill_is_rejected() {
    let mut m = Mkt::standard();
    // Cancel is sent at tick 2 (30 ms) and arrives at 31 ms; the trade-through is at 30.5 ms.
    m.trade(30 * MS + 500 * US, Side::Sell, 99.5, 1.0).keepalive(80 * MS);
    let id = order_id(0, 0);
    let out = run(
        &m,
        &config(),
        vec![(0, buy(0, 100.0, 1.0, Tif::Gtc)), (2, Act::Cancel { order_id: id })],
    );
    let ev = of(&out, id);
    let fills = find(&ev, "exch_fill");
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].ts, 30 * MS + 500 * US);
    let responses = find(&ev, "exch_response");
    let FlightBody::ExchResponse { request_rejected, .. } = responses.last().unwrap().body else {
        panic!()
    };
    assert!(request_rejected, "the late cancel must be rejected: {:?}", types(&ev));
    assert_eq!(responses.last().unwrap().ts, tick_ts(2) + ENTRY);
    assert_eq!(out.fills.len(), 1);
    assert!(out.engine_check.agrees);
}

/// Cancelling an order whose new-order request is still in flight is refused by the engine
/// synchronously; nothing is sent.
#[test]
fn cancel_while_the_new_order_is_in_flight_is_refused() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let mut cfg = config();
    // Response comes back after the next tick, so at tick 1 the order is still unacknowledged.
    cfg.execution.order_latency = OrderLatencyConfig::Constant {
        entry_ns: ENTRY,
        response_ns: 15 * MS,
    };
    let id = order_id(0, 0);
    let out = run(
        &m,
        &cfg,
        vec![(0, buy(0, 100.0, 1.0, Tif::Gtc)), (1, Act::Cancel { order_id: id })],
    );
    let ev = of(&out, id);
    let refused = find(&ev, "request_refused");
    assert_eq!(refused.len(), 1);
    let FlightBody::RequestRefused { req, error } = &refused[0].body else { panic!() };
    assert_eq!(*req, ReqKind::Cancel);
    assert!(!error.is_empty());
    assert_eq!(refused[0].ts, tick_ts(1));
    // Only the original new-order request was ever sent.
    assert_eq!(find(&ev, "request_sent").len(), 1);
}

/// With the partial-fill exchange model a resting order is filled in pieces. BlackBox's ledger
/// counts every piece. The engine's own local account (0.9.4) applies only the final `Filled`
/// response, so it under-counts; the cross-check must report that rather than hide it.
#[test]
fn partial_maker_fills_are_all_counted_and_the_engine_gap_is_reported() {
    let mut m = Mkt::standard();
    m.trade(20 * MS, Side::Sell, 100.0, 6.0)
        .bid(20 * MS, 100.0, 4.0)
        .trade(30 * MS, Side::Sell, 100.0, 2.0)
        .bid(30 * MS, 100.0, 2.0)
        .keepalive(80 * MS);
    let mut cfg = config();
    cfg.execution.exchange_model = ExchangeModel::PartialFill;
    let out = run(&m, &cfg, vec![(0, buy(0, 100.0, 3.0, Tif::Gtc))]);
    let ev = of(&out, order_id(0, 0));

    // Queue 5 ahead; trade of 6 overshoots by 1 -> 1 filled. Then a trade of 2 fills the rest.
    let fills = find(&ev, "exch_fill");
    assert_eq!(fills.len(), 2);
    let qtys: Vec<(f64, f64)> = fills
        .iter()
        .map(|e| match e.body {
            FlightBody::ExchFill { qty, leaves_qty, .. } => (qty, leaves_qty),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(qtys, [(1.0, 2.0), (2.0, 0.0)]);
    let statuses: Vec<OrderStatus> = find(&ev, "local_response")
        .iter()
        .map(|e| match e.body {
            FlightBody::LocalResponse { status, .. } => status,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        statuses,
        [OrderStatus::New, OrderStatus::PartiallyFilled, OrderStatus::Filled]
    );

    let last = out.equity.last().unwrap();
    assert_eq!(last.position, 3.0);
    assert_eq!(last.known_position, 3.0);
    assert_eq!(out.fills.iter().map(|f| f.position_after).collect::<Vec<_>>(), [1.0, 3.0]);

    let c = &out.engine_check;
    assert!(!c.agrees);
    assert_eq!((c.ledger_position, c.engine_position), (3.0, 2.0));
    assert_eq!((c.ledger_fills, c.engine_num_trades), (2, 1));
    assert!(c.note.contains("partial-fill"), "{}", c.note);
}

/// A taking order under the partial-fill model walks displayed depth level by level. Each level
/// is one recorded execution at its own price.
#[test]
fn taker_sweep_records_one_execution_per_level() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let mut cfg = config();
    cfg.execution.exchange_model = ExchangeModel::PartialFill;
    let out = run(&m, &cfg, vec![(0, buy(0, 101.5, 8.0, Tif::Ioc))]);
    let ev = of(&out, order_id(0, 0));
    let fills: Vec<(i64, f64, bool)> = find(&ev, "exch_fill")
        .iter()
        .map(|e| match e.body {
            FlightBody::ExchFill { price_tick, qty, maker, .. } => (price_tick, qty, maker),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(fills, [(tk(101.0), 5.0, false), (tk(101.5), 3.0, false)]);
    // One response reports the whole arrival.
    assert_eq!(find(&ev, "exch_response").len(), 1);
    let last = out.equity.last().unwrap();
    assert_eq!((last.position, last.known_position), (8.0, 8.0));
    assert_eq!(last.cash, -(101.0 * 5.0 + 101.5 * 3.0));
    // The engine's local account only saw the last level.
    assert_eq!(out.engine_check.engine_position, 3.0);
    assert!(!out.engine_check.agrees);
}

/// The probability queue model credits part of a cancellation at the order's level to the queue
/// ahead; the risk-adverse model credits none. Same market, same order, different estimate.
#[test]
fn queue_models_differ_on_cancellations_ahead() {
    let mut m = Mkt::standard();
    // 5 -> 8: three more join behind our order. Then 8 -> 4 with no trade: four were cancelled.
    m.bid(20 * MS, 100.0, 8.0).bid(30 * MS, 100.0, 4.0).keepalive(80 * MS);
    let ahead_after = |queue_model| {
        let mut cfg = config();
        cfg.execution.queue_model = queue_model;
        let out = run(&m, &cfg, vec![(0, buy(0, 100.0, 1.0, Tif::Gtc))]);
        let ev = of(&out, order_id(0, 0));
        let updates = find(&ev, "queue_update");
        assert_eq!(updates.len(), 2);
        match updates[1].body {
            FlightBody::QueueUpdate { ahead_before, ahead_after, .. } => {
                assert_eq!(ahead_before, Some(5.0));
                ahead_after.unwrap()
            }
            _ => unreachable!(),
        }
    };
    // Risk-adverse: min(5, 4) = 4. Only the cap by the new displayed quantity applies.
    assert_eq!(ahead_after(QueueModelConfig::RiskAdverse), 4.0);
    // PowerProb3 with n = 1: p(behind) = 1 - front/(front+back) = 1 - 5/8 = 3/8, so of the 4
    // cancelled, 4 * 5/8 = 2.5 are assumed to have been ahead: 5 - 2.5 = 2.5.
    let est = ahead_after(QueueModelConfig::PowerProb3 { n: 1.0 });
    assert!((est - 2.5).abs() < 1e-12, "{est}");
}

/// Entry latency decides whether an order reaches the exchange before a trade it could have
/// filled against.
#[test]
fn entry_latency_decides_which_market_events_the_order_sees() {
    let mut m = Mkt::standard();
    // A trade-through 5 ms after the first tick.
    m.trade(15 * MS, Side::Sell, 99.5, 1.0).keepalive(80 * MS);
    let filled = |entry_ns| {
        let mut cfg = config();
        cfg.execution.order_latency = OrderLatencyConfig::Constant {
            entry_ns,
            response_ns: RESPONSE,
        };
        let out = run(&m, &cfg, vec![(0, buy(0, 100.0, 1.0, Tif::Gtc))]);
        let ev = of(&out, order_id(0, 0));
        let queued = find(&ev, "exch_queued");
        assert_eq!(queued[0].ts, tick_ts(0) + entry_ns);
        out.fills.len()
    };
    assert_eq!(filled(MS), 1, "arrives at 11 ms, before the trade at 15 ms");
    assert_eq!(filled(8 * MS), 0, "arrives at 18 ms, after the trade");
}

/// A feed latency offset delays what the strategy knows without changing when things happened at
/// the exchange, and the strategy tick grid does not move.
#[test]
fn feed_latency_offset_delays_the_strategys_view_only() {
    let mut m = Mkt::standard();
    // The best bid improves to 100.5 at 5 ms.
    m.bid(5 * MS, 100.5, 2.0).keepalive(80 * MS);
    let view_at_tick0 = |offset| {
        let mut cfg = config();
        cfg.execution.feed_latency_offset_ns = offset;
        let script = Script::new(vec![]);
        let seen = script.seen.clone();
        run_with(&m, &cfg, script);
        let seen = seen.borrow();
        (seen[0].1, seen[0].2.best_bid_tick, seen[0].2.feed_local_ts)
    };
    // No offset: by 10 ms the improvement (received at 5.1 ms) is known.
    assert_eq!(view_at_tick0(0), (tick_ts(0), Some(tk(100.5)), Some(5 * MS + FEED)));
    // 20 ms offset: at 10 ms nothing has arrived yet.
    let (now, bid, feed_ts) = view_at_tick0(20 * MS);
    assert_eq!((now, bid, feed_ts), (tick_ts(0), None, None));
}

#[test]
fn pre_trade_risk_rules_block_before_the_engine() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let mut cfg = config();
    cfg.risk.max_position = 2.0;
    cfg.risk.max_order_qty = 1.5;
    cfg.risk.max_open_orders = 3;
    let out = run(
        &m,
        &cfg,
        vec![
            (0, buy(0, 100.0, 2.0, Tif::Gtc)),  // over max_order_qty
            (0, buy(1, 100.0, 0.25, Tif::Gtc)), // not a lot multiple
            (0, buy(2, 99.5, 1.0, Tif::Gtc)),   // ok, working buy 1.0
            (0, buy(3, 99.0, 1.0, Tif::Gtc)),   // ok, working buy 2.0
            (0, buy(4, 99.0, 1.0, Tif::Gtc)),   // worst case 3.0 > max_position
            (0, buy(4, 99.0, 0.1, Tif::Gtc)),   // slot reused
            (0, sell(5, 101.0, 1.0, Tif::Gtc)), // ok, third working order
            (0, sell(6, 101.5, 1.0, Tif::Gtc)), // over max_open_orders
        ],
    );
    let rule = |slot: u8| -> Option<RiskRule> {
        of(&out, order_id(0, slot)).iter().find_map(|e| match &e.body {
            FlightBody::RiskBlocked { rule, .. } => Some(*rule),
            _ => None,
        })
    };
    assert_eq!(rule(0), Some(RiskRule::MaxOrderQty));
    assert_eq!(rule(1), Some(RiskRule::InvalidOrder));
    assert_eq!(rule(2), None);
    assert_eq!(rule(3), None);
    assert_eq!(rule(6), Some(RiskRule::MaxOpenOrders));
    // Slot 4 was asked for twice: first blocked on position, second as a duplicate slot.
    let slot4: Vec<RiskRule> = of(&out, order_id(0, 4))
        .iter()
        .filter_map(|e| match &e.body {
            FlightBody::RiskBlocked { rule, .. } => Some(*rule),
            _ => None,
        })
        .collect();
    assert_eq!(slot4, [RiskRule::MaxPosition, RiskRule::InvalidOrder]);
    assert_eq!(rule(5), None);
    // Blocked orders never reach the engine.
    for slot in [0u8, 1, 4, 6] {
        assert!(find(&of(&out, order_id(0, slot)), "request_sent").is_empty());
    }
    for slot in [2u8, 3, 5] {
        assert_eq!(find(&of(&out, order_id(0, slot)), "request_sent").len(), 1);
    }
}

/// Buying at the ask and watching the market fall trips the drawdown rule: one halt event, every
/// working order is pulled, and the strategy is not consulted again.
#[test]
fn drawdown_halt_cancels_working_orders_and_stops_the_strategy() {
    let mut m = Mkt::standard();
    // The whole book drops by 5 between 30 and 31 ms.
    m.bid(30 * MS, 100.0, 0.0)
        .bid(30 * MS, 99.5, 0.0)
        .bid(30 * MS, 99.0, 0.0)
        .ask(31 * MS, 96.0, 5.0)
        .bid(31 * MS, 95.0, 5.0)
        .keepalive(120 * MS);
    let mut cfg = config();
    cfg.risk.max_drawdown = Some(5.0);
    let out = run(
        &m,
        &cfg,
        vec![
            (0, buy(0, 101.0, 2.0, Tif::Ioc)),  // long 2 at 101
            (1, sell(0, 110.0, 1.0, Tif::Gtc)), // a resting order far away
            (6, buy(0, 95.0, 1.0, Tif::Gtc)),   // after the halt
        ],
    );
    let halts: Vec<_> = out
        .tape
        .iter()
        .filter(|e| matches!(e.body, FlightBody::RiskHalt { .. }))
        .collect();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].order_id, None);
    // Mid falls from 100.5 to 95.5: equity drops by 2 * 5 = 10 > 5. First tick after 31.1 ms.
    assert_eq!(halts[0].ts, tick_ts(3));

    let resting = of(&out, order_id(1, 0));
    assert!(types(&resting).contains(&"cancel_intent"));
    let FlightBody::ExchResponse { status, .. } = find(&resting, "exch_response").last().unwrap().body
    else {
        panic!()
    };
    assert_eq!(status, OrderStatus::Canceled);

    // After the halt the strategy is no longer evaluated, so its tick-6 order never exists.
    assert!(of(&out, order_id(6, 0)).is_empty());
}

/// A request sent on the last tick cannot be resolved before the data ends. It stays "sent" on
/// the tape; nothing is invented about its fate.
#[test]
fn request_in_flight_when_data_ends_stays_unresolved() {
    let mut m = Mkt::standard();
    m.keepalive(30 * MS);
    let mut cfg = config();
    cfg.execution.order_latency = OrderLatencyConfig::Constant {
        entry_ns: 50 * MS,
        response_ns: RESPONSE,
    };
    // Ticks at 10, 20, 30 ms are evaluated (last feed event is received at 30.1 ms).
    let out = run(&m, &cfg, vec![(2, buy(0, 100.0, 1.0, Tif::Gtc))]);
    assert_eq!(out.ticks, 3);
    let ev = of(&out, order_id(2, 0));
    assert_eq!(types(&ev), ["order_intent", "request_sent"]);
    // Samples for ticks 0..=2, then the closing sample one interval later.
    let closing = out.equity.last().unwrap();
    assert_eq!((closing.tick_no, closing.ts), (3, tick_ts(3)));
    assert_eq!(out.equity.len(), 4);
}

/// Depth-driven queue updates beyond the per-order cap are dropped only when they do not move the
/// estimate, and the drop is declared on the tape.
#[test]
fn queue_update_cap_is_explicit() {
    let mut m = Mkt::standard();
    // 40 changes at the order's level that never reduce the estimate (quantity only grows).
    for i in 0..40 {
        m.bid(20 * MS + i * 100 * US, 100.0, 6.0 + i as f64);
    }
    m.trade(40 * MS, Side::Sell, 100.0, 1.0).keepalive(80 * MS);
    let mut cfg = config();
    cfg.record.max_queue_updates_per_order = 10;
    let out = run(&m, &cfg, vec![(0, buy(0, 100.0, 1.0, Tif::Gtc))]);
    let ev = of(&out, order_id(0, 0));
    let updates = find(&ev, "queue_update");
    // 10 kept depth updates, plus the trade update, which is always kept.
    assert_eq!(updates.len(), 11);
    assert!(is_trade_update(updates.last().unwrap()));
    let marker = find(&ev, "queue_truncated");
    assert_eq!(marker.len(), 1);
    assert_eq!(marker[0].body, FlightBody::QueueTruncated { dropped: 30 });
    assert_eq!(marker[0].source, Source::Recorder);
    // The marker is stamped with the time of the first dropped update.
    assert_eq!(marker[0].ts, 20 * MS + 10 * 100 * US);
}

/// Review finding: a cancel that loses the race against a fill is rejected by the exchange, and
/// that rejection must reach the tape even though the order became terminal locally before the
/// rejection arrived (the engine forgets terminal orders when they are cleared).
#[test]
fn late_cancel_rejection_is_recorded_after_the_order_is_terminal() {
    let mut m = Mkt::standard();
    // Trade-through at 45 ms fills the order; the cancel sent at 40 ms only arrives at 60 ms.
    m.trade(45 * MS, Side::Sell, 99.5, 1.0).keepalive(120 * MS);
    let mut cfg = config();
    cfg.execution.order_latency = OrderLatencyConfig::Constant {
        entry_ns: 20 * MS,
        response_ns: RESPONSE,
    };
    let id = order_id(0, 0);
    let out = run(
        &m,
        &cfg,
        vec![(0, buy(0, 100.0, 1.0, Tif::Gtc)), (3, Act::Cancel { order_id: id })],
    );
    let ev = of(&out, id);
    let locals: Vec<(i64, OrderStatus, bool, f64)> = find(&ev, "local_response")
        .iter()
        .map(|e| match e.body {
            FlightBody::LocalResponse { status, request_rejected, exec_qty, .. } => {
                (e.ts, status, request_rejected, exec_qty)
            }
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        locals,
        [
            (30 * MS + RESPONSE, OrderStatus::New, false, 0.0),
            (45 * MS + RESPONSE, OrderStatus::Filled, false, 1.0),
            // The rejection of the late cancel, with no execution attached to it. Its status is
            // the engine's copy of the cancel request (the order as known when the cancel was
            // sent), not the order's current state; consumers ignore status on rejections.
            (60 * MS + RESPONSE, OrderStatus::New, true, 0.0),
        ]
    );
}

/// Review finding: risk limits are hard limits, not "limit plus half a lot".
#[test]
fn risk_limits_allow_no_extra_lot() {
    let mut m = Mkt::standard();
    m.keepalive(80 * MS);
    let mut cfg = config();
    cfg.risk.max_order_qty = 0.15;
    cfg.risk.max_position = 1_000.0;
    let out = run(&m, &cfg, vec![(0, buy(0, 101.0, 0.2, Tif::Ioc))]);
    let ev = of(&out, order_id(0, 0));
    assert_eq!(types(&ev), ["order_intent", "risk_blocked"]);

    let mut cfg = config();
    cfg.risk.max_position = 0.15;
    let out = run(&m, &cfg, vec![(0, buy(0, 101.0, 0.2, Tif::Ioc))]);
    let FlightBody::RiskBlocked { rule, .. } = &of(&out, order_id(0, 0))[1].body else {
        panic!()
    };
    assert_eq!(*rule, RiskRule::MaxPosition);
    assert!(out.fills.is_empty());

    // Exactly at the limit is allowed.
    let mut cfg = config();
    cfg.risk.max_order_qty = 0.2;
    cfg.risk.max_position = 0.2;
    let out = run(&m, &cfg, vec![(0, buy(0, 101.0, 0.2, Tif::Ioc))]);
    assert_eq!(out.fills.len(), 1);
}

/// Review findings: after a partial maker fill the engine resets the remainder's queue estimate
/// to zero; the tape records that reset right after the fill (keeping the exhausting trade
/// directly before the fill). A cancel rejected after the remainder filled carries no execution.
#[test]
fn partial_fill_records_the_queue_reset_and_rejected_cancel_has_no_execution() {
    let mut m = Mkt::standard();
    // 5 ahead of a 3-lot buy. Trade of 6 at 20 ms fills 1 (partial), queue reset to 0.
    // Cancel sent at tick 2 (30 ms) arrives at 31 ms; a trade of 2 at 30.5 ms fills the rest.
    m.trade(20 * MS, Side::Sell, 100.0, 6.0)
        .bid(20 * MS, 100.0, 4.0)
        .trade(30 * MS + 500 * US, Side::Sell, 100.0, 2.0)
        .bid(30 * MS + 500 * US, 100.0, 2.0)
        .keepalive(80 * MS);
    let mut cfg = config();
    cfg.execution.exchange_model = ExchangeModel::PartialFill;
    let id = order_id(0, 0);
    let out = run(
        &m,
        &cfg,
        vec![(0, buy(0, 100.0, 3.0, Tif::Gtc)), (2, Act::Cancel { order_id: id })],
    );
    let ev = of(&out, id);
    let first_fill = ev
        .iter()
        .position(|e| e.body.type_name() == "exch_fill")
        .unwrap();
    assert!(is_trade_update(ev[first_fill - 1]));
    assert_eq!(
        ev[first_fill + 1].body,
        FlightBody::QueueUpdate {
            cause: QueueCause::FillReset,
            ahead_before: Some(-1.0),
            ahead_after: Some(0.0),
        }
    );
    assert_eq!(out.fills.iter().map(|f| f.qty).collect::<Vec<_>>(), [1.0, 2.0]);
    let last = find(&ev, "local_response").last().copied().unwrap();
    let FlightBody::LocalResponse { request_rejected, exec_qty, exec_price_tick, .. } = last.body
    else {
        panic!()
    };
    assert!(request_rejected);
    assert_eq!((exec_qty, exec_price_tick), (0.0, 0));
}
