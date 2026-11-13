//! Assembles one [`OrderRecord`] per order from the flight tape.

use std::collections::BTreeMap;

use bb_core::{
    flight::{FlightBody, FlightEvent, QueueCause},
    result::{OrderOutcome, OrderRecord},
    Clock, OrderStatus, ReqKind,
};

/// An order record under construction, plus the facts that decide its outcome.
struct Building {
    rec: OrderRecord,
    risk_blocked: bool,
    refused: bool,
    exch_response_seen: bool,
    /// Sum of `price_tick * qty` over fills, for the average fill price.
    fill_tick_qty: f64,
    /// Reason of the latest cancel intent whose request has not been sent yet.
    pending_cancel_reason: Option<String>,
}

impl Building {
    fn finish(mut self) -> OrderRecord {
        let rec = &mut self.rec;
        if rec.filled_qty > 0.0 {
            rec.avg_fill_tick = Some(self.fill_tick_qty / rec.filled_qty);
        }
        rec.outcome = if self.risk_blocked {
            OrderOutcome::RiskBlocked
        } else if self.refused {
            OrderOutcome::Refused
        } else if rec.fill_count > 0 {
            if rec.final_status == OrderStatus::Filled {
                OrderOutcome::Filled
            } else {
                OrderOutcome::PartiallyFilled
            }
        } else {
            match rec.final_status {
                OrderStatus::Canceled => OrderOutcome::Canceled,
                OrderStatus::Expired => OrderOutcome::Expired,
                _ if rec.exch_arrival_ts.is_some() => OrderOutcome::WorkingAtEnd,
                _ => OrderOutcome::InFlightAtEnd,
            }
        };
        self.rec
    }
}

fn track_min(slot: &mut Option<f64>, value: Option<f64>) {
    if let Some(v) = value {
        *slot = Some(slot.map_or(v, |m| m.min(v)));
    }
}

/// One record per order id, ascending. Events of an order that has no `OrderIntent` on the tape
/// are skipped: without the intent there is no side, price or quantity to report.
pub fn build_orders(tape: &[FlightEvent]) -> Vec<OrderRecord> {
    let mut orders: BTreeMap<u64, Building> = BTreeMap::new();

    for ev in tape {
        let Some(id) = ev.order_id else { continue };

        if let FlightBody::OrderIntent {
            tick_no,
            slot,
            side,
            price_tick,
            qty,
            tif,
            ord_type,
            reason,
            ..
        } = &ev.body
        {
            orders.entry(id).or_insert_with(|| Building {
                rec: OrderRecord {
                    order_id: id,
                    tick_no: *tick_no,
                    slot: *slot,
                    side: *side,
                    price_tick: *price_tick,
                    qty: *qty,
                    tif: *tif,
                    ord_type: *ord_type,
                    reason: reason.clone(),
                    outcome: OrderOutcome::InFlightAtEnd,
                    decision_ts: ev.ts,
                    exch_arrival_ts: None,
                    first_response_ts: None,
                    queue_ahead_at_entry: None,
                    min_queue_ahead: None,
                    traded_at_price_qty: 0.0,
                    filled_qty: 0.0,
                    maker_qty: 0.0,
                    taker_qty: 0.0,
                    avg_fill_tick: None,
                    fees: 0.0,
                    fill_count: 0,
                    first_fill_ts: None,
                    last_fill_ts: None,
                    cancel_sent_ts: None,
                    cancel_reason: None,
                    cancel_rejected: false,
                    terminal_ts: None,
                    final_status: OrderStatus::None,
                    first_seq: ev.seq,
                    last_seq: ev.seq,
                },
                risk_blocked: false,
                refused: false,
                exch_response_seen: false,
                fill_tick_qty: 0.0,
                pending_cancel_reason: None,
            });
        }

        let Some(b) = orders.get_mut(&id) else { continue };
        let rec = &mut b.rec;
        rec.last_seq = ev.seq;
        if ev.clock == Clock::Exchange && rec.exch_arrival_ts.is_none() {
            rec.exch_arrival_ts = Some(ev.ts);
        }

        match &ev.body {
            FlightBody::OrderIntent { .. } | FlightBody::RiskHalt { .. } => {}
            FlightBody::CancelIntent { reason, .. } => {
                b.pending_cancel_reason = Some(reason.clone());
            }
            FlightBody::RiskBlocked { .. } => b.risk_blocked = true,
            FlightBody::RequestSent { req, .. } => {
                if *req == ReqKind::Cancel {
                    rec.cancel_sent_ts = Some(ev.ts);
                    rec.cancel_reason = b.pending_cancel_reason.take();
                }
            }
            FlightBody::RequestRefused { req, .. } => match req {
                ReqKind::New => b.refused = true,
                // A refused cancel was never sent, so it leaves no trace on the record.
                ReqKind::Cancel => b.pending_cancel_reason = None,
                ReqKind::Modify | ReqKind::None => {}
            },
            FlightBody::ExchQueued { queue_ahead, .. } => {
                if rec.queue_ahead_at_entry.is_none() {
                    rec.queue_ahead_at_entry = *queue_ahead;
                }
                track_min(&mut rec.min_queue_ahead, *queue_ahead);
            }
            FlightBody::QueueUpdate {
                cause, ahead_after, ..
            } => {
                if let QueueCause::Trade { qty } = cause {
                    rec.traded_at_price_qty += qty;
                }
                track_min(&mut rec.min_queue_ahead, *ahead_after);
            }
            FlightBody::ExchFill {
                price_tick,
                qty,
                maker,
                fee,
                ..
            } => {
                rec.filled_qty += qty;
                if *maker {
                    rec.maker_qty += qty;
                } else {
                    rec.taker_qty += qty;
                }
                b.fill_tick_qty += *price_tick as f64 * qty;
                rec.fees += fee;
                rec.fill_count += 1;
                if rec.first_fill_ts.is_none() {
                    rec.first_fill_ts = Some(ev.ts);
                }
                rec.last_fill_ts = Some(ev.ts);
            }
            FlightBody::ExchResponse {
                status,
                request_rejected,
                ..
            } => {
                b.exch_response_seen = true;
                if *request_rejected {
                    // The status of a rejected cancel describes the request, not the order.
                    rec.cancel_rejected = true;
                } else {
                    rec.final_status = *status;
                    if status.is_terminal() && rec.terminal_ts.is_none() {
                        rec.terminal_ts = Some(ev.ts);
                    }
                }
            }
            FlightBody::LocalResponse {
                status,
                request_rejected,
                ..
            } => {
                if rec.first_response_ts.is_none() {
                    rec.first_response_ts = Some(ev.ts);
                }
                // A response with no exchange-side counterpart is the engine rejecting a request
                // before it reached the matching engine (negative entry latency). It is the only
                // status the order will ever get.
                if !b.exch_response_seen && !*request_rejected {
                    rec.final_status = *status;
                }
            }
            FlightBody::QueueTruncated { .. } => {}
        }
    }

    orders.into_values().map(Building::finish).collect()
}

#[cfg(test)]
mod tests {
    use bb_core::{flight::RiskRule, OrdType, Side, Tif};

    use super::*;
    use crate::testkit::{Tape, MS};

    #[test]
    fn records_are_sorted_by_order_id_and_cover_the_lifecycle() {
        let mut t = Tape::new();
        // Order 512 is decided first on the tape but must come second in the output.
        t.rest(10 * MS, 512, Side::Sell, 20_002, 1.0, Tif::Gtx, Some(0.8));
        t.rest(10 * MS, 256, Side::Buy, 20_000, 2.0, Tif::Gtc, None);
        t.trade(20 * MS, 512, 0.5, 0.8, 0.3);
        t.trade(30 * MS, 512, 0.75, 0.3, -0.45);
        let fill = t.fill(30 * MS, 512, 20_002, 1.0, true, 0.25, 0.0);
        t.resp(30 * MS, 512, OrderStatus::Filled, 0.0);
        let last = t.local(31 * MS, 512, OrderStatus::Filled, 1.0, 0.0, 30 * MS);

        let orders = build_orders(&t.0);
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].order_id, 256);
        assert_eq!(orders[1].order_id, 512);

        let o = &orders[1];
        assert_eq!((o.tick_no, o.slot), (2, 0));
        assert_eq!(o.side, Side::Sell);
        assert_eq!(o.price_tick, 20_002);
        assert_eq!(o.tif, Tif::Gtx);
        assert_eq!(o.ord_type, OrdType::Limit);
        assert_eq!(o.outcome, OrderOutcome::Filled);
        assert_eq!(o.decision_ts, 10 * MS);
        assert_eq!(o.exch_arrival_ts, Some(11 * MS));
        assert_eq!(o.first_response_ts, Some(12 * MS));
        assert_eq!(o.queue_ahead_at_entry, Some(0.8));
        assert_eq!(o.min_queue_ahead, Some(-0.45));
        assert_eq!(o.traded_at_price_qty, 1.25);
        assert_eq!(o.filled_qty, 1.0);
        assert_eq!(o.maker_qty, 1.0);
        assert_eq!(o.taker_qty, 0.0);
        assert_eq!(o.avg_fill_tick, Some(20_002.0));
        assert_eq!(o.fees, 0.25);
        assert_eq!(o.fill_count, 1);
        assert_eq!(o.first_fill_ts, Some(30 * MS));
        assert_eq!(o.last_fill_ts, Some(30 * MS));
        assert_eq!(o.terminal_ts, Some(30 * MS));
        assert_eq!(o.final_status, OrderStatus::Filled);
        assert_eq!(o.first_seq, 0);
        assert_eq!(o.last_seq, last);
        assert!(fill < last);

        // The unreadable queue state stays unknown; it is never reported as zero.
        let w = &orders[0];
        assert_eq!(w.outcome, OrderOutcome::WorkingAtEnd);
        assert_eq!(w.queue_ahead_at_entry, None);
        assert_eq!(w.min_queue_ahead, None);
        assert_eq!(w.terminal_ts, None);
        assert_eq!(w.final_status, OrderStatus::New);
    }

    #[test]
    fn orders_that_never_reach_the_exchange() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc);
        t.push(
            10 * MS,
            Some(256),
            FlightBody::RiskBlocked {
                tick_no: 1,
                rule: RiskRule::MaxPosition,
                detail: "position 5 + 1 > 5".into(),
            },
        );
        t.intent(20 * MS, 512, Side::Buy, 20_000, 1.0, Tif::Gtc);
        t.push(
            20 * MS,
            Some(512),
            FlightBody::RequestRefused {
                req: ReqKind::New,
                error: "order id exists".into(),
            },
        );
        t.intent(30 * MS, 768, Side::Buy, 20_000, 1.0, Tif::Gtc);
        t.sent(30 * MS, 768, ReqKind::New, MS);

        let orders = build_orders(&t.0);
        let outcomes: Vec<_> = orders.iter().map(|o| o.outcome).collect();
        assert_eq!(
            outcomes,
            [
                OrderOutcome::RiskBlocked,
                OrderOutcome::Refused,
                OrderOutcome::InFlightAtEnd
            ]
        );
        for o in &orders {
            assert_eq!(o.exch_arrival_ts, None);
            assert_eq!(o.first_response_ts, None);
            assert_eq!(o.final_status, OrderStatus::None);
            assert_eq!(o.filled_qty, 0.0);
        }
    }

    #[test]
    fn cancel_bookkeeping_distinguishes_sent_refused_and_rejected() {
        let mut t = Tape::new();
        // 256: cancel refused, then a second cancel sent and effective.
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(3.0));
        t.cancel_intent(20 * MS, 256, "first try");
        t.push(
            20 * MS,
            Some(256),
            FlightBody::RequestRefused {
                req: ReqKind::Cancel,
                error: "request in flight".into(),
            },
        );
        t.cancel_intent(40 * MS, 256, "requote");
        t.sent(40 * MS, 256, ReqKind::Cancel, MS);
        t.resp(41 * MS, 256, OrderStatus::Canceled, 1.0);
        t.local(42 * MS, 256, OrderStatus::Canceled, 0.0, 1.0, 41 * MS);

        // 512: filled while the cancel was in flight, so the exchange rejects the cancel.
        t.rest(50 * MS, 512, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(0.5));
        t.cancel_intent(60 * MS, 512, "requote");
        t.sent(60 * MS, 512, ReqKind::Cancel, MS);
        t.trade(60 * MS + 500_000, 512, 1.0, 0.5, -0.5);
        t.fill(60 * MS + 500_000, 512, 20_000, 1.0, true, 0.1, 0.0);
        t.resp(60 * MS + 500_000, 512, OrderStatus::Filled, 0.0);
        t.rejected_cancel(61 * MS, 512);

        let orders = build_orders(&t.0);
        let a = &orders[0];
        assert_eq!(a.outcome, OrderOutcome::Canceled);
        assert_eq!(a.cancel_sent_ts, Some(40 * MS));
        assert_eq!(a.cancel_reason.as_deref(), Some("requote"));
        assert!(!a.cancel_rejected);
        assert_eq!(a.terminal_ts, Some(41 * MS));

        let b = &orders[1];
        assert_eq!(b.outcome, OrderOutcome::Filled);
        assert_eq!(b.cancel_sent_ts, Some(60 * MS));
        assert!(b.cancel_rejected);
        // The rejected cancel's response does not overwrite the order's own status.
        assert_eq!(b.final_status, OrderStatus::Filled);
        assert_eq!(b.terminal_ts, Some(60 * MS + 500_000));
    }

    #[test]
    fn partial_fills_average_the_price_and_keep_the_outcome_partial() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_004, 3.0, Tif::Ioc);
        t.sent(10 * MS, 256, ReqKind::New, MS);
        t.fill(11 * MS, 256, 20_002, 1.0, false, 0.1, 2.0);
        t.fill(11 * MS, 256, 20_004, 1.0, false, 0.1, 1.0);
        t.resp(11 * MS, 256, OrderStatus::Expired, 1.0);

        let o = &build_orders(&t.0)[0];
        assert_eq!(o.outcome, OrderOutcome::PartiallyFilled);
        assert_eq!(o.final_status, OrderStatus::Expired);
        assert_eq!(o.filled_qty, 2.0);
        assert_eq!(o.taker_qty, 2.0);
        assert_eq!(o.fill_count, 2);
        assert_eq!(o.avg_fill_tick, Some(20_003.0));
        assert_eq!(o.fees, 0.2);
        assert_eq!(o.terminal_ts, Some(11 * MS));
    }
}
