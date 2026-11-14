//! The stock hftbacktest engine with no BlackBox wrapper, and an open-loop driver that replays a
//! BlackBox run's request stream through it.
//!
//! Shared by `examples/bench.rs` (cost of the same order flow) and `tests/raw_engine_parity.rs`
//! (the same order flow must give the same trade count).
#![allow(dead_code)]

use bb_core::{dataset::InstrumentMeta, flight::FlightBody};
use bb_sim::{Event, SimOutput};
use hftbacktest::{
    backtest::{
        Backtest, ExchangeKind, L2AssetBuilder,
        assettype::LinearAsset,
        data::{Data, DataSource},
        models::{
            CommonFees, ConstantLatency, PowerProbQueueFunc3, ProbQueueModel, QueueModel,
            TradingValueFeeModel,
        },
    },
    depth::HashMapMarketDepth,
    types::{Bot, ElapseResult, LOCAL_EVENT, OrdType, TimeInForce},
};

pub type Depth = HashMapMarketDepth;

/// What the stock engine is built from: the data, and the execution assumptions that must match
/// the BlackBox configuration it is compared with.
pub struct RawSetup<'a> {
    pub events: &'a [Event],
    pub instrument: InstrumentMeta,
    /// `ConstantLatency` entry and response latency.
    pub latency_ns: i64,
    pub maker_fee: f64,
    pub taker_fee: f64,
}

/// The stock engine, assembled exactly as in `tests/engine_defects.rs`: `L2AssetBuilder`,
/// `NoPartialFillExchange`, `ConstantLatency`, the given queue model.
pub fn raw_engine_with<QM>(s: &RawSetup<'_>, queue_model: QM) -> Backtest<Depth>
where
    QM: QueueModel<Depth> + 'static,
{
    let inst = s.instrument;
    let asset = L2AssetBuilder::new()
        .data(vec![DataSource::Data(Data::from_data(s.events))])
        .latency_model(ConstantLatency::new(s.latency_ns, s.latency_ns))
        .asset_type(LinearAsset::new(inst.contract_size))
        .fee_model(TradingValueFeeModel::new(CommonFees::new(
            s.maker_fee,
            s.taker_fee,
        )))
        .exchange(ExchangeKind::NoPartialFillExchange)
        .queue_model(queue_model)
        .depth(move || Depth::new(inst.tick_size, inst.lot_size))
        .build()
        .expect("asset builds");
    Backtest::builder()
        .add_asset(asset)
        .build()
        .expect("backtest builds")
}

/// One request the adapter actually issued, keyed by the strategy tick it was issued at.
#[derive(Clone, Copy)]
pub enum Action {
    Place {
        tick: u64,
        id: u64,
        side: bb_core::Side,
        price: f64,
        qty: f64,
        tif: TimeInForce,
        ord_type: OrdType,
    },
    Cancel {
        tick: u64,
        id: u64,
    },
}

impl Action {
    pub fn tick(&self) -> u64 {
        match self {
            Action::Place { tick, .. } | Action::Cancel { tick, .. } => *tick,
        }
    }
}

/// The adapter's request stream, read back from its tape: every order intent that passed the
/// risk checks and every cancel intent, in tape (= issue) order.
pub fn actions_of(out: &SimOutput, inst: InstrumentMeta) -> Vec<Action> {
    let mut actions = Vec::new();
    for (i, e) in out.tape.iter().enumerate() {
        let id = e.order_id.unwrap_or(0);
        match &e.body {
            FlightBody::OrderIntent {
                tick_no,
                side,
                price_tick,
                qty,
                tif,
                ord_type,
                ..
            } => {
                let blocked = matches!(
                    out.tape.get(i + 1).map(|n| &n.body),
                    Some(FlightBody::RiskBlocked { .. })
                );
                if blocked {
                    continue;
                }
                actions.push(Action::Place {
                    tick: *tick_no,
                    id,
                    side: *side,
                    price: inst.price(*price_tick),
                    qty: *qty,
                    tif: match tif {
                        bb_core::Tif::Gtc => TimeInForce::GTC,
                        bb_core::Tif::Gtx => TimeInForce::GTX,
                        bb_core::Tif::Ioc => TimeInForce::IOC,
                        bb_core::Tif::Fok => TimeInForce::FOK,
                    },
                    ord_type: match ord_type {
                        bb_core::OrdType::Limit => OrdType::Limit,
                        bb_core::OrdType::Market => OrdType::Market,
                    },
                });
            }
            FlightBody::CancelIntent { tick_no, .. } => {
                actions.push(Action::Cancel { tick: *tick_no, id })
            }
            _ => {}
        }
    }
    // Tape order is already issue order; a stable sort by tick only guards that assumption.
    actions.sort_by_key(Action::tick);
    actions
}

/// Absolute local time after which the adapter stops evaluating ticks (its `data_end` with no
/// feed offset).
pub fn data_end(events: &[Event]) -> i64 {
    events
        .iter()
        .filter(|e| e.ev & LOCAL_EVENT != 0)
        .map(|e| e.local_ts)
        .max()
        .expect("dataset has local events")
}

/// The stock engine (`NoPartialFillExchange`, `ProbQueueModel<PowerProbQueueFunc3(n)>`) driven
/// open-loop by a minimal loop that issues exactly the adapter's recorded request stream at the
/// same ticks, calling `clear_inactive_orders` each tick as any hftbacktest strategy loop does.
/// No views, recording or risk checks. Returns (ticks, engine-reported trade count).
pub fn raw_order_replay(
    s: &RawSetup<'_>,
    power_n: f64,
    interval_ns: i64,
    end: i64,
    actions: &[Action],
) -> (u64, i64) {
    let mut hbt = raw_engine_with(
        s,
        ProbQueueModel::<_, Depth>::new(PowerProbQueueFunc3::new(power_n)),
    );
    let mut tick = 0u64;
    let mut next = 0usize;
    loop {
        if hbt.elapse(interval_ns).expect("raw elapse") == ElapseResult::EndOfData {
            break;
        }
        if hbt.current_timestamp() > end {
            break;
        }
        hbt.clear_inactive_orders(Some(0));
        while let Some(a) = actions.get(next).filter(|a| a.tick() == tick) {
            // Errors (e.g. cancelling an order that just filled) are what the adapter records as
            // refusals; the request stream is replayed regardless.
            let _ = match *a {
                Action::Place {
                    id,
                    side,
                    price,
                    qty,
                    tif,
                    ord_type,
                    ..
                } => match side {
                    bb_core::Side::Buy => {
                        hbt.submit_buy_order(0, id, price, qty, tif, ord_type, false)
                    }
                    bb_core::Side::Sell => {
                        hbt.submit_sell_order(0, id, price, qty, tif, ord_type, false)
                    }
                },
                Action::Cancel { id, .. } => hbt.cancel(0, id, false),
            };
            next += 1;
        }
        tick += 1;
    }
    (tick, hbt.state_values(0).num_trades)
}
