//! Hand-made views and intent accessors shared by the unit tests.

use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::{Intent, LocalView, MarketView, OpenOrder, Strategy};
use bb_core::{OrderStatus, Side, Tif};

pub(crate) const INSTRUMENT: InstrumentMeta = InstrumentMeta {
    tick_size: 0.01,
    lot_size: 0.001,
    contract_size: 1.0,
};

/// Strategy grid used by the tests: 100 ms.
pub(crate) const INTERVAL_NS: i64 = 100_000_000;

pub(crate) fn market_with_qty(
    best_bid_tick: Option<i64>,
    best_ask_tick: Option<i64>,
    best_bid_qty: f64,
    best_ask_qty: f64,
) -> MarketView {
    MarketView {
        best_bid_tick,
        best_ask_tick,
        best_bid_qty,
        best_ask_qty,
        feed_exch_ts: None,
        feed_local_ts: None,
    }
}

/// A book with equal size on both sides.
pub(crate) fn market(best_bid_tick: Option<i64>, best_ask_tick: Option<i64>) -> MarketView {
    market_with_qty(best_bid_tick, best_ask_tick, 1.0, 1.0)
}

pub(crate) fn view<'a>(
    tick_no: u64,
    market: MarketView,
    position: f64,
    open_orders: &'a [OpenOrder],
) -> LocalView<'a> {
    LocalView {
        now: tick_no as i64 * INTERVAL_NS,
        tick_no,
        tick_size: INSTRUMENT.tick_size,
        lot_size: INSTRUMENT.lot_size,
        market,
        position,
        open_orders,
        recent_trades: &[],
    }
}

fn open(
    order_id: u64,
    side: Side,
    price_tick: i64,
    status: OrderStatus,
    request_pending: bool,
) -> OpenOrder {
    OpenOrder {
        order_id,
        side,
        price_tick,
        qty: 1.0,
        leaves_qty: 1.0,
        status,
        request_pending,
    }
}

/// Acknowledged and resting with nothing in flight: the only state that may be cancelled.
pub(crate) fn resting(order_id: u64, side: Side, price_tick: i64) -> OpenOrder {
    open(order_id, side, price_tick, OrderStatus::New, false)
}

/// Sent but not yet acknowledged by the exchange.
pub(crate) fn pending_new(order_id: u64, side: Side, price_tick: i64) -> OpenOrder {
    open(order_id, side, price_tick, OrderStatus::None, true)
}

/// Resting with a cancel request in flight.
pub(crate) fn pending_cancel(order_id: u64, side: Side, price_tick: i64) -> OpenOrder {
    open(order_id, side, price_tick, OrderStatus::New, true)
}

pub(crate) fn run(strategy: &mut dyn Strategy, view: &LocalView<'_>) -> Vec<Intent> {
    let mut out = Vec::new();
    strategy.on_tick(view, &mut out);
    out
}

/// `(slot, side, price_tick, qty, tif)` of every placement, in emission order.
pub(crate) fn placed(intents: &[Intent]) -> Vec<(u8, Side, i64, f64, Tif)> {
    intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::Place {
                slot,
                side,
                price_tick,
                qty,
                tif,
                ..
            } => Some((*slot, *side, *price_tick, *qty, *tif)),
            Intent::Cancel { .. } => None,
        })
        .collect()
}

/// Order ids of every cancel, in emission order.
pub(crate) fn cancelled(intents: &[Intent]) -> Vec<u64> {
    intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::Cancel { order_id, .. } => Some(*order_id),
            Intent::Place { .. } => None,
        })
        .collect()
}

pub(crate) fn reasons(intents: &[Intent]) -> Vec<&str> {
    intents
        .iter()
        .map(|intent| match intent {
            Intent::Place { reason, .. } | Intent::Cancel { reason, .. } => reason.as_str(),
        })
        .collect()
}
