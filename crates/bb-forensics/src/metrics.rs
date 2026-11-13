//! Run-level metrics and fill enrichment.

use std::collections::{BTreeMap, BTreeSet};

use bb_core::{
    dataset::{InstrumentMeta, Quantiles},
    flight::{FlightBody, FlightEvent},
    market::BboSeries,
    result::{
        EquitySample, FillRecord, MarkoutStat, Metrics, OrderOutcome, OrderRecord,
        MARKOUT_HORIZONS_NS,
    },
    RelNs, ReqKind,
};

/// Sets `mid_tick` and `markout_ticks` on every fill from the dataset's top-of-book series.
///
/// A markout is `None` when its horizon lies after the last top-of-book entry (the dataset ends
/// first) or when the book had no valid mid at that time. It is never extrapolated.
pub fn enrich_fills(fills: &mut [FillRecord], bbo: &BboSeries) {
    let last_ts = bbo.ts.last().copied();
    for fill in fills {
        fill.mid_tick = bbo.mid_tick_at(fill.ts);
        fill.markout_ticks = MARKOUT_HORIZONS_NS
            .iter()
            .map(|horizon| {
                let t = fill.ts.checked_add(*horizon)?;
                if t > last_ts? {
                    return None;
                }
                let mid = bbo.mid_tick_at(t)?;
                Some(fill.side.sign() * (mid - fill.price_tick as f64))
            })
            .collect();
    }
}

/// Nearest-rank quantiles: `p = sorted[ceil(q * n) - 1]`. `None` without samples.
pub(crate) fn quantiles(mut values: Vec<f64>) -> Option<Quantiles> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    // Integer arithmetic: `ceil(q * n)` in floating point is off by one for some `n`.
    let rank = |num: usize, den: usize| values[(n * num).div_ceil(den).max(1) - 1];
    Some(Quantiles {
        n: n as u64,
        min: values[0],
        p50: rank(1, 2),
        p90: rank(9, 10),
        p99: rank(99, 100),
        max: values[n - 1],
        mean: values.iter().sum::<f64>() / n as f64,
    })
}

/// Every field of [`Metrics`], computed from the tape, the order records, the enriched fills and
/// the equity samples.
///
/// `max_long` and `max_short` are both magnitudes (non-negative): the largest long and the
/// largest short position held after any fill.
pub fn compute_metrics(
    tape: &[FlightEvent],
    orders: &[OrderRecord],
    fills: &[FillRecord],
    equity: &[EquitySample],
    instrument: &InstrumentMeta,
) -> Metrics {
    // ----- tape: latencies, cancels, risk -----
    let mut rested: BTreeSet<u64> = BTreeSet::new();
    let mut new_sent_ts: BTreeMap<u64, RelNs> = BTreeMap::new();
    let mut entry_latency = Vec::new();
    let mut response_latency = Vec::new();
    let mut cancels_sent = 0u64;
    let mut cancels_rejected = 0u64;
    let mut risk_blocks = 0u64;
    let mut risk_halted = false;

    for ev in tape {
        match &ev.body {
            FlightBody::RequestSent {
                req,
                entry_latency_ns,
                ..
            } => {
                // A negative value is the engine's rejection marker, not a latency.
                if *entry_latency_ns >= 0 {
                    entry_latency.push(*entry_latency_ns as f64);
                }
                match req {
                    ReqKind::New => {
                        if let Some(id) = ev.order_id {
                            new_sent_ts.entry(id).or_insert(ev.ts);
                        }
                    }
                    ReqKind::Cancel => cancels_sent += 1,
                    ReqKind::Modify | ReqKind::None => {}
                }
            }
            FlightBody::ExchQueued { .. } => {
                if let Some(id) = ev.order_id {
                    rested.insert(id);
                }
            }
            FlightBody::ExchResponse {
                request_rejected,
                response_latency_ns,
                ..
            } => {
                response_latency.push(*response_latency_ns as f64);
                if *request_rejected {
                    cancels_rejected += 1;
                }
            }
            FlightBody::RiskBlocked { .. } => risk_blocks += 1,
            FlightBody::RiskHalt { .. } => risk_halted = true,
            _ => {}
        }
    }

    // ----- orders -----
    let mut orders_risk_blocked = 0u64;
    let mut orders_refused = 0u64;
    let mut orders_filled = 0u64;
    let mut orders_partially_filled = 0u64;
    let mut orders_canceled = 0u64;
    let mut orders_expired = 0u64;
    let mut orders_working_at_end = 0u64;
    let mut orders_submitted = 0u64;
    let mut orders_with_fill = 0u64;
    let mut submitted_qty = 0.0;
    let mut filled_qty = 0.0;
    let mut order_age = Vec::new();
    let mut time_to_first_fill = Vec::new();
    let mut round_trip = Vec::new();

    for o in orders {
        match o.outcome {
            OrderOutcome::RiskBlocked => {
                orders_risk_blocked += 1;
                continue;
            }
            OrderOutcome::Refused => {
                orders_refused += 1;
                continue;
            }
            OrderOutcome::InFlightAtEnd => {}
            OrderOutcome::Filled => orders_filled += 1,
            OrderOutcome::PartiallyFilled => orders_partially_filled += 1,
            OrderOutcome::Canceled => orders_canceled += 1,
            OrderOutcome::Expired => orders_expired += 1,
            OrderOutcome::WorkingAtEnd => orders_working_at_end += 1,
        }
        orders_submitted += 1;
        submitted_qty += o.qty;
        filled_qty += o.filled_qty;
        if o.fill_count > 0 {
            orders_with_fill += 1;
        }
        if let Some(arrival) = o.exch_arrival_ts {
            if let Some(terminal) = o.terminal_ts
                && rested.contains(&o.order_id)
            {
                order_age.push((terminal - arrival) as f64);
            }
            if let Some(first_fill) = o.first_fill_ts {
                time_to_first_fill.push((first_fill - arrival) as f64);
            }
        }
        if let (Some(sent), Some(response)) = (new_sent_ts.get(&o.order_id), o.first_response_ts) {
            round_trip.push((response - *sent) as f64);
        }
    }

    // ----- fills -----
    let mut maker_fill_count = 0u64;
    let mut volume = 0.0;
    let mut turnover = 0.0;
    let mut fees = 0.0;
    let mut max_long = 0.0f64;
    let mut max_short = 0.0f64;
    let mut capture_tick_qty = 0.0;
    let mut capture_qty = 0.0;
    let mut markout_sum = [0.0f64; MARKOUT_HORIZONS_NS.len()];
    let mut markout_qty = [0.0f64; MARKOUT_HORIZONS_NS.len()];
    let mut markout_fills = [0u64; MARKOUT_HORIZONS_NS.len()];

    for f in fills {
        volume += f.qty;
        turnover += f.value;
        fees += f.fee;
        max_long = max_long.max(f.position_after);
        max_short = max_short.max(-f.position_after);
        if f.maker {
            maker_fill_count += 1;
            // A maker fill without a valid mid cannot be scored and is left out of both sums.
            if let Some(mid) = f.mid_tick {
                capture_tick_qty += f.side.sign() * (mid - f.price_tick as f64) * f.qty;
                capture_qty += f.qty;
            }
        }
        for (i, markout) in f.markout_ticks.iter().take(MARKOUT_HORIZONS_NS.len()).enumerate() {
            if let Some(m) = markout {
                markout_sum[i] += m * f.qty;
                markout_qty[i] += f.qty;
                markout_fills[i] += 1;
            }
        }
    }

    let markouts = MARKOUT_HORIZONS_NS
        .iter()
        .enumerate()
        .map(|(i, horizon_ns)| MarkoutStat {
            horizon_ns: *horizon_ns,
            fills: markout_fills[i],
            mean_ticks: (markout_qty[i] > 0.0).then(|| markout_sum[i] / markout_qty[i]),
        })
        .collect();

    // ----- equity -----
    let mut peak = f64::NEG_INFINITY;
    let mut max_drawdown = 0.0f64;
    let mut abs_position_sum = 0.0;
    for s in equity {
        peak = peak.max(s.equity);
        max_drawdown = max_drawdown.max(peak - s.equity);
        abs_position_sum += s.position.abs();
    }
    let last = equity.last();

    let fill_count = fills.len() as u64;
    Metrics {
        orders_intended: orders.len() as u64,
        orders_risk_blocked,
        orders_refused,
        orders_submitted,
        orders_filled,
        orders_partially_filled,
        orders_canceled,
        orders_expired,
        orders_working_at_end,
        cancels_sent,
        cancels_rejected,
        fill_count,
        maker_fill_count,
        taker_fill_count: fill_count - maker_fill_count,
        fill_ratio_orders: (orders_submitted > 0)
            .then(|| orders_with_fill as f64 / orders_submitted as f64),
        fill_ratio_qty: (submitted_qty > 0.0).then(|| filled_qty / submitted_qty),
        order_age_ns: quantiles(order_age),
        time_to_first_fill_ns: quantiles(time_to_first_fill),
        entry_latency_ns: quantiles(entry_latency),
        response_latency_ns: quantiles(response_latency),
        round_trip_ns: quantiles(round_trip),
        volume,
        turnover,
        fees,
        final_position: fills.last().map_or(0.0, |f| f.position_after),
        max_long,
        max_short,
        mean_abs_position: if equity.is_empty() {
            0.0
        } else {
            abs_position_sum / equity.len() as f64
        },
        realized_pnl: last.map_or(0.0, |s| s.realized_pnl),
        unrealized_pnl: last.map_or(0.0, |s| s.unrealized_pnl),
        net_pnl: last.map_or(0.0, |s| s.equity),
        max_drawdown,
        spread_capture: capture_tick_qty * instrument.tick_size * instrument.contract_size,
        spread_capture_ticks: (capture_qty > 0.0).then(|| capture_tick_qty / capture_qty),
        markouts,
        risk_blocks,
        risk_halted,
    }
}

#[cfg(test)]
mod tests {
    use bb_core::{flight::RiskRule, OrderStatus, Side, Tif};

    use super::*;
    use crate::{
        build_orders,
        testkit::{close, equity_sample, fill_record, Tape, INSTRUMENT, MS},
    };

    fn q(values: &[f64]) -> Quantiles {
        quantiles(values.to_vec()).expect("non-empty input")
    }

    #[test]
    fn quantiles_use_nearest_rank() {
        assert_eq!(quantiles(Vec::new()), None);

        let one = q(&[7.0]);
        assert_eq!((one.n, one.min, one.p50, one.p90, one.p99, one.max), (1, 7.0, 7.0, 7.0, 7.0, 7.0));
        assert_eq!(one.mean, 7.0);

        // Unsorted input; n = 3: ceil(1.5) = 2nd, ceil(2.7) = 3rd, ceil(2.97) = 3rd.
        let three = q(&[3.0, 1.0, 2.0]);
        assert_eq!((three.p50, three.p90, three.p99), (2.0, 3.0, 3.0));
        assert_eq!(three.mean, 2.0);

        // n = 10: 5th, 9th, 10th.
        let ten: Vec<f64> = (1..=10).map(f64::from).collect();
        let ten = q(&ten);
        assert_eq!((ten.min, ten.p50, ten.p90, ten.p99, ten.max), (1.0, 5.0, 9.0, 10.0, 10.0));
        assert_eq!(ten.mean, 5.5);

        // n = 100: exactly the 50th, 90th and 99th values, not one above.
        let hundred: Vec<f64> = (1..=100).map(f64::from).collect();
        let hundred = q(&hundred);
        assert_eq!((hundred.p50, hundred.p90, hundred.p99), (50.0, 90.0, 99.0));

        // n = 4: ceil(2.0) = 2nd (the lower middle), ceil(3.6) = 4th.
        let four = q(&[40.0, 10.0, 30.0, 20.0]);
        assert_eq!((four.p50, four.p90), (20.0, 40.0));
    }

    #[test]
    fn markouts_follow_the_mid_and_stop_at_the_end_of_data() {
        let bbo = BboSeries {
            ts: vec![0, 150 * MS, 1_200 * MS, 2_150 * MS],
            bid_tick: vec![Some(200), Some(202), None, Some(198)],
            ask_tick: vec![Some(201), Some(203), Some(205), Some(199)],
            bid_qty: vec![1.0; 4],
            ask_qty: vec![1.0; 4],
        };
        // Buy at tick 200 at t = 100 ms.
        let mut fills = vec![
            fill_record(0, 256, 100 * MS, Side::Buy, 200, 1.0, true, 0.0, 1.0),
            fill_record(1, 512, 100 * MS, Side::Sell, 201, 1.0, true, 0.0, 0.0),
        ];
        enrich_fills(&mut fills, &bbo);

        assert_eq!(fills[0].mid_tick, Some(200.5));
        // +100 ms: mid 202.5; +1 s: mid 202.5 still; +5 s and +30 s: after the last BBO entry.
        assert_eq!(
            fills[0].markout_ticks,
            vec![Some(2.5), Some(2.5), None, None]
        );
        // The sell sees the same moves with the opposite sign.
        assert_eq!(
            fills[1].markout_ticks,
            vec![Some(-1.5), Some(-1.5), None, None]
        );

        // One-sided book at the horizon: unavailable, not guessed.
        let mut late = vec![fill_record(2, 768, 1_150 * MS, Side::Buy, 203, 1.0, true, 0.0, 1.0)];
        enrich_fills(&mut late, &bbo);
        assert_eq!(late[0].mid_tick, Some(202.5));
        assert_eq!(late[0].markout_ticks, vec![None, Some(-4.5), None, None]);

        // No book at all.
        let mut none = vec![fill_record(3, 1024, 5, Side::Buy, 1, 1.0, true, 0.0, 1.0)];
        enrich_fills(&mut none, &BboSeries::default());
        assert_eq!(none[0].mid_tick, None);
        assert_eq!(none[0].markout_ticks, vec![None; 4]);
    }

    #[test]
    fn three_fill_example_matches_hand_computation() {
        let mut t = Tape::new();
        // 256: buy 2 @ 200, rests from 11 ms, maker fill at 30 ms.
        t.rest(10 * MS, 256, Side::Buy, 200, 2.0, Tif::Gtc, Some(1.0));
        // 512: sell 1 @ 202, rests from 21 ms, maker fill at 50 ms.
        t.rest(20 * MS, 512, Side::Sell, 202, 1.0, Tif::Gtc, Some(1.0));
        let f1 = t.fill(30 * MS, 256, 200, 2.0, true, 0.02, 0.0);
        t.resp(30 * MS, 256, OrderStatus::Filled, 0.0);
        t.local(31 * MS, 256, OrderStatus::Filled, 2.0, 0.0, 30 * MS);
        let f2 = t.fill(50 * MS, 512, 202, 1.0, true, 0.0101, 0.0);
        t.resp(50 * MS, 512, OrderStatus::Filled, 0.0);
        t.local(51 * MS, 512, OrderStatus::Filled, 1.0, 0.0, 50 * MS);
        // 768: sell 3 @ 199 IOC, takes on arrival at 61 ms.
        t.intent(60 * MS, 768, Side::Sell, 199, 3.0, Tif::Ioc);
        t.sent(60 * MS, 768, ReqKind::New, MS);
        let f3 = t.fill(61 * MS, 768, 199, 3.0, false, 0.1, 0.0);
        t.resp(61 * MS, 768, OrderStatus::Filled, 0.0);
        t.local(62 * MS, 768, OrderStatus::Filled, 3.0, 0.0, 61 * MS);
        // 1024: buy 2 @ 198, rests from 71 ms, cancelled at 91 ms.
        t.rest(70 * MS, 1024, Side::Buy, 198, 2.0, Tif::Gtc, Some(4.0));
        t.cancel_intent(90 * MS, 1024, "requote");
        t.sent(90 * MS, 1024, ReqKind::Cancel, MS);
        t.resp(91 * MS, 1024, OrderStatus::Canceled, 2.0);
        t.local(92 * MS, 1024, OrderStatus::Canceled, 0.0, 2.0, 91 * MS);
        // 1280: blocked by risk, never sent.
        t.intent(100 * MS, 1280, Side::Buy, 198, 9.0, Tif::Gtc);
        t.push(
            100 * MS,
            Some(1280),
            FlightBody::RiskBlocked {
                tick_no: 5,
                rule: RiskRule::MaxOrderQty,
                detail: "9 > 5".into(),
            },
        );
        t.push(
            110 * MS,
            None,
            FlightBody::RiskHalt {
                rule: RiskRule::MaxDrawdown,
                detail: "drawdown 6 > 5".into(),
            },
        );

        let mut fills = vec![
            fill_record(f1, 256, 30 * MS, Side::Buy, 200, 2.0, true, 0.02, 2.0),
            fill_record(f2, 512, 50 * MS, Side::Sell, 202, 1.0, true, 0.0101, 1.0),
            fill_record(f3, 768, 61 * MS, Side::Sell, 199, 3.0, false, 0.1, -2.0),
        ];
        fills[0].mid_tick = Some(200.5);
        fills[0].markout_ticks = vec![Some(1.0), Some(2.0), None, None];
        fills[1].mid_tick = Some(201.0);
        fills[1].markout_ticks = vec![Some(-2.0), None, None, None];
        fills[2].mid_tick = Some(199.5);
        fills[2].markout_ticks = vec![Some(1.0), Some(-1.0), None, None];

        let equity: Vec<EquitySample> = [
            (0.0, 0.0),
            (5.0, 2.0),
            (3.0, 1.0),
            (8.0, -2.0),
            (2.0, -2.0),
            (4.0, -2.0),
        ]
        .iter()
        .enumerate()
        .map(|(i, (eq, pos))| equity_sample(i as u64, *eq, *pos))
        .collect();

        let orders = build_orders(&t.0);
        let m = compute_metrics(&t.0, &orders, &fills, &equity, &INSTRUMENT);

        // Orders.
        assert_eq!(m.orders_intended, 5);
        assert_eq!(m.orders_risk_blocked, 1);
        assert_eq!(m.orders_refused, 0);
        assert_eq!(m.orders_submitted, 4);
        assert_eq!(m.orders_filled, 3);
        assert_eq!(m.orders_partially_filled, 0);
        assert_eq!(m.orders_canceled, 1);
        assert_eq!(m.orders_expired, 0);
        assert_eq!(m.orders_working_at_end, 0);
        assert_eq!(m.cancels_sent, 1);
        assert_eq!(m.cancels_rejected, 0);
        assert_eq!(m.risk_blocks, 1);
        assert!(m.risk_halted);

        // Fills: 3 of 4 submitted orders filled; 6 of 2 + 1 + 3 + 2 = 8 quantity.
        assert_eq!((m.fill_count, m.maker_fill_count, m.taker_fill_count), (3, 2, 1));
        assert!(close(m.fill_ratio_orders.unwrap(), 0.75));
        assert!(close(m.fill_ratio_qty.unwrap(), 0.75));
        assert!(close(m.volume, 6.0));
        // value = price_tick * tick_size * qty: 200 + 101 + 298.5.
        assert!(close(m.turnover, 599.5));
        assert!(close(m.fees, 0.1301));

        // Inventory: +2, +1, -2 after the three fills.
        assert!(close(m.final_position, -2.0));
        assert!(close(m.max_long, 2.0));
        assert!(close(m.max_short, 2.0));
        assert!(close(m.mean_abs_position, 9.0 / 6.0));

        // P&L from the last sample; drawdown is the fall from 8 to 2.
        let last = equity[5];
        assert_eq!(m.realized_pnl, last.realized_pnl);
        assert_eq!(m.unrealized_pnl, last.unrealized_pnl);
        assert_eq!(m.net_pnl, 4.0);
        assert!(close(m.max_drawdown, 6.0));

        // Spread capture, maker fills only: buy (200.5 - 200) * 2 + sell (202 - 201) * 1 = 2
        // tick-quantity, times tick size 0.5 = 1.0 quote; 2 / 3 ticks per unit.
        assert!(close(m.spread_capture, 1.0));
        assert!(close(m.spread_capture_ticks.unwrap(), 2.0 / 3.0));

        // Markouts, quantity-weighted over the fills that have the horizon.
        assert_eq!(m.markouts.len(), 4);
        assert_eq!(m.markouts[0].horizon_ns, MARKOUT_HORIZONS_NS[0]);
        assert_eq!(m.markouts[0].fills, 3);
        assert!(close(m.markouts[0].mean_ticks.unwrap(), (2.0 - 2.0 + 3.0) / 6.0));
        assert_eq!(m.markouts[1].fills, 2);
        assert!(close(m.markouts[1].mean_ticks.unwrap(), (4.0 - 3.0) / 5.0));
        assert_eq!(m.markouts[2].fills, 0);
        assert_eq!(m.markouts[2].mean_ticks, None);
        assert_eq!(m.markouts[3].mean_ticks, None);

        // Timing. Rested orders with a terminal state: 19, 29 and 20 ms.
        let age = m.order_age_ns.unwrap();
        assert_eq!((age.n, age.min, age.p50, age.max), (3, 19e6, 20e6, 29e6));
        // Arrival to first fill: 19 ms, 29 ms and 0 for the taker.
        let ttf = m.time_to_first_fill_ns.unwrap();
        assert_eq!((ttf.n, ttf.min, ttf.p50, ttf.max), (3, 0.0, 19e6, 29e6));
        // Four new-order requests and one cancel, all 1 ms.
        let entry = m.entry_latency_ns.unwrap();
        assert_eq!((entry.n, entry.p50, entry.p99), (5, 1e6, 1e6));
        // New + terminal response for the three resting orders, one for the taker.
        let response = m.response_latency_ns.unwrap();
        assert_eq!((response.n, response.p50), (7, 1e6));
        // Sent to first response on the local clock: 1 ms each way.
        let rt = m.round_trip_ns.unwrap();
        assert_eq!((rt.n, rt.min, rt.max), (4, 2e6, 2e6));
    }

    #[test]
    fn empty_run_has_zeroes_and_no_ratios() {
        let m = compute_metrics(&[], &[], &[], &[], &INSTRUMENT);
        assert_eq!(m.orders_intended, 0);
        assert_eq!(m.fill_ratio_orders, None);
        assert_eq!(m.fill_ratio_qty, None);
        assert_eq!(m.order_age_ns, None);
        assert_eq!(m.net_pnl, 0.0);
        assert_eq!(m.max_drawdown, 0.0);
        assert_eq!(m.mean_abs_position, 0.0);
        assert_eq!(m.spread_capture_ticks, None);
        assert_eq!(m.markouts.len(), MARKOUT_HORIZONS_NS.len());
        assert!(m.markouts.iter().all(|s| s.fills == 0 && s.mean_ticks.is_none()));
        assert!(!m.risk_halted);
    }
}
