//! Shrinkable synthetic markets and open-loop orders; no recorded/vendor data is used.
mod common;

use std::collections::{BTreeMap, BTreeSet};

use bb_core::{
    OrdType, OrderStatus, ReqKind, Side, Tif,
    config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig, RunConfig},
    flight::{FlightBody, FlightEvent, QueueCause, RiskRule},
    hash::sha256_json,
    strategy::order_id,
};
use bb_sim::{Event, SimInput, SimOutput, simulate};
use common::{Act, INST, INTERVAL, MS, Mkt, ORIGIN, Script, US};
use hftbacktest::{
    backtest::{
        Backtest, ExchangeKind, L2AssetBuilder,
        assettype::LinearAsset,
        data::{Data, DataSource},
        models::{
            CommonFees, ConstantLatency, LogProbQueueFunc, LogProbQueueFunc2, PowerProbQueueFunc,
            PowerProbQueueFunc2, PowerProbQueueFunc3, ProbQueueModel, QueueModel,
            RiskAdverseQueueModel, TradingValueFeeModel,
        },
    },
    depth::HashMapMarketDepth,
    types::{BUY_EVENT, Bot, DEPTH_EVENT, ElapseResult, OrdType as HOrdType, TimeInForce},
};
use proptest::prelude::*;
use proptest::{
    strategy::ValueTree,
    test_runner::{RngAlgorithm, RngSeed, TestRunner},
};

const TOL: f64 = 1e-8;
const END: i64 = 1_600 * MS;

// Tuples/vectors, rather than an opaque RNG seed, let proptest remove individual operations,
// shrink their quantities and move requests towards tick zero in a failing case.
#[derive(Clone, Debug)]
enum BookOp {
    Set { bid: bool, level: u8, lots: u8 },
    Trade { buy: bool, lots: u8 },
    Clear { bid: bool },
}

fn book_op() -> impl Strategy<Value = BookOp> {
    prop_oneof![
        5 => (any::<bool>(), 0u8..3, 1u8..31).prop_map(|(bid, level, lots)| BookOp::Set { bid, level, lots }),
        5 => (any::<bool>(), 1u8..41).prop_map(|(buy, lots)| BookOp::Trade { buy, lots }),
        1 => any::<bool>().prop_map(|bid| BookOp::Clear { bid }),
    ]
}

fn book_ops() -> impl Strategy<Value = Vec<BookOp>> {
    prop::collection::vec(book_op(), 0..301)
}

#[derive(Clone, Debug)]
struct Request {
    tick: u8,
    buy: bool,
    offset: i8,
    lots: u8,
    kind: u8,
    // A small id domain deliberately targets existing, terminal and non-existent orders.
    cancel_tick: u8,
    cancel_slot: u8,
}

fn requests() -> impl Strategy<Value = Vec<Request>> {
    prop::collection::vec(
        (
            0u8..161,
            any::<bool>(),
            -4i8..5,
            1u8..16,
            0u8..7,
            0u8..161,
            0u8..60,
        )
            .prop_map(
                |(tick, buy, offset, lots, kind, cancel_tick, cancel_slot)| Request {
                    tick,
                    buy,
                    offset,
                    lots,
                    kind,
                    cancel_tick,
                    cancel_slot,
                },
            ),
        0..61,
    )
    .prop_map(|mut reqs| {
        // Half the cancels target earlier limit decisions shortly after submission; the other
        // half retain arbitrary ids. This exercises accepted cancels, races and local refusals.
        for index in 0..reqs.len() {
            if reqs[index].kind != 5 {
                continue;
            }
            let candidates = reqs[..index].iter().filter(|r| r.kind <= 3).count();
            if candidates == 0 {
                continue;
            }
            let choice = usize::from(reqs[index].cancel_slot) % candidates;
            let (slot, target) = reqs[..index]
                .iter()
                .enumerate()
                .filter(|(_, r)| r.kind <= 3)
                .nth(choice)
                .unwrap();
            let target_tick = target.tick;
            reqs[index].cancel_tick = target_tick;
            reqs[index].cancel_slot = slot as u8;
            reqs[index].tick = target_tick.saturating_add(reqs[index].tick % 4);
        }
        reqs
    })
}

#[derive(Clone, Debug)]
struct Assumptions {
    partial: bool,
    queue: u8,
    power: u8,
    jitter: bool,
    entry_us: u16,
    response_us: u16,
    jitter_us: u16,
    feed_us: i16,
    maker_bps: i8,
    taker_bps: i8,
    position_lots: u8,
    order_lots: u8,
    open_orders: u8,
    seed: u64,
}

fn assumptions() -> impl Strategy<Value = Assumptions> {
    (
        (any::<bool>(), 0u8..6, 1u8..5, any::<bool>()),
        (
            prop_oneof![Just(0u16), 1u16..25_001],
            prop_oneof![Just(0u16), 1u16..25_001],
            prop_oneof![Just(0u16), 1u16..5_001],
            prop_oneof![Just(0i16), 1i16..501],
        ),
        (-5i8..11, -5i8..11, 1u8..31, 1u8..16, 1u8..9, any::<u64>()),
    )
        .prop_map(
            |(
                (partial, queue, power, jitter),
                (entry_us, response_us, jitter_us, feed_us),
                (maker_bps, taker_bps, position_lots, order_lots, open_orders, seed),
            )| Assumptions {
                partial,
                queue,
                power,
                jitter,
                entry_us,
                response_us,
                jitter_us,
                feed_us,
                maker_bps,
                taker_bps,
                position_lots,
                order_lots,
                open_orders,
                seed,
            },
        )
}

fn config(a: &Assumptions) -> RunConfig {
    let mut cfg = common::config();
    cfg.execution.exchange_model = if a.partial {
        ExchangeModel::PartialFill
    } else {
        ExchangeModel::NoPartialFill
    };
    cfg.execution.queue_model = match a.queue {
        0 => QueueModelConfig::RiskAdverse,
        1 => QueueModelConfig::LogProb,
        2 => QueueModelConfig::LogProb2,
        3 => QueueModelConfig::PowerProb { n: a.power as f64 },
        4 => QueueModelConfig::PowerProb2 { n: a.power as f64 },
        _ => QueueModelConfig::PowerProb3 { n: a.power as f64 },
    };
    let entry_ns = i64::from(a.entry_us) * US;
    let response_ns = i64::from(a.response_us) * US;
    cfg.execution.order_latency = if a.jitter {
        OrderLatencyConfig::Jitter {
            entry_ns,
            response_ns,
            jitter_ns: i64::from(a.jitter_us) * US,
        }
    } else {
        OrderLatencyConfig::Constant {
            entry_ns,
            response_ns,
        }
    };
    cfg.execution.feed_latency_offset_ns = i64::from(a.feed_us) * US;
    cfg.execution.fees.maker = f64::from(a.maker_bps) / 10_000.0;
    cfg.execution.fees.taker = f64::from(a.taker_bps) / 10_000.0;
    cfg.risk.max_position = f64::from(a.position_lots) * INST.lot_size;
    cfg.risk.max_order_qty = f64::from(a.order_lots) * INST.lot_size;
    cfg.risk.max_open_orders = u32::from(a.open_orders);
    cfg.seed = a.seed;
    cfg
}

fn market(ops: &[BookOp]) -> Mkt {
    let mut m = Mkt::default();
    let mut bids = [10u8; 3];
    let mut asks = [10u8; 3];
    for level in 0..3 {
        m.bid(0, 100.0 - level as f64 * INST.tick_size, 1.0).ask(
            0,
            101.0 + level as f64 * INST.tick_size,
            1.0,
        );
    }
    for (i, op) in ops.iter().enumerate() {
        let at = (i as i64 + 1) * 5 * MS;
        match *op {
            BookOp::Set { bid, level, lots } => {
                let book = if bid { &mut bids } else { &mut asks };
                book[level as usize] = lots;
                let px = if bid {
                    100.0 - f64::from(level) * INST.tick_size
                } else {
                    101.0 + f64::from(level) * INST.tick_size
                };
                if bid {
                    m.bid(at, px, f64::from(lots) * INST.lot_size);
                } else {
                    m.ask(at, px, f64::from(lots) * INST.lot_size);
                }
            }
            BookOp::Trade { buy, lots } => {
                let book = if buy { &mut asks } else { &mut bids };
                let Some(level) = book.iter().position(|qty| *qty > 0) else {
                    continue;
                };
                let px = if buy {
                    101.0 + level as f64 * INST.tick_size
                } else {
                    100.0 - level as f64 * INST.tick_size
                };
                let executed = lots.min(book[level]);
                m.trade(
                    at,
                    if buy { Side::Buy } else { Side::Sell },
                    px,
                    f64::from(executed) * INST.lot_size,
                );
                book[level] -= executed;
                // The trade precedes its corresponding depth update at the same timestamp.
                if buy {
                    m.ask(at, px, f64::from(book[level]) * INST.lot_size);
                } else {
                    m.bid(at, px, f64::from(book[level]) * INST.lot_size);
                }
                if book[level] == 0 {
                    book[level] = 10;
                    if buy {
                        m.ask(at, px, 1.0);
                    } else {
                        m.bid(at, px, 1.0);
                    }
                }
            }
            BookOp::Clear { bid } => {
                let book = if bid { &mut bids } else { &mut asks };
                for (level, qty) in book.iter_mut().enumerate() {
                    *qty = 0;
                    if bid {
                        m.bid(at, 100.0 - level as f64 * INST.tick_size, 0.0);
                    } else {
                        m.ask(at, 101.0 + level as f64 * INST.tick_size, 0.0);
                    }
                }
            }
        }
    }
    m.keepalive(END);
    m
}

fn actions(reqs: &[Request]) -> Vec<(u64, Act)> {
    reqs.iter()
        .enumerate()
        .map(|(slot, r)| {
            let side = if r.buy { Side::Buy } else { Side::Sell };
            let qty = f64::from(r.lots) * INST.lot_size;
            let act = match r.kind {
                0..=3 => Act::Place {
                    slot: slot as u8,
                    side,
                    price: (if r.buy { 100.0 } else { 101.0 })
                        + f64::from(r.offset) * INST.tick_size,
                    qty,
                    tif: [Tif::Gtc, Tif::Gtx, Tif::Ioc, Tif::Fok][r.kind as usize],
                },
                4 => Act::Market {
                    slot: slot as u8,
                    side,
                    qty,
                },
                _ => Act::Cancel {
                    order_id: order_id(u64::from(r.cancel_tick), r.cancel_slot),
                },
            };
            (u64::from(r.tick), act)
        })
        .collect()
}

fn run(m: &Mkt, cfg: &RunConfig, reqs: &[Request]) -> SimOutput {
    simulate(
        &SimInput {
            events: &m.rows,
            instrument: INST,
            origin_ns: ORIGIN,
            config: cfg,
        },
        Box::new(Script::new(actions(reqs))),
    )
    .unwrap_or_else(|error| panic!("simulation failed: {error}"))
}

fn close(actual: f64, expected: f64, label: &str) {
    assert!(actual.is_finite() && expected.is_finite());
    assert!(
        (actual - expected).abs() <= TOL * (1.0 + expected.abs()),
        "{label}: {actual} != {expected}"
    );
}

fn by_order(out: &SimOutput) -> BTreeMap<u64, Vec<&FlightEvent>> {
    let mut orders = BTreeMap::<u64, Vec<_>>::new();
    for e in &out.tape {
        if let Some(id) = e.order_id {
            orders.entry(id).or_default().push(e);
        }
    }
    orders
}

fn check_lifecycles(out: &SimOutput) {
    for (id, events) in by_order(out) {
        let intent = events.iter().find_map(|e| match e.body {
            FlightBody::OrderIntent { qty, .. } => Some(qty),
            _ => None,
        });
        let Some(qty) = intent else {
            // A cancellation of an id never submitted must only be refused locally.
            assert!(events.iter().all(|e| matches!(
                e.body,
                FlightBody::CancelIntent { .. }
                    | FlightBody::RequestRefused {
                        req: ReqKind::Cancel,
                        ..
                    }
            )));
            continue;
        };
        if let Some(index) = events.iter().position(|e| {
            matches!(
                e.body,
                FlightBody::OrderIntent {
                    ord_type: OrdType::Market,
                    ..
                }
            )
        }) {
            // Unsupported in the pinned engine; see docs/EXECUTION_MODEL.md, Defect 5.
            // A cancellation targeting this id may precede the order's decision.
            assert!(matches!(
                events[index + 1].body,
                FlightBody::RiskBlocked {
                    rule: RiskRule::InvalidOrder,
                    ..
                }
            ));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e.body, FlightBody::RequestSent { .. }))
            );
        }
        let mut filled = 0.0;
        let mut exchange_status = OrderStatus::None;
        let mut local_status = OrderStatus::None;
        let mut awaiting_fill_response = false;
        for (index, e) in events.iter().enumerate() {
            match e.body {
                FlightBody::ExchQueued { .. } | FlightBody::QueueUpdate { .. } => {
                    assert!(
                        !exchange_status.is_terminal(),
                        "order {id}: queue event after terminal"
                    );
                }
                FlightBody::ExchFill {
                    qty: executed,
                    leaves_qty,
                    ..
                } => {
                    assert!(
                        !exchange_status.is_terminal(),
                        "order {id}: fill after terminal"
                    );
                    assert!(executed > 0.0 && leaves_qty >= -TOL);
                    filled += executed;
                    assert!(filled <= qty + TOL, "order {id}: filled {filled} > {qty}");
                    close(leaves_qty, qty - filled, "fill leaves");
                    awaiting_fill_response = true;
                    // A multi-level taker sweep may report several executions in one response.
                    let response = events[index + 1..].iter().find(|next| {
                        !matches!(
                            next.body,
                            FlightBody::ExchFill { .. }
                                | FlightBody::QueueUpdate {
                                    cause: QueueCause::FillReset,
                                    ..
                                }
                        )
                    });
                    assert!(
                        matches!(
                            response.map(|next| &next.body),
                            Some(FlightBody::ExchResponse {
                                request_rejected: false,
                                ..
                            })
                        ),
                        "order {id}: fill at seq {} has no response",
                        e.seq
                    );
                }
                FlightBody::ExchResponse {
                    status,
                    request_rejected,
                    leaves_qty,
                    ..
                } => {
                    assert!(leaves_qty >= -TOL);
                    if !request_rejected {
                        assert!(
                            !exchange_status.is_terminal(),
                            "order {id}: response after terminal"
                        );
                        check_transition(exchange_status, status, filled, qty);
                        exchange_status = status;
                        close(leaves_qty, qty - filled, "response leaves");
                        awaiting_fill_response = false;
                    }
                }
                FlightBody::LocalResponse {
                    status,
                    request_rejected,
                    leaves_qty,
                    ..
                } => {
                    assert!(leaves_qty >= -TOL);
                    if !request_rejected {
                        assert!(
                            !local_status.is_terminal(),
                            "order {id}: local response after terminal"
                        );
                        check_transition(local_status, status, filled, qty);
                        local_status = status;
                    }
                }
                _ => {}
            }
        }
        assert!(
            !awaiting_fill_response,
            "order {id}: fill without exchange response"
        );
    }
}

fn check_transition(before: OrderStatus, after: OrderStatus, filled: f64, qty: f64) {
    assert!(matches!(
        after,
        OrderStatus::New
            | OrderStatus::PartiallyFilled
            | OrderStatus::Filled
            | OrderStatus::Expired
            | OrderStatus::Canceled
    ));
    assert!(!(before == OrderStatus::PartiallyFilled && after == OrderStatus::New));
    if after == OrderStatus::Filled {
        close(filled, qty, "Filled before full quantity");
    }
    if after == OrderStatus::PartiallyFilled {
        assert!(filled > 0.0 && filled < qty + TOL);
    }
}

fn check_ledger(out: &SimOutput) {
    let mut cash = 0.0;
    let mut fees = 0.0;
    let mut position = 0.0;
    for f in &out.fills {
        cash -= f.side.sign() * f.value;
        fees += f.fee;
        position += f.side.sign() * f.qty;
        close(f.position_after, position, "running position");
        assert!(matches!(
            out.tape[f.seq as usize].body,
            FlightBody::ExchFill { .. }
        ));
    }
    if let Some(last) = out.equity.last() {
        close(last.cash, cash, "closing cash before fees");
        close(last.fees, fees, "closing fees");
        close(last.position, position, "closing position");
        close(last.cash - last.fees, cash - fees, "closing net cash");
    } else {
        assert!(out.fills.is_empty(), "fills without closing equity sample");
    }
    for sample in &out.equity {
        close(
            sample.equity,
            sample.realized_pnl + sample.unrealized_pnl - sample.fees,
            "equity decomposition",
        );
        if let Some(mid) = sample.mid_tick {
            close(
                sample.realized_pnl + sample.unrealized_pnl,
                sample.cash + sample.position * mid * INST.tick_size * INST.contract_size,
                "marked accounting identity",
            );
        }
    }
}

fn check_risk(out: &SimOutput, cfg: &RunConfig) {
    for f in &out.fills {
        assert!(
            f.position_after.abs() <= cfg.risk.max_position + TOL,
            "fill position {} > {}",
            f.position_after,
            cfg.risk.max_position
        );
    }
    for sample in &out.equity {
        assert!(sample.position.abs() <= cfg.risk.max_position + TOL);
        assert!(sample.open_orders <= cfg.risk.max_open_orders);
    }
    for events in by_order(out).values() {
        if events.iter().any(|e| {
            matches!(
                e.body,
                FlightBody::RequestSent {
                    req: ReqKind::New,
                    ..
                }
            )
        }) {
            let qty = events
                .iter()
                .find_map(|e| match e.body {
                    FlightBody::OrderIntent { qty, .. } => Some(qty),
                    _ => None,
                })
                .unwrap();
            assert!(qty <= cfg.risk.max_order_qty + TOL);
        }
    }
}

fn raw_with<Q: QueueModel<HashMapMarketDepth> + 'static>(
    rows: &[Event],
    cfg: &RunConfig,
    queue: Q,
) -> Backtest<HashMapMarketDepth> {
    let OrderLatencyConfig::Constant {
        entry_ns,
        response_ns,
    } = cfg.execution.order_latency
    else {
        unreachable!()
    };
    let asset = L2AssetBuilder::new()
        .data(vec![DataSource::Data(Data::from_data(rows))])
        .latency_model(ConstantLatency::new(entry_ns, response_ns))
        .asset_type(LinearAsset::new(INST.contract_size))
        .fee_model(TradingValueFeeModel::new(CommonFees::new(
            cfg.execution.fees.maker,
            cfg.execution.fees.taker,
        )))
        .exchange(ExchangeKind::NoPartialFillExchange)
        .queue_model(queue)
        .depth(|| HashMapMarketDepth::new(INST.tick_size, INST.lot_size))
        .build()
        .expect("raw asset builds");
    Backtest::builder()
        .add_asset(asset)
        .build()
        .expect("raw engine builds")
}

fn raw_engine(rows: &[Event], cfg: &RunConfig) -> Backtest<HashMapMarketDepth> {
    match cfg.execution.queue_model {
        QueueModelConfig::RiskAdverse => raw_with(rows, cfg, RiskAdverseQueueModel::new()),
        QueueModelConfig::LogProb => {
            raw_with(rows, cfg, ProbQueueModel::new(LogProbQueueFunc::new()))
        }
        QueueModelConfig::LogProb2 => {
            raw_with(rows, cfg, ProbQueueModel::new(LogProbQueueFunc2::new()))
        }
        QueueModelConfig::PowerProb { n } => {
            raw_with(rows, cfg, ProbQueueModel::new(PowerProbQueueFunc::new(n)))
        }
        QueueModelConfig::PowerProb2 { n } => {
            raw_with(rows, cfg, ProbQueueModel::new(PowerProbQueueFunc2::new(n)))
        }
        QueueModelConfig::PowerProb3 { n } => {
            raw_with(rows, cfg, ProbQueueModel::new(PowerProbQueueFunc3::new(n)))
        }
    }
}

fn check_raw_replay(m: &Mkt, cfg: &RunConfig, out: &SimOutput) {
    // This is the only preprocessing: exactly the same constant feed offset as the adapter.
    let mut rows = m.rows.clone();
    for row in &mut rows {
        row.local_ts += cfg.execution.feed_latency_offset_ns;
    }
    let end = rows.iter().map(|e| e.local_ts).max().unwrap();
    let mut raw = raw_engine(&rows, cfg);
    let intents: BTreeMap<_, _> = out
        .tape
        .iter()
        .filter_map(|e| match e.body {
            FlightBody::OrderIntent {
                side,
                price_tick,
                qty,
                tif,
                ord_type,
                ..
            } => Some((e.order_id.unwrap(), (side, price_tick, qty, tif, ord_type))),
            _ => None,
        })
        .collect();
    let sent: Vec<_> = out
        .tape
        .iter()
        .filter(|e| matches!(e.body, FlightBody::RequestSent { .. }))
        .collect();
    let mut next = 0;
    loop {
        if raw.elapse(INTERVAL).expect("raw elapse") == ElapseResult::EndOfData {
            break;
        }
        let now = raw.current_timestamp();
        if now > end {
            break;
        }
        // Unique order ids make keeping terminal orders inert for matching. Do not clear them:
        // the adapter also retains terminal orders while their cancel response is in flight.
        while let Some(e) = sent.get(next).filter(|e| e.ts + ORIGIN <= now) {
            assert_eq!(e.ts + ORIGIN, now, "raw request timestamp differs");
            let id = e.order_id.unwrap();
            let FlightBody::RequestSent { req, .. } = e.body else {
                unreachable!()
            };
            match req {
                ReqKind::New => {
                    let (side, price, qty, tif, kind) = intents[&id];
                    let tif = match tif {
                        Tif::Gtc => TimeInForce::GTC,
                        Tif::Gtx => TimeInForce::GTX,
                        Tif::Ioc => TimeInForce::IOC,
                        Tif::Fok => TimeInForce::FOK,
                    };
                    let kind = match kind {
                        OrdType::Limit => HOrdType::Limit,
                        OrdType::Market => HOrdType::Market,
                    };
                    let result = match side {
                        Side::Buy => {
                            raw.submit_buy_order(0, id, INST.price(price), qty, tif, kind, false)
                        }
                        Side::Sell => {
                            raw.submit_sell_order(0, id, INST.price(price), qty, tif, kind, false)
                        }
                    };
                    result.expect("same new request accepted by raw engine");
                }
                ReqKind::Cancel => {
                    raw.cancel(0, id, false)
                        .expect("same cancel accepted by raw engine");
                }
                _ => panic!("unexpected request kind"),
            }
            next += 1;
        }
    }
    assert_eq!(next, sent.len(), "all requests replayed");
    let state = raw.state_values(0);
    let last = out.equity.last().unwrap();
    close(
        state.position,
        last.position,
        "raw position vs exchange ledger",
    );
    close(state.balance, last.cash, "raw balance vs ledger");
    close(state.fee, last.fees, "raw fees vs ledger");
    assert_eq!(
        state.num_trades,
        out.fills.len() as i64,
        "raw trades vs recorded fills"
    );
}

/// Count runs, not events: a single busy replay cannot conceal an otherwise idle corpus.
#[test]
fn generated_corpus_exercises_execution_paths() {
    let mut runner = TestRunner::new(ProptestConfig {
        rng_seed: RngSeed::Fixed(0x4242_4846_545f_5052),
        rng_algorithm: RngAlgorithm::ChaCha,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    let cases = (book_ops(), requests(), assumptions());
    let mut counts = BTreeMap::from([
        ("maker fills", 0usize),
        ("taker fills", 0),
        ("partial fills", 0),
        ("cancels accepted", 0),
        ("cancels rejected (lost race)", 0),
        ("queue fill_reset", 0),
        ("risk blocks", 0),
        ("refused requests", 0),
        ("working at end", 0),
        ("in-flight at end", 0),
        ("one-sided book periods", 0),
    ]);
    for _ in 0..512 {
        let (ops, reqs, a) = cases.new_tree(&mut runner).unwrap().current();
        let m = market(&ops);
        let cfg = config(&a);
        let out = run(&m, &cfg, &reqs);
        check_lifecycles(&out);
        check_ledger(&out);
        check_risk(&out, &cfg);

        let mut hit = |name, present| {
            if present {
                *counts.get_mut(name).unwrap() += 1;
            }
        };
        hit("maker fills", out.fills.iter().any(|f| f.maker));
        hit("taker fills", out.fills.iter().any(|f| !f.maker));
        hit(
            "partial fills",
            a.partial
                && out.tape.iter().any(|e| {
                    matches!(
                        e.body,
                        FlightBody::ExchResponse {
                            status: OrderStatus::PartiallyFilled,
                            ..
                        }
                    )
                }),
        );
        hit(
            "cancels accepted",
            out.tape.iter().any(|e| {
                matches!(
                    e.body,
                    FlightBody::ExchResponse {
                        status: OrderStatus::Canceled,
                        request_rejected: false,
                        ..
                    }
                )
            }),
        );
        hit(
            "cancels rejected (lost race)",
            out.tape.iter().any(|e| {
                matches!(
                    e.body,
                    FlightBody::ExchResponse {
                        request_rejected: true,
                        ..
                    }
                )
            }),
        );
        hit(
            "queue fill_reset",
            out.tape.iter().any(|e| {
                matches!(
                    e.body,
                    FlightBody::QueueUpdate {
                        cause: QueueCause::FillReset,
                        ..
                    }
                )
            }),
        );
        hit(
            "risk blocks",
            out.tape
                .iter()
                .any(|e| matches!(e.body, FlightBody::RiskBlocked { .. })),
        );
        hit(
            "refused requests",
            out.tape
                .iter()
                .any(|e| matches!(e.body, FlightBody::RequestRefused { .. })),
        );
        hit("working at end", out.equity.last().unwrap().open_orders > 0);
        hit(
            "in-flight at end",
            by_order(&out).values().any(|events| {
                events.iter().any(|e| {
                    matches!(
                        e.body,
                        FlightBody::RequestSent {
                            req: ReqKind::New,
                            ..
                        }
                    )
                }) && !events.iter().any(|e| {
                    matches!(
                        e.body,
                        FlightBody::LocalResponse {
                            request_rejected: false,
                            ..
                        }
                    )
                })
            }),
        );

        // Assess the book after an entire exchange timestamp, not halfway through a snapshot.
        let mut bids = BTreeSet::new();
        let mut asks = BTreeSet::new();
        let mut one_sided = false;
        for rows in m.rows.chunk_by(|a, b| a.exch_ts == b.exch_ts) {
            for row in rows.iter().filter(|row| row.ev & DEPTH_EVENT != 0) {
                let side = if row.ev & BUY_EVENT != 0 {
                    &mut bids
                } else {
                    &mut asks
                };
                let tick = INST.tick(row.px);
                if row.qty > 0.0 {
                    side.insert(tick);
                } else {
                    side.remove(&tick);
                }
            }
            one_sided |= bids.is_empty() != asks.is_empty();
        }
        hit("one-sided book periods", one_sided);
    }
    for (name, count) in counts {
        println!("{name}: {count}/512 runs");
        assert!(count > 0, "generator never exercised {name}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, max_shrink_iters: 4096, ..ProptestConfig::default() })]

    #[test]
    fn simulation_accepts_well_formed_markets(ops in book_ops(), reqs in requests(), a in assumptions()) {
        let out = run(&market(&ops), &config(&a), &reqs);
        for (seq, event) in out.tape.iter().enumerate() {
            prop_assert_eq!(event.seq, seq as u64);
            prop_assert_eq!(event.clock, event.body.clock());
            prop_assert_eq!(event.source, event.body.source());
        }
    }

    #[test]
    fn quantities_and_order_states_are_legal(ops in book_ops(), reqs in requests(), a in assumptions()) {
        check_lifecycles(&run(&market(&ops), &config(&a), &reqs));
    }

    #[test]
    fn fills_reconcile_to_the_ledger(ops in book_ops(), reqs in requests(), a in assumptions()) {
        check_ledger(&run(&market(&ops), &config(&a), &reqs));
    }

    #[test]
    fn hard_risk_limits_are_never_exceeded(ops in book_ops(), reqs in requests(), a in assumptions()) {
        let cfg = config(&a);
        check_risk(&run(&market(&ops), &cfg, &reqs), &cfg);
    }

    #[test]
    fn identical_inputs_have_identical_hashes(ops in book_ops(), reqs in requests(), a in assumptions()) {
        let m = market(&ops);
        let cfg = config(&a);
        let first = run(&m, &cfg, &reqs);
        let second = run(&m, &cfg, &reqs);
        prop_assert_eq!(sha256_json(&(&first.tape, &first.fills, &first.equity)), sha256_json(&(&second.tape, &second.fills, &second.equity)));
    }

    #[test]
    fn instrumentation_matches_the_unwrapped_engine(ops in book_ops(), mut reqs in requests(), mut a in assumptions()) {
        a.partial = false;
        a.jitter = false;
        // Compare settled exchange fills with the raw engine's local account. Other properties
        // retain end-of-data requests; here reserve enough time for every response to arrive.
        for request in &mut reqs { request.tick = request.tick.min(140); }
        let m = market(&ops);
        let cfg = config(&a);
        let out = run(&m, &cfg, &reqs);
        check_raw_replay(&m, &cfg, &out);
    }

    #[test]
    fn sparse_books_and_zero_trades_are_safe(reqs in requests(), a in assumptions()) {
        // Every case exercises each edge shape, not just a random choice of one shape.
        for shape in 0..4 {
            let mut m = Mkt::default();
            match shape {
                0 => { // Empty period between two complete books.
                    m.bid(0, 100.0, 1.0).ask(0, 101.0, 1.0)
                        .bid(50 * MS, 100.0, 0.0).ask(50 * MS, 101.0, 0.0)
                        .bid(150 * MS, 100.0, 1.0).ask(150 * MS, 101.0, 1.0);
                }
                1 => { m.bid(0, 100.0, 1.0); }
                2 => { m.ask(0, 101.0, 1.0); }
                _ => { m.bid(0, 100.0, 1.0).ask(0, 101.0, 1.0); }
            }
            // Keepalive updates the existing side, preserving one-sided shapes.
            if shape == 1 { m.bid(END, 100.0, 1.0); } else { m.ask(END, 101.0, 1.0); }
            let cfg = config(&a);
            let out = run(&m, &cfg, &reqs);
            check_lifecycles(&out);
            check_ledger(&out);
            check_risk(&out, &cfg);
        }
    }

    #[test]
    fn market_intents_are_blocked_before_the_engine(
        buy in any::<bool>(), lots in 1u8..16, tick in 0u8..25, a in assumptions()
    ) {
        let req = Request {
            tick, buy, offset: 0, lots, kind: 4, cancel_tick: 0, cancel_slot: 0,
        };
        let out = run(&market(&[]), &config(&a), &[req]);
        prop_assert_eq!(out.tape.len(), 2);
        prop_assert!(matches!(out.tape[0].body,
            FlightBody::OrderIntent { ord_type: OrdType::Market, .. }), "market intent missing");
        prop_assert!(matches!(out.tape[1].body,
            FlightBody::RiskBlocked { rule: RiskRule::InvalidOrder, .. }), "market intent not blocked");
        prop_assert!(out.fills.is_empty());
    }

    #[test]
    fn single_event_dataset_is_safe(bid in any::<bool>(), lots in 1u8..31, a in assumptions()) {
        let mut m = Mkt::default();
        if bid { m.bid(0, 100.0, f64::from(lots) * INST.lot_size); }
        else { m.ask(0, 101.0, f64::from(lots) * INST.lot_size); }
        let out = run(&m, &config(&a), &[]);
        prop_assert!(out.fills.is_empty());
        prop_assert!(out.tape.is_empty());
        check_ledger(&out);
    }
}
