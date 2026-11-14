//! Hand-built tapes and results for unit tests. Compiled only for tests.

use bb_core::{
    OrdType, OrderStatus, RelNs, ReqKind, Side, Tif,
    config::{
        ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig, QueueModelConfig,
        RecordConfig, RiskConfig, RunConfig, ScriptedParams, StrategyConfig, StrategyKind,
    },
    dataset::InstrumentMeta,
    flight::{FlightBody, FlightEvent, QueueCause},
    result::{
        DatasetRef, EngineInfo, EngineStateCheck, EquitySample, FillRecord, RunManifest, RunResult,
    },
    strategy::{MarketView, split_order_id},
};

pub(crate) const MS: i64 = 1_000_000;

/// Tick 0.5, lot 0.001: price tick 20 001 prints as `10000.5`, quantities with three decimals.
pub(crate) const INSTRUMENT: InstrumentMeta = InstrumentMeta {
    tick_size: 0.5,
    lot_size: 0.001,
    contract_size: 1.0,
};

/// Best bid/ask every test decision is made against.
pub(crate) const VIEW_BID: i64 = 20_000;
pub(crate) const VIEW_ASK: i64 = 20_002;

pub(crate) fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// The local market view at a decision made at `ts`: last feed event received 1 ms earlier,
/// stamped by the exchange 2 ms before that.
fn view(ts: RelNs) -> MarketView {
    MarketView {
        best_bid_tick: Some(VIEW_BID),
        best_ask_tick: Some(VIEW_ASK),
        best_bid_qty: 5.0,
        best_ask_qty: 4.0,
        feed_exch_ts: Some(ts - 3 * MS),
        feed_local_ts: Some(ts - MS),
    }
}

/// A tape under construction. `seq` is assigned in push order, as the recorder does.
pub(crate) struct Tape(pub(crate) Vec<FlightEvent>);

impl Tape {
    pub(crate) fn new() -> Self {
        Tape(Vec::new())
    }

    /// Appends an event and returns its `seq`.
    pub(crate) fn push(&mut self, ts: RelNs, order_id: Option<u64>, body: FlightBody) -> u64 {
        let seq = self.0.len() as u64;
        self.0.push(FlightEvent::new(seq, ts, order_id, body));
        seq
    }

    pub(crate) fn intent(
        &mut self,
        ts: RelNs,
        id: u64,
        side: Side,
        price_tick: i64,
        qty: f64,
        tif: Tif,
    ) -> u64 {
        let (tick_no, slot) = split_order_id(id);
        self.push(
            ts,
            Some(id),
            FlightBody::OrderIntent {
                tick_no,
                slot,
                side,
                price_tick,
                qty,
                tif,
                ord_type: OrdType::Limit,
                reason: "join the touch".into(),
                market: view(ts),
                known_position: 0.0,
            },
        )
    }

    /// A request sent at `ts` with a modeled entry latency of `latency_ns`.
    pub(crate) fn sent(&mut self, ts: RelNs, id: u64, req: ReqKind, latency_ns: i64) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::RequestSent {
                req,
                entry_latency_ns: latency_ns,
                exch_arrival_ts: Some(ts + latency_ns),
            },
        )
    }

    pub(crate) fn queued(&mut self, ts: RelNs, id: u64, ahead: Option<f64>, level_qty: f64) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::ExchQueued {
                queue_ahead: ahead,
                level_qty,
                best_bid_tick: Some(VIEW_BID),
                best_ask_tick: Some(VIEW_ASK),
            },
        )
    }

    /// A trade of `qty` at the order's price moving the estimate from `before` to `after`.
    pub(crate) fn trade(&mut self, ts: RelNs, id: u64, qty: f64, before: f64, after: f64) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::QueueUpdate {
                cause: QueueCause::Trade { qty },
                ahead_before: Some(before),
                ahead_after: Some(after),
            },
        )
    }

    /// A displayed-quantity change at the order's price moving the estimate.
    pub(crate) fn depth(
        &mut self,
        ts: RelNs,
        id: u64,
        prev_qty: f64,
        new_qty: f64,
        before: f64,
        after: f64,
    ) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::QueueUpdate {
                cause: QueueCause::Depth { prev_qty, new_qty },
                ahead_before: Some(before),
                ahead_after: Some(after),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn fill(
        &mut self,
        ts: RelNs,
        id: u64,
        price_tick: i64,
        qty: f64,
        maker: bool,
        fee: f64,
        leaves_qty: f64,
    ) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::ExchFill {
                price_tick,
                qty,
                maker,
                fee,
                value: INSTRUMENT.price(price_tick) * qty,
                leaves_qty,
            },
        )
    }

    /// An exchange response with a 1 ms response latency.
    pub(crate) fn resp(&mut self, ts: RelNs, id: u64, status: OrderStatus, leaves_qty: f64) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::ExchResponse {
                status,
                request_rejected: false,
                leaves_qty,
                response_latency_ns: MS,
                local_arrival_ts: ts + MS,
            },
        )
    }

    /// The exchange rejecting a cancel for an order that is no longer there.
    pub(crate) fn rejected_cancel(&mut self, ts: RelNs, id: u64) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::ExchResponse {
                status: OrderStatus::New,
                request_rejected: true,
                leaves_qty: 0.0,
                response_latency_ns: MS,
                local_arrival_ts: ts + MS,
            },
        )
    }

    pub(crate) fn local(
        &mut self,
        ts: RelNs,
        id: u64,
        status: OrderStatus,
        exec_qty: f64,
        leaves_qty: f64,
        exch_ts: RelNs,
    ) -> u64 {
        self.push(
            ts,
            Some(id),
            FlightBody::LocalResponse {
                status,
                request_rejected: false,
                exec_qty,
                exec_price_tick: 0,
                leaves_qty,
                exch_ts,
            },
        )
    }

    pub(crate) fn cancel_intent(&mut self, ts: RelNs, id: u64, reason: &str) -> u64 {
        let (tick_no, _) = split_order_id(id);
        self.push(
            ts,
            Some(id),
            FlightBody::CancelIntent {
                tick_no: tick_no + 1,
                reason: reason.into(),
                market: view(ts),
                known_position: 0.0,
            },
        )
    }

    /// The opening of a resting order decided at `ts`: intent, request (1 ms entry latency),
    /// queue assignment at `ts + 1 ms` with `ahead` displayed and assigned, acknowledgement, and
    /// its receipt at `ts + 2 ms`. Returns the `seq` of the `ExchQueued` event.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rest(
        &mut self,
        ts: RelNs,
        id: u64,
        side: Side,
        price_tick: i64,
        qty: f64,
        tif: Tif,
        ahead: Option<f64>,
    ) -> u64 {
        self.intent(ts, id, side, price_tick, qty, tif);
        self.sent(ts, id, ReqKind::New, MS);
        let queued = self.queued(ts + MS, id, ahead, ahead.unwrap_or(1.0));
        self.resp(ts + MS, id, OrderStatus::New, qty);
        self.local(ts + 2 * MS, id, OrderStatus::New, 0.0, qty, ts + MS);
        queued
    }

    /// The events of one order, in tape order.
    pub(crate) fn of(&self, id: u64) -> Vec<FlightEvent> {
        self.0
            .iter()
            .filter(|e| e.order_id == Some(id))
            .cloned()
            .collect()
    }
}

/// Constant 1 ms latency, risk-adverse queue, no-partial-fill exchange, 1 bp maker / 5 bp taker.
pub(crate) fn config() -> RunConfig {
    RunConfig {
        dataset_id: "unit".into(),
        strategy: StrategyConfig {
            interval_ns: 10 * MS,
            kind: StrategyKind::Scripted(ScriptedParams { orders: Vec::new() }),
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::NoPartialFill,
            queue_model: QueueModelConfig::RiskAdverse,
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: MS,
                response_ns: MS,
            },
            think_time_ns: 0,
            feed_latency_offset_ns: 0,
            fees: FeeConfig {
                maker: 0.0001,
                taker: 0.0005,
            },
        },
        risk: RiskConfig {
            max_position: 10.0,
            max_order_qty: 5.0,
            max_open_orders: 8,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 7,
    }
}

/// A fill record whose `value` follows from [`INSTRUMENT`]; derived fields are left unset.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_record(
    seq: u64,
    order_id: u64,
    ts: RelNs,
    side: Side,
    price_tick: i64,
    qty: f64,
    maker: bool,
    fee: f64,
    position_after: f64,
) -> FillRecord {
    FillRecord {
        seq,
        order_id,
        ts,
        side,
        price_tick,
        qty,
        maker,
        fee,
        value: INSTRUMENT.price(price_tick) * qty,
        position_after,
        realized_pnl: 0.0,
        mid_tick: None,
        markout_ticks: Vec::new(),
    }
}

/// An equity sample with the given equity and position; `fees` is zero.
pub(crate) fn equity_sample(tick_no: u64, equity: f64, position: f64) -> EquitySample {
    EquitySample {
        ts: tick_no as i64 * 10 * MS,
        tick_no,
        mid_tick: Some(20_001.0),
        position,
        known_position: position,
        cash: 0.0,
        fees: 0.0,
        realized_pnl: equity,
        unrealized_pnl: 0.0,
        equity,
        open_orders: 0,
    }
}

/// A complete run over `tape`, with orders and metrics computed by this crate.
pub(crate) fn run_result(
    run_id: &str,
    config: RunConfig,
    tape: Vec<FlightEvent>,
    fills: Vec<FillRecord>,
    equity: Vec<EquitySample>,
) -> RunResult {
    let orders = crate::build_orders(&tape);
    let metrics = crate::compute_metrics(&tape, &orders, &fills, &equity, &INSTRUMENT);
    RunResult {
        manifest: RunManifest {
            contract_version: bb_core::CONTRACT_VERSION,
            run_id: run_id.into(),
            blackbox_version: "0.1.0".into(),
            simulation_revision: bb_core::SIMULATION_REVISION,
            engine: EngineInfo {
                name: bb_core::ENGINE_NAME.into(),
                version: bb_core::ENGINE_VERSION.into(),
                features: vec!["backtest".into()],
            },
            platform: "test-x86_64".into(),
            profile: "debug".into(),
            dataset: DatasetRef {
                id: config.dataset_id.clone(),
                sha256: "d".repeat(64),
                rows: 0,
                origin_ns: "0".into(),
            },
            config,
            config_sha256: format!("{run_id}-config"),
            result_sha256: crate::result_sha256(&tape, &fills, &equity),
            created_unix_ms: 0,
            wall_time_ms: 0,
        },
        metrics,
        engine_check: EngineStateCheck {
            agrees: true,
            ledger_position: 0.0,
            engine_position: 0.0,
            ledger_cash: 0.0,
            engine_balance: 0.0,
            ledger_fees: 0.0,
            engine_fee: 0.0,
            ledger_fills: fills.len() as u64,
            engine_num_trades: fills.len() as i64,
            note: String::new(),
        },
        ticks: 0,
        orders,
        fills,
        equity,
        tape,
    }
}
