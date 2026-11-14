use super::*;
use proptest::prelude::*;

fn instrument() -> InstrumentMeta {
    InstrumentMeta {
        tick_size: 0.5,
        lot_size: 1.0,
        contract_size: 1.0,
    }
}
fn event(ts: i64, tick: i64, qty: i64, flags: u64) -> Event {
    Event {
        ev: flags | EXCHANGE,
        exch_ts: ts,
        local_ts: ts,
        px: if flags & 0xff == CLEAR {
            f64::NAN
        } else {
            tick as f64 * 0.5
        },
        qty: qty as f64,
        order_id: 0,
        ival: 0,
        fval: 0.0,
    }
}
fn bid(ts: i64, tick: i64, qty: i64) -> Event {
    event(ts, tick, qty, DEPTH | BUY)
}
fn ask(ts: i64, tick: i64, qty: i64) -> Event {
    event(ts, tick, qty, DEPTH | SELL)
}
fn trade(ts: i64, tick: i64, qty: i64, side: Option<Side>) -> Event {
    event(
        ts,
        tick,
        qty,
        TRADE
            | match side {
                Some(Side::Buy) => BUY,
                Some(Side::Sell) => SELL,
                None => 0,
            },
    )
}
fn fixture(ts: i64) -> Vec<Event> {
    vec![
        bid(ts, 200, 3),
        bid(ts, 199, 5),
        bid(ts, 198, 2),
        ask(ts, 202, 1),
        ask(ts, 203, 4),
        ask(ts, 204, 6),
    ]
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}
fn metric<'a>(report: &'a MarketReport, name: &str) -> &'a MetricSummary {
    report.metrics.iter().find(|m| m.name == name).unwrap()
}
fn report(events: &[Event]) -> MarketReport {
    compute_market(
        events,
        &instrument(),
        0,
        &MarketConfig {
            series_max_points: Some(100),
            ..MarketConfig::default()
        },
    )
    .unwrap()
}
fn l1(b: i64, qb: i64, a: i64, qa: i64) -> Bbo {
    Bbo {
        bid: Some((b, qb as f64)),
        ask: Some((a, qa as f64)),
    }
}

/// F: S=2 ticks, relative spread=40000/402 bps, band depths 8/5,
/// notionals 797.5/507; OBI=(.5,3/13,-1/21), opposite-size weighted mid=100.75.
#[test]
fn fixtures_m1_through_m5() {
    let r = report(&fixture(0));
    for (name, expected) in [
        ("quoted_spread", 2.0),
        ("relative_spread", 40_000.0 / 402.0),
        ("bid_depth", 8.0),
        ("bid_notional", 797.5),
        ("ask_depth", 5.0),
        ("ask_notional", 507.0),
        ("imbalance_1", 0.5),
        ("imbalance_2", 3.0 / 13.0),
        ("imbalance_3", -1.0 / 21.0),
        ("weighted_mid", 100.75),
    ] {
        near(metric(&r, name).event_mean.get().unwrap(), expected);
    }
    assert!(!r.bid_depth_truncated && !r.ask_depth_truncated);
    assert!(metric(&r, "stoikov_microprice").event_mean.get().is_none());
}

/// S=2 for 300ms, 4 for 100ms: duration mean 2.5 versus BBO-event mean 3.
#[test]
fn fixture_m1_time_weighting_differs_from_events() {
    let mut e = vec![bid(0, 200, 3), ask(0, 202, 1)];
    e.extend([
        ask(300_000_000, 202, 0),
        ask(300_000_000, 204, 1),
        ask(400_000_000, 204, 1),
    ]);
    let r = report(&e);
    near(metric(&r, "quoted_spread").event_mean.get().unwrap(), 3.0);
    near(
        metric(&r, "quoted_spread")
            .time_weighted_mean
            .get()
            .unwrap(),
        2.5,
    );
    assert_eq!(r.event_series.unwrap().points.len(), 2);
}

#[test]
fn m3_band_boundary_is_inclusive_and_unknown_tail_is_flagged() {
    // Mid=100, 100bps lower boundary is exactly 99 (tick198).
    let e = vec![
        bid(0, 199, 2),
        bid(0, 198, 7),
        ask(0, 201, 3),
        ask(0, 202, 4),
    ];
    let r = report(&e);
    near(metric(&r, "bid_depth").event_mean.get().unwrap(), 9.0);
    assert!(!r.bid_depth_truncated);
    let shallow = report(&[bid(0, 200, 3), ask(0, 202, 1)]);
    assert!(shallow.bid_depth_truncated && shallow.ask_depth_truncated);
    assert!(metric(&shallow, "imbalance_2").event_mean.get().is_none());
}

fn model_config() -> StoikovConfig {
    StoikovConfig {
        train_start_ns: 0,
        train_end_ns: 100,
        test_start_ns: 101,
        test_end_ns: 200,
        imbalance_buckets: 2,
        spread_ticks: vec![2],
        min_transitions_per_state: 1,
        max_iterations: 1000,
        sampling: CalibrationSampling::Event,
    }
}
/// G1=(1/6,-1/6), B=[[5/9,4/9],[4/9,5/9]], G2=(1/54,-1/54).
/// Geometric sum is (3/16,-3/16) ticks; F in H gives 100.59375.
#[test]
fn fixture_m6_lu_and_convergent_stoikov_series() {
    // Public model order is L,H (the research table writes H,L).
    let q = vec![vec![0.3, 0.2], vec![0.2, 0.3]];
    let model = solve_stoikov(
        model_config(),
        0.5,
        vec![100, 100],
        q.clone(),
        q,
        vec![-0.15, 0.15],
    )
    .unwrap();
    near(model.first_adjustment_ticks[1], 1.0 / 6.0);
    near(model.move_transition[0][0], 5.0 / 9.0);
    near(model.move_transition[0][1], 4.0 / 9.0);
    near(
        model.move_transition[1][0] * model.first_adjustment_ticks[0]
            + model.move_transition[1][1] * model.first_adjustment_ticks[1],
        1.0 / 54.0,
    );
    near(model.adjustment_ticks[1], 3.0 / 16.0);
    near(
        model
            .evaluate(l1(200, 3, 202, 1), &instrument(), 150)
            .get()
            .unwrap(),
        100.59375,
    );
    assert!(
        model
            .evaluate(l1(200, 3, 202, 1), &instrument(), 100)
            .get()
            .is_none()
    );
    assert!(
        model
            .evaluate(l1(200, 3, 203, 1), &instrument(), 150)
            .get()
            .is_none()
    );
    assert!(model.residual_infinity_ticks < 1e-12);
}

#[test]
fn m6_singular_and_nonconvergent_chains_fail() {
    let q = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    assert!(matches!(
        solve_stoikov(
            model_config(),
            0.5,
            vec![1, 1],
            q,
            vec![vec![0.0; 2]; 2],
            vec![0.0, 0.0]
        ),
        Err(MarketError::Calibration(_))
    ));
    let q = vec![vec![0.0; 2]; 2];
    let t = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    assert!(matches!(
        solve_stoikov(model_config(), 0.5, vec![1, 1], q, t, vec![-0.1, 0.1]),
        Err(MarketError::Calibration(_))
    ));
}

#[test]
fn m6_calibration_uses_training_only_and_mirrors_observations() {
    let mut e = vec![
        bid(0, 200, 3),
        ask(0, 202, 1),
        bid(10, 201, 3),
        bid(10, 200, 0),
        ask(10, 202, 0),
        ask(10, 203, 1),
        bid(20, 201, 1),
        ask(20, 203, 3),
        bid(30, 201, 0),
        bid(30, 200, 1),
        ask(30, 203, 0),
        ask(30, 202, 3),
        bid(100, 200, 3),
        ask(100, 202, 1),
        bid(200, 200, 3),
    ];
    let a = calibrate_stoikov(&e, &instrument(), 0, &model_config(), 100).unwrap();
    e.last_mut().unwrap().qty = 1000.0;
    let b = calibrate_stoikov(&e, &instrument(), 0, &model_config(), 100).unwrap();
    assert_eq!(a.q, b.q);
    assert_eq!(a.t, b.t);
    assert_eq!(a.adjustment_ticks, b.adjustment_ticks);
    near(a.adjustment_ticks[0], -a.adjustment_ticks[1]);
    let mut overlap = model_config();
    overlap.test_start_ns = overlap.train_end_ns;
    assert!(matches!(
        calibrate_stoikov(&e, &instrument(), 0, &overlap, 100),
        Err(MarketError::Configuration(_))
    ));
    let mut clock = model_config();
    clock.sampling = CalibrationSampling::Clock { step_ns: 10 };
    assert!(calibrate_stoikov(&e, &instrument(), 0, &clock, 100).is_ok());
}

/// CKS increments +2,+1,+2,-4 lots: total +1; 100ms buckets +3,-2.
#[test]
fn fixture_m7_completed_batch_ofi() {
    let states = [
        l1(200, 3, 202, 1),
        l1(200, 5, 202, 1),
        l1(200, 5, 203, 4),
        l1(201, 2, 203, 4),
        l1(200, 5, 203, 6),
    ];
    let increments: Vec<_> = states
        .windows(2)
        .map(|w| order_flow_increment(w[0], w[1], &instrument()).unwrap())
        .collect();
    assert_eq!(increments, vec![2, 1, 2, -4]);
    let e = vec![
        bid(0, 200, 3),
        ask(0, 202, 1),
        bid(10_000_000, 200, 5),
        ask(60_000_000, 202, 0),
        ask(60_000_000, 203, 4),
        bid(130_000_000, 201, 2),
        bid(180_000_000, 201, 0),
        ask(180_000_000, 203, 6),
        ask(200_000_000, 203, 6),
    ];
    let r = report(&e);
    near(r.order_flow_imbalance.get().unwrap(), 1.0);
    near(r.ofi_intervals[0].ofi.get().unwrap(), 3.0);
    near(r.ofi_intervals[1].ofi.get().unwrap(), -2.0);
    near(
        (mid(states[4], &instrument()).unwrap() - mid(states[0], &instrument()).unwrap()) / 0.5,
        0.5,
    );
}

/// Event RV=4*(.5)^2=1, clock RV=2*(.5)^2=.5 over .3s.
#[test]
fn fixture_m8_event_and_previous_tick_clock_variance() {
    let mut e = Vec::new();
    let mut old = None;
    for (ts, b) in [
        (0, 200),
        (40_000_000, 201),
        (120_000_000, 200),
        (130_000_000, 201),
        (250_000_000, 202),
        (300_000_000, 202),
    ] {
        if let Some((b, a)) = old {
            e.push(bid(ts, b, 0));
            e.push(ask(ts, a, 0));
        }
        e.push(bid(ts, b, 3));
        e.push(ask(ts, b + 2, 1));
        old = Some((b, b + 2));
    }
    let r = report(&e);
    near(r.event_variance.realised_variance.get().unwrap(), 1.0);
    near(r.clock_variance.realised_variance.get().unwrap(), 0.5);
    near(r.event_variance.variance_rate.get().unwrap(), 10.0 / 3.0);
    near(r.clock_variance.variance_rate.get().unwrap(), 5.0 / 3.0);
}

/// Known buy volume=3, sell=4, unknown=3: TAI=-1/7, coverage=.7.
#[test]
fn fixture_m9_volume_imbalance_and_unknown_coverage() {
    let mut e = fixture(0);
    e.extend([
        trade(1, 202, 2, Some(Side::Buy)),
        trade(2, 202, 1, Some(Side::Buy)),
        trade(3, 200, 4, Some(Side::Sell)),
        trade(4, 201, 3, None),
        bid(5, 200, 3),
    ]);
    let r = report(&e);
    near(r.trade_imbalance.imbalance.get().unwrap(), -1.0 / 7.0);
    near(r.trade_imbalance.coverage.get().unwrap(), 0.7);
    near(r.trade_imbalance.unknown_volume.get().unwrap(), 3.0);
    let r = report(&fixture(0));
    assert!(r.trade_imbalance.imbalance.get().is_none());
    assert!(r.trade_imbalance.coverage.get().is_none());
}

fn liquidity_config(end_ns: i64, lag: i64) -> MarketConfig {
    MarketConfig {
        liquidity_windows: vec![LiquidityWindow {
            start_ns: 0,
            end_ns,
            side: Side::Sell,
            band: LiquidityBand::BasisPoints { bps: 100 },
            trade_match_lag_ns: lag,
            max_matched_trades: 100,
        }],
        ..MarketConfig::default()
    }
}
/// Fixed F ask band: depth 5→4→2→4→7; trade=1, cancel=2, replenish=5, net=+2;
/// spread recovery30ms and depth recovery50ms from depletion at0.
#[test]
fn fixture_m10_fixed_pre_event_band_and_recovery() {
    let mut e = fixture(-1);
    e.extend([
        trade(0, 202, 1, Some(Side::Buy)),
        ask(0, 202, 0),
        ask(20_000_000, 203, 2),
        ask(30_000_000, 202, 2),
        ask(50_000_000, 203, 5),
    ]);
    let r = compute_market(
        &e,
        &instrument(),
        0,
        &liquidity_config(50_000_000, 10_000_000),
    )
    .unwrap();
    let l = &r.liquidity[0];
    assert!(l.usable);
    near(l.initial_depth.get().unwrap(), 5.0);
    near(l.final_depth.get().unwrap(), 7.0);
    near(l.trade_depletion.get().unwrap(), 1.0);
    near(l.cancel_depletion.get().unwrap(), 2.0);
    near(l.replenishment.get().unwrap(), 5.0);
    near(l.net_flow.get().unwrap(), 2.0);
    near(l.spread_recovery.elapsed.get().unwrap(), 30_000_000.0);
    near(l.depth_recovery.elapsed.get().unwrap(), 50_000_000.0);
    assert!(!l.spread_recovery.censored && !l.depth_recovery.censored);
}

#[test]
fn m10_trade_depth_lag_and_right_censoring() {
    let mut e = fixture(-1);
    e.extend([
        trade(0, 202, 1, Some(Side::Buy)),
        ask(10_000_000, 202, 0),
        ask(20_000_000, 203, 2),
    ]);
    let r = compute_market(
        &e,
        &instrument(),
        0,
        &liquidity_config(20_000_000, 10_000_000),
    )
    .unwrap();
    let l = &r.liquidity[0];
    near(l.cancel_depletion.get().unwrap(), 2.0);
    assert!(l.spread_recovery.censored && l.depth_recovery.censored);
    near(l.spread_recovery.elapsed.get().unwrap(), 10_000_000.0);
    near(l.depth_recovery.elapsed.get().unwrap(), 10_000_000.0);
    let unmatched = compute_market(&e, &instrument(), 0, &liquidity_config(20_000_000, 0)).unwrap();
    near(unmatched.liquidity[0].cancel_depletion.get().unwrap(), 3.0);
    assert!(
        unmatched.liquidity[0]
            .spread_recovery
            .elapsed
            .get()
            .is_none()
    );
}

#[test]
fn m10_attribution_never_reuses_trade_volume() {
    let mut e = fixture(-1);
    e.extend([
        trade(0, 202, 1, Some(Side::Buy)),
        ask(1, 202, 0),
        ask(2, 202, 1),
        ask(3, 202, 0),
    ]);
    let r = compute_market(&e, &instrument(), 0, &liquidity_config(3, 10)).unwrap();
    near(r.liquidity[0].trade_depletion.get().unwrap(), 1.0);
    near(r.liquidity[0].cancel_depletion.get().unwrap(), 1.0);
    near(r.liquidity[0].replenishment.get().unwrap(), 1.0);
}

#[test]
fn invalid_interval_is_not_carried_or_bridged_and_batches_are_atomic() {
    let mut e = fixture(0);
    e.extend([
        event(40_000_000, 0, 0, CLEAR | SELL),
        ask(60_000_000, 204, 1),
        ask(200_000_000, 204, 1),
    ]);
    let r = report(&e);
    // 100ms is valid, but the 0→100ms increment spans invalid depth; 100→200 is valid.
    match r.clock_variance.realised_variance {
        Value::Available { excluded, .. } => assert!(excluded > 0),
        _ => panic!("second clock increment is valid"),
    }
    assert!(r.invalid_ofi_transitions >= 2);
    let mut atomic = fixture(0);
    atomic.extend([
        ask(10, 202, 0),
        ask(10, 203, 0),
        ask(10, 204, 0),
        ask(10, 205, 1),
    ]);
    let r = report(&atomic);
    assert_eq!(r.invalid_ofi_transitions, 0);
    near(r.order_flow_imbalance.get().unwrap(), 1.0);
}

#[test]
fn invalid_grid_is_unavailable_and_arbitrary_start_uses_previous_tick() {
    let mut e = fixture(0);
    e.extend([
        event(50_000_000, 0, 0, CLEAR | SELL),
        ask(150_000_000, 204, 1),
        ask(200_000_000, 204, 1),
    ]);
    let r = report(&e);
    assert!(
        r.clock_series.as_ref().unwrap().points[1].values[0]
            .get()
            .is_none()
    );
    let c = MarketConfig {
        start_ns: Some(25_000_000),
        end_ns: Some(40_000_000),
        series_max_points: Some(10),
        ..MarketConfig::default()
    };
    let r = compute_market(&e, &instrument(), 0, &c).unwrap();
    near(metric(&r, "quoted_spread").event_mean.get().unwrap(), 2.0);
    near(metric(&r, "quoted_spread").clock_mean.get().unwrap(), 2.0);
    near(
        metric(&r, "quoted_spread")
            .time_weighted_mean
            .get()
            .unwrap(),
        2.0,
    );
}

#[test]
fn series_bounds_do_not_change_exact_aggregates() {
    let mut e = fixture(0);
    for ts in 1..100 {
        e.push(bid(ts, 200, 3 + ts));
    }
    let a = report(&e);
    let c = MarketConfig {
        clock_step_ns: 1,
        series_max_points: Some(3),
        ..MarketConfig::default()
    };
    let b = compute_market(&e, &instrument(), 0, &c).unwrap();
    assert!(b.event_series.as_ref().unwrap().points.len() <= 3);
    assert!(b.clock_series.as_ref().unwrap().points.len() <= 3);
    assert!(b.ofi_intervals.len() <= 3);
    near(
        a.order_flow_imbalance.get().unwrap(),
        b.order_flow_imbalance.get().unwrap(),
    );
    near(
        metric(&a, "weighted_mid").event_mean.get().unwrap(),
        metric(&b, "weighted_mid").event_mean.get().unwrap(),
    );
    let c = MarketConfig {
        max_book_levels: 2,
        ..MarketConfig::default()
    };
    assert!(matches!(
        compute_market(&e, &instrument(), 0, &c),
        Err(MarketError::ResourceLimit(_))
    ));
}

#[test]
fn m10_invalid_window_marks_flows_unavailable() {
    let mut e = fixture(-1);
    e.extend([
        ask(0, 202, 0),
        ask(1, 202, 2),
        event(2, 0, 0, CLEAR | BUY),
        bid(3, 200, 3),
    ]);
    let mut c = liquidity_config(3, 1);
    c.liquidity_windows[0].band = LiquidityBand::FirstLevels { levels: 2 };
    let r = compute_market(&e, &instrument(), 0, &c).unwrap();
    assert!(!r.liquidity[0].usable);
    assert!(r.liquidity[0].cancel_depletion.get().is_none());
    assert!(r.liquidity[0].depth_recovery.elapsed.get().is_none());
}

proptest! {
    #[test]
    fn prop_spread_nonnegative_imbalance_range_weighted_mid_bounds(
        b in 1i64..1_000_000, gap in 1i64..1000, qb in 1i64..1_000_000, qa in 1i64..1_000_000) {
        let r = report(&[bid(0, b, qb), ask(0, b + gap, qa)]);
        prop_assert!(metric(&r, "quoted_spread").event_mean.get().unwrap() >= 1.0);
        prop_assert!((-1.0..=1.0).contains(&metric(&r, "imbalance_1").event_mean.get().unwrap()));
        let weighted = metric(&r, "weighted_mid").event_mean.get().unwrap();
        prop_assert!(weighted >= b as f64 * 0.5 && weighted <= (b + gap) as f64 * 0.5);
    }
    #[test]
    fn prop_ofi_interval_additivity(quantities in prop::collection::vec((1i64..10000, 1i64..10000), 3..100), split in 1usize..100) {
        let states: Vec<_> = quantities.iter().map(|&(b, a)| l1(200, b, 202, a)).collect();
        let increments: Vec<_> = states.windows(2).map(|s| order_flow_increment(s[0], s[1], &instrument()).unwrap()).collect();
        let split = split.min(increments.len()); let whole: i128 = increments.iter().sum();
        let left: i128 = increments[..split].iter().sum(); let right: i128 = increments[split..].iter().sum();
        prop_assert_eq!(whole, left + right);
        // With unchanged prices OFI telescopes to change in signed displayed lots.
        prop_assert_eq!(whole, i128::from(quantities.last().unwrap().0 - quantities[0].0)
            - i128::from(quantities.last().unwrap().1 - quantities[0].1));
    }
}

#[test]
fn m9_clock_intervals_retain_volume_coverage_without_inference() {
    let mut e = fixture(0);
    e.extend([
        trade(10_000_000, 202, 2, Some(Side::Buy)),
        trade(100_000_000, 201, 3, None),
        trade(110_000_000, 200, 4, Some(Side::Sell)),
        bid(200_000_000, 200, 3),
    ]);
    let r = report(&e);
    assert_eq!(r.trade_intervals.len(), 2);
    near(
        r.trade_intervals[0]
            .trade_imbalance
            .imbalance
            .get()
            .unwrap(),
        1.0,
    );
    near(
        r.trade_intervals[0].trade_imbalance.coverage.get().unwrap(),
        0.4,
    );
    near(
        r.trade_intervals[1]
            .trade_imbalance
            .imbalance
            .get()
            .unwrap(),
        -1.0,
    );
    near(
        r.trade_intervals[1].trade_imbalance.coverage.get().unwrap(),
        1.0,
    );
}

#[test]
fn m6_held_out_clock_evaluation_uses_in_force_state_not_last_batch_time() {
    let e = vec![
        bid(0, 200, 3),
        ask(0, 202, 1),
        bid(10, 201, 3),
        bid(10, 200, 0),
        ask(10, 202, 0),
        ask(10, 203, 1),
        bid(20, 201, 1),
        ask(20, 203, 3),
        bid(30, 201, 0),
        bid(30, 200, 1),
        ask(30, 203, 0),
        ask(30, 202, 3),
        bid(100, 200, 3),
        ask(100, 202, 1),
        bid(200, 200, 3),
    ];
    let c = MarketConfig {
        clock_step_ns: 10,
        stoikov: Some(model_config()),
        series_max_points: Some(100),
        ..MarketConfig::default()
    };
    let r = compute_market(&e, &instrument(), 0, &c).unwrap();
    let index = r
        .metrics
        .iter()
        .position(|m| m.name == "stoikov_microprice")
        .unwrap();
    let points = &r.clock_series.as_ref().unwrap().points;
    assert!(
        points.iter().find(|p| p.ts_ns == 100).unwrap().values[index]
            .get()
            .is_none()
    );
    assert!(
        points.iter().find(|p| p.ts_ns == 110).unwrap().values[index]
            .get()
            .is_some()
    );
    assert!(
        metric(&r, "stoikov_microprice")
            .time_weighted_mean
            .get()
            .is_some()
    );
}

#[test]
fn m10_first_levels_define_fixed_price_band_including_new_levels() {
    // Initial first two asks are 101 and 102, so new ask101.5 must be included.
    let mut e = vec![bid(-1, 200, 3), ask(-1, 202, 1), ask(-1, 204, 6)];
    e.extend([ask(0, 203, 2), ask(1, 205, 9)]);
    let mut c = liquidity_config(1, 0);
    c.liquidity_windows[0].band = LiquidityBand::FirstLevels { levels: 2 };
    let r = compute_market(&e, &instrument(), 0, &c).unwrap();
    near(r.liquidity[0].initial_depth.get().unwrap(), 7.0);
    near(r.liquidity[0].final_depth.get().unwrap(), 9.0);
    near(r.liquidity[0].replenishment.get().unwrap(), 2.0);
}

#[test]
fn m10_empty_event_window_has_zero_flows_not_fake_recovery() {
    let mut e = fixture(-1);
    e.push(bid(10, 200, 3));
    let r = compute_market(&e, &instrument(), 0, &liquidity_config(5, 0)).unwrap();
    let l = &r.liquidity[0];
    near(l.trade_depletion.get().unwrap(), 0.0);
    near(l.cancel_depletion.get().unwrap(), 0.0);
    near(l.replenishment.get().unwrap(), 0.0);
    near(l.final_depth.get().unwrap(), 5.0);
    assert!(l.spread_recovery.elapsed.get().is_none());
    assert!(!l.spread_recovery.censored);
}

#[test]
fn clock_coverage_is_never_extrapolated_and_local_duplicates_are_ignored() {
    let mut e = fixture(0);
    e.push(bid(10, 200, 5));
    let c = MarketConfig {
        end_ns: Some(11),
        ..MarketConfig::default()
    };
    assert!(matches!(
        compute_market(&e, &instrument(), 0, &c),
        Err(MarketError::Configuration(_))
    ));
    let mut duplicate = bid(-100, 200, 999);
    duplicate.ev &= !EXCHANGE;
    e.push(duplicate);
    near(report(&e).order_flow_imbalance.get().unwrap(), 2.0);
}

#[test]
fn m8_sub_tick_mid_changes_are_differenced_before_large_price_conversion() {
    // Ticks are exactly representable, but these half-price midpoints are not both f64-exact.
    // The ask moves two ticks (.5 price change in the mid), so RV must be .25, not 0 or1.
    let b = (1i64 << 53) + 4;
    let e = [
        bid(0, b, 3),
        ask(0, b + 2, 1),
        ask(10, b + 2, 0),
        ask(10, b + 4, 1),
    ];
    near(
        report(&e).event_variance.realised_variance.get().unwrap(),
        0.25,
    );
}

#[test]
fn event_sampling_tracks_inputs_even_when_ratio_is_unchanged() {
    let e = [
        bid(0, 200, 1),
        bid(0, 199, 1),
        ask(0, 202, 1),
        ask(0, 203, 1),
        bid(10, 200, 2),
        bid(10, 199, 2),
        ask(10, 202, 2),
        ask(10, 203, 2),
        bid(20, 198, 1),
        ask(20, 204, 1),
    ];
    let r = report(&e);
    match metric(&r, "imbalance_2").event_mean {
        Value::Available { value, n, .. } => {
            near(value, 0.0);
            assert_eq!(n, 2);
        }
        _ => panic!("two valid top-two-level observations"),
    }
}
