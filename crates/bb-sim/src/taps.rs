//! Recording wrappers around the engine's own models.
//!
//! Each wrapper implements a public hftbacktest trait, delegates every decision to the genuine
//! engine model, and reports what the engine did to the [`Tape`]. No simulation logic lives here.

use std::marker::PhantomData;

use bb_core::{OrderStatus, ReqKind, Side, flight::QueueCause, rng::derive_seed};
use hftbacktest::{
    backtest::models::{ConstantLatency, FeeModel, LatencyModel, QueueModel, QueuePos},
    depth::{INVALID_MAX, INVALID_MIN, MarketDepth},
    types::{Order, Side as HSide, Status},
};

use crate::{
    latency::{StochasticLatency, keyed_rng},
    tape::Tape,
};

pub(crate) fn side_of(side: HSide) -> Option<Side> {
    match side {
        HSide::Buy => Some(Side::Buy),
        HSide::Sell => Some(Side::Sell),
        _ => None,
    }
}

pub(crate) fn status_of(status: Status) -> OrderStatus {
    match status {
        Status::None => OrderStatus::None,
        Status::New => OrderStatus::New,
        Status::Expired => OrderStatus::Expired,
        Status::Filled => OrderStatus::Filled,
        Status::Canceled => OrderStatus::Canceled,
        Status::PartiallyFilled => OrderStatus::PartiallyFilled,
        Status::Rejected | Status::Replaced | Status::Unsupported => OrderStatus::Unsupported,
    }
}

fn req_of(req: Status) -> ReqKind {
    match req {
        Status::New => ReqKind::New,
        Status::Canceled => ReqKind::Cancel,
        Status::Replaced => ReqKind::Modify,
        _ => ReqKind::None,
    }
}

// ---------------------------------------------------------------------------------------------
// Queue position
// ---------------------------------------------------------------------------------------------

/// Reads the modeled quantity ahead from the queue model's private per-order state.
pub(crate) trait QueueProbe {
    fn ahead(order: &Order) -> Option<f64>;

    fn before(order: &Order, _tape: &Tape) -> Option<f64> {
        Self::ahead(order)
    }
}

/// `RiskAdverseQueueModel` stores the quantity ahead as a bare `f64`.
pub(crate) struct F64Probe;

impl QueueProbe for F64Probe {
    fn ahead(order: &Order) -> Option<f64> {
        (*order.q).as_any().downcast_ref::<f64>().copied()
    }
}

/// `ProbQueueModel` stores a `QueuePos` whose fields are private in 0.9.4. `QueuePos` derives
/// `Debug`, and Rust's `Debug` for `f64` prints the shortest string that round-trips exactly, so
/// the value is recovered bit-for-bit from the debug representation. This keeps the genuine
/// engine model in charge of the arithmetic instead of re-implementing it. A test pins the
/// format; if a future engine version changes it the probe returns `None` and the recorder
/// reports the estimate as unreadable rather than guessing.
pub(crate) struct QueuePosProbe;

impl QueueProbe for QueuePosProbe {
    fn ahead(order: &Order) -> Option<f64> {
        let pos = (*order.q).as_any().downcast_ref::<QueuePos>()?;
        crate::queue_probe::front_qty(pos)
    }

    fn before(order: &Order, tape: &Tape) -> Option<f64> {
        // All mutations of a surviving order go through this tap. Cache every observed value,
        // even dropped depth updates and partial-fill resets; terminal orders need no next read.
        tape.queue_ahead(order.order_id)
    }
}

#[cfg(test)]
use crate::queue_probe::parse_front_qty;

/// Wraps an engine queue model and records every estimate it produces.
pub(crate) struct TapQueue<QM, PR> {
    inner: QM,
    tape: Tape,
    /// The exchange model is `PartialFillExchange`, where an order can survive a fill.
    partial_fills: bool,
    _probe: PhantomData<PR>,
}

impl<QM, PR> TapQueue<QM, PR> {
    pub fn new(inner: QM, tape: Tape, partial_fills: bool) -> Self {
        Self {
            inner,
            tape,
            partial_fills,
            _probe: PhantomData,
        }
    }
}

impl<QM, PR, MD> QueueModel<MD> for TapQueue<QM, PR>
where
    QM: QueueModel<MD>,
    PR: QueueProbe,
    MD: MarketDepth,
{
    fn new_order(&self, order: &mut Order, depth: &MD) {
        self.inner.new_order(order, depth);
        let level_qty = match order.side {
            HSide::Buy => depth.bid_qty_at_tick(order.price_tick),
            _ => depth.ask_qty_at_tick(order.price_tick),
        };
        let bid = depth.best_bid_tick();
        let ask = depth.best_ask_tick();
        self.tape.exch_queued(
            order.order_id,
            PR::ahead(order),
            level_qty,
            (bid != INVALID_MIN).then_some(bid),
            (ask != INVALID_MAX).then_some(ask),
        );
    }

    fn trade(&self, order: &mut Order, qty: f64, depth: &MD) {
        let before = PR::before(order, &self.tape);
        self.inner.trade(order, qty, depth);
        self.tape.queue_update(
            order.order_id,
            QueueCause::Trade { qty },
            before,
            PR::ahead(order),
        );
    }

    fn depth(&self, order: &mut Order, prev_qty: f64, new_qty: f64, depth: &MD) {
        let before = PR::before(order, &self.tape);
        self.inner.depth(order, prev_qty, new_qty, depth);
        self.tape.queue_update(
            order.order_id,
            QueueCause::Depth { prev_qty, new_qty },
            before,
            PR::ahead(order),
        );
    }

    /// Delegates to the engine model, with one correction for a defect in 0.9.4's
    /// `PartialFillExchange`: it removes a completed resting order from its book only when the
    /// reported executable quantity is strictly greater than the order's remaining quantity. When
    /// the two are equal the order is marked `Filled` but stays in the book, and the next trade
    /// at its price aborts the backtest with `InvalidOrderStatus` (pinned by
    /// `tests/engine_defects.rs`). Reporting "remaining plus one lot" whenever the engine's own
    /// model says the whole remainder is executable routes the exchange through its removal
    /// branch. The executed quantity is unaffected: the exchange caps it at the remainder.
    /// `NoPartialFillExchange` only tests the result for being positive, so it is unaffected.
    fn is_filled(&self, order: &mut Order, depth: &MD) -> f64 {
        let before = PR::before(order, &self.tape);
        let executable = self.inner.is_filled(order, depth);
        let lot = depth.lot_size();
        if executable > 0.0 && executable >= order.leaves_qty - lot * 0.5 {
            order.leaves_qty + lot
        } else {
            // Under the partial-fill model a positive amount below the remainder is a partial
            // fill: the order survives and the engine model has just reset its estimate.
            if executable > 0.0 && self.partial_fills {
                let after = PR::ahead(order);
                if after != before {
                    self.tape.queue_reset(order.order_id, before, after);
                }
            }
            executable
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Order latency
// ---------------------------------------------------------------------------------------------

/// Constant base latency plus seeded uniform jitter.
///
/// The jitter is a pure function of the seed and of what is being delayed, not a draw from a
/// sequential stream: the engine may emit responses for several orders in an unspecified order
/// within one step (see `tape.rs`), and a sequential stream would hand them different draws from
/// run to run.
///
/// * Entry jitter depends on `(order id, request time)`.
/// * Response jitter depends on the exchange timestamp only. Every response the exchange emits at
///   one instant therefore shares one latency, so the order bus's "never overtake an earlier
///   message" rule cannot make arrival times depend on emission order either.
#[derive(Clone)]
pub(crate) struct JitterLatency {
    entry_ns: i64,
    response_ns: i64,
    jitter_ns: i64,
    entry_seed: u64,
    response_seed: u64,
}

impl JitterLatency {
    pub fn new(entry_ns: i64, response_ns: i64, jitter_ns: i64, seed: u64) -> Self {
        Self {
            entry_ns,
            response_ns,
            jitter_ns,
            entry_seed: derive_seed(seed, "latency.entry"),
            response_seed: derive_seed(seed, "latency.response"),
        }
    }

    fn draw(&self, stream_seed: u64, a: u64, b: u64) -> i64 {
        if self.jitter_ns == 0 {
            return 0;
        }
        keyed_rng(stream_seed, a, b).below(self.jitter_ns as u64 + 1) as i64
    }
}

impl LatencyModel for JitterLatency {
    fn entry(&mut self, timestamp: i64, order: &Order) -> i64 {
        self.entry_ns + self.draw(self.entry_seed, order.order_id, timestamp as u64)
    }

    fn response(&mut self, timestamp: i64, _order: &Order) -> i64 {
        self.response_ns + self.draw(self.response_seed, timestamp as u64, 0)
    }
}

/// The configured latency model: the engine's own `ConstantLatency`, or one of BlackBox's seeded
/// models (uniform jitter; log-normal and heavy-tailed legs).
#[derive(Clone)]
pub(crate) enum LatencyKind {
    Constant(ConstantLatency),
    Jitter(JitterLatency),
    Stochastic(StochasticLatency),
}

/// Wraps the latency model and records each request leaving the local side and each response
/// leaving the exchange.
#[derive(Clone)]
pub(crate) struct TapLatency {
    inner: LatencyKind,
    tape: Tape,
}

impl TapLatency {
    pub fn new(inner: LatencyKind, tape: Tape) -> Self {
        Self { inner, tape }
    }
}

impl LatencyModel for TapLatency {
    /// Called by `LocalToExch::request` with `timestamp == order.local_timestamp`.
    fn entry(&mut self, timestamp: i64, order: &Order) -> i64 {
        let latency = match &mut self.inner {
            LatencyKind::Constant(m) => m.entry(timestamp, order),
            LatencyKind::Jitter(m) => m.entry(timestamp, order),
            LatencyKind::Stochastic(m) => m.entry(timestamp, order),
        };
        self.tape
            .request_sent(timestamp, order.order_id, req_of(order.req), latency);
        latency
    }

    /// Called by `ExchToLocal::respond` with `timestamp == order.exch_timestamp`.
    fn response(&mut self, timestamp: i64, order: &Order) -> i64 {
        let latency = match &mut self.inner {
            LatencyKind::Constant(m) => m.response(timestamp, order),
            LatencyKind::Jitter(m) => m.response(timestamp, order),
            LatencyKind::Stochastic(m) => m.response(timestamp, order),
        };
        self.tape.exch_response(
            timestamp,
            order.order_id,
            status_of(order.status),
            order.req == Status::Rejected,
            order.leaves_qty,
            latency,
        );
        latency
    }
}

// ---------------------------------------------------------------------------------------------
// Fills
// ---------------------------------------------------------------------------------------------

/// Wraps the fee model given to the *exchange-side* account. The engine calls `amount` exactly
/// once per execution from `State::apply_fill`, after it has set the order's execution price,
/// quantity, maker flag and exchange timestamp, which makes this the one place every simulated
/// fill can be observed, including each level of a taker sweep and each partial maker fill.
///
/// The local-side account gets a plain fee model so that fills are not recorded twice.
#[derive(Clone)]
pub(crate) struct TapFee<F> {
    inner: F,
    tape: Tape,
}

impl<F> TapFee<F> {
    pub fn new(inner: F, tape: Tape) -> Self {
        Self { inner, tape }
    }
}

impl<F: FeeModel> FeeModel for TapFee<F> {
    fn amount(&self, order: &Order, amount: f64) -> f64 {
        let fee = self.inner.amount(order, amount);
        if let Some(side) = side_of(order.side) {
            self.tape.exch_fill(
                order.exch_timestamp,
                order.order_id,
                side,
                order.exec_price_tick,
                order.exec_qty,
                order.maker,
                fee,
                amount,
                order.leaves_qty,
            );
        }
        fee
    }
}

#[cfg(test)]
mod tests {
    use hftbacktest::{
        backtest::models::{LogProbQueueFunc, ProbQueueModel, RiskAdverseQueueModel},
        depth::{HashMapMarketDepth, L2MarketDepth},
        types::{OrdType, TimeInForce},
    };

    use super::*;

    fn resting_buy(tick: i64) -> Order {
        Order::new(
            1,
            tick,
            0.5,
            1.0,
            HSide::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        )
    }

    #[test]
    fn debug_format_parser_handles_float_forms() {
        assert_eq!(
            parse_front_qty("QueuePos { front_q_qty: 12.5, cum_trade_qty: 0.0 }"),
            Some(12.5)
        );
        assert_eq!(
            parse_front_qty("QueuePos { front_q_qty: -0.25, cum_trade_qty: 3.0 }"),
            Some(-0.25)
        );
        assert_eq!(
            parse_front_qty("QueuePos { front_q_qty: 1e-7, cum_trade_qty: 0.0 }"),
            Some(1e-7)
        );
        assert_eq!(parse_front_qty("QueuePos { other: 1.0 }"), None);
    }

    /// Pins the engine behaviour the probes rely on: after `new_order` the readable state equals
    /// the displayed quantity at the order's price, for both state representations, and the
    /// probe tracks the engine's own arithmetic exactly through a trade.
    #[test]
    fn probes_read_the_engine_models_real_state() {
        let mut depth = HashMapMarketDepth::new(0.5, 0.001);
        depth.update_bid_depth(100.0, 7.123, 1);
        depth.update_ask_depth(100.5, 3.0, 1);

        let risk = RiskAdverseQueueModel::<HashMapMarketDepth>::new();
        let mut o = resting_buy(200);
        risk.new_order(&mut o, &depth);
        assert_eq!(F64Probe::ahead(&o), Some(7.123));
        risk.trade(&mut o, 2.0, &depth);
        assert_eq!(F64Probe::ahead(&o), Some(7.123 - 2.0));
        // A probe for the wrong representation reports "unreadable", never a wrong number.
        assert_eq!(QueuePosProbe::ahead(&o), None);

        let prob = ProbQueueModel::<_, HashMapMarketDepth>::new(LogProbQueueFunc::new());
        let mut o = resting_buy(200);
        prob.new_order(&mut o, &depth);
        assert_eq!(QueuePosProbe::ahead(&o), Some(7.123));
        prob.trade(&mut o, 0.123, &depth);
        assert_eq!(QueuePosProbe::ahead(&o), Some(7.123 - 0.123));
        assert_eq!(F64Probe::ahead(&o), None);
    }

    #[test]
    fn jitter_is_a_pure_function_within_bounds() {
        let mut a = JitterLatency::new(1_000, 2_000, 500, 9);
        let mut b = JitterLatency::new(1_000, 2_000, 500, 9);
        let o1 = resting_buy(10);
        let mut o2 = resting_buy(10);
        o2.order_id = 2;
        // Same inputs give the same latency regardless of call order.
        let x1 = a.entry(5_000, &o1);
        let x2 = a.entry(5_000, &o2);
        let y2 = b.entry(5_000, &o2);
        let y1 = b.entry(5_000, &o1);
        assert_eq!((x1, x2), (y1, y2));
        for t in 0..2_000i64 {
            let e = a.entry(t, &o1);
            assert!((1_000..=1_500).contains(&e));
            let r = a.response(t, &o1);
            assert!((2_000..=2_500).contains(&r));
            // Responses at one exchange instant share a latency across orders.
            assert_eq!(r, a.response(t, &o2));
        }
        let mut other_seed = JitterLatency::new(1_000, 2_000, 500, 10);
        let differs = (0..64i64).any(|t| other_seed.entry(t, &o1) != a.entry(t, &o1));
        assert!(differs);
    }
}
