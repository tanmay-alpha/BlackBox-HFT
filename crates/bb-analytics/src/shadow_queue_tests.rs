use bb_data::{
    itch::decode::{
        AddOrder, FrameReader, Header, Message, OrderDelete, OrderExecuted, OrderReplace, Price4,
        Side, Stock, StockDirectory, decode,
    },
    l3synth::{L3SynthConfig, generate},
};
use hftbacktest::{
    backtest::models::{
        LogProbQueueFunc, LogProbQueueFunc2, PowerProbQueueFunc, PowerProbQueueFunc2,
        PowerProbQueueFunc3, ProbQueueModel, QueueModel, QueuePos, RiskAdverseQueueModel,
    },
    depth::{HashMapMarketDepth, L2MarketDepth},
    types::{OrdType, Order, Side as HSide, TimeInForce},
};

use super::*;

static SYM: [u8; 8] = *b"TEST    ";
static SUB: [u8; 2] = *b"  ";
const P: u32 = 1_000_000; // $100.0000
const ASK: u32 = 1_010_000;

fn h(ts: u64) -> Header {
    Header {
        stock_locate: 1,
        tracking_number: 0,
        timestamp_ns: ts,
    }
}

fn dir(ts: u64) -> Message<'static> {
    Message::StockDirectory(StockDirectory {
        header: h(ts),
        stock: Stock(&SYM),
        market_category: b'Q',
        financial_status: b'N',
        round_lot_size: 100,
        round_lots_only: b'N',
        issue_classification: b'C',
        issue_sub_type: &SUB,
        authenticity: b'P',
        short_sale_threshold: b'N',
        ipo_flag: b'N',
        luld_tier: b'1',
        etp_flag: b'N',
        etp_leverage_factor: 0,
        inverse: b'N',
    })
}

fn add(ts: u64, order_ref: u64, side: Side, shares: u32, price: u32) -> Message<'static> {
    Message::AddOrder(AddOrder {
        header: h(ts),
        order_ref,
        side,
        shares,
        stock: Stock(&SYM),
        price: Price4(price),
        attribution: None,
    })
}

fn exec(ts: u64, order_ref: u64, shares: u32) -> Message<'static> {
    Message::OrderExecuted(OrderExecuted {
        header: h(ts),
        order_ref,
        shares,
        match_number: ts * 1_000 + order_ref,
    })
}

fn delete(ts: u64, order_ref: u64) -> Message<'static> {
    Message::OrderDelete(OrderDelete {
        header: h(ts),
        order_ref,
    })
}

fn replace(ts: u64, original_ref: u64, new_ref: u64, shares: u32, price: u32) -> Message<'static> {
    Message::OrderReplace(OrderReplace {
        header: h(ts),
        original_ref,
        new_ref,
        shares,
        price: Price4(price),
    })
}

/// One buy shadow (k = 0) arriving at 1,000 ns; horizons 100 ns and 10 µs; checkpoints 5 and
/// 50 ns; window end 1 ms.
fn design(latencies: Vec<u64>) -> Design {
    Design {
        window_start_ns: 1_000,
        window_end_ns: 1_000_000,
        interval_ns: 1_000_000,
        shares: 100,
        lot_size_shares: 1,
        latencies_ns: latencies,
        horizons_ns: vec![100, 10_000],
        checkpoints_ns: vec![5, 50],
        models: ModelSpec::design_set(),
        rules: Vec::new(),
    }
}

/// Bids at P: order 1 (30 shares) then order 2 (20); one ask above.
fn base() -> Vec<Message<'static>> {
    vec![
        dir(1),
        add(5, 100, Side::Sell, 50, ASK),
        add(10, 1, Side::Buy, 30, P),
        add(20, 2, Side::Buy, 20, P),
    ]
}

fn run_with(design: Design, msgs: &[Message<'_>]) -> ShadowRun {
    let mut engine = ShadowEngine::new(design, &["TEST".to_string()]).unwrap();
    for m in msgs {
        engine.on_message(m);
    }
    engine.finish()
}

fn run(mut msgs: Vec<Message<'static>>) -> ShadowRun {
    msgs.push(add(2_000_000, 999, Side::Sell, 1, ASK + 100)); // reaches the window end
    run_with(design(vec![0]), &msgs)
}

fn model(id: &str) -> usize {
    ModelSpec::design_set()
        .iter()
        .position(|m| m.id() == id)
        .unwrap()
}

fn fill(elapsed_ns: u64, cause: FillCause) -> Option<Fill> {
    Some(Fill { elapsed_ns, cause })
}

#[test]
fn cancel_ahead_advances_the_l3_queue_and_every_model_agrees_on_a_fifo_fill() {
    let mut msgs = base();
    msgs.extend([
        delete(1_010, 1),
        exec(1_020, 2, 20),
        add(1_030, 3, Side::Buy, 10, P),
        exec(1_040, 3, 5),
    ]);
    let r = run(msgs);
    assert!(r.stream_reached_window_end);
    assert_eq!(r.shadows.len(), 1);
    let s = &r.shadows[0];
    assert_eq!((s.side, s.price, s.initial_ahead), (Side::Buy, P, 50));
    assert_eq!(s.l3, fill(40, FillCause::Queue));
    // The delete left nothing behind the shadow, so every model credits it ahead.
    for (m, f) in ModelSpec::design_set().iter().zip(&s.models) {
        assert_eq!(*f, fill(40, FillCause::Queue), "{}", m.id());
    }
    assert_eq!(s.cancelled, vec![(30, 0), (30, 0)]);
}

#[test]
fn cancel_behind_leaves_l3_unchanged_while_probability_models_advance() {
    let mut msgs = base();
    msgs.extend([
        add(1_010, 3, Side::Buy, 10, P),
        delete(1_020, 3),
        add(1_060, 101, Side::Sell, 5, ASK + 100),
    ]);
    let r = run(msgs);
    let s = &r.shadows[0];
    assert_eq!(s.l3, None);
    assert_eq!(s.cancelled, vec![(0, 10), (0, 10)]);
    let cp = s.checkpoints[1]
        .as_ref()
        .expect("checkpoint at 50 ns was reached");
    assert_eq!(cp.l3_ahead, Some(50));
    assert_eq!(cp.fronts[model("risk_adverse")], Some(50.0));
    // power_prob3 n = 3: prev 60, new 50, front 50, back 10, p = 1 - (5/6)^3.
    let p = 1.0 - (50.0f64 / 60.0).powi(3);
    let expected = 50.0 - (1.0 - p) * 10.0 + (10.0 - p * 10.0).min(0.0);
    let front = cp.fronts[model("power_prob3_n3")].unwrap();
    assert!((front - expected).abs() < 1e-12 && (front - 44.212_962_962_962_96).abs() < 1e-9);
    // The checkpoint at 5 ns precedes both messages.
    let early = s.checkpoints[0].as_ref().unwrap();
    assert_eq!(early.fronts[model("power_prob3_n3")], Some(50.0));

    let agg = aggregate(&design(vec![0]), &r);
    let cell = |id: &str| {
        agg.checkpoints
            .iter()
            .find(|c| c.model == id && c.checkpoint_ns == 50)
            .unwrap()
    };
    assert_eq!(cell("risk_adverse").mean_abs_error_shares.get(), Some(0.0));
    let err = cell("power_prob3_n3")
        .mean_signed_error_shares
        .get()
        .unwrap();
    assert!((err - (expected - 50.0)).abs() < 1e-12);
    let cancelled = &agg.cancelled_volume[0];
    assert_eq!((cancelled.ahead_shares, cancelled.behind_shares), (0, 10));
    assert_eq!(cancelled.share_ahead.get(), Some(0.0));
}

#[test]
fn partial_executions_ahead_never_fill_the_shadow() {
    let mut msgs = base();
    msgs.extend([exec(1_010, 1, 10), exec(1_020, 1, 20), exec(1_030, 2, 20)]);
    let r = run(msgs);
    let s = &r.shadows[0];
    assert_eq!(s.l3, None);
    assert!(s.models.iter().all(Option::is_none), "{:?}", s.models);
    assert_eq!(s.cancelled, vec![(0, 0), (0, 0)]);
    // Unfilled when its longest horizon (10 µs) passed inside the window: observed through it.
    assert_eq!(s.observed_ns, 10_000);
}

#[test]
fn an_execution_through_the_shadow_price_fills_every_path() {
    let mut msgs = base();
    msgs.extend([
        add(1_001, 4, Side::Buy, 10, P - 100),
        delete(1_002, 1),
        delete(1_003, 2),
        exec(1_010, 4, 5),
    ]);
    let r = run(msgs);
    let s = &r.shadows[0];
    assert_eq!(s.l3, fill(10, FillCause::TradeThrough));
    assert!(
        s.models
            .iter()
            .all(|f| *f == fill(10, FillCause::TradeThrough))
    );
}

#[test]
fn an_opposite_quote_at_the_shadow_price_fills_as_a_book_cross() {
    let mut msgs = base();
    msgs.extend([add(1_010, 5, Side::Sell, 10, P)]);
    let r = run(msgs);
    let s = &r.shadows[0];
    assert_eq!(s.l3, fill(10, FillCause::BookCross));
    assert!(
        s.models
            .iter()
            .all(|f| *f == fill(10, FillCause::BookCross))
    );
}

#[test]
fn a_replace_loses_priority_in_l3_but_not_under_risk_adverse() {
    let mut msgs = base();
    msgs.extend([
        replace(1_010, 1, 5, 30, P),
        exec(1_020, 2, 20),
        exec(1_030, 5, 1),
    ]);
    let r = run(msgs);
    let s = &r.shadows[0];
    // The replaced order's 30 shares left the queue ahead; its new order is behind.
    assert_eq!(s.cancelled[0], (30, 0));
    assert_eq!(s.l3, fill(30, FillCause::Queue));
    // An unchanged L2 level (50 -> 50) gives RiskAdverse no credit: front 50 - 20 - 1 = 29.
    assert_eq!(s.models[model("risk_adverse")], None);
}

#[test]
fn same_timestamp_messages_keep_file_order_and_arrivals_precede_them() {
    let mut msgs = base();
    msgs.extend([
        add(999, 6, Side::Buy, 7, P),
        add(1_000, 7, Side::Buy, 9, P),
        delete(1_000, 1),
        delete(1_000, 2),
        delete(1_000, 6),
        exec(1_000, 7, 1),
        add(1_500, 102, Side::Sell, 1, ASK + 200),
        add(2_000_000, 999, Side::Sell, 1, ASK + 100),
    ]);
    let r = run_with(design(vec![0, 1]), &msgs);
    assert_eq!(r.shadows.len(), 2);
    // L = 0: placed before the first message stamped 1,000 ns, behind order 6 (999 ns).
    let zero = &r.shadows[0];
    assert_eq!((zero.latency, zero.initial_ahead), (0, 57));
    assert_eq!(zero.l3, fill(0, FillCause::Queue));
    assert_eq!(zero.cancelled[0], (57, 0));
    // L = 1 ns: placed after every message stamped 1,000 ns, behind order 7's 8 shares.
    let one = &r.shadows[1];
    assert_eq!(
        (one.latency, one.arrival_ns, one.initial_ahead),
        (1, 1_001, 8)
    );
    assert_eq!(one.l3, None);
}

#[test]
fn censoring_at_the_end_of_the_stream_limits_eligibility() {
    let mut msgs = base();
    msgs.push(add(1_500, 103, Side::Sell, 1, ASK + 100));
    let d = design(vec![0]);
    let r = run_with(d.clone(), &msgs);
    assert!(!r.stream_reached_window_end);
    assert_eq!(r.tracking_end_ns, 1_500);
    let s = &r.shadows[0];
    assert_eq!((s.l3, s.observed_ns), (None, 500));
    assert!(s.checkpoints.iter().all(Option::is_some));
    let agg = aggregate(&d, &r);
    let cell = |h: u64| {
        agg.cells
            .iter()
            .find(|c| c.model == L3_REFERENCE && c.horizon_ns == h)
            .unwrap()
    };
    // 1,000 + 100 <= 1,500: eligible and unfilled.
    assert_eq!((cell(100).eligible, cell(100).filled), (1, 0));
    assert_eq!(cell(100).fill_fraction.get(), Some(0.0));
    // 1,000 + 10,000 > 1,500: not eligible; Kaplan–Meier keeps it as censored.
    assert_eq!(cell(10_000).eligible, 0);
    assert!(cell(10_000).fill_fraction.get().is_none());
    assert_eq!((cell(10_000).placed, cell(10_000).km_censored), (1, 1));
    assert_eq!(agg.symbols[0].placed, 1);
}

#[test]
fn messages_at_or_after_the_window_end_are_ignored() {
    let mut msgs = base();
    msgs.push(add(1_000_000, 104, Side::Sell, 1, ASK + 100));
    msgs.push(add(1_000_001, 5, Side::Sell, 10, P));
    let r = run_with(design(vec![0]), &msgs);
    assert!(r.stream_reached_window_end);
    assert_eq!(r.shadows[0].l3, None);
    assert_eq!(r.messages, 4);
}

#[test]
fn empty_side_and_locked_book_skip_placement() {
    let empty = run(vec![dir(1), add(5, 100, Side::Sell, 50, ASK)]);
    assert_eq!(empty.shadows.len(), 0);
    assert_eq!(empty.symbols[0].skipped_empty_side, 1);
    let locked = run(vec![
        dir(1),
        add(5, 100, Side::Sell, 50, P),
        add(10, 1, Side::Buy, 30, P),
    ]);
    assert_eq!(locked.symbols[0].skipped_locked_or_crossed, 1);
    assert_eq!(locked.symbols[0].scheduled, 1);
}

#[test]
fn wilson_and_newcombe_match_published_values() {
    let (lo, hi) = wilson(0, 10).unwrap();
    assert_eq!(lo, 0.0);
    assert!((hi - Z95 * Z95 / (10.0 + Z95 * Z95)).abs() < 1e-12);
    let (lo, _) = wilson(10, 10).unwrap();
    assert!((lo - 10.0 / (10.0 + Z95 * Z95)).abs() < 1e-12);
    assert!(wilson(0, 0).is_none());
    // Newcombe (1998), Table III, method 10.
    for ((e, f, g, h), (lo, hi)) in [
        ((36, 12, 2, 0), (0.0569, 0.3404)),
        ((20, 12, 2, 16), (0.0562, 0.3292)),
        ((18, 12, 2, 18), (0.0562, 0.3290)),
    ] {
        let (_, l, u) = newcombe_paired(e, f, g, h).unwrap();
        assert!(
            (l - lo).abs() < 5e-5 && (u - hi).abs() < 5e-5,
            "{e} {f} {g} {h}: {l} {u}"
        );
    }
}

/// The offline formulas against hftbacktest 0.9.4's own `QueueModel` implementations on the same
/// call sequence, compared bit for bit after every call.
#[test]
fn offline_formulas_equal_hftbacktest_queue_models() {
    fn front(order: &Order) -> f64 {
        if let Some(f) = order.q.as_any().downcast_ref::<f64>() {
            return *f;
        }
        let pos = order.q.as_any().downcast_ref::<QueuePos>().unwrap();
        let debug = format!("{pos:?}");
        let rest = debug.split_once("front_q_qty: ").unwrap().1;
        rest[..rest.find(',').unwrap()].parse().unwrap()
    }
    fn engine(spec: &ModelSpec) -> Box<dyn QueueModel<HashMapMarketDepth>> {
        let n = f64::from(spec.n.unwrap_or(1));
        match spec.family {
            Family::RiskAdverse => Box::new(RiskAdverseQueueModel::new()),
            Family::LogProb => Box::new(ProbQueueModel::new(LogProbQueueFunc::new())),
            Family::LogProb2 => Box::new(ProbQueueModel::new(LogProbQueueFunc2::new())),
            Family::PowerProb => Box::new(ProbQueueModel::new(PowerProbQueueFunc::new(n))),
            Family::PowerProb2 => Box::new(ProbQueueModel::new(PowerProbQueueFunc2::new(n))),
            Family::PowerProb3 => Box::new(ProbQueueModel::new(PowerProbQueueFunc3::new(n))),
        }
    }
    let mut specs = ModelSpec::design_set();
    for family in [Family::PowerProb, Family::PowerProb2, Family::PowerProb3] {
        specs.push(ModelSpec::new(family, Some(1)));
    }
    let mut seed = 0x5eed_u64;
    let mut next = |below: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % below
    };
    let mut calls = 0u64;
    for spec in &specs {
        let model = engine(spec);
        for _ in 0..200 {
            let mut depth = HashMapMarketDepth::new(0.01, 1.0);
            let displayed = (1 + next(500)) as f64;
            depth.update_bid_depth(100.0, displayed, 0);
            let mut order = Order::new(
                1,
                10_000,
                0.01,
                100.0,
                HSide::Buy,
                OrdType::Limit,
                TimeInForce::GTX,
            );
            model.new_order(&mut order, &depth);
            let mut mine = QueueState::new(displayed);
            let mut level = displayed;
            assert_eq!(front(&order).to_bits(), mine.front.to_bits());
            for _ in 0..40 {
                if next(3) == 0 {
                    let qty = (1 + next(60)) as f64;
                    model.trade(&mut order, qty, &depth);
                    mine.trade(spec, qty);
                    assert_eq!(
                        front(&order).to_bits(),
                        mine.front.to_bits(),
                        "{}",
                        spec.id()
                    );
                    let theirs = model.is_filled(&mut order, &depth);
                    assert_eq!(
                        theirs.to_bits(),
                        mine.is_filled(1.0).to_bits(),
                        "{}",
                        spec.id()
                    );
                } else {
                    let new = next(700) as f64;
                    model.depth(&mut order, level, new, &depth);
                    mine.depth(spec, level, new);
                    level = new;
                }
                assert_eq!(
                    front(&order).to_bits(),
                    mine.front.to_bits(),
                    "{}",
                    spec.id()
                );
                calls += 1;
            }
        }
    }
    assert_eq!(calls, 12 * 200 * 40);
}

fn synthetic_run(design: &Design, stop_before_ns: Option<u64>) -> ShadowRun {
    let bytes = generate(&L3SynthConfig::default()).unwrap();
    let mut frames = FrameReader::new(bytes.as_slice());
    let mut engine = ShadowEngine::new(design.clone(), &["SYNTH".to_string()]).unwrap();
    while let Some(frame) = frames.next_frame().unwrap() {
        let msg = decode(frame).unwrap();
        if stop_before_ns.is_some_and(|t| msg.header().is_some_and(|h| h.timestamp_ns >= t)) {
            break;
        }
        engine.on_message(&msg);
    }
    engine.finish()
}

#[test]
fn synthetic_output_is_deterministic() {
    let design = Design::preregistered();
    let a = synthetic_run(&design, None);
    let b = synthetic_run(&design, None);
    assert!(a.shadows.len() > 10, "{}", a.shadows.len());
    assert_eq!(a, b);
    let ja = serde_json::to_vec(&aggregate(&design, &a)).unwrap();
    let jb = serde_json::to_vec(&aggregate(&design, &b)).unwrap();
    assert_eq!(ja, jb);
}

/// A shadow resolved before time T has the same outcome whether or not the stream continues
/// after T: nothing later in the stream can change it.
#[test]
fn outcomes_never_depend_on_later_messages() {
    let design = Design::preregistered();
    let full = synthetic_run(&design, None);
    let cut = design.window_start_ns + 4 * NS_PER_S;
    let truncated = synthetic_run(&design, Some(cut));
    let longest = design.longest_ns();
    let mut compared = 0;
    for s in &truncated.shadows {
        let resolved = s.l3.is_some() && s.models.iter().all(Option::is_some);
        if !resolved && s.arrival_ns + longest > truncated.tracking_end_ns {
            continue;
        }
        let twin = full
            .shadows
            .iter()
            .find(|f| (f.symbol, f.latency, f.k) == (s.symbol, s.latency, s.k))
            .unwrap();
        assert_eq!(s, twin);
        compared += 1;
    }
    assert!(compared > 0);
    // Placements before the cut are identical too.
    for s in &truncated.shadows {
        let twin = full
            .shadows
            .iter()
            .find(|f| (f.symbol, f.latency, f.k) == (s.symbol, s.latency, s.k))
            .unwrap();
        assert_eq!(
            (s.side, s.price, s.arrival_ns, s.initial_ahead),
            (twin.side, twin.price, twin.arrival_ns, twin.initial_ahead)
        );
    }
}

#[test]
fn review_exact_longest_horizon_fill_is_included() {
    let mut msgs = base();
    msgs.extend([
        delete(1_010, 1),
        delete(1_020, 2),
        add(1_030, 3, Side::Buy, 10, P),
        exec(11_000, 3, 1),
        add(12_000, 4, Side::Sell, 1, ASK + 100),
    ]);
    let d = design(vec![0]);
    let r = run_with(d.clone(), &msgs);
    let agg = aggregate(&d, &r);
    let cell = agg
        .cells
        .iter()
        .find(|c| c.model == L3_REFERENCE && c.horizon_ns == 10_000)
        .unwrap();
    assert_eq!(cell.eligible, 1);
    assert_eq!(
        cell.filled, 1,
        "execution exactly at arrival + tau must be included by <= tau"
    );
}

#[test]
fn review_checkpoint_at_horizon_excludes_already_filled_paths() {
    let mut msgs = base();
    msgs.extend([
        delete(1_010, 1),
        delete(1_020, 2),
        add(1_030, 3, Side::Buy, 10, P),
        exec(1_100, 3, 1),
        add(1_200, 4, Side::Sell, 1, ASK + 100),
    ]);
    let mut d = design(vec![0]);
    d.checkpoints_ns = vec![100];
    let r = run_with(d.clone(), &msgs);
    let agg = aggregate(&d, &r);
    let c = agg
        .checkpoints
        .iter()
        .find(|c| c.model == "risk_adverse")
        .unwrap();
    assert_eq!(
        c.pairs, 0,
        "a fill exactly at the checkpoint is already filled under <= tau"
    );
}

#[test]
fn same_timestamp_checkpoint_uses_the_whole_group_without_look_ahead() {
    let mut msgs = base();
    msgs.extend([
        add(1_000, 3, Side::Buy, 10, P),
        delete(1_100, 1),
        delete(1_100, 3),
        exec(1_100, 2, 5),
    ]);
    let mut d = design(vec![0]);
    d.checkpoints_ns = vec![100, 10_000];
    // Finishing on the boundary must flush the final timestamp, even without a later message.
    let cut = run_with(d.clone(), &msgs);
    let cp = cut.shadows[0].checkpoints[0].as_ref().unwrap();
    assert_eq!(cp.l3_ahead, Some(15));
    assert_eq!(cp.fronts[model("risk_adverse")], Some(15.0));
    assert_eq!(cut.shadows[0].cancelled[0], (30, 10));
    assert!(cut.shadows[0].checkpoints[1].is_none());
    msgs.extend([delete(1_101, 2), add(11_001, 4, Side::Sell, 1, ASK + 100)]);
    let full = run_with(d, &msgs);
    assert_eq!(
        full.shadows[0].checkpoints[0],
        cut.shadows[0].checkpoints[0]
    );
    assert_eq!(full.shadows[0].cancelled[0], cut.shadows[0].cancelled[0]);
}

#[test]
fn same_timestamp_longest_deadline_keeps_paths_until_the_group_ends() {
    let mut d = design(vec![0]);
    d.checkpoints_ns = vec![10_000];
    let mut msgs = base();
    msgs.extend([
        add(1_000, 3, Side::Buy, 10, P),
        exec(11_000, 1, 30),
        exec(11_000, 2, 20),
        exec(11_000, 3, 1),
    ]);
    let cut = run_with(d.clone(), &msgs);
    let sh = &cut.shadows[0];
    assert_eq!(sh.l3, fill(10_000, FillCause::Queue));
    assert!(sh.models.iter().all(|f| *f == sh.l3));
    let cp = sh.checkpoints[0].as_ref().unwrap();
    assert_eq!(cp.l3_ahead, None);
    assert!(cp.fronts.iter().all(Option::is_none));
    assert_eq!(sh.cancelled[1], (0, 0));
    assert!(
        aggregate(&d, &cut)
            .cells
            .iter()
            .all(|c| { c.horizon_ns != 10_000 || (c.eligible == 1 && c.filled == 1) })
    );
    msgs.push(add(11_001, 4, Side::Sell, 1, ASK + 100));
    assert_eq!(run_with(d, &msgs).shadows, cut.shadows);
}

#[test]
fn a_fill_after_the_longest_deadline_is_excluded() {
    let mut msgs = base();
    msgs.extend([
        delete(11_000, 1),
        delete(11_000, 2),
        add(11_000, 3, Side::Buy, 10, P),
        exec(11_001, 3, 1),
    ]);
    let d = design(vec![0]);
    let r = run_with(d.clone(), &msgs);
    let sh = &r.shadows[0];
    assert_eq!(sh.l3, None);
    assert!(sh.models.iter().all(Option::is_none));
    assert_eq!(sh.observed_ns, 10_000);
    assert_eq!(sh.cancelled[1], (50, 0));
    assert!(aggregate(&d, &r).cells.iter().all(|c| c.filled == 0));
}
