//! Hand-built micro-markets and scripted strategies for adapter tests.
#![allow(dead_code)]

use bb_core::{
    OrdType, Side, Tif,
    config::{
        ExchangeModel, ExecutionConfig, FeeConfig, OrderLatencyConfig, QueueModelConfig,
        RecordConfig, RiskConfig, RunConfig, ScriptedParams, StrategyConfig, StrategyKind,
    },
    dataset::InstrumentMeta,
    flight::{FlightBody, FlightEvent},
    strategy::{Intent, LocalView, MarketView, Strategy},
};
use bb_sim::{Event, SimInput, SimOutput, simulate};
use hftbacktest::types::{
    BUY_EVENT, DEPTH_EVENT, EXCH_EVENT, LOCAL_EVENT, SELL_EVENT, TRADE_EVENT,
};

pub const ORIGIN: i64 = 1_700_000_000_000_000_000;
pub const MS: i64 = 1_000_000;
pub const US: i64 = 1_000;
/// Feed latency of every hand-built event.
pub const FEED: i64 = 100 * US;
pub const INTERVAL: i64 = 10 * MS;
pub const ENTRY: i64 = MS;
pub const RESPONSE: i64 = MS;
pub const MAKER_FEE: f64 = 0.0002;
pub const TAKER_FEE: f64 = 0.0005;

pub const INST: InstrumentMeta = InstrumentMeta {
    tick_size: 0.5,
    lot_size: 0.1,
    contract_size: 1.0,
};

/// Price in ticks for the test instrument.
pub fn tk(price: f64) -> i64 {
    INST.tick(price)
}

/// A market built row by row. Every row is visible to both the exchange and the local side, with
/// a constant feed latency, so the two clocks agree on ordering.
#[derive(Default)]
pub struct Mkt {
    pub rows: Vec<Event>,
}

impl Mkt {
    fn row(&mut self, kind: u64, side: u64, at: i64, px: f64, qty: f64) -> &mut Self {
        if let Some(last) = self.rows.last() {
            assert!(
                ORIGIN + at >= last.exch_ts,
                "rows must be added in time order"
            );
        }
        self.rows.push(Event {
            ev: kind | side | EXCH_EVENT | LOCAL_EVENT,
            exch_ts: ORIGIN + at,
            local_ts: ORIGIN + at + FEED,
            px,
            qty,
            order_id: 0,
            ival: 0,
            fval: 0.0,
        });
        self
    }

    pub fn bid(&mut self, at: i64, px: f64, qty: f64) -> &mut Self {
        self.row(DEPTH_EVENT, BUY_EVENT, at, px, qty)
    }

    pub fn ask(&mut self, at: i64, px: f64, qty: f64) -> &mut Self {
        self.row(DEPTH_EVENT, SELL_EVENT, at, px, qty)
    }

    /// A market trade with the given aggressor side.
    pub fn trade(&mut self, at: i64, aggressor: Side, px: f64, qty: f64) -> &mut Self {
        let side = match aggressor {
            Side::Buy => BUY_EVENT,
            Side::Sell => SELL_EVENT,
        };
        self.row(TRADE_EVENT, side, at, px, qty)
    }

    /// Bids 100.0 x5 / 99.5 x5 / 99.0 x5 and asks 101.0 x5 / 101.5 x5 / 102.0 x5 at time 0.
    pub fn standard() -> Self {
        let mut m = Mkt::default();
        m.bid(0, 100.0, 5.0)
            .bid(0, 99.5, 5.0)
            .bid(0, 99.0, 5.0)
            .ask(0, 101.0, 5.0)
            .ask(0, 101.5, 5.0)
            .ask(0, 102.0, 5.0);
        m
    }

    /// A far-from-touch depth change whose only purpose is to keep the dataset going until `at`.
    pub fn keepalive(&mut self, at: i64) -> &mut Self {
        self.ask(at, 150.0, 1.0)
    }
}

pub fn config() -> RunConfig {
    RunConfig {
        dataset_id: "test".into(),
        strategy: StrategyConfig {
            interval_ns: INTERVAL,
            // The tests pass their own strategy object; this only has to validate.
            kind: StrategyKind::Scripted(ScriptedParams { orders: vec![] }),
        },
        execution: ExecutionConfig {
            exchange_model: ExchangeModel::NoPartialFill,
            queue_model: QueueModelConfig::RiskAdverse,
            order_latency: OrderLatencyConfig::Constant {
                entry_ns: ENTRY,
                response_ns: RESPONSE,
            },
            feed_latency_offset_ns: 0,
            fees: FeeConfig {
                maker: MAKER_FEE,
                taker: TAKER_FEE,
            },
        },
        risk: RiskConfig {
            max_position: 1_000.0,
            max_order_qty: 1_000.0,
            max_open_orders: 64,
            max_drawdown: None,
        },
        record: RecordConfig::default(),
        seed: 1,
    }
}

/// One scripted action.
pub enum Act {
    Place {
        slot: u8,
        side: Side,
        price: f64,
        qty: f64,
        tif: Tif,
    },
    Market {
        slot: u8,
        side: Side,
        qty: f64,
    },
    Cancel {
        order_id: u64,
    },
}

/// What the strategy was shown on each tick: (tick number, local time, market, known position).
pub type SeenViews = std::rc::Rc<std::cell::RefCell<Vec<(u64, i64, MarketView, f64)>>>;

/// Emits fixed intents at fixed ticks and remembers the views it was shown.
#[derive(Default)]
pub struct Script {
    pub acts: Vec<(u64, Act)>,
    pub seen: SeenViews,
}

impl Script {
    pub fn new(acts: Vec<(u64, Act)>) -> Self {
        Script {
            acts,
            seen: Default::default(),
        }
    }
}

impl Strategy for Script {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        self.seen
            .borrow_mut()
            .push((view.tick_no, view.now, view.market, view.position));
        for (tick, act) in &self.acts {
            if *tick != view.tick_no {
                continue;
            }
            out.push(match act {
                Act::Place {
                    slot,
                    side,
                    price,
                    qty,
                    tif,
                } => Intent::Place {
                    slot: *slot,
                    side: *side,
                    price_tick: tk(*price),
                    qty: *qty,
                    tif: *tif,
                    ord_type: OrdType::Limit,
                    reason: "scripted".into(),
                },
                Act::Market { slot, side, qty } => Intent::Place {
                    slot: *slot,
                    side: *side,
                    price_tick: 0,
                    qty: *qty,
                    tif: Tif::Gtc,
                    ord_type: OrdType::Market,
                    reason: "scripted market".into(),
                },
                Act::Cancel { order_id } => Intent::Cancel {
                    order_id: *order_id,
                    reason: "scripted cancel".into(),
                },
            });
        }
    }
}

pub fn run(mkt: &Mkt, cfg: &RunConfig, acts: Vec<(u64, Act)>) -> SimOutput {
    run_with(mkt, cfg, Script::new(acts))
}

pub fn run_with(mkt: &Mkt, cfg: &RunConfig, script: Script) -> SimOutput {
    simulate(
        &SimInput {
            events: &mkt.rows,
            instrument: INST,
            origin_ns: ORIGIN,
            config: cfg,
        },
        Box::new(script),
    )
    .expect("simulation succeeds")
}

/// The tape events of one order, in tape order.
pub fn of(out: &SimOutput, order_id: u64) -> Vec<&FlightEvent> {
    out.tape
        .iter()
        .filter(|e| e.order_id == Some(order_id))
        .collect()
}

pub fn types(events: &[&FlightEvent]) -> Vec<&'static str> {
    events.iter().map(|e| e.body.type_name()).collect()
}

/// Local-clock time of strategy tick `n`.
pub fn tick_ts(n: u64) -> i64 {
    (n as i64 + 1) * INTERVAL
}

pub fn buy(slot: u8, price: f64, qty: f64, tif: Tif) -> Act {
    Act::Place {
        slot,
        side: Side::Buy,
        price,
        qty,
        tif,
    }
}

pub fn sell(slot: u8, price: f64, qty: f64, tif: Tif) -> Act {
    Act::Place {
        slot,
        side: Side::Sell,
        price,
        qty,
        tif,
    }
}

pub fn find<'a>(events: &[&'a FlightEvent], type_name: &str) -> Vec<&'a FlightEvent> {
    events
        .iter()
        .copied()
        .filter(|e| e.body.type_name() == type_name)
        .collect()
}

pub fn is_trade_update(e: &FlightEvent) -> bool {
    matches!(
        e.body,
        FlightBody::QueueUpdate {
            cause: bb_core::flight::QueueCause::Trade { .. },
            ..
        }
    )
}

pub fn try_run(
    mkt: &Mkt,
    cfg: &RunConfig,
    acts: Vec<(u64, Act)>,
) -> Result<SimOutput, bb_sim::SimError> {
    simulate(
        &SimInput {
            events: &mkt.rows,
            instrument: INST,
            origin_ns: ORIGIN,
            config: cfg,
        },
        Box::new(Script::new(acts)),
    )
}
