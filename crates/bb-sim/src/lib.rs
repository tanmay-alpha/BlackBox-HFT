//! Simulation adapter and flight recorder around the pinned hftbacktest engine.
//!
//! [`simulate`] replays a dataset through the engine with the configured execution assumptions,
//! evaluates a [`Strategy`] on a fixed local-time grid, enforces pre-trade risk rules, and returns
//! the flight tape, every simulated fill and periodic account samples.
//!
//! What this crate adds on top of the engine is observation and bookkeeping only. Matching, queue
//! estimation, latency scheduling and fees are the engine's (see `engine.rs` and `taps.rs`).
//!
//! # Limits that apply to every result
//!
//! * Historical replay: simulated orders never change the replayed prices or liquidity. A taking
//!   order executes against displayed depth without consuming it.
//! * Level-2 data: queue position is a model estimate, never an observation.
//! * Only `submit_buy_order` / `submit_sell_order` are used. `Bot::submit_order` in 0.9.4 ignores
//!   the side and always sells.

mod engine;
mod ledger;
mod procs;
mod queue_probe;
mod tape;
mod taps;

use bb_core::{
    OrdType, RelNs, ReqKind, Side, Tif,
    config::ExchangeModel,
    config::RunConfig,
    dataset::InstrumentMeta,
    flight::{FlightBody, FlightEvent, RiskRule},
    result::{EngineStateCheck, EquitySample, FillRecord},
    strategy::{Intent, LocalView, MarketView, OpenOrder, Strategy, TradePrint, order_id},
};
pub use hftbacktest::types::Event;
use hftbacktest::{
    backtest::Backtest,
    depth::{INVALID_MAX, INVALID_MIN, MarketDepth},
    types::{
        BUY_EVENT, Bot, ElapseResult, LOCAL_EVENT, OrdType as HOrdType, SELL_EVENT, Status,
        TimeInForce,
    },
};

pub use crate::ledger::Book;
use crate::{
    engine::Depth,
    tape::Tape,
    taps::{side_of, status_of},
};

/// Everything a simulation needs.
pub struct SimInput<'a> {
    /// Engine event rows with absolute-nanosecond timestamps, as produced by `bb-data`.
    pub events: &'a [Event],
    pub instrument: InstrumentMeta,
    /// Absolute epoch nanoseconds that all output timestamps are made relative to.
    pub origin_ns: i64,
    pub config: &'a RunConfig,
}

/// Raw simulation output, before post-run analysis.
#[derive(Debug)]
pub struct SimOutput {
    pub tape: Vec<FlightEvent>,
    /// `mid_tick` is `None` and `markout_ticks` is empty here; `bb-forensics` fills them.
    pub fills: Vec<FillRecord>,
    pub equity: Vec<EquitySample>,
    /// Number of strategy ticks evaluated.
    pub ticks: u64,
    pub engine_check: EngineStateCheck,
}

#[derive(thiserror::Error, Debug)]
pub enum SimError {
    #[error("invalid run configuration: {}", .0.join("; "))]
    InvalidConfig(Vec<String>),
    #[error("invalid simulation input: {0}")]
    InvalidInput(String),
    #[error("engine error: {0}")]
    Engine(String),
}

const ASSET: usize = 0;

/// Runs one simulation to completion.
///
/// The run ends at the first strategy tick that falls after the last feed event's local receipt
/// time. Requests still in flight at that point are left unresolved and appear on the tape as
/// sent but never processed; nothing is assumed about what would have happened to them.
pub fn simulate(
    input: &SimInput<'_>,
    mut strategy: Box<dyn Strategy>,
) -> Result<SimOutput, SimError> {
    let cfg = input.config;
    let problems = cfg.validate();
    if !problems.is_empty() {
        return Err(SimError::InvalidConfig(problems));
    }
    let inst = input.instrument;
    for (name, v) in [
        ("tick_size", inst.tick_size),
        ("lot_size", inst.lot_size),
        ("contract_size", inst.contract_size),
    ] {
        if !(v.is_finite() && v > 0.0) {
            return Err(SimError::InvalidInput(format!(
                "instrument {name} must be finite and positive, got {v}"
            )));
        }
    }
    let offset = cfg.execution.feed_latency_offset_ns;
    let Some(last_local_ts) = input
        .events
        .iter()
        .filter(|e| e.ev & LOCAL_EVENT != 0)
        .map(|e| e.local_ts)
        .max()
    else {
        return Err(SimError::InvalidInput(
            "dataset has no locally visible events".into(),
        ));
    };
    let data_end = last_local_ts + offset;

    let tape = Tape::new(input.origin_ns, inst, &cfg.record);
    let mut hbt = engine::build(input, &tape)?;
    let mut run = Harness {
        tape: tape.clone(),
        inst,
        cfg,
        half_lot: inst.lot_size * 0.5,
        equity: Vec::new(),
        peak_equity: f64::NEG_INFINITY,
        last_mid: None,
        halted: false,
        open: Vec::new(),
        trades: Vec::new(),
        intents: Vec::new(),
    };

    let interval = cfg.strategy.interval_ns;
    let sample_every = cfg.record.sample_every_ticks as u64;
    let mut tick_no: u64 = 0;
    // Local time of the most recent evaluated tick.
    let mut last_tick_abs: Option<i64> = None;
    loop {
        if hbt.elapse(interval).map_err(engine_err)? == ElapseResult::EndOfData {
            break;
        }
        let now_abs = hbt.current_timestamp();
        if now_abs > data_end {
            break;
        }
        run.tick(&mut hbt, strategy.as_mut(), tick_no)?;
        if tick_no.is_multiple_of(sample_every) {
            run.sample(&hbt, tick_no, now_abs);
        }
        last_tick_abs = Some(now_abs);
        tick_no += 1;
    }
    // The final `elapse` processed whatever data and order events followed the last tick, which
    // can include fills. Close the series one interval after the last tick so the last sample
    // always reflects everything on the tape. (When the engine ran out of events its clock is
    // not advanced, so the time is computed rather than read.)
    if let Some(last) = last_tick_abs {
        run.sample(&hbt, tick_no, last + interval);
    }

    let engine_check = run.engine_check(&hbt);
    let equity = std::mem::take(&mut run.equity);
    drop(run);
    // The engine holds clones of the tape handle inside its models; release them first.
    drop(hbt);
    let (tape, fills) = tape.finish();
    Ok(SimOutput {
        tape,
        fills,
        equity,
        ticks: tick_no,
        engine_check,
    })
}

fn engine_err(e: hftbacktest::backtest::BacktestError) -> SimError {
    SimError::Engine(e.to_string())
}

struct Harness<'a> {
    tape: Tape,
    inst: InstrumentMeta,
    cfg: &'a RunConfig,
    half_lot: f64,
    equity: Vec<EquitySample>,
    peak_equity: f64,
    /// Last local mid seen, in price units; used to mark while the book is one-sided.
    last_mid: Option<f64>,
    halted: bool,
    // Reused per-tick buffers.
    open: Vec<OpenOrder>,
    trades: Vec<TradePrint>,
    intents: Vec<Intent>,
}

impl Harness<'_> {
    fn market(&self, hbt: &Backtest<Depth>) -> MarketView {
        let d = hbt.depth(ASSET);
        let bid = d.best_bid_tick();
        let ask = d.best_ask_tick();
        let has_bid = bid != INVALID_MIN;
        let has_ask = ask != INVALID_MAX;
        let feed = hbt.feed_latency(ASSET);
        MarketView {
            best_bid_tick: has_bid.then_some(bid),
            best_ask_tick: has_ask.then_some(ask),
            best_bid_qty: if has_bid { d.best_bid_qty() } else { 0.0 },
            best_ask_qty: if has_ask { d.best_ask_qty() } else { 0.0 },
            feed_exch_ts: feed.map(|(exch, _)| self.tape.rel(exch)),
            feed_local_ts: feed.map(|(_, local)| self.tape.rel(local)),
        }
    }

    /// Net account value marked at `mid` (price units), and its components, for one ledger.
    fn mark_book(&mut self, market: &MarketView, book: Book) -> (f64, f64, f64) {
        if let Some(mid_tick) = market.mid_tick() {
            self.last_mid = Some(mid_tick * self.inst.tick_size);
        }
        let unrealized = match self.last_mid {
            Some(mid) => book.unrealized(mid, self.inst.contract_size, self.half_lot),
            None => 0.0,
        };
        let equity = book.realized + unrealized - book.fees;
        (equity, book.realized, unrealized)
    }

    /// Equity from every simulated fill (exchange-side truth): what the run reports.
    fn mark(&mut self, market: &MarketView) -> (f64, f64, f64) {
        let truth = self.tape.truth();
        self.mark_book(market, truth)
    }

    fn tick(
        &mut self,
        hbt: &mut Backtest<Depth>,
        strategy: &mut dyn Strategy,
        tick_no: u64,
    ) -> Result<(), SimError> {
        // The engine forgets a terminal order on clearing. If a cancel for it is still in flight
        // (a cancel racing a fill), its eventual rejection would then be consumed silently,
        // because the engine only reports responses for orders it still holds. The engine's own
        // `req` field cannot tell: a fill response overwrites it. Clearing is therefore deferred
        // while any terminal order has an unanswered cancel, which the tape tracks.
        let tape = &self.tape;
        let must_keep = hbt
            .orders(ASSET)
            .values()
            .any(|o| status_of(o.status).is_terminal() && tape.cancel_in_flight(o.order_id));
        if !must_keep {
            hbt.clear_inactive_orders(Some(ASSET));
        }
        let now_abs = hbt.current_timestamp();
        let now = self.tape.rel(now_abs);
        let market = self.market(hbt);

        // The strategy's working orders, in a reproducible order (the engine stores them in a
        // HashMap).
        self.open.clear();
        for o in hbt.orders(ASSET).values() {
            let Some(side) = side_of(o.side) else {
                continue;
            };
            if status_of(o.status).is_terminal() {
                continue;
            }
            self.open.push(OpenOrder {
                order_id: o.order_id,
                side,
                price_tick: o.price_tick,
                qty: o.qty,
                leaves_qty: o.leaves_qty,
                status: status_of(o.status),
                request_pending: o.req != Status::None,
            });
        }
        self.open.sort_by_key(|o| o.order_id);

        self.trades.clear();
        for ev in hbt.last_trades(ASSET) {
            self.trades.push(TradePrint {
                local_ts: self.tape.rel(ev.local_ts),
                price_tick: self.inst.tick(ev.px),
                qty: ev.qty,
                aggressor: if ev.ev & BUY_EVENT != 0 {
                    Some(Side::Buy)
                } else if ev.ev & SELL_EVENT != 0 {
                    Some(Side::Sell)
                } else {
                    None
                },
            });
        }
        hbt.clear_last_trades(Some(ASSET));

        let known_position = self.tape.known().position;

        // Run-level drawdown rule. It is a decision of the strategy host, so it may only use what
        // that host knows: fills whose notification has arrived, marked at the local mid. Using
        // the exchange-side ledger would react to fills up to one response latency early.
        let known = self.tape.known();
        let (equity, _, _) = self.mark_book(&market, known);
        self.peak_equity = self.peak_equity.max(equity);
        if !self.halted
            && let Some(limit) = self.cfg.risk.max_drawdown
            && self.peak_equity - equity >= limit
        {
            self.halted = true;
            self.tape.run_event(
                now,
                FlightBody::RiskHalt {
                    rule: RiskRule::MaxDrawdown,
                    detail: format!(
                        "equity {equity:.6} is {:.6} below its peak {:.6}; limit {limit:.6}",
                        self.peak_equity - equity,
                        self.peak_equity
                    ),
                },
            );
        }

        self.intents.clear();
        if self.halted {
            // Pull every working order; place nothing.
            for o in &self.open {
                if o.cancellable() {
                    self.intents.push(Intent::Cancel {
                        order_id: o.order_id,
                        reason: "risk halt: cancelling all working orders".into(),
                    });
                }
            }
        } else {
            let view = LocalView {
                now,
                tick_no,
                tick_size: self.inst.tick_size,
                lot_size: self.inst.lot_size,
                market,
                position: known_position,
                open_orders: &self.open,
                recent_trades: &self.trades,
            };
            strategy.on_tick(&view, &mut self.intents);
        }

        // Worst-case exposure bookkeeping for the position rule: working quantity per side,
        // updated as this tick's orders are accepted.
        let mut working_buy: f64 = 0.0;
        let mut working_sell: f64 = 0.0;
        for o in &self.open {
            match o.side {
                Side::Buy => working_buy += o.leaves_qty,
                Side::Sell => working_sell += o.leaves_qty,
            }
        }
        let mut open_count = self.open.len() as u32;
        let mut slots_used = [false; 256];

        let intents = std::mem::take(&mut self.intents);
        for intent in &intents {
            match intent {
                Intent::Place {
                    slot,
                    side,
                    price_tick,
                    qty,
                    tif,
                    ord_type,
                    reason,
                } => {
                    let id = order_id(tick_no, *slot);
                    self.tape.strategy_event(
                        now,
                        id,
                        FlightBody::OrderIntent {
                            tick_no,
                            slot: *slot,
                            side: *side,
                            price_tick: *price_tick,
                            qty: *qty,
                            tif: *tif,
                            ord_type: *ord_type,
                            reason: reason.clone(),
                            market,
                            known_position,
                        },
                    );
                    let working_same_side = match side {
                        Side::Buy => working_buy,
                        Side::Sell => working_sell,
                    };
                    let ruling = self.pre_trade_check(
                        *side,
                        *price_tick,
                        *qty,
                        *ord_type,
                        known_position,
                        working_same_side,
                        open_count,
                        slots_used[*slot as usize],
                    );
                    // A slot is consumed by its first intent whether or not that intent passes,
                    // so an order id can never refer to two different decisions.
                    slots_used[*slot as usize] = true;
                    if let Some((rule, detail)) = ruling {
                        self.tape.strategy_event(
                            now,
                            id,
                            FlightBody::RiskBlocked {
                                tick_no,
                                rule,
                                detail,
                            },
                        );
                        continue;
                    }
                    let price = self.inst.price(*price_tick);
                    let h_tif = match tif {
                        Tif::Gtc => TimeInForce::GTC,
                        Tif::Gtx => TimeInForce::GTX,
                        Tif::Ioc => TimeInForce::IOC,
                        Tif::Fok => TimeInForce::FOK,
                    };
                    let h_type = match ord_type {
                        OrdType::Limit => HOrdType::Limit,
                        OrdType::Market => HOrdType::Market,
                    };
                    // `submit_order(OrderRequest)` is deliberately not used: in 0.9.4 it ignores
                    // the requested side.
                    let sent = match side {
                        Side::Buy => {
                            hbt.submit_buy_order(ASSET, id, price, *qty, h_tif, h_type, false)
                        }
                        Side::Sell => {
                            hbt.submit_sell_order(ASSET, id, price, *qty, h_tif, h_type, false)
                        }
                    };
                    match sent {
                        Ok(_) => {
                            open_count += 1;
                            match side {
                                Side::Buy => working_buy += qty,
                                Side::Sell => working_sell += qty,
                            }
                        }
                        Err(e) => {
                            self.tape
                                .request_refused(now_abs, id, ReqKind::New, e.to_string())
                        }
                    }
                }
                Intent::Cancel {
                    order_id: id,
                    reason,
                } => {
                    self.tape.strategy_event(
                        now,
                        *id,
                        FlightBody::CancelIntent {
                            tick_no,
                            reason: reason.clone(),
                            market,
                            known_position,
                        },
                    );
                    if let Err(e) = hbt.cancel(ASSET, *id, false) {
                        self.tape
                            .request_refused(now_abs, *id, ReqKind::Cancel, e.to_string());
                    }
                }
            }
        }
        self.intents = intents;
        Ok(())
    }

    /// Returns the rule an order breaks, if any. Checks run in a fixed order so the reported
    /// rule is reproducible.
    #[allow(clippy::too_many_arguments)]
    fn pre_trade_check(
        &self,
        side: Side,
        price_tick: i64,
        qty: f64,
        ord_type: OrdType,
        known_position: f64,
        working_same_side: f64,
        open_count: u32,
        slot_taken: bool,
    ) -> Option<(RiskRule, String)> {
        let risk = &self.cfg.risk;
        let lot = self.inst.lot_size;
        if slot_taken {
            return Some((
                RiskRule::InvalidOrder,
                "a second order was requested for the same slot in one tick".into(),
            ));
        }
        let lots = qty / lot;
        if !qty.is_finite() || lots.round() < 1.0 || (lots - lots.round()).abs() > 1e-6 {
            return Some((
                RiskRule::InvalidOrder,
                format!("quantity {qty} is not a positive multiple of the lot size {lot}"),
            ));
        }
        if ord_type == OrdType::Market {
            // hftbacktest 0.9.4 mishandles market orders when the opposite side is empty (a fill
            // at tick i64::MAX, or integer overflow) and the partial-fill model scans only 100
            // ticks. See docs/EXECUTION_MODEL.md, defect 5. A marketable IOC limit order is the
            // supported equivalent.
            return Some((
                RiskRule::InvalidOrder,
                "market orders are not supported by BlackBox on hftbacktest 0.9.4 (engine defect: \
                 wrong price or overflow when the opposite side is empty, 100-tick sweep limit); \
                 use an immediate-or-cancel limit order at the price you are willing to pay"
                    .into(),
            ));
        }
        if ord_type == OrdType::Limit && price_tick <= 0 {
            return Some((
                RiskRule::InvalidOrder,
                format!("limit price tick {price_tick} is not positive"),
            ));
        }
        // Limits are hard: only floating-point noise is tolerated, never extra lots.
        let tol = lot * 1e-6;
        if qty > risk.max_order_qty + tol {
            return Some((
                RiskRule::MaxOrderQty,
                format!(
                    "quantity {qty} exceeds max_order_qty {}",
                    risk.max_order_qty
                ),
            ));
        }
        if open_count >= risk.max_open_orders {
            return Some((
                RiskRule::MaxOpenOrders,
                format!(
                    "{open_count} orders already working; max_open_orders is {}",
                    risk.max_open_orders
                ),
            ));
        }
        // Worst case: every working order on this side fills, then this one does.
        let worst = known_position + side.sign() * (working_same_side + qty);
        if worst.abs() > risk.max_position + tol && worst.abs() > known_position.abs() {
            return Some((
                RiskRule::MaxPosition,
                format!(
                    "known position {known_position} with {working_same_side} already working on \
                     this side plus {qty} could reach {worst}; max_position is {}",
                    risk.max_position
                ),
            ));
        }
        None
    }

    fn sample(&mut self, hbt: &Backtest<Depth>, tick_no: u64, now_abs: i64) {
        let market = self.market(hbt);
        let (equity, realized_pnl, unrealized_pnl) = self.mark(&market);
        let truth = self.tape.truth();
        let ts: RelNs = self.tape.rel(now_abs);
        let open_orders = hbt
            .orders(ASSET)
            .values()
            .filter(|o| {
                !matches!(
                    o.status,
                    Status::Filled | Status::Canceled | Status::Expired
                )
            })
            .count() as u32;
        self.equity.push(EquitySample {
            ts,
            tick_no,
            mid_tick: market.mid_tick(),
            position: truth.position,
            known_position: self.tape.known().position,
            cash: truth.cash,
            fees: truth.fees,
            realized_pnl,
            unrealized_pnl,
            equity,
            open_orders,
        });
    }

    /// Compares the ledger of locally known fills with the engine's own local account.
    fn engine_check(&self, hbt: &Backtest<Depth>) -> EngineStateCheck {
        let known = self.tape.known();
        let sv = hbt.state_values(ASSET);
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs()));
        let agrees = close(known.position, sv.position)
            && close(known.cash, sv.balance)
            && close(known.fees, sv.fee)
            && known.fills as i64 == sv.num_trades;
        let note = match (agrees, self.cfg.execution.exchange_model) {
            (true, _) => String::new(),
            (false, ExchangeModel::PartialFill) => {
                "Expected with the partial-fill exchange model: the engine's local account in \
                 hftbacktest 0.9.4 applies only responses whose status is Filled, using the last \
                 execution quantity, so partial fills and multi-level taker executions are \
                 under-counted there. BlackBox's ledger records every execution from the exchange \
                 side and is the figure reported everywhere."
                    .into()
            }
            (false, ExchangeModel::NoPartialFill) => {
                "Unexpected: with the no-partial-fill exchange model every execution is one \
                 Filled response, so the engine's local account and BlackBox's ledger of locally \
                 known fills should match. Treat this run's accounting as suspect."
                    .into()
            }
        };
        EngineStateCheck {
            agrees,
            ledger_position: known.position,
            engine_position: sv.position,
            ledger_cash: known.cash,
            engine_balance: sv.balance,
            ledger_fees: known.fees,
            engine_fee: sv.fee,
            ledger_fills: known.fills,
            engine_num_trades: sv.num_trades,
            note,
        }
    }
}
