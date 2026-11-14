//! The shared flight-recorder state that every engine hook writes to.
//!
//! # Deterministic ordering
//!
//! hftbacktest 0.9.4 keeps the strategy's orders in `HashMap`s and iterates them when a market
//! event may fill several orders at once. `HashMap` iteration order is randomized per instance, so
//! the order in which the engine emits callbacks for *different* orders inside one processing step
//! is not reproducible. Each order's own outcome does not depend on that order (the engine
//! evaluates every order independently against the same market event), but the tape sequence and
//! the running position would.
//!
//! The recorder therefore buffers everything emitted inside one engine step and commits it sorted
//! by order id, keeping each order's events in their original relative order. Side effects on the
//! ledger are applied at commit, in that same canonical order.

use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

use bb_core::{
    OrderStatus, RelNs, ReqKind, Side,
    config::RecordConfig,
    dataset::InstrumentMeta,
    flight::{FlightBody, FlightEvent, QueueCause},
    result::FillRecord,
};

use crate::ledger::Book;

/// Ledger effect attached to a buffered event.
#[derive(Clone, Copy)]
enum Effect {
    None,
    /// An exchange-side execution: update the true position and remember the fill.
    Fill {
        side: Side,
    },
    /// A response reached the local side: fills of this order up to `exch_ts` become known.
    Release {
        exch_ts: RelNs,
    },
}

struct Pending {
    ts: RelNs,
    order_id: u64,
    body: FlightBody,
    effect: Effect,
}

#[derive(Default)]
struct QueueCount {
    kept: u32,
    dropped: u64,
    first_drop_ts: RelNs,
    /// Last engine state read, not merely the last value retained on the capped tape.
    ahead: Option<f64>,
}

pub(crate) struct Inner {
    origin: i64,
    instrument: InstrumentMeta,
    half_lot: f64,
    queue_cap: u32,
    /// Absolute exchange time of the engine step in progress.
    exch_now: i64,
    step: Option<Vec<Pending>>,
    /// Draining retains the allocation for the next engine step.
    step_buffer: Vec<Pending>,
    events: Vec<FlightEvent>,
    queue_counts: BTreeMap<u64, QueueCount>,
    /// All simulated fills as they happened at the exchange.
    truth: Book,
    /// Fills whose notification has reached the strategy host.
    known: Book,
    fills: Vec<FillRecord>,
    /// Per order: indices into `fills` not yet known locally.
    unreleased: BTreeMap<u64, VecDeque<usize>>,
    /// Queue-estimate resets reported by the queue model while deciding a fill, waiting for the
    /// `ExchFill` they belong to: `(before, after)`.
    pending_reset: BTreeMap<u64, (Option<f64>, Option<f64>)>,
    /// Cancel requests sent and not yet answered locally, per order. A cancel is answered by
    /// exactly one response: `Canceled`, or a rejection when the order was already gone.
    cancels_in_flight: BTreeMap<u64, u32>,
    /// Executions the engine's local account applied for responses that report none: 0.9.4's
    /// `Local::process_recv_order_` applies every response whose status is `Filled`, including
    /// the rejection of a cancel that lost the race against that fill, so its account counts
    /// that fill twice. Used only to explain the engine cross-check.
    engine_refills: u64,
}

/// Cheap handle shared by the engine hooks and the harness. Single-threaded by construction: the
/// engine itself is `Rc`-based and a simulation never leaves the thread that built it.
#[derive(Clone)]
pub(crate) struct Tape(Rc<RefCell<Inner>>);

impl Tape {
    pub fn new(origin: i64, instrument: InstrumentMeta, record: &RecordConfig) -> Self {
        Tape(Rc::new(RefCell::new(Inner {
            origin,
            instrument,
            half_lot: instrument.lot_size * 0.5,
            queue_cap: record.max_queue_updates_per_order,
            exch_now: origin,
            step: None,
            step_buffer: Vec::new(),
            events: Vec::new(),
            queue_counts: BTreeMap::new(),
            truth: Book::default(),
            known: Book::default(),
            fills: Vec::new(),
            unreleased: BTreeMap::new(),
            pending_reset: BTreeMap::new(),
            cancels_in_flight: BTreeMap::new(),
            engine_refills: 0,
        })))
    }

    /// Converts absolute engine nanoseconds to dataset-relative nanoseconds.
    pub fn rel(&self, abs: i64) -> RelNs {
        abs - self.0.borrow().origin
    }

    /// Events committed to the tape so far.
    pub fn event_count(&self) -> usize {
        self.0.borrow().events.len()
    }

    /// Marks the start of one engine processing step. Local-side steps leave the last exchange
    /// timestamp unchanged.
    pub fn begin_step(&self, exch_now: Option<i64>) {
        let mut t = self.0.borrow_mut();
        if let Some(now) = exch_now {
            t.exch_now = now;
        }
        debug_assert!(t.step.is_none(), "engine steps do not nest");
        t.step = Some(std::mem::take(&mut t.step_buffer));
    }

    /// Commits the step's events in canonical order.
    pub fn end_step(&self) {
        let mut t = self.0.borrow_mut();
        let Some(mut pending) = t.step.take() else {
            return;
        };
        // Stable: each order's events keep their engine order.
        pending.sort_by_key(|p| p.order_id);
        for p in pending.drain(..) {
            t.commit(p);
        }
        t.step_buffer = pending;
    }

    fn push(&self, ts: RelNs, order_id: u64, body: FlightBody, effect: Effect) {
        let mut t = self.0.borrow_mut();
        let p = Pending {
            ts,
            order_id,
            body,
            effect,
        };
        match t.step.as_mut() {
            Some(step) => step.push(p),
            None => t.commit(p),
        }
    }

    /// Records a harness (strategy-side) event for an order. Never called inside an engine step.
    pub fn strategy_event(&self, ts: RelNs, order_id: u64, body: FlightBody) {
        self.push(ts, order_id, body, Effect::None);
    }

    /// Records a run-level event with no order.
    pub fn run_event(&self, ts: RelNs, body: FlightBody) {
        let mut t = self.0.borrow_mut();
        debug_assert!(
            t.step.is_none(),
            "run-level events are emitted by the harness only"
        );
        let seq = t.events.len() as u64;
        t.events.push(FlightEvent::new(seq, ts, None, body));
    }

    // ----- engine hooks -----

    pub fn request_sent(&self, local_abs: i64, order_id: u64, req: ReqKind, latency: i64) {
        let ts = self.rel(local_abs);
        let exch_arrival_ts = (latency >= 0).then(|| ts + latency);
        if req == ReqKind::Cancel && latency >= 0 {
            *self
                .0
                .borrow_mut()
                .cancels_in_flight
                .entry(order_id)
                .or_default() += 1;
        }
        self.push(
            ts,
            order_id,
            FlightBody::RequestSent {
                req,
                entry_latency_ns: latency,
                exch_arrival_ts,
            },
            Effect::None,
        );
    }

    pub fn request_refused(&self, local_abs: i64, order_id: u64, req: ReqKind, error: String) {
        let ts = self.rel(local_abs);
        self.push(
            ts,
            order_id,
            FlightBody::RequestRefused { req, error },
            Effect::None,
        );
    }

    pub fn exch_queued(
        &self,
        order_id: u64,
        queue_ahead: Option<f64>,
        level_qty: f64,
        best_bid_tick: Option<i64>,
        best_ask_tick: Option<i64>,
    ) {
        self.0
            .borrow_mut()
            .queue_counts
            .entry(order_id)
            .or_default()
            .ahead = queue_ahead;
        let ts = self.exch_now_rel();
        self.push(
            ts,
            order_id,
            FlightBody::ExchQueued {
                queue_ahead,
                level_qty,
                best_bid_tick,
                best_ask_tick,
            },
            Effect::None,
        );
    }

    /// Records a queue estimate change, subject to the per-order cap. Trade-driven updates are
    /// always kept: they are what fills rest on. Once an order has `queue_cap` updates, a
    /// depth-driven update is kept only if it moves the estimate by at least one lot (or the
    /// estimate is unreadable); the rest are counted and reported by a `QueueTruncated` marker.
    pub fn queue_update(
        &self,
        order_id: u64,
        cause: QueueCause,
        ahead_before: Option<f64>,
        ahead_after: Option<f64>,
    ) {
        let ts = self.exch_now_rel();
        {
            let mut t = self.0.borrow_mut();
            let lot = t.instrument.lot_size;
            let cap = t.queue_cap;
            let count = t.queue_counts.entry(order_id).or_default();
            count.ahead = ahead_after;
            let significant = match (cause, ahead_before, ahead_after) {
                (QueueCause::Trade { .. }, _, _) => true,
                (_, Some(b), Some(a)) => (a - b).abs() >= lot,
                _ => true,
            };
            if count.kept >= cap && !significant {
                if count.dropped == 0 {
                    count.first_drop_ts = ts;
                }
                count.dropped += 1;
                return;
            }
            count.kept = count.kept.saturating_add(1);
        }
        self.push(
            ts,
            order_id,
            FlightBody::QueueUpdate {
                cause,
                ahead_before,
                ahead_after,
            },
            Effect::None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn exch_fill(
        &self,
        exch_abs: i64,
        order_id: u64,
        side: Side,
        price_tick: i64,
        qty: f64,
        maker: bool,
        fee: f64,
        value: f64,
        leaves_qty: f64,
    ) {
        let ts = self.rel(exch_abs);
        self.push(
            ts,
            order_id,
            FlightBody::ExchFill {
                price_tick,
                qty,
                maker,
                fee,
                value,
                leaves_qty,
            },
            Effect::Fill { side },
        );
        // The engine reset the surviving remainder's queue estimate while deciding this fill.
        // Recorded after the fill so the exhausting trade update stays directly before it.
        let reset = self.0.borrow_mut().pending_reset.remove(&order_id);
        if let Some((before, after)) = reset {
            self.push(
                ts,
                order_id,
                FlightBody::QueueUpdate {
                    cause: QueueCause::FillReset,
                    ahead_before: before,
                    ahead_after: after,
                },
                Effect::None,
            );
        }
    }

    /// Notes that the queue model reset an order's estimate while deciding a partial fill.
    pub fn queue_reset(&self, order_id: u64, before: Option<f64>, after: Option<f64>) {
        let mut t = self.0.borrow_mut();
        t.queue_counts.entry(order_id).or_default().ahead = after;
        t.pending_reset.insert(order_id, (before, after));
    }

    pub fn exch_response(
        &self,
        exch_abs: i64,
        order_id: u64,
        status: OrderStatus,
        request_rejected: bool,
        leaves_qty: f64,
        latency: i64,
    ) {
        let ts = self.rel(exch_abs);
        self.push(
            ts,
            order_id,
            FlightBody::ExchResponse {
                status,
                request_rejected,
                leaves_qty,
                response_latency_ns: latency,
                local_arrival_ts: ts + latency,
            },
            Effect::None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn local_response(
        &self,
        local_abs: i64,
        order_id: u64,
        status: OrderStatus,
        request_rejected: bool,
        exec_qty: f64,
        exec_price_tick: i64,
        leaves_qty: f64,
        exch_abs: i64,
    ) {
        let ts = self.rel(local_abs);
        let exch_ts = self.rel(exch_abs);
        self.push(
            ts,
            order_id,
            FlightBody::LocalResponse {
                status,
                request_rejected,
                exec_qty,
                exec_price_tick,
                leaves_qty,
                exch_ts,
            },
            Effect::Release { exch_ts },
        );
    }

    // ----- harness queries -----

    /// Only the Debug-based probe uses this cache: a direct f64 downcast is cheaper.
    pub fn queue_ahead(&self, order_id: u64) -> Option<f64> {
        self.0
            .borrow()
            .queue_counts
            .get(&order_id)
            .and_then(|c| c.ahead)
    }

    fn exch_now_rel(&self) -> RelNs {
        let t = self.0.borrow();
        t.exch_now - t.origin
    }

    /// The ledger of all simulated fills so far.
    pub fn truth(&self) -> Book {
        self.0.borrow().truth
    }

    /// The ledger of fills the strategy host has been told about.
    pub fn known(&self) -> Book {
        self.0.borrow().known
    }

    /// A cancel for this order has been sent and its answer has not reached the local side.
    pub fn cancel_in_flight(&self, order_id: u64) -> bool {
        self.0.borrow().cancels_in_flight.contains_key(&order_id)
    }

    /// Records executions the engine's local account applied a second time (see
    /// `Inner::engine_refills`).
    pub fn engine_refills_add(&self, n: u64) {
        self.0.borrow_mut().engine_refills += n;
    }

    pub fn engine_refills(&self) -> u64 {
        self.0.borrow().engine_refills
    }

    /// Appends one `QueueTruncated` marker per order that had updates dropped, then returns the
    /// finished tape and fills.
    pub fn finish(self) -> (Vec<FlightEvent>, Vec<FillRecord>) {
        let mut t = self.0.borrow_mut();
        debug_assert!(t.step.is_none(), "finish inside an engine step");
        let markers: Vec<(u64, RelNs, u64)> = t
            .queue_counts
            .iter()
            .filter(|(_, c)| c.dropped > 0)
            .map(|(id, c)| (*id, c.first_drop_ts, c.dropped))
            .collect();
        for (order_id, ts, dropped) in markers {
            let seq = t.events.len() as u64;
            t.events.push(FlightEvent::new(
                seq,
                ts,
                Some(order_id),
                FlightBody::QueueTruncated { dropped },
            ));
        }
        (std::mem::take(&mut t.events), std::mem::take(&mut t.fills))
    }
}

impl Inner {
    fn commit(&mut self, p: Pending) {
        let seq = self.events.len() as u64;
        match p.effect {
            Effect::None => {}
            Effect::Fill { side } => {
                if let FlightBody::ExchFill {
                    price_tick,
                    qty,
                    maker,
                    fee,
                    value,
                    ..
                } = p.body
                {
                    let price = self.instrument.price(price_tick);
                    let realized_pnl = self.truth.apply(
                        side.sign(),
                        price,
                        qty,
                        value,
                        fee,
                        self.instrument.contract_size,
                        self.half_lot,
                    );
                    self.unreleased
                        .entry(p.order_id)
                        .or_default()
                        .push_back(self.fills.len());
                    self.fills.push(FillRecord {
                        seq,
                        order_id: p.order_id,
                        ts: p.ts,
                        side,
                        price_tick,
                        qty,
                        maker,
                        fee,
                        value,
                        position_after: self.truth.position,
                        realized_pnl,
                        mid_tick: None,
                        markout_ticks: Vec::new(),
                    });
                }
            }
            Effect::Release { exch_ts } => {
                if let FlightBody::LocalResponse {
                    status,
                    request_rejected,
                    ..
                } = p.body
                    && (request_rejected || status == OrderStatus::Canceled)
                    && let Some(n) = self.cancels_in_flight.get_mut(&p.order_id)
                {
                    *n -= 1;
                    if *n == 0 {
                        self.cancels_in_flight.remove(&p.order_id);
                    }
                }
                if let Some(queue) = self.unreleased.get_mut(&p.order_id) {
                    while let Some(&idx) = queue.front() {
                        let f = &self.fills[idx];
                        if f.ts > exch_ts {
                            break;
                        }
                        let price = self.instrument.price(f.price_tick);
                        self.known.apply(
                            f.side.sign(),
                            price,
                            f.qty,
                            f.value,
                            f.fee,
                            self.instrument.contract_size,
                            self.half_lot,
                        );
                        queue.pop_front();
                    }
                    if queue.is_empty() {
                        self.unreleased.remove(&p.order_id);
                    }
                }
            }
        }
        self.events
            .push(FlightEvent::new(seq, p.ts, Some(p.order_id), p.body));
    }
}
