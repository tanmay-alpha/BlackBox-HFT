//! Recording wrappers around the engine's local and exchange processors.
//!
//! They delegate everything. Their only jobs are to bracket each engine processing step so the
//! tape can commit its events in a reproducible order, to tell the tape the current exchange
//! time, and to observe order responses as the local side receives them.

use std::collections::HashMap;

use hftbacktest::{
    backtest::{
        assettype::AssetType,
        models::{FeeModel, LatencyModel},
        proc::{Local, LocalProcessor, Processor},
        BacktestError,
    },
    depth::{L2MarketDepth, MarketDepth},
    types::{Event, OrdType, Order, OrderId, Side, StateValues, Status, TimeInForce},
};

use crate::{taps::status_of, tape::Tape};

/// Wraps an exchange processor (`NoPartialFillExchange` or `PartialFillExchange`).
pub(crate) struct RecExch<P> {
    inner: P,
    tape: Tape,
}

impl<P> RecExch<P> {
    pub fn new(inner: P, tape: Tape) -> Self {
        Self { inner, tape }
    }
}

impl<P: Processor> Processor for RecExch<P> {
    fn event_seen_timestamp(&self, event: &Event) -> Option<i64> {
        self.inner.event_seen_timestamp(event)
    }

    fn process(&mut self, event: &Event) -> Result<(), BacktestError> {
        self.tape.begin_step(Some(event.exch_ts));
        let result = self.inner.process(event);
        self.tape.end_step();
        result
    }

    fn process_recv_order(
        &mut self,
        timestamp: i64,
        wait_resp_order_id: Option<OrderId>,
    ) -> Result<bool, BacktestError> {
        self.tape.begin_step(Some(timestamp));
        let result = self.inner.process_recv_order(timestamp, wait_resp_order_id);
        self.tape.end_step();
        result
    }

    fn earliest_recv_order_timestamp(&self) -> i64 {
        self.inner.earliest_recv_order_timestamp()
    }

    fn earliest_send_order_timestamp(&self) -> i64 {
        self.inner.earliest_send_order_timestamp()
    }
}

/// Wraps the engine's `Local` processor.
pub(crate) struct RecLocal<AT, LM, MD, FM>
where
    AT: AssetType,
    LM: LatencyModel,
    MD: MarketDepth,
    FM: FeeModel,
{
    inner: Local<AT, LM, MD, FM>,
    tape: Tape,
}

impl<AT, LM, MD, FM> RecLocal<AT, LM, MD, FM>
where
    AT: AssetType,
    LM: LatencyModel,
    MD: MarketDepth,
    FM: FeeModel,
{
    pub fn new(inner: Local<AT, LM, MD, FM>, tape: Tape) -> Self {
        Self { inner, tape }
    }
}

impl<AT, LM, MD, FM> Processor for RecLocal<AT, LM, MD, FM>
where
    AT: AssetType,
    LM: LatencyModel,
    MD: MarketDepth + L2MarketDepth,
    FM: FeeModel,
{
    fn event_seen_timestamp(&self, event: &Event) -> Option<i64> {
        self.inner.event_seen_timestamp(event)
    }

    fn process(&mut self, event: &Event) -> Result<(), BacktestError> {
        self.inner.process(event)
    }

    fn process_recv_order(
        &mut self,
        timestamp: i64,
        wait_resp_order_id: Option<OrderId>,
    ) -> Result<bool, BacktestError> {
        let tape = self.tape.clone();
        tape.begin_step(None);
        // `process_recv_order_` is the engine's own receive routine with its optional observer
        // enabled; the default trait method calls the same routine with the observer disabled.
        let result =
            self.inner
                .process_recv_order_::<true, _>(timestamp, wait_resp_order_id, |order: &Order| {
                    // A rejected request (a cancel that arrived after the order was gone) carries
                    // the order's previous execution fields unchanged; it reports no execution.
                    let is_fill_report = order.req != Status::Rejected
                        && matches!(order.status, Status::Filled | Status::PartiallyFilled);
                    tape.local_response(
                        timestamp,
                        order.order_id,
                        status_of(order.status),
                        order.req == Status::Rejected,
                        // The engine leaves the previous execution's values on later responses;
                        // only fill reports carry a meaningful execution.
                        if is_fill_report { order.exec_qty } else { 0.0 },
                        if is_fill_report {
                            order.exec_price_tick
                        } else {
                            0
                        },
                        order.leaves_qty,
                        order.exch_timestamp,
                    );
                });
        tape.end_step();
        result
    }

    fn earliest_recv_order_timestamp(&self) -> i64 {
        self.inner.earliest_recv_order_timestamp()
    }

    fn earliest_send_order_timestamp(&self) -> i64 {
        self.inner.earliest_send_order_timestamp()
    }
}

impl<AT, LM, MD, FM> LocalProcessor<MD> for RecLocal<AT, LM, MD, FM>
where
    AT: AssetType,
    LM: LatencyModel,
    MD: MarketDepth + L2MarketDepth,
    FM: FeeModel,
{
    fn submit_order(
        &mut self,
        order_id: OrderId,
        side: Side,
        price: f64,
        qty: f64,
        order_type: OrdType,
        time_in_force: TimeInForce,
        current_timestamp: i64,
    ) -> Result<(), BacktestError> {
        self.inner.submit_order(
            order_id,
            side,
            price,
            qty,
            order_type,
            time_in_force,
            current_timestamp,
        )
    }

    fn modify(
        &mut self,
        order_id: OrderId,
        price: f64,
        qty: f64,
        current_timestamp: i64,
    ) -> Result<(), BacktestError> {
        self.inner.modify(order_id, price, qty, current_timestamp)
    }

    fn cancel(&mut self, order_id: OrderId, current_timestamp: i64) -> Result<(), BacktestError> {
        self.inner.cancel(order_id, current_timestamp)
    }

    fn clear_inactive_orders(&mut self) {
        self.inner.clear_inactive_orders()
    }

    fn position(&self) -> f64 {
        self.inner.position()
    }

    fn state_values(&self) -> &StateValues {
        self.inner.state_values()
    }

    fn depth(&self) -> &MD {
        self.inner.depth()
    }

    fn orders(&self) -> &HashMap<OrderId, Order> {
        self.inner.orders()
    }

    fn last_trades(&self) -> &[Event] {
        self.inner.last_trades()
    }

    fn clear_last_trades(&mut self) {
        self.inner.clear_last_trades()
    }

    fn feed_latency(&self) -> Option<(i64, i64)> {
        self.inner.feed_latency()
    }

    fn order_latency(&self) -> Option<(i64, i64, i64)> {
        self.inner.order_latency()
    }
}
