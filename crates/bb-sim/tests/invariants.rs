//! Property-style checks: the bundled strategies run through the adapter on a generated market,
//! under every queue model and both exchange models, and the tape must obey the grammar that the
//! forensics layer is built on.

mod common;

use std::collections::BTreeMap;

use bb_core::{
    Clock, OrderStatus, ReqKind, Side,
    config::{
        AvellanedaStoikovParams, CubicLeg, ExchangeModel, GlftParams, ImbalanceTakerParams,
        LogNormalLeg, OrderLatencyConfig, QueueModelConfig, RunConfig, SigmaSource, StrategyConfig,
        StrategyKind, TouchMakerParams,
    },
    flight::{FlightBody, FlightEvent, QueueCause},
    hash::sha256_json,
    rng::Rng,
};
use bb_sim::{Event, SimInput, SimOutput, simulate};
use common::{INST, MS, ORIGIN, US, check_causality};
use hftbacktest::types::{
    BUY_EVENT, DEPTH_EVENT, EXCH_EVENT, LOCAL_EVENT, SELL_EVENT, TRADE_EVENT,
};

const LOT: f64 = INST.lot_size;
const FEED: i64 = 150 * US;

/// A random but well-formed L2 market in whole lots: never crossed, a trade is always followed by
/// the depth update it caused at the same exchange timestamp.
fn generate_market(seed: u64, events: usize) -> Vec<Event> {
    let mut rng = Rng::new(seed);
    let mut bids: BTreeMap<i64, i64> = BTreeMap::new();
    let mut asks: BTreeMap<i64, i64> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut now = ORIGIN;

    let push = |rows: &mut Vec<Event>, now: i64, kind: u64, side: u64, tick: i64, lots: i64| {
        rows.push(Event {
            ev: kind | side | EXCH_EVENT | LOCAL_EVENT,
            exch_ts: now,
            local_ts: now + FEED,
            px: tick as f64 * INST.tick_size,
            qty: lots as f64 * LOT,
            order_id: 0,
            ival: 0,
            fval: 0.0,
        });
    };

    for i in 0..12 {
        bids.insert(2000 - i, 40);
        asks.insert(2001 + i, 40);
        push(&mut rows, now, DEPTH_EVENT, BUY_EVENT, 2000 - i, 40);
        push(&mut rows, now, DEPTH_EVENT, SELL_EVENT, 2001 + i, 40);
    }

    for _ in 0..events {
        now += rng.range_i64(100 * US, 1_500 * US);
        let best_bid = *bids.keys().next_back().unwrap();
        let best_ask = *asks.keys().next().unwrap();
        let roll = rng.below(100);
        if roll < 22 {
            // Market trade against the best level of one side.
            let buy_aggressor = rng.chance(0.5);
            let (book, tick, agg, book_side) = if buy_aggressor {
                (&mut asks, best_ask, BUY_EVENT, SELL_EVENT)
            } else {
                (&mut bids, best_bid, SELL_EVENT, BUY_EVENT)
            };
            let level = book[&tick];
            let lots = rng.range_i64(1, level.min(25));
            push(&mut rows, now, TRADE_EVENT, agg, tick, lots);
            let left = level - lots;
            push(&mut rows, now, DEPTH_EVENT, book_side, tick, left);
            if left == 0 {
                book.remove(&tick);
                // Keep the side at least ten levels deep by extending it away from the touch.
                let far = if buy_aggressor {
                    *book.keys().next_back().unwrap() + 1
                } else {
                    *book.keys().next().unwrap() - 1
                };
                book.insert(far, 40);
                push(&mut rows, now, DEPTH_EVENT, book_side, far, 40);
            } else {
                book.insert(tick, left);
            }
        } else if roll < 60 {
            // Cancellation at one of the five best levels of a side; never empties a level.
            let on_bids = rng.chance(0.5);
            let (book, side) = if on_bids {
                (&mut bids, BUY_EVENT)
            } else {
                (&mut asks, SELL_EVENT)
            };
            let k = rng.below(5) as usize;
            let tick = if on_bids {
                *book.keys().rev().nth(k).unwrap()
            } else {
                *book.keys().nth(k).unwrap()
            };
            let level = book[&tick];
            if level > 1 {
                let left = level - rng.range_i64(1, (level - 1).min(15));
                book.insert(tick, left);
                push(&mut rows, now, DEPTH_EVENT, side, tick, left);
            }
        } else {
            // Limit add: usually near the touch, sometimes one tick inside a wide spread.
            let on_bids = rng.chance(0.5);
            let improve = best_ask - best_bid > 1 && rng.chance(0.5);
            let tick = match (on_bids, improve) {
                (true, true) => best_bid + 1,
                (false, true) => best_ask - 1,
                (true, false) => best_bid - rng.range_i64(0, 4),
                (false, false) => best_ask + rng.range_i64(0, 4),
            };
            let (book, side) = if on_bids {
                (&mut bids, BUY_EVENT)
            } else {
                (&mut asks, SELL_EVENT)
            };
            let lots = book.get(&tick).copied().unwrap_or(0) + rng.range_i64(1, 20);
            book.insert(tick, lots);
            push(&mut rows, now, DEPTH_EVENT, side, tick, lots);
        }
        let bb = *bids.keys().next_back().unwrap();
        let ba = *asks.keys().next().unwrap();
        assert!(bb < ba, "generator produced a crossed book");
    }
    rows
}

fn base_config(kind: StrategyKind) -> RunConfig {
    let mut cfg = common::config();
    cfg.strategy = StrategyConfig {
        interval_ns: 5 * MS,
        kind,
    };
    cfg.risk.max_position = 60.0 * LOT;
    cfg.risk.max_order_qty = 20.0 * LOT;
    cfg.risk.max_open_orders = 6;
    cfg
}

fn strategies() -> Vec<StrategyKind> {
    vec![
        StrategyKind::TouchMaker(TouchMakerParams {
            order_qty: 5.0 * LOT,
            max_position: 40.0 * LOT,
            offset_ticks: 0,
            skew_ticks_per_unit: 0.5,
        }),
        StrategyKind::AvellanedaStoikov(AvellanedaStoikovParams {
            order_qty: 5.0 * LOT,
            max_position: 40.0 * LOT,
            gamma: 0.2,
            kappa: 2.0,
            horizon_s: 1.0,
            vol_halflife_ticks: 20,
        }),
        StrategyKind::Glft(GlftParams {
            order_qty: 5.0 * LOT,
            max_position: 40.0 * LOT,
            gamma: 0.2,
            kappa: 2.0,
            intensity_a: 1.0,
            sigma_source: SigmaSource::WeightedMid,
            vol_halflife_ticks: 20,
        }),
        StrategyKind::ImbalanceTaker(ImbalanceTakerParams {
            order_qty: 8.0 * LOT,
            max_position: 40.0 * LOT,
            threshold: 0.4,
            hold_ticks: 12,
            cooldown_ticks: 4,
        }),
    ]
}

fn run(rows: &[Event], cfg: &RunConfig) -> SimOutput {
    simulate(
        &SimInput {
            events: rows,
            instrument: INST,
            origin_ns: ORIGIN,
            config: cfg,
        },
        bb_strategies::build(&cfg.strategy, &INST),
    )
    .unwrap_or_else(|e| panic!("{e}\nconfig: {cfg:?}"))
}

fn by_order(tape: &[FlightEvent]) -> BTreeMap<u64, Vec<&FlightEvent>> {
    let mut map: BTreeMap<u64, Vec<&FlightEvent>> = BTreeMap::new();
    for e in tape {
        if let Some(id) = e.order_id {
            map.entry(id).or_default().push(e);
        }
    }
    map
}

/// Everything the forensics layer assumes about a tape, checked for one run.
fn check_invariants(out: &SimOutput, cfg: &RunConfig, what: &str) {
    let half_lot = LOT * 0.5;
    for (i, e) in out.tape.iter().enumerate() {
        assert_eq!(e.seq, i as u64, "{what}: seq not contiguous");
        assert_eq!(e.clock, e.body.clock(), "{what}: clock mismatch");
        assert_eq!(e.source, e.body.source(), "{what}: source mismatch");
    }

    let mut total_filled = 0.0;
    for (id, events) in by_order(&out.tape) {
        let ctx = format!("{what}: order {id}");
        let FlightBody::OrderIntent {
            qty: order_qty,
            side,
            ..
        } = &events[0].body
        else {
            panic!(
                "{ctx}: first event is {}, not order_intent",
                events[0].body.type_name()
            );
        };
        let order_side = *side;

        // Exactly one disposition of the intent, directly after it.
        let second = events.get(1).map(|e| &e.body);
        assert!(
            matches!(
                second,
                Some(FlightBody::RiskBlocked { .. })
                    | Some(FlightBody::RequestRefused {
                        req: ReqKind::New,
                        ..
                    })
                    | Some(FlightBody::RequestSent {
                        req: ReqKind::New,
                        ..
                    })
            ),
            "{ctx}: second event is {:?}",
            second.map(|b| b.type_name())
        );
        let sent = matches!(second, Some(FlightBody::RequestSent { .. }));
        if !sent {
            assert_eq!(
                events.len(),
                2,
                "{ctx}: events after a blocked or refused intent"
            );
            continue;
        }

        // Per clock, time never goes backwards within an order.
        for clock in [Clock::Exchange, Clock::Local] {
            let mut last = i64::MIN;
            for e in events.iter().filter(|e| e.clock == clock) {
                // The truncation marker is appended at the end with an earlier timestamp.
                if matches!(e.body, FlightBody::QueueTruncated { .. }) {
                    continue;
                }
                assert!(
                    e.ts >= last,
                    "{ctx}: {clock:?} time went backwards at seq {}",
                    e.seq
                );
                last = e.ts;
            }
        }

        let mut filled = 0.0;
        let mut resting = false;
        let mut terminal = false;
        let mut exch_responses = 0u32;
        let mut local_responses = 0u32;
        for (i, e) in events.iter().enumerate() {
            match &e.body {
                FlightBody::ExchQueued {
                    queue_ahead,
                    level_qty,
                    ..
                } => {
                    assert!(!resting, "{ctx}: queued twice");
                    resting = true;
                    // A new order joins behind everything displayed, under every queue model.
                    assert_eq!(*queue_ahead, Some(*level_qty), "{ctx}: initial queue");
                }
                FlightBody::QueueUpdate {
                    ahead_before,
                    ahead_after,
                    ..
                } => {
                    assert!(
                        resting && !terminal,
                        "{ctx}: queue update while not resting"
                    );
                    assert!(
                        ahead_before.is_some() && ahead_after.is_some(),
                        "{ctx}: unreadable"
                    );
                }
                FlightBody::ExchFill {
                    qty,
                    maker,
                    leaves_qty,
                    value,
                    price_tick,
                    ..
                } => {
                    assert!(!terminal, "{ctx}: fill after terminal state");
                    assert!(*qty > half_lot, "{ctx}: empty fill");
                    assert_eq!(*maker, resting, "{ctx}: maker flag vs resting state");
                    let expect_value = INST.price(*price_tick) * qty * INST.contract_size;
                    assert!((value - expect_value).abs() < 1e-9, "{ctx}: fill value");
                    filled += qty;
                    assert!(
                        (order_qty - filled - leaves_qty).abs() < half_lot,
                        "{ctx}: leaves {leaves_qty} after filling {filled} of {order_qty}"
                    );
                    // The queue-exhausted grammar: such a fill directly follows the trade update
                    // that took the estimate negative, at the same instant.
                    if let Some(prev) = i.checked_sub(1).map(|j| events[j])
                        && let FlightBody::QueueUpdate {
                            cause: QueueCause::Trade { .. },
                            ahead_after: Some(after),
                            ..
                        } = prev.body
                        && after < 0.0
                    {
                        assert_eq!(prev.ts, e.ts, "{ctx}: fill not at the exhausting trade");
                        assert_eq!(prev.seq + 1, e.seq, "{ctx}: fill not adjacent to its trade");
                    }
                    // A fill is always followed by the response that reports it (a partial
                    // fill's queue reset may sit in between).
                    let next = events[i + 1..].iter().find(|n| {
                        !matches!(
                            n.body,
                            FlightBody::ExchFill { .. }
                                | FlightBody::QueueUpdate {
                                    cause: QueueCause::FillReset,
                                    ..
                                }
                        )
                    });
                    assert!(
                        matches!(next.map(|n| &n.body), Some(FlightBody::ExchResponse { .. })),
                        "{ctx}: fill at seq {} has no exchange response",
                        e.seq
                    );
                }
                FlightBody::ExchResponse {
                    status,
                    request_rejected,
                    local_arrival_ts,
                    response_latency_ns,
                    ..
                } => {
                    exch_responses += 1;
                    assert_eq!(*local_arrival_ts, e.ts + response_latency_ns, "{ctx}");
                    if !request_rejected && status.is_terminal() {
                        assert!(!terminal, "{ctx}: two terminal responses");
                        terminal = true;
                    }
                    if *status == OrderStatus::Filled {
                        assert!((order_qty - filled).abs() < half_lot, "{ctx}: Filled early");
                    }
                }
                FlightBody::LocalResponse { .. } => local_responses += 1,
                _ => {}
            }
        }
        assert!(filled <= order_qty + half_lot, "{ctx}: overfilled");
        assert!(
            local_responses <= exch_responses,
            "{ctx}: response from nowhere"
        );
        total_filled += order_side.sign() * filled;
    }

    // Ledger consistency.
    let signed_fills: f64 = out.fills.iter().map(|f| f.side.sign() * f.qty).sum();
    assert!(
        (signed_fills - total_filled).abs() < 1e-9,
        "{what}: fills vs tape"
    );
    let mut position = 0.0;
    for f in &out.fills {
        position += f.side.sign() * f.qty;
        assert!(
            (f.position_after - position).abs() < 1e-9,
            "{what}: running position"
        );
    }
    for w in out.fills.windows(2) {
        assert!(w[0].seq < w[1].seq, "{what}: fills out of tape order");
    }
    let last = out.equity.last().expect("at least one sample");
    assert!(
        (last.position - position).abs() < 1e-9,
        "{what}: final position"
    );
    assert!(
        last.position.abs() <= cfg.risk.max_position + half_lot,
        "{what}: position {} breaches the risk limit",
        last.position
    );
    for s in &out.equity {
        assert!(
            (s.equity - (s.realized_pnl + s.unrealized_pnl - s.fees)).abs() < 1e-9,
            "{what}: equity decomposition"
        );
        if let Some(mid_tick) = s.mid_tick {
            // Average-cost P&L must equal cash plus the marked position.
            let marked = s.cash + s.position * mid_tick * INST.tick_size * INST.contract_size;
            assert!(
                (s.realized_pnl + s.unrealized_pnl - marked).abs() < 1e-6,
                "{what}: accounting identity at tick {}: {} vs {marked}",
                s.tick_no,
                s.realized_pnl + s.unrealized_pnl
            );
        }
        assert!(
            s.position.abs() <= cfg.risk.max_position + half_lot,
            "{what}: sampled position breaches the risk limit"
        );
    }

    // With one Filled response per execution the engine's own account must match the ledger,
    // except for the engine's documented double count of a fill whose cancel lost the race
    // (defect 6), which the cross-check identifies by exact count.
    if cfg.execution.exchange_model == ExchangeModel::NoPartialFill {
        let c = &out.engine_check;
        assert!(
            c.agrees
                || (c
                    .note
                    .starts_with("Expected: The engine's local account also applied")
                    && c.engine_num_trades > c.ledger_fills as i64),
            "{what}: {c:?}"
        );
    }
}

#[test]
fn bundled_strategies_obey_the_tape_grammar_under_every_model() {
    let rows = generate_market(11, 12_000);
    let queue_models = [
        QueueModelConfig::RiskAdverse,
        QueueModelConfig::LogProb,
        QueueModelConfig::LogProb2,
        QueueModelConfig::PowerProb { n: 2.0 },
        QueueModelConfig::PowerProb2 { n: 2.0 },
        QueueModelConfig::PowerProb3 { n: 3.0 },
    ];
    // (order latency, think time)
    let latencies = [
        (
            OrderLatencyConfig::Constant {
                entry_ns: 800 * US,
                response_ns: 800 * US,
            },
            0,
        ),
        (
            OrderLatencyConfig::Jitter {
                entry_ns: 2 * MS,
                response_ns: 2 * MS,
                jitter_ns: 9 * MS,
            },
            0,
        ),
        (
            OrderLatencyConfig::LogNormal {
                entry: LogNormalLeg {
                    median_ns: 2 * MS,
                    sigma: 0.8,
                },
                response: LogNormalLeg {
                    median_ns: MS,
                    sigma: 0.5,
                },
                cancel: Some(LogNormalLeg {
                    median_ns: 500 * US,
                    sigma: 0.3,
                }),
            },
            0,
        ),
        (
            OrderLatencyConfig::HeavyTail {
                entry: CubicLeg {
                    min_ns: MS,
                    jitter: 0.5,
                    clip: 0.1,
                    unit: 10.0,
                },
                response: CubicLeg {
                    min_ns: 500 * US,
                    jitter: 0.5,
                    clip: 0.2,
                    unit: 10.0,
                },
                cancel: None,
            },
            2 * MS,
        ),
    ];
    let mut any_maker_fill = false;
    let mut any_taker_fill = false;
    let mut any_partial = false;
    for kind in strategies() {
        for exchange_model in [ExchangeModel::NoPartialFill, ExchangeModel::PartialFill] {
            for queue_model in queue_models {
                for (order_latency, think) in latencies {
                    let mut cfg = base_config(kind.clone());
                    cfg.execution.exchange_model = exchange_model;
                    cfg.execution.queue_model = queue_model;
                    cfg.execution.order_latency = order_latency;
                    cfg.execution.think_time_ns = think;
                    let what = format!(
                        "{} / {exchange_model:?} / {} / {order_latency:?} / think {think}",
                        kind.label(),
                        queue_model.label()
                    );
                    let out = run(&rows, &cfg);
                    assert!(out.ticks > 1_000, "{what}: only {} ticks", out.ticks);
                    check_invariants(&out, &cfg, &what);
                    check_causality(&out, &cfg);
                    any_maker_fill |= out.fills.iter().any(|f| f.maker);
                    any_taker_fill |= out.fills.iter().any(|f| !f.maker);
                    any_partial |= out.tape.iter().any(|e| {
                        matches!(
                            e.body,
                            FlightBody::ExchResponse {
                                status: OrderStatus::PartiallyFilled,
                                ..
                            }
                        )
                    });

                    // Same inputs, same outputs.
                    let again = run(&rows, &cfg);
                    assert_eq!(
                        sha256_json(&(&out.tape, &out.fills, &out.equity)),
                        sha256_json(&(&again.tape, &again.fills, &again.equity)),
                        "{what}: not reproducible"
                    );
                }
            }
        }
    }
    // The sweep must actually have exercised the paths it claims to check.
    assert!(
        any_maker_fill,
        "no maker fill occurred in any configuration"
    );
    assert!(
        any_taker_fill,
        "no taker fill occurred in any configuration"
    );
    assert!(any_partial, "no partial fill occurred in any configuration");
}

/// A feed latency offset must be accepted whenever it keeps local time after exchange time, and
/// must not disturb the invariants.
#[test]
fn feed_latency_offset_keeps_the_invariants() {
    let rows = generate_market(5, 6_000);
    let mut cfg = base_config(strategies().remove(0));
    cfg.execution.feed_latency_offset_ns = 3 * MS;
    let out = run(&rows, &cfg);
    check_invariants(&out, &cfg, "touch_maker with feed offset");
    // What the strategy saw at each decision is older by exactly the offset.
    let intent = out
        .tape
        .iter()
        .find_map(|e| match &e.body {
            FlightBody::OrderIntent { market, .. } => Some(*market),
            _ => None,
        })
        .expect("at least one intent");
    let (exch, local) = (intent.feed_exch_ts.unwrap(), intent.feed_local_ts.unwrap());
    assert_eq!(local - exch, FEED + 3 * MS);
}

/// Side sanity: a buy fill raises the position and a sell fill lowers it.
#[test]
fn fill_side_moves_position_in_the_right_direction() {
    let rows = generate_market(3, 6_000);
    let cfg = base_config(strategies().remove(0));
    let out = run(&rows, &cfg);
    assert!(!out.fills.is_empty());
    let mut prev = 0.0;
    for f in &out.fills {
        match f.side {
            Side::Buy => assert!(f.position_after > prev),
            Side::Sell => assert!(f.position_after < prev),
        }
        prev = f.position_after;
    }
}
