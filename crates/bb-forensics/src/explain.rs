//! Deterministic, rule-based explanation of one order's outcome.
//!
//! The tape of one order is parsed once into a [`Life`]; the verdict, the steps, the headline and
//! the latency breakdown are all read off that structure. Every step names its [`Basis`] and the
//! tape events it rests on. Wording rules:
//!
//! * displayed quantities and trade prints are *observed*; queue positions, latencies, fills and
//!   fees are *modeled*; nothing modeled is ever described as having happened at an exchange;
//! * an unknown queue estimate is reported as unknown, never as zero;
//! * when the tape cannot tell two causes apart (a trade through the price versus the opposite
//!   best crossing it), the statement names both and says so.

use bb_core::{
    config::{ExchangeModel, OrderLatencyConfig, QueueModelConfig, RunConfig},
    dataset::{InstrumentMeta, LocalTsProvenance},
    explain::{ExplainStep, Explanation, LatencyBreakdown, QueuePoint, Question, Verdict},
    flight::{FlightBody, FlightEvent, QueueCause, RiskRule},
    result::{OrderOutcome, OrderRecord},
    strategy::MarketView,
    Basis, Clock, OrdType, OrderStatus, RelNs, ReqKind, Side, Tif,
};

use crate::text::{at, count, dur, num, rule_label, side_verb, status_label, tif_label, Fmt};

pub(crate) const CAVEAT_QUEUE: &str = "Queue position is a model estimate: this is \
    Market-By-Price (L2) data, which shows the total quantity at each price and not individual \
    orders, so the order's true place in the queue is not observable.";
pub(crate) const CAVEAT_IMPACT: &str = "Historical replay does not include the market's reaction \
    to this order: prices, depth and trades are exactly as recorded, with no market impact from \
    the simulated order.";
pub(crate) const CAVEAT_SIMULATED: &str = "This is a simulated order, not an exchange execution; \
    any fill shown is hypothetical.";
pub(crate) const CAVEAT_NO_PARTIAL: &str = "The configured exchange model (no partial fills) \
    fills the whole order at once when its fill condition is met, without regard to how much \
    quantity actually traded or was displayed; fill sizes are optimistic.";

// ----- the parsed life of one order -----

/// A strategy decision (to place or to cancel) with what the strategy knew.
struct Decision<'a> {
    seq: u64,
    ts: RelNs,
    reason: &'a str,
    market: &'a MarketView,
    known_position: f64,
}

struct Sent {
    seq: u64,
    ts: RelNs,
    entry_latency_ns: i64,
    exch_arrival_ts: Option<RelNs>,
}

struct Queued {
    seq: u64,
    ts: RelNs,
    queue_ahead: Option<f64>,
    level_qty: f64,
    best_bid_tick: Option<i64>,
    best_ask_tick: Option<i64>,
}

struct Update {
    seq: u64,
    ts: RelNs,
    cause: QueueCause,
    before: Option<f64>,
    after: Option<f64>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Mechanism {
    /// Took liquidity on arrival.
    Taker,
    /// Maker fill immediately preceded by a trade that took the modeled queue ahead below zero.
    QueueExhausted,
    /// Maker fill with no such trade: the market traded through the price or crossed it.
    PriceCrossed,
}

struct Fill {
    seq: u64,
    ts: RelNs,
    price_tick: i64,
    qty: f64,
    fee: f64,
    value: f64,
    leaves_qty: f64,
    mechanism: Mechanism,
    /// Index into `Life::updates` of the queue-exhausting trade.
    trigger: Option<usize>,
    /// The exchange response that reported this fill.
    response_seq: Option<u64>,
}

struct Resp {
    seq: u64,
    ts: RelNs,
    status: OrderStatus,
    rejected: bool,
    latency_ns: i64,
    local_arrival_ts: RelNs,
}

struct Local {
    seq: u64,
    ts: RelNs,
    status: OrderStatus,
    rejected: bool,
    exec_qty: f64,
    exch_ts: RelNs,
}

#[derive(Default)]
struct Cancel<'a> {
    decision: Option<Decision<'a>>,
    sent: Option<Sent>,
    refused: Option<(u64, &'a str)>,
    /// Index into `Life::exch` of the exchange's answer (cancelled, or cancel rejected).
    response: Option<usize>,
}

/// How far the modeled quantity ahead moved, by cause. Values are clamped at zero on both ends of
/// each update, so a trade that overshoots the queue counts only for what was still ahead.
#[derive(Default)]
struct Movement {
    by_trades: f64,
    by_depth: f64,
    depth_changes: u64,
    unreadable: u64,
}

#[derive(Default)]
struct Life<'a> {
    intent: Option<Decision<'a>>,
    risk_block: Option<(u64, RiskRule, &'a str)>,
    new_refused: Option<(u64, &'a str)>,
    new_sent: Option<Sent>,
    queued: Option<Queued>,
    updates: Vec<Update>,
    fills: Vec<Fill>,
    exch: Vec<Resp>,
    local: Vec<Local>,
    cancels: Vec<Cancel<'a>>,
    /// Number of queue updates the recorder dropped, when it truncated.
    truncated: Option<u64>,
    /// `(seq, ts)` of the first exchange-clock event: the order's arrival.
    first_exch: Option<(u64, RelNs)>,
}

/// Tape grammar: a queue-exhausted maker fill is immediately preceded, at the same timestamp, by
/// a trade update of the same order whose `ahead_after` is negative.
fn exhausts_queue(prev: &FlightEvent, fill_ts: RelNs) -> bool {
    prev.ts == fill_ts
        && matches!(
            prev.body,
            FlightBody::QueueUpdate {
                cause: QueueCause::Trade { .. },
                ahead_after: Some(after),
                ..
            } if after < 0.0
        )
}

impl<'a> Life<'a> {
    fn parse(events: &'a [FlightEvent]) -> Self {
        let mut life = Life::default();
        for (i, ev) in events.iter().enumerate() {
            if ev.clock == Clock::Exchange && life.first_exch.is_none() {
                life.first_exch = Some((ev.seq, ev.ts));
            }
            match &ev.body {
                FlightBody::OrderIntent {
                    reason,
                    market,
                    known_position,
                    ..
                } => {
                    if life.intent.is_none() {
                        life.intent = Some(Decision {
                            seq: ev.seq,
                            ts: ev.ts,
                            reason: reason.as_str(),
                            market,
                            known_position: *known_position,
                        });
                    }
                }
                FlightBody::CancelIntent {
                    reason,
                    market,
                    known_position,
                    ..
                } => life.cancels.push(Cancel {
                    decision: Some(Decision {
                        seq: ev.seq,
                        ts: ev.ts,
                        reason: reason.as_str(),
                        market,
                        known_position: *known_position,
                    }),
                    ..Cancel::default()
                }),
                FlightBody::RiskBlocked { rule, detail, .. } => {
                    life.risk_block = Some((ev.seq, *rule, detail.as_str()));
                }
                FlightBody::RiskHalt { .. } => {}
                FlightBody::RequestSent {
                    req,
                    entry_latency_ns,
                    exch_arrival_ts,
                } => {
                    let sent = Sent {
                        seq: ev.seq,
                        ts: ev.ts,
                        entry_latency_ns: *entry_latency_ns,
                        exch_arrival_ts: *exch_arrival_ts,
                    };
                    match req {
                        ReqKind::New => {
                            if life.new_sent.is_none() {
                                life.new_sent = Some(sent);
                            }
                        }
                        ReqKind::Cancel => life.open_cancel().sent = Some(sent),
                        ReqKind::Modify | ReqKind::None => {}
                    }
                }
                FlightBody::RequestRefused { req, error } => match req {
                    ReqKind::New => life.new_refused = Some((ev.seq, error.as_str())),
                    ReqKind::Cancel => {
                        life.open_cancel().refused = Some((ev.seq, error.as_str()));
                    }
                    ReqKind::Modify | ReqKind::None => {}
                },
                FlightBody::ExchQueued {
                    queue_ahead,
                    level_qty,
                    best_bid_tick,
                    best_ask_tick,
                } => {
                    if life.queued.is_none() {
                        life.queued = Some(Queued {
                            seq: ev.seq,
                            ts: ev.ts,
                            queue_ahead: *queue_ahead,
                            level_qty: *level_qty,
                            best_bid_tick: *best_bid_tick,
                            best_ask_tick: *best_ask_tick,
                        });
                    }
                }
                FlightBody::QueueUpdate {
                    cause,
                    ahead_before,
                    ahead_after,
                } => life.updates.push(Update {
                    seq: ev.seq,
                    ts: ev.ts,
                    cause: *cause,
                    before: *ahead_before,
                    after: *ahead_after,
                }),
                FlightBody::ExchFill {
                    price_tick,
                    qty,
                    maker,
                    fee,
                    value,
                    leaves_qty,
                } => {
                    // When the previous event is the exhausting trade it is also the last update
                    // parsed, so its index is `updates.len() - 1`.
                    let trigger = (*maker && i > 0 && exhausts_queue(&events[i - 1], ev.ts))
                        .then(|| life.updates.len() - 1);
                    let mechanism = if !*maker {
                        Mechanism::Taker
                    } else if trigger.is_some() {
                        Mechanism::QueueExhausted
                    } else {
                        Mechanism::PriceCrossed
                    };
                    let response_seq = events[i + 1..]
                        .iter()
                        .find(|e| matches!(e.body, FlightBody::ExchResponse { .. }))
                        .filter(|e| e.ts == ev.ts)
                        .map(|e| e.seq);
                    life.fills.push(Fill {
                        seq: ev.seq,
                        ts: ev.ts,
                        price_tick: *price_tick,
                        qty: *qty,
                        fee: *fee,
                        value: *value,
                        leaves_qty: *leaves_qty,
                        mechanism,
                        trigger,
                        response_seq,
                    });
                }
                FlightBody::ExchResponse {
                    status,
                    request_rejected,
                    leaves_qty: _,
                    response_latency_ns,
                    local_arrival_ts,
                } => {
                    let index = life.exch.len();
                    life.exch.push(Resp {
                        seq: ev.seq,
                        ts: ev.ts,
                        status: *status,
                        rejected: *request_rejected,
                        latency_ns: *response_latency_ns,
                        local_arrival_ts: *local_arrival_ts,
                    });
                    if (*request_rejected || *status == OrderStatus::Canceled)
                        && let Some(cancel) = life
                            .cancels
                            .iter_mut()
                            .rev()
                            .find(|c| c.sent.is_some() && c.response.is_none())
                    {
                        cancel.response = Some(index);
                    }
                }
                FlightBody::LocalResponse {
                    status,
                    request_rejected,
                    exec_qty,
                    exch_ts,
                    ..
                } => life.local.push(Local {
                    seq: ev.seq,
                    ts: ev.ts,
                    status: *status,
                    rejected: *request_rejected,
                    exec_qty: *exec_qty,
                    exch_ts: *exch_ts,
                }),
                FlightBody::QueueTruncated { dropped } => life.truncated = Some(*dropped),
            }
        }
        life
    }

    /// The cancel attempt a request event belongs to: the latest one still waiting for its
    /// request, or a new one when the tape has a cancel request without a recorded intent.
    fn open_cancel(&mut self) -> &mut Cancel<'a> {
        let waiting = self
            .cancels
            .last()
            .is_some_and(|c| c.sent.is_none() && c.refused.is_none());
        if !waiting {
            self.cancels.push(Cancel::default());
        }
        self.cancels
            .last_mut()
            .expect("a cancel attempt exists: one was waiting or was just pushed")
    }

    /// Total quantity and number of trade prints at the order's price while it rested.
    fn traded(&self) -> (f64, u64) {
        self.updates
            .iter()
            .fold((0.0, 0), |(qty, n), u| match u.cause {
                QueueCause::Trade { qty: q } => (qty + q, n + 1),
                QueueCause::Depth { .. } | QueueCause::FillReset => (qty, n),
            })
    }

    fn movement(&self) -> Movement {
        let mut m = Movement::default();
        for u in &self.updates {
            if matches!(u.cause, QueueCause::Depth { .. }) {
                m.depth_changes += 1;
            }
            let (Some(before), Some(after)) = (u.before, u.after) else {
                m.unreadable += 1;
                continue;
            };
            let fall = before.max(0.0) - after.max(0.0);
            match u.cause {
                QueueCause::Trade { .. } => m.by_trades += fall,
                QueueCause::Depth { .. } => m.by_depth += fall,
                // Moves an exhausted (negative) estimate back to zero: no queue was passed.
                QueueCause::FillReset => {}
            }
        }
        m
    }

    /// One point per `ExchQueued`/`QueueUpdate` with a readable estimate, in tape order.
    fn queue_path(&self) -> Vec<QueuePoint> {
        let mut path = Vec::with_capacity(self.updates.len() + 1);
        if let Some(q) = &self.queued
            && let Some(ahead) = q.queue_ahead
        {
            path.push(QueuePoint {
                ts: q.ts,
                seq: q.seq,
                ahead,
            });
        }
        path.extend(self.updates.iter().filter_map(|u| {
            u.after.map(|ahead| QueuePoint {
                ts: u.ts,
                seq: u.seq,
                ahead,
            })
        }));
        path
    }

    /// `(seq, value)` of the latest readable estimate recorded before tape position `seq`.
    fn ahead_before(&self, seq: u64) -> Option<(u64, f64)> {
        self.updates
            .iter()
            .rev()
            .filter(|u| u.seq < seq)
            .find_map(|u| u.after.map(|a| (u.seq, a)))
            .or_else(|| {
                self.queued
                    .as_ref()
                    .filter(|q| q.seq < seq)
                    .and_then(|q| q.queue_ahead.map(|a| (q.seq, a)))
            })
    }

    /// The exchange response that made the order terminal.
    fn terminal(&self) -> Option<&Resp> {
        self.exch
            .iter()
            .find(|r| !r.rejected && r.status.is_terminal())
    }
}

// ----- steps -----

struct Steps(Vec<ExplainStep>);

impl Steps {
    fn push(&mut self, basis: Basis, statement: String, mut event_seqs: Vec<u64>) {
        event_seqs.sort_unstable();
        event_seqs.dedup();
        self.0.push(ExplainStep {
            basis,
            statement,
            event_seqs,
        });
    }
}

struct Ctx<'a> {
    order: &'a OrderRecord,
    life: Life<'a>,
    fmt: Fmt,
    config: &'a RunConfig,
    local_ts: LocalTsProvenance,
}

fn latency_model_clause(model: &OrderLatencyConfig) -> String {
    match model {
        OrderLatencyConfig::Constant {
            entry_ns,
            response_ns,
        } => format!(
            "constant: {} entry, {} response",
            dur(*entry_ns),
            dur(*response_ns)
        ),
        OrderLatencyConfig::Jitter {
            entry_ns,
            response_ns,
            jitter_ns,
        } => format!(
            "{} entry and {} response, each plus a seeded uniform draw of up to {}",
            dur(*entry_ns),
            dur(*response_ns),
            dur(*jitter_ns)
        ),
    }
}

/// One clause on how the configured queue model treats a decrease in displayed quantity.
fn queue_model_clause(model: &QueueModelConfig) -> String {
    let split = |p_behind: String| {
        format!(
            "it assumes each decrease came from behind the order with probability {p_behind}, \
             where front and back are the modeled quantities ahead of and behind the order, and \
             takes the remaining share off the quantity ahead"
        )
    };
    match model {
        QueueModelConfig::RiskAdverse => "it assumes every decrease in displayed quantity came \
            from behind the order, so the quantity ahead falls only through trades, except that \
            it is capped at the quantity still displayed"
            .to_string(),
        QueueModelConfig::LogProb => {
            split("f(back) / (f(back) + f(front)) with f(x) = ln(1 + x)".into())
        }
        QueueModelConfig::LogProb2 => split("f(back) / f(back + front) with f(x) = ln(1 + x)".into()),
        QueueModelConfig::PowerProb { n } => {
            split(format!("f(back) / (f(back) + f(front)) with f(x) = x^{n}"))
        }
        QueueModelConfig::PowerProb2 { n } => {
            split(format!("f(back) / f(back + front) with f(x) = x^{n}"))
        }
        QueueModelConfig::PowerProb3 { n } => {
            split(format!("1 - (front / (front + back))^{n}"))
        }
    }
}

impl Ctx<'_> {
    fn price(&self) -> String {
        self.fmt.price(self.order.price_tick)
    }

    /// A modeled quantity ahead for prose: exhausted estimates read as zero, unknown as unknown.
    fn ahead_txt(&self, ahead: Option<f64>) -> String {
        match ahead {
            Some(a) => self.fmt.est(a.max(0.0)),
            None => "an unknown quantity".to_string(),
        }
    }

    /// The order's unfilled quantity.
    fn remaining(&self) -> f64 {
        (self.order.qty - self.order.filled_qty).max(0.0)
    }

    /// What kind of non-resting order this is, for prose.
    fn kind(&self) -> &'static str {
        match (self.order.ord_type, self.order.tif) {
            (OrdType::Market, _) => "market",
            (OrdType::Limit, Tif::Ioc) => "IOC",
            (OrdType::Limit, Tif::Fok) => "FOK",
            (OrdType::Limit, Tif::Gtx) => "post-only",
            (OrdType::Limit, Tif::Gtc) => "GTC",
        }
    }

    fn view(&self, market: &MarketView, known_position: f64) -> String {
        let side = |tick: Option<i64>, qty: f64, name: &str| match tick {
            Some(t) => format!(
                "best {name} {} ({} displayed)",
                self.fmt.price(t),
                self.fmt.qty(qty)
            ),
            None => format!("no {name}"),
        };
        format!(
            "Its local view of the market was {} and {}; its locally known position was {}.",
            side(market.best_bid_tick, market.best_bid_qty, "bid"),
            side(market.best_ask_tick, market.best_ask_qty, "ask"),
            self.fmt.qty(known_position)
        )
    }

    // --- verdict ---

    fn fill_verdict(&self) -> Verdict {
        let all = |m: Mechanism| self.life.fills.iter().all(|f| f.mechanism == m);
        if all(Mechanism::Taker) {
            Verdict::FilledTakerOnArrival
        } else if all(Mechanism::QueueExhausted) {
            Verdict::FilledQueueExhausted
        } else if all(Mechanism::PriceCrossed) {
            Verdict::FilledPriceCrossed
        } else {
            Verdict::FilledMixed
        }
    }

    fn expired_verdict(&self) -> Verdict {
        if self.life.first_exch.is_none() {
            // Expired without any exchange-side event: the engine rejected the request before it
            // reached the matching engine. No supported configuration produces this.
            Verdict::NotFilledRefused
        } else if self.order.ord_type == OrdType::Limit && self.order.tif == Tif::Gtx {
            Verdict::NotFilledPostOnlyWouldCross
        } else {
            Verdict::NotFilledNoLiquidityOnArrival
        }
    }

    fn verdict(&self) -> Verdict {
        let o = self.order;
        match o.outcome {
            OrderOutcome::Filled => self.fill_verdict(),
            OrderOutcome::RiskBlocked => Verdict::NotFilledRiskBlocked,
            OrderOutcome::Refused => Verdict::NotFilledRefused,
            OrderOutcome::InFlightAtEnd => Verdict::NotFilledDataEndedInFlight,
            OrderOutcome::Canceled => Verdict::NotFilledCanceled,
            OrderOutcome::Expired => self.expired_verdict(),
            OrderOutcome::WorkingAtEnd => {
                if self.life.queued.is_none() {
                    Verdict::NotFilledWorkingAtEnd
                } else if self.life.traded().0 > 0.0 {
                    Verdict::NotFilledQueueNotReached
                } else {
                    Verdict::NotFilledNoTradeAtPrice
                }
            }
            // The verdict of a partial fill explains why the remainder did not fill.
            OrderOutcome::PartiallyFilled => match o.final_status {
                OrderStatus::Canceled => Verdict::NotFilledCanceled,
                OrderStatus::Expired => self.expired_verdict(),
                _ => Verdict::NotFilledWorkingAtEnd,
            },
        }
    }

    // --- decision ---

    fn decision(&self, out: &mut Steps) {
        let Some(intent) = &self.life.intent else {
            return;
        };
        let o = self.order;
        let what = match o.ord_type {
            OrdType::Market => format!(
                "{} {} with a market order",
                side_verb(o.side),
                self.fmt.qty(o.qty)
            ),
            OrdType::Limit => format!(
                "{} {} at {} with a {} limit order",
                side_verb(o.side),
                self.fmt.qty(o.qty),
                self.price(),
                tif_label(o.tif)
            ),
        };
        let reason = if intent.reason.is_empty() {
            "It recorded no reason.".to_string()
        } else {
            format!("Its stated reason: \"{}\".", intent.reason)
        };
        out.push(
            Basis::Derived,
            format!(
                "At {} the strategy decided to {what} (order {}, strategy tick {}, slot {}). \
                 {reason} {}",
                at(intent.ts),
                o.order_id,
                o.tick_no,
                o.slot,
                self.view(intent.market, intent.known_position)
            ),
            vec![intent.seq],
        );
        self.staleness(intent, out);
    }

    /// How old the strategy's market view was, and whether that age is a measurement.
    fn staleness(&self, intent: &Decision<'_>, out: &mut Steps) {
        let Some(received) = intent.market.feed_local_ts else {
            out.push(
                Basis::Derived,
                "No feed event had reached the strategy host before this decision, so it was \
                 made without market data."
                    .to_string(),
                vec![intent.seq],
            );
            return;
        };
        let mut s = format!(
            "That view was {} old: the last feed event had reached the strategy host at {}.",
            dur(intent.ts - received),
            at(received)
        );
        if let Some(exch) = intent.market.feed_exch_ts {
            s.push_str(&format!(
                " The exchange timestamp of that event is {}, a feed latency of {}.",
                at(exch),
                dur(received - exch)
            ));
        }
        let offset = self.config.execution.feed_latency_offset_ns;
        let plus_offset = if offset != 0 {
            format!(" plus the configured feed latency offset of {}", dur(offset))
        } else {
            String::new()
        };
        let basis = match self.local_ts {
            LocalTsProvenance::Measured if offset == 0 => {
                s.push_str(
                    " Receipt times in this dataset were recorded by the capturing host, so \
                     these are measurements at that host's location, which is generally not \
                     where a strategy would run.",
                );
                Basis::Observed
            }
            LocalTsProvenance::Measured => {
                s.push_str(&format!(
                    " Receipt times were recorded by the capturing host, but the configured feed \
                     latency offset of {} is added to every one of them, so these values are \
                     modeled, not measured.",
                    dur(offset)
                ));
                Basis::Modeled
            }
            LocalTsProvenance::Synthetic => {
                s.push_str(&format!(
                    " Receipt times in this dataset were produced by the synthetic generator \
                     from a configured latency{plus_offset}, so these values are modeled, not \
                     measured."
                ));
                Basis::Modeled
            }
            LocalTsProvenance::Assumed => {
                s.push_str(&format!(
                    " The source data had no receipt times; they were set to the exchange time \
                     plus an assumed latency{plus_offset}, so these values are modeled \
                     assumptions, not measurements."
                ));
                Basis::Modeled
            }
        };
        out.push(basis, s, vec![intent.seq]);
    }

    // --- request ---

    fn request(&self, out: &mut Steps) {
        let life = &self.life;
        if let Some((seq, rule, detail)) = life.risk_block {
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            };
            out.push(
                Basis::Derived,
                format!(
                    "The pre-trade risk rule {} stopped the order before any request was sent \
                     to the engine{detail}. No simulated exchange ever saw it.",
                    rule_label(rule)
                ),
                vec![seq],
            );
            return;
        }
        if let Some((seq, error)) = life.new_refused {
            out.push(
                Basis::Modeled,
                format!(
                    "The engine refused the new-order request synchronously and nothing was \
                     sent ({error})."
                ),
                vec![seq],
            );
            return;
        }
        let Some(sent) = &life.new_sent else { return };
        if sent.entry_latency_ns < 0 {
            out.push(
                Basis::Modeled,
                format!(
                    "The new-order request was sent at {}, but the latency model returned a \
                     negative entry latency ({} ns). That is the engine's convention for a \
                     request rejected before it reaches the matching engine: the exchange model \
                     never processed this order.",
                    at(sent.ts),
                    sent.entry_latency_ns
                ),
                vec![sent.seq],
            );
            return;
        }
        let scheduled = sent
            .exch_arrival_ts
            .unwrap_or(sent.ts + sent.entry_latency_ns);
        let mut s = format!(
            "The new-order request left the strategy host at {}. The latency model ({}) gave it \
             an entry latency of {}, scheduling its arrival at the simulated exchange for {}.",
            at(sent.ts),
            latency_model_clause(&self.config.execution.order_latency),
            dur(sent.entry_latency_ns),
            at(scheduled)
        );
        let mut seqs = vec![sent.seq];
        match life.first_exch {
            Some((seq, ts)) if ts > scheduled => {
                s.push_str(&format!(
                    " The exchange processed it at {}, {} later: the engine's order bus never \
                     lets a request overtake an earlier one.",
                    at(ts),
                    dur(ts - scheduled)
                ));
                seqs.push(seq);
            }
            Some((seq, ts)) if ts < scheduled => {
                s.push_str(&format!(
                    " The first exchange-side event of the order is at {}.",
                    at(ts)
                ));
                seqs.push(seq);
            }
            Some(_) => {}
            None => s.push_str(
                " No exchange-side event exists for this order: the dataset ended before the \
                 request arrived, so the simulation produced no outcome for it.",
            ),
        }
        out.push(Basis::Modeled, s, seqs);
    }

    // --- arrival ---

    fn arrival(&self, out: &mut Steps) {
        self.taker_arrival(out);
        let Some(q) = &self.life.queued else { return };
        out.push(
            Basis::Observed,
            format!(
                "When the order arrived at the exchange at {}, the historical book displayed {} \
                 at {}; the best bid was {} and the best ask {}.",
                at(q.ts),
                self.fmt.qty(q.level_qty),
                self.price(),
                self.opt_price(q.best_bid_tick),
                self.opt_price(q.best_ask_tick)
            ),
            vec![q.seq],
        );
        let assigned = match q.queue_ahead {
            Some(ahead) => format!(
                "The queue model assigned {} ahead of the order: its assumption that the order \
                 joins the queue behind everything displayed at that price.",
                self.fmt.est(ahead)
            ),
            None => "The queue model's state could not be read when the order started resting, \
                     so the modeled quantity ahead at entry is unknown; it is not treated as \
                     zero."
                .to_string(),
        };
        out.push(Basis::Modeled, assigned, vec![q.seq]);
    }

    fn opt_price(&self, tick: Option<i64>) -> String {
        match tick {
            Some(t) => self.fmt.price(t),
            None => "absent".to_string(),
        }
    }

    fn taker_arrival(&self, out: &mut Steps) {
        let takers: Vec<&Fill> = self
            .life
            .fills
            .iter()
            .filter(|f| f.mechanism == Mechanism::Taker)
            .collect();
        let Some(first) = takers.first() else { return };

        // Consecutive executions at one price read as one line.
        let mut levels: Vec<(i64, f64)> = Vec::new();
        for f in &takers {
            let same_level = levels
                .last()
                .is_some_and(|(tick, _)| *tick == f.price_tick);
            if same_level {
                if let Some((_, qty)) = levels.last_mut() {
                    *qty += f.qty;
                }
            } else {
                levels.push((f.price_tick, f.qty));
            }
        }
        const SHOWN: usize = 6;
        let mut breakdown = levels
            .iter()
            .take(SHOWN)
            .map(|(tick, qty)| format!("{} at {}", self.fmt.qty(*qty), self.fmt.price(*tick)))
            .collect::<Vec<_>>()
            .join(", ");
        if levels.len() > SHOWN {
            breakdown.push_str(&format!(
                " and {} more",
                count((levels.len() - SHOWN) as u64, "price level")
            ));
        }
        let total: f64 = takers.iter().map(|f| f.qty).sum();
        let model = match self.config.execution.exchange_model {
            ExchangeModel::NoPartialFill => {
                "The configured no-partial-fill exchange model fills the whole quantity at the \
                 best opposite price without checking how much is displayed there."
            }
            ExchangeModel::PartialFill
                if self.order.ord_type == OrdType::Limit && self.order.tif == Tif::Gtc =>
            {
                "The configured partial-fill exchange model consumes the displayed opposite \
                 side level by level; for a GTC order it then executes whatever is left at the \
                 order's own limit price, because a replayed book cannot hold the remainder as \
                 a new quote."
            }
            ExchangeModel::PartialFill => {
                "The configured partial-fill exchange model consumes the displayed opposite \
                 side level by level."
            }
        };
        let mut seqs: Vec<u64> = takers.iter().map(|f| f.seq).collect();
        seqs.extend(takers.iter().filter_map(|f| f.response_seq));
        out.push(
            Basis::Modeled,
            format!(
                "On arrival at {} the order was marketable and the exchange model executed {} \
                 immediately as a taker: {breakdown}. {model} The displayed book is observed \
                 market data, but the execution is hypothetical: the historical market never \
                 saw this order.",
                at(first.ts),
                self.fmt.qty(total)
            ),
            seqs,
        );
    }

    // --- queue progress ---

    fn queue_progress(&self, out: &mut Steps) {
        let life = &self.life;
        let Some(q) = &life.queued else { return };
        let price = self.price();
        let (traded, prints) = life.traded();
        let dropped = match life.truncated {
            Some(n) => format!(
                " The recorder did not keep {} that each moved the estimate by less than one \
                 lot; these were depth-driven updates. Trade-driven updates are always retained.",
                count(n, "queue update")
            ),
            None => String::new(),
        };
        if prints > 0 {
            let seqs = life
                .updates
                .iter()
                .filter(|u| matches!(u.cause, QueueCause::Trade { .. }))
                .map(|u| u.seq)
                .collect();
            out.push(
                Basis::Observed,
                format!(
                    "While the order rested, {} totalling {} occurred at {price}.{dropped}",
                    count(prints, "trade print"),
                    self.fmt.qty(traded)
                ),
                seqs,
            );
        } else {
            out.push(
                Basis::Observed,
                format!(
                    "No trade printed at {price} while the order rested: the tape holds no \
                     trade-driven queue update for it.{dropped}"
                ),
                vec![q.seq],
            );
        }

        if life.updates.is_empty() {
            out.push(
                Basis::Modeled,
                format!(
                    "The modeled quantity ahead stayed at {}: no trade or change in displayed \
                     quantity at {price} was recorded while the order rested.",
                    self.ahead_txt(q.queue_ahead)
                ),
                vec![q.seq],
            );
            return;
        }

        let m = life.movement();
        let depth = count(m.depth_changes, "change");
        let mut s = match (prints > 0, m.depth_changes > 0) {
            (true, true) => format!(
                "The modeled quantity ahead fell by {} through those trades and by {} through \
                 {depth} in the displayed quantity at the price.",
                self.fmt.est(m.by_trades),
                self.fmt.est(m.by_depth)
            ),
            (true, false) => format!(
                "The modeled quantity ahead fell by {} through those trades.",
                self.fmt.est(m.by_trades)
            ),
            (false, _) => format!(
                "The modeled quantity ahead fell by {} through {depth} in the displayed quantity \
                 at the price.",
                self.fmt.est(m.by_depth)
            ),
        };
        if m.depth_changes > 0 {
            s.push_str(&format!(
                " How much of a decrease in displayed quantity was ahead of the order is not in \
                 the data; the configured queue model ({}) decides it: {}.",
                self.config.execution.queue_model.label(),
                queue_model_clause(&self.config.execution.queue_model)
            ));
        }
        let lowest = life
            .queue_path()
            .iter()
            .map(|p| p.ahead)
            .min_by(f64::total_cmp);
        match lowest {
            Some(low) if low < 0.0 => s.push_str(&format!(
                " The estimate reached zero; trades went up to {} beyond it.",
                self.fmt.est(-low)
            )),
            Some(low) => s.push_str(&format!(
                " The smallest modeled quantity ahead reached was {}.",
                self.fmt.est(low)
            )),
            None => s.push_str(" No readable estimate was recorded."),
        }
        if m.unreadable > 0 {
            s.push_str(&format!(
                " {} with an unreadable estimate could not be included in these sums.",
                count(m.unreadable, "update")
            ));
        }
        out.push(
            Basis::Modeled,
            s,
            life.updates.iter().map(|u| u.seq).collect(),
        );
    }

    // --- fill mechanism ---

    fn maker_fills(&self, out: &mut Steps) {
        // One step per mechanism, in the order each first occurred.
        let mut groups: Vec<(Mechanism, Vec<&Fill>)> = Vec::new();
        for f in &self.life.fills {
            if f.mechanism == Mechanism::Taker {
                continue;
            }
            match groups.iter().position(|(m, _)| *m == f.mechanism) {
                Some(i) => groups[i].1.push(f),
                None => groups.push((f.mechanism, vec![f])),
            }
        }
        for (mechanism, group) in &groups {
            match mechanism {
                Mechanism::QueueExhausted => self.exhausted_fills(group, out),
                Mechanism::PriceCrossed => self.crossed_fills(group, out),
                Mechanism::Taker => {}
            }
        }
    }

    fn fill_seqs(&self, group: &[&Fill]) -> Vec<u64> {
        let mut seqs: Vec<u64> = group.iter().map(|f| f.seq).collect();
        seqs.extend(group.iter().filter_map(|f| f.response_seq));
        seqs.extend(
            group
                .iter()
                .filter_map(|f| f.trigger.map(|i| self.life.updates[i].seq)),
        );
        seqs
    }

    fn exhausted_fills(&self, group: &[&Fill], out: &mut Steps) {
        let price = self.price();
        let partial = self.config.execution.exchange_model == ExchangeModel::PartialFill;
        let model = if partial {
            " The partial-fill exchange model fills the order only by the traded quantity that \
             went beyond the queue ahead."
        } else {
            " The no-partial-fill exchange model fills the whole order as soon as the modeled \
             queue ahead is exhausted, whatever quantity traded beyond it."
        };
        let total: f64 = group.iter().map(|f| f.qty).sum();
        let s = if let [f] = group {
            let trigger = f.trigger.map(|i| &self.life.updates[i]);
            let trade = match trigger.map(|u| u.cause) {
                Some(QueueCause::Trade { qty }) => format!("a trade of {}", self.fmt.qty(qty)),
                _ => "a trade".to_string(),
            };
            let before = match trigger.and_then(|u| u.before) {
                Some(b) => format!("{} modeled ahead of the order", self.fmt.est(b.max(0.0))),
                None => "an unreadable estimate of the quantity ahead".to_string(),
            };
            let beyond = match trigger.and_then(|u| u.after) {
                Some(a) => format!(", going {} beyond it", self.fmt.est(-a)),
                None => String::new(),
            };
            let left = if self.fmt.has_qty(f.leaves_qty) {
                format!(" {} of the order stayed resting.", self.fmt.qty(f.leaves_qty))
            } else {
                String::new()
            };
            format!(
                "At {} {trade} printed at {price} with {before}; it exhausted the modeled queue \
                 ahead{beyond}, and the exchange model filled {} of the order as maker at \
                 {price}.{model}{left}",
                at(f.ts),
                self.fmt.qty(f.qty)
            )
        } else {
            let (Some(first), Some(last)) = (group.first(), group.last()) else {
                return;
            };
            format!(
                "{} totalling {} at {price}, between {} and {}, each immediately followed a \
                 trade at that price that took the modeled quantity ahead below zero.{model}",
                count(group.len() as u64, "maker execution"),
                self.fmt.qty(total),
                at(first.ts),
                at(last.ts)
            )
        };
        out.push(Basis::Modeled, s, self.fill_seqs(group));
    }

    fn crossed_fills(&self, group: &[&Fill], out: &mut Steps) {
        let Some(first) = group.first() else { return };
        let price = self.price();
        let total: f64 = group.iter().map(|f| f.qty).sum();
        let ahead = self.life.ahead_before(first.seq);
        let ahead_note = match ahead {
            Some((_, a)) => format!(
                "the latest modeled quantity ahead was {}",
                self.fmt.est(a.max(0.0))
            ),
            None => "the modeled quantity ahead was unknown".to_string(),
        };
        let lead = if group.len() == 1 {
            format!(
                "At {} the exchange model filled {} of the order as maker at {price}",
                at(first.ts),
                self.fmt.qty(total)
            )
        } else {
            format!(
                "Starting at {} the exchange model filled {} of the order as maker at {price} \
                 in {}",
                at(first.ts),
                self.fmt.qty(total),
                count(group.len() as u64, "execution")
            )
        };
        let mut seqs = self.fill_seqs(group);
        seqs.extend(ahead.map(|(seq, _)| seq));
        out.push(
            Basis::Modeled,
            format!(
                "{lead} without a trade at that price exhausting the modeled queue ({ahead_note}). \
                 No queue-exhausting trade immediately precedes the fill on the tape, so it was \
                 triggered by the market trading through {price} or by the opposite best price \
                 crossing it; the exchange model fills a resting order in that case regardless \
                 of queue position. The tape does not record which of the two occurred."
            ),
            seqs,
        );
    }

    // --- expiry on arrival ---

    fn expiry(&self, out: &mut Steps) {
        let o = self.order;
        if o.final_status != OrderStatus::Expired {
            return;
        }
        // An order expired before reaching the exchange is covered by the request step.
        let Some(resp) = self
            .life
            .exch
            .iter()
            .find(|r| !r.rejected && r.status == OrderStatus::Expired)
        else {
            return;
        };
        let price = self.price();
        let verb = side_verb(o.side);
        let (crosses_when, opposite) = match o.side {
            Side::Buy => ("at or above the best ask", "ask"),
            Side::Sell => ("at or below the best bid", "bid"),
        };

        if o.fill_count > 0 {
            let scope = if o.ord_type == OrdType::Market {
                String::new()
            } else {
                format!(" at {price} or better")
            };
            out.push(
                Basis::Modeled,
                format!(
                    "After those executions {} of the order was left. A {} order does not rest \
                     and the exchange model found no further displayed liquidity{scope}, so it \
                     expired the remainder at {}.",
                    self.fmt.qty(self.remaining()),
                    self.kind(),
                    at(resp.ts)
                ),
                vec![resp.seq],
            );
            return;
        }

        let post_only = o.ord_type == OrdType::Limit && o.tif == Tif::Gtx;
        let s = if post_only {
            format!(
                "The order reached the exchange at {}. A post-only {verb} crosses the book when \
                 its price is {crosses_when}; at {price} it would have crossed, so the exchange \
                 model expired it without execution instead of letting it take liquidity.",
                at(resp.ts)
            )
        } else if o.ord_type == OrdType::Market {
            format!(
                "The market order reached the exchange at {} and the exchange model found no \
                 displayed liquidity on the opposite side of the book, so it expired the order \
                 without execution.",
                at(resp.ts)
            )
        } else if o.tif == Tif::Fok {
            format!(
                "The order reached the exchange at {}. The exchange model could not fill the \
                 FOK {verb} completely against displayed liquidity at {price} or better, and an \
                 FOK order neither fills in part nor rests, so it expired without execution.",
                at(resp.ts)
            )
        } else if o.tif == Tif::Ioc {
            format!(
                "The order reached the exchange at {}. The exchange model found no displayed \
                 liquidity the IOC {verb} could execute against at {price} or better, and an \
                 IOC order does not rest, so it expired without execution.",
                at(resp.ts)
            )
        } else {
            format!(
                "The exchange model expired the order at {} without execution.",
                at(resp.ts)
            )
        };
        out.push(Basis::Modeled, s, vec![resp.seq]);

        // What the strategy believed when it decided, set against what the order met.
        let Some(intent) = &self.life.intent else {
            return;
        };
        if o.ord_type == OrdType::Market {
            return;
        }
        let known = match o.side {
            Side::Buy => intent.market.best_ask_tick,
            Side::Sell => intent.market.best_bid_tick,
        };
        let Some(known) = known else { return };
        let marketable_in_view = match o.side {
            Side::Buy => o.price_tick >= known,
            Side::Sell => o.price_tick <= known,
        };
        let elapsed = dur(resp.ts - intent.ts);
        let known_price = self.fmt.price(known);
        let gap = count((o.price_tick - known).unsigned_abs(), "tick");
        let s = match (post_only, marketable_in_view) {
            (true, false) => format!(
                "At decision time the strategy knew a best {opposite} of {known_price}, {gap} \
                 away from the order's price, so the order did not cross the book the strategy \
                 saw. The book it met at the exchange {elapsed} later was different: the \
                 strategy's view was out of date, the market moved while the request was in \
                 flight, or both."
            ),
            (true, true) => format!(
                "At decision time the order's price already crossed the best {opposite} the \
                 strategy knew ({known_price})."
            ),
            (false, true) => format!(
                "At decision time the strategy knew a best {opposite} of {known_price}, which \
                 the order's price reached, so the order was marketable against the book the \
                 strategy saw. The book it met at the exchange {elapsed} later no longer offered \
                 that: the strategy's view was out of date, the market moved while the request \
                 was in flight, or both."
            ),
            (false, false) => format!(
                "At decision time the order's price did not reach the best {opposite} the \
                 strategy knew ({known_price}), so it was not marketable against the \
                 strategy's own view either."
            ),
        };
        out.push(Basis::Derived, s, vec![intent.seq, resp.seq]);
    }

    // --- cancellation ---

    fn cancellation(&self, out: &mut Steps) {
        for cancel in &self.life.cancels {
            if let Some(d) = &cancel.decision {
                let reason = if d.reason.is_empty() {
                    "It recorded no reason.".to_string()
                } else {
                    format!("Its stated reason: \"{}\".", d.reason)
                };
                out.push(
                    Basis::Derived,
                    format!(
                        "At {} the strategy decided to cancel the order. {reason} {}",
                        at(d.ts),
                        self.view(d.market, d.known_position)
                    ),
                    vec![d.seq],
                );
            }
            if let Some((seq, error)) = cancel.refused {
                out.push(
                    Basis::Modeled,
                    format!(
                        "The engine refused the cancel request synchronously and nothing was \
                         sent ({error})."
                    ),
                    vec![seq],
                );
                continue;
            }
            let Some(sent) = &cancel.sent else { continue };
            if sent.entry_latency_ns < 0 {
                out.push(
                    Basis::Modeled,
                    format!(
                        "The cancel request was sent at {}, but the latency model returned a \
                         negative entry latency ({} ns), the engine's convention for a request \
                         rejected before it reaches the matching engine; the cancel never \
                         arrived.",
                        at(sent.ts),
                        sent.entry_latency_ns
                    ),
                    vec![sent.seq],
                );
                continue;
            }

            let resp = cancel.response.map(|i| &self.life.exch[i]);
            let mut seqs = vec![sent.seq];
            let mut s = format!(
                "The cancel request was sent at {} with a modeled entry latency of {}",
                at(sent.ts),
                dur(sent.entry_latency_ns)
            );
            match resp {
                Some(r) if r.rejected => {
                    seqs.push(r.seq);
                    let gone = match self.life.terminal() {
                        Some(t) if t.seq < r.seq => {
                            seqs.push(t.seq);
                            format!(" (it had become {} at {})", status_label(t.status), at(t.ts))
                        }
                        _ => String::new(),
                    };
                    s.push_str(&format!(
                        " and reached the exchange at {}, where the order no longer \
                         existed{gone}; the exchange rejected the cancel.",
                        at(r.ts)
                    ));
                }
                Some(r) => {
                    seqs.push(r.seq);
                    s.push_str(&format!(
                        " and took effect at the exchange at {}, removing the order with {} \
                         unfilled.",
                        at(r.ts),
                        self.fmt.qty(self.remaining())
                    ));
                }
                None => {
                    let scheduled = sent
                        .exch_arrival_ts
                        .unwrap_or(sent.ts + sent.entry_latency_ns);
                    s.push_str(&format!(
                        ", scheduling its arrival at the exchange for {}; the dataset ended \
                         before the exchange processed it.",
                        at(scheduled)
                    ));
                }
            }
            out.push(Basis::Modeled, s, seqs);
            self.cancel_race(sent, resp, out);
            if let Some(r) = resp
                && !r.rejected
            {
                self.queue_at_cancel(r, out);
            }
        }
    }

    /// What happened to the order between the cancel being sent and it reaching the exchange.
    fn cancel_race(&self, sent: &Sent, resp: Option<&Resp>, out: &mut Steps) {
        let life = &self.life;
        let price = self.price();
        let mut seqs = vec![sent.seq];
        seqs.extend(resp.map(|r| r.seq));

        // Already terminal at the exchange before the cancel was even sent: the strategy acted
        // on a status it had not received yet. Nothing about the flight window can be claimed.
        if let Some(t) = life.terminal()
            && t.seq < sent.seq
        {
            let mut s = format!(
                "The order had already become {} at the exchange at {}, {} before the cancel \
                 was sent.",
                status_label(t.status),
                at(t.ts),
                dur(sent.ts - t.ts)
            );
            seqs.push(t.seq);
            let notice = life
                .local
                .iter()
                .find(|l| !l.rejected && l.status == t.status && l.exch_ts == t.ts);
            match notice {
                Some(l) if l.ts > sent.ts => {
                    s.push_str(&format!(
                        " The strategy did not know: the response carrying that status reached \
                         it only at {}, after a modeled response latency of {}.",
                        at(l.ts),
                        dur(t.latency_ns)
                    ));
                    seqs.push(l.seq);
                }
                Some(_) => {}
                None => s.push_str(
                    " The response carrying that status had not reached the strategy when the \
                     data ended.",
                ),
            }
            out.push(Basis::Modeled, s, seqs);
            return;
        }

        let end = resp.map_or(u64::MAX, |r| r.seq);
        let within = |seq: u64| seq > sent.seq && seq < end;
        let fills: Vec<&Fill> = life.fills.iter().filter(|f| within(f.seq)).collect();
        let mut traded = 0.0;
        let mut prints = 0u64;
        for u in life.updates.iter().filter(|u| within(u.seq)) {
            if let QueueCause::Trade { qty } = u.cause {
                traded += qty;
                prints += 1;
                seqs.push(u.seq);
            }
        }
        seqs.extend(fills.iter().map(|f| f.seq));
        let flight = match resp {
            Some(r) => format!(
                "the {} between the cancel being sent and reaching the exchange",
                dur(r.ts - sent.ts)
            ),
            None => "the time between the cancel being sent and the end of the data".to_string(),
        };

        let s = if let Some(first) = fills.first() {
            let filled: f64 = fills.iter().map(|f| f.qty).sum();
            let consequence = match resp {
                Some(r) if r.rejected => {
                    "the order was gone when the cancel arrived, so the cancel lost the race"
                }
                Some(_) => "the cancel removed only what was left",
                None => "the cancel had not arrived when the data ended",
            };
            format!(
                "Latency race: during {flight}, the order received {} totalling {}, the first \
                 {} after the cancel was sent; {consequence}.",
                count(fills.len() as u64, "fill"),
                self.fmt.qty(filled),
                dur(first.ts - sent.ts)
            )
        } else if prints > 0 {
            let from = life.ahead_before(sent.seq).map(|(_, a)| a);
            let to = life.ahead_before(end).map(|(_, a)| a);
            let consequence = match resp {
                Some(r) if !r.rejected => {
                    "the modeled queue ahead was not exhausted before the cancel took effect"
                }
                _ => "the modeled queue ahead was not exhausted",
            };
            format!(
                "Latency race: during {flight}, {} traded at {price} in {} and the modeled \
                 quantity ahead moved from {} to {}; {consequence}.",
                self.fmt.qty(traded),
                count(prints, "trade print"),
                self.ahead_txt(from),
                self.ahead_txt(to)
            )
        } else {
            format!(
                "During {flight}, no trade at {price} was recorded for the order and it \
                 received no fill, so no fill was won or lost to the cancel's latency."
            )
        };
        out.push(Basis::Modeled, s, seqs);
    }

    /// The queue state a cancelled order was removed in.
    fn queue_at_cancel(&self, cancelled: &Resp, out: &mut Steps) {
        let Some(q) = &self.life.queued else { return };
        let price = self.price();
        let last = self.life.ahead_before(cancelled.seq);
        let (traded, prints) = self.life.traded();
        let activity = if prints > 0 {
            format!("{} traded at {price}", self.fmt.qty(traded))
        } else {
            format!("no trade at {price} was recorded")
        };
        let mut seqs = vec![q.seq, cancelled.seq];
        seqs.extend(last.map(|(seq, _)| seq));
        out.push(
            Basis::Modeled,
            format!(
                "When the cancel took effect the modeled quantity ahead was {}; it was {} at \
                 entry. Over the {} the order rested, {activity}.",
                self.ahead_txt(last.map(|(_, a)| a)),
                self.ahead_txt(q.queue_ahead),
                dur(cancelled.ts - q.ts)
            ),
            seqs,
        );
    }

    // --- still working when the data ended ---

    fn end_of_data(&self, out: &mut Steps) {
        let o = self.order;
        let working = match o.outcome {
            OrderOutcome::WorkingAtEnd => true,
            OrderOutcome::PartiallyFilled => !o.final_status.is_terminal(),
            _ => false,
        };
        if !working {
            return;
        }
        let life = &self.life;
        let price = self.price();
        let last = life.ahead_before(u64::MAX);
        let mut seqs: Vec<u64> = life.queued.iter().map(|q| q.seq).collect();
        seqs.extend(last.map(|(seq, _)| seq));

        if let Some(fill) = life.fills.last() {
            seqs.push(fill.seq);
            out.push(
                Basis::Modeled,
                format!(
                    "The dataset ended with {} of the order still resting. After its last fill \
                     at {} no fill condition of the exchange model was met again.",
                    self.fmt.qty(self.remaining()),
                    at(fill.ts)
                ),
                seqs,
            );
            return;
        }

        let (traded, prints) = life.traded();
        let queue = if prints > 0 {
            format!(
                "trades at {price} did not exhaust the modeled queue ahead ({} traded; the \
                 estimate went from {} to {})",
                self.fmt.qty(traded),
                self.ahead_txt(life.queued.as_ref().and_then(|q| q.queue_ahead)),
                self.ahead_txt(last.map(|(_, a)| a))
            )
        } else {
            format!(
                "no trade printed at {price} while the order rested (the modeled quantity ahead \
                 ended at {})",
                self.ahead_txt(last.map(|(_, a)| a))
            )
        };
        out.push(
            Basis::Modeled,
            format!(
                "The dataset ended with the order still resting and unfilled. Neither fill \
                 condition of the exchange model was met before that: {queue}, and the model \
                 produced no fill from the market trading through or crossing that price."
            ),
            seqs,
        );
    }

    // --- fees and response latency ---

    fn fees_and_response(&self, out: &mut Steps) {
        let life = &self.life;
        if !life.fills.is_empty() {
            let fee: f64 = life.fills.iter().map(|f| f.fee).sum();
            let value: f64 = life.fills.iter().map(|f| f.value).sum();
            let rates = self.config.execution.fees;
            out.push(
                Basis::Modeled,
                format!(
                    "The fee model charged {} on {} of simulated traded value, in quote \
                     currency (configured rates as a fraction of traded value: maker {}, taker \
                     {}; a negative amount is a rebate).",
                    num(fee),
                    num(value),
                    num(rates.maker),
                    num(rates.taker)
                ),
                life.fills.iter().map(|f| f.seq).collect(),
            );
        }

        let Some(first) = life.exch.first() else {
            return;
        };
        let mut seqs = vec![first.seq];
        let status = if first.rejected {
            "cancel rejected"
        } else {
            status_label(first.status)
        };
        let mut s = format!(
            "The exchange's first response ({status}) was emitted at {} with a modeled response \
             latency of {}",
            at(first.ts),
            dur(first.latency_ns)
        );
        match life.local.first() {
            Some(l) => {
                seqs.push(l.seq);
                s.push_str(&format!(
                    " and reached the strategy host at {}; until then the strategy did not know \
                     the order's state.",
                    at(l.ts)
                ));
            }
            None => s.push_str(&format!(
                "; it was scheduled to reach the strategy host at {}, but the dataset ended \
                 first.",
                at(first.local_arrival_ts)
            )),
        }
        if let Some(fill) = life.fills.first() {
            match life.local.iter().find(|l| self.fmt.has_qty(l.exec_qty)) {
                Some(l) => {
                    seqs.push(l.seq);
                    seqs.push(fill.seq);
                    s.push_str(&format!(
                        " The strategy learned of its first fill at {}, {} after the fill at \
                         the simulated exchange.",
                        at(l.ts),
                        dur(l.ts - fill.ts)
                    ));
                }
                None => s.push_str(
                    " No fill notification had reached the strategy when the data ended.",
                ),
            }
        }
        out.push(Basis::Modeled, s, seqs);
    }

    // --- what cannot be known ---

    fn limits(&self, out: &mut Steps) {
        let s = if self.life.queued.is_some() {
            "Market-By-Price data shows the total quantity at each price, not individual \
             orders, so the true position of any order in the queue is not observable; whether \
             a real order placed at this price and time would have filled, and when, is not \
             knowable from this replay."
        } else if self.life.first_exch.is_some() {
            "Whether a real order would have met the same book and obtained the same result is \
             not knowable from this replay: its arrival time is modeled and the historical \
             market never interacted with it."
        } else if self.order.outcome == OrderOutcome::InFlightAtEnd {
            "What would have happened to the order after the end of the data is not knowable \
             from this replay."
        } else {
            return;
        };
        out.push(Basis::Unobservable, s.to_string(), Vec::new());
    }

    // --- summary parts ---

    fn caveats(&self) -> Vec<String> {
        let mut caveats = vec![
            CAVEAT_QUEUE.to_string(),
            CAVEAT_IMPACT.to_string(),
            CAVEAT_SIMULATED.to_string(),
        ];
        if self.config.execution.exchange_model == ExchangeModel::NoPartialFill {
            caveats.push(CAVEAT_NO_PARTIAL.to_string());
        }
        if let Some(dropped) = self.life.truncated {
            caveats.push(format!(
                "The recorder's cap on queue updates per order was reached: {} that each moved \
                 the estimate by less than one lot were not kept, so the queue path is \
                 incomplete between the retained points. Only depth-driven updates can be \
                 dropped; trade-driven updates are always retained.",
                count(dropped, "update")
            ));
        }
        caveats
    }

    fn latency(&self) -> LatencyBreakdown {
        let life = &self.life;
        let o = self.order;
        let feed_local = life.intent.as_ref().and_then(|i| i.market.feed_local_ts);
        let feed_exch = life.intent.as_ref().and_then(|i| i.market.feed_exch_ts);
        let decided = life.intent.as_ref().map(|i| i.ts);
        let non_negative = |sent: &Sent| (sent.entry_latency_ns >= 0).then_some(sent.entry_latency_ns);
        LatencyBreakdown {
            feed_staleness_ns: decided.zip(feed_local).map(|(d, l)| d - l),
            feed_latency_ns: feed_local.zip(feed_exch).map(|(l, e)| l - e),
            entry_latency_ns: life.new_sent.as_ref().and_then(non_negative),
            response_latency_ns: life.exch.first().map(|r| r.latency_ns),
            time_to_first_fill_ns: o.exch_arrival_ts.zip(o.first_fill_ts).map(|(a, f)| f - a),
            resting_time_ns: o.exch_arrival_ts.zip(o.terminal_ts).map(|(a, t)| t - a),
            cancel_entry_latency_ns: life
                .cancels
                .iter()
                .rev()
                .find_map(|c| c.sent.as_ref())
                .and_then(non_negative),
        }
    }

    // --- headline ---

    fn headline(&self, verdict: Verdict) -> String {
        let o = self.order;
        let life = &self.life;
        let price = self.price();
        let (traded, prints) = life.traded();
        let entry = life.queued.as_ref().and_then(|q| q.queue_ahead);

        if o.outcome == OrderOutcome::PartiallyFilled {
            let lead = format!(
                "Partially filled ({} of {})",
                self.fmt.qty(o.filled_qty),
                self.fmt.qty(o.qty)
            );
            let rest = self.fmt.qty(self.remaining());
            return match verdict {
                Verdict::NotFilledCanceled => {
                    format!("{lead}: the remaining {rest} was cancelled while still resting.")
                }
                Verdict::NotFilledNoLiquidityOnArrival => format!(
                    "{lead}: the remaining {rest} found no further displayed liquidity on \
                     arrival and expired."
                ),
                Verdict::NotFilledWorkingAtEnd => format!(
                    "{lead}: the remaining {rest} was still resting when the dataset ended."
                ),
                _ => format!("{lead}: the remaining {rest} was not filled."),
            };
        }

        match verdict {
            Verdict::FilledQueueExhausted => {
                let by_depth = life.movement().by_depth;
                match entry {
                    Some(e) if self.fmt.has_qty(by_depth) => format!(
                        "Filled as maker at {price}: {} traded at this price and, with {} taken \
                         off by the queue model for depth decreases, exhausted the modeled {} \
                         queued ahead.",
                        self.fmt.qty(traded),
                        self.fmt.est(by_depth),
                        self.fmt.est(e)
                    ),
                    Some(e) => format!(
                        "Filled as maker at {price}: {} traded at this price and exhausted the \
                         modeled {} queued ahead.",
                        self.fmt.qty(traded),
                        self.fmt.est(e)
                    ),
                    None => format!(
                        "Filled as maker at {price}: {} traded at this price and exhausted the \
                         modeled queue ahead, whose size at entry is unknown.",
                        self.fmt.qty(traded)
                    ),
                }
            }
            Verdict::FilledPriceCrossed => {
                let ahead = life
                    .fills
                    .first()
                    .and_then(|f| life.ahead_before(f.seq))
                    .map(|(_, a)| a);
                let regardless = match ahead {
                    Some(a) => format!(
                        "regardless of the modeled {} still queued ahead",
                        self.fmt.est(a.max(0.0))
                    ),
                    None => "regardless of its modeled queue position".to_string(),
                };
                format!(
                    "Filled as maker at {price}: the market traded through this price or the \
                     opposite best price crossed it, and the exchange model fills a resting \
                     order then {regardless}."
                )
            }
            Verdict::FilledTakerOnArrival => {
                let avg = match o.avg_fill_tick {
                    Some(tick) => self.fmt.price_frac(tick),
                    None => price,
                };
                format!(
                    "Filled as taker on arrival at {avg}: the order was marketable and the \
                     exchange model executed {} immediately.",
                    self.fmt.qty(o.filled_qty)
                )
            }
            Verdict::FilledMixed => {
                let sum = |m: Mechanism| -> f64 {
                    life.fills
                        .iter()
                        .filter(|f| f.mechanism == m)
                        .map(|f| f.qty)
                        .sum()
                };
                let parts: Vec<String> = [
                    (Mechanism::Taker, "as taker on arrival"),
                    (
                        Mechanism::QueueExhausted,
                        "as maker after trades exhausted the modeled queue ahead",
                    ),
                    (
                        Mechanism::PriceCrossed,
                        "as maker when the market traded through or crossed the price",
                    ),
                ]
                .iter()
                .filter_map(|(m, how)| {
                    let qty = sum(*m);
                    self.fmt
                        .has_qty(qty)
                        .then(|| format!("{} {how}", self.fmt.qty(qty)))
                })
                .collect();
                format!(
                    "Filled {} by more than one mechanism: {}.",
                    self.fmt.qty(o.filled_qty),
                    parts.join(", ")
                )
            }
            Verdict::NotFilledQueueNotReached => {
                let last = life.ahead_before(u64::MAX).map(|(_, a)| a);
                match (entry, last) {
                    (Some(e), Some(l)) => format!(
                        "Not filled: {} traded at {price} while the order rested, leaving {} of \
                         the modeled {} queued ahead when the dataset ended.",
                        self.fmt.qty(traded),
                        self.fmt.est(l.max(0.0)),
                        self.fmt.est(e)
                    ),
                    _ => format!(
                        "Not filled: {} traded at {price} while the order rested, and the \
                         modeled queue ahead was not exhausted before the dataset ended.",
                        self.fmt.qty(traded)
                    ),
                }
            }
            Verdict::NotFilledNoTradeAtPrice => format!(
                "Not filled: no trade printed at {price} while the order rested, and it was \
                 still working when the dataset ended."
            ),
            Verdict::NotFilledCanceled => {
                let end = life
                    .exch
                    .iter()
                    .find(|r| !r.rejected && r.status == OrderStatus::Canceled)
                    .map_or(u64::MAX, |r| r.seq);
                let last = life.ahead_before(end).map(|(_, a)| a);
                match (life.queued.is_some(), last) {
                    (true, Some(l)) if prints > 0 => format!(
                        "Not filled: cancelled with the modeled {} still queued ahead, after {} \
                         traded at {price}.",
                        self.fmt.est(l.max(0.0)),
                        self.fmt.qty(traded)
                    ),
                    (true, Some(l)) => format!(
                        "Not filled: cancelled with the modeled {} still queued ahead; no trade \
                         printed at {price} while it rested.",
                        self.fmt.est(l.max(0.0))
                    ),
                    (true, None) => "Not filled: cancelled while resting; the modeled quantity \
                                     ahead is unknown."
                        .to_string(),
                    (false, _) => "Not filled: the order was cancelled.".to_string(),
                }
            }
            Verdict::NotFilledPostOnlyWouldCross => format!(
                "Not filled: the post-only order would have crossed the book on arrival at \
                 {price}, so the exchange model expired it."
            ),
            Verdict::NotFilledNoLiquidityOnArrival => {
                if o.ord_type == OrdType::Market {
                    "Not filled: the market order found no displayed liquidity on arrival and \
                     expired."
                        .to_string()
                } else {
                    format!(
                        "Not filled: the {} order found no displayed liquidity it could execute \
                         against at {price} or better on arrival and expired.",
                        self.kind()
                    )
                }
            }
            Verdict::NotFilledRiskBlocked => match life.risk_block {
                Some((_, rule, _)) => format!(
                    "Not filled: the {} risk rule blocked the order before it was sent.",
                    rule_label(rule)
                ),
                None => "Not filled: a pre-trade risk rule blocked the order before it was sent."
                    .to_string(),
            },
            Verdict::NotFilledRefused => {
                if life.new_refused.is_some() {
                    "Not filled: the engine refused the request and nothing was sent.".to_string()
                } else {
                    "Not filled: the request was rejected before it reached the matching engine."
                        .to_string()
                }
            }
            Verdict::NotFilledDataEndedInFlight => {
                "Not filled: the dataset ended before the request reached the exchange.".to_string()
            }
            Verdict::NotFilledWorkingAtEnd => {
                "Not filled: the order was still working when the dataset ended.".to_string()
            }
        }
    }
}

/// Explains one order's outcome from its own tape events (`events`, in `seq` order).
///
/// `local_ts` is the provenance of the dataset's receipt timestamps: it decides whether the age
/// of the strategy's market view is presented as observed or as modeled.
pub fn explain_order(
    order: &OrderRecord,
    events: &[FlightEvent],
    instrument: &InstrumentMeta,
    config: &RunConfig,
    local_ts: LocalTsProvenance,
) -> Explanation {
    let cx = Ctx {
        order,
        life: Life::parse(events),
        fmt: Fmt::new(instrument),
        config,
        local_ts,
    };
    let question = match order.outcome {
        OrderOutcome::Filled => Question::WhyFilled,
        OrderOutcome::PartiallyFilled => Question::WhyPartiallyFilled,
        _ => Question::WhyNotFilled,
    };
    let verdict = cx.verdict();

    let mut steps = Steps(Vec::new());
    cx.decision(&mut steps);
    cx.request(&mut steps);
    cx.arrival(&mut steps);
    cx.queue_progress(&mut steps);
    cx.maker_fills(&mut steps);
    cx.expiry(&mut steps);
    cx.cancellation(&mut steps);
    cx.end_of_data(&mut steps);
    cx.fees_and_response(&mut steps);
    cx.limits(&mut steps);

    Explanation {
        order_id: order.order_id,
        question,
        verdict,
        headline: cx.headline(verdict),
        steps: steps.0,
        caveats: cx.caveats(),
        latency: cx.latency(),
        queue_path: cx.life.queue_path(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{
        build_orders,
        testkit::{config, Tape, INSTRUMENT, MS},
    };

    /// Explains order `id` of `tape` and checks the invariants every explanation must hold.
    fn explain_with(
        tape: &Tape,
        id: u64,
        config: &RunConfig,
        local_ts: LocalTsProvenance,
    ) -> (OrderRecord, Explanation) {
        let order = build_orders(&tape.0)
            .into_iter()
            .find(|o| o.order_id == id)
            .expect("the order is on the tape");
        let events = tape.of(id);
        let e = explain_order(&order, &events, &INSTRUMENT, config, local_ts);
        check(&e, &events, config);
        (order, e)
    }

    fn explain(tape: &Tape, id: u64, config: &RunConfig) -> (OrderRecord, Explanation) {
        explain_with(tape, id, config, LocalTsProvenance::Measured)
    }

    fn check(e: &Explanation, events: &[FlightEvent], config: &RunConfig) {
        let on_tape: BTreeSet<u64> = events.iter().map(|ev| ev.seq).collect();
        assert!(e.headline.ends_with('.'), "headline: {}", e.headline);
        assert!(!e.steps.is_empty());
        for step in &e.steps {
            assert!(!step.statement.is_empty());
            for seq in &step.event_seqs {
                assert!(
                    on_tape.contains(seq),
                    "step cites seq {seq}, which is not one of the order's events: {}",
                    step.statement
                );
            }
            // Only a statement about what cannot be known may stand without a tape citation.
            if step.basis != Basis::Unobservable {
                assert!(!step.event_seqs.is_empty(), "uncited: {}", step.statement);
            }
        }
        for caveat in [CAVEAT_QUEUE, CAVEAT_IMPACT, CAVEAT_SIMULATED] {
            assert_eq!(e.caveats.iter().filter(|c| c.as_str() == caveat).count(), 1);
        }
        let no_partial = config.execution.exchange_model == ExchangeModel::NoPartialFill;
        assert_eq!(
            e.caveats
                .iter()
                .filter(|c| c.as_str() == CAVEAT_NO_PARTIAL)
                .count(),
            usize::from(no_partial)
        );
    }

    fn step_with<'e>(e: &'e Explanation, needle: &str) -> &'e ExplainStep {
        e.steps
            .iter()
            .find(|s| s.statement.contains(needle))
            .unwrap_or_else(|| panic!("no step mentions {needle:?}; steps: {:#?}", e.steps))
    }

    fn partial_fill_config() -> RunConfig {
        let mut cfg = config();
        cfg.execution.exchange_model = ExchangeModel::PartialFill;
        cfg
    }

    #[test]
    fn queue_exhausted_maker_fill() {
        let mut t = Tape::new();
        let queued = t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtx, Some(0.8));
        let t1 = t.trade(20 * MS, 256, 0.5, 0.8, 0.3);
        let t2 = t.trade(30 * MS, 256, 0.75, 0.3, -0.45);
        let fill = t.fill(30 * MS, 256, 20_000, 1.0, true, 1.0, 0.0);
        let resp = t.resp(30 * MS, 256, OrderStatus::Filled, 0.0);
        t.local(31 * MS, 256, OrderStatus::Filled, 1.0, 0.0, 30 * MS);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Filled);
        assert_eq!(order.queue_ahead_at_entry, Some(0.8));
        assert_eq!(order.traded_at_price_qty, 1.25);
        assert_eq!(order.maker_qty, 1.0);
        assert_eq!(e.order_id, 256);
        assert_eq!(e.question, Question::WhyFilled);
        assert_eq!(e.verdict, Verdict::FilledQueueExhausted);
        assert_eq!(
            e.headline,
            "Filled as maker at 10000.0: 1.250 traded at this price and exhausted the modeled \
             0.800 queued ahead."
        );

        // The decision comes first, with the strategy's own reason; its staleness is a
        // measurement on a dataset with recorded receipt times.
        assert_eq!(e.steps[0].basis, Basis::Derived);
        assert!(e.steps[0].statement.contains("join the touch"));
        assert!(e.steps[0].statement.contains("buy 1.000 at 10000.0"));
        assert_eq!(e.steps[1].basis, Basis::Observed);
        assert!(e.steps[1].statement.contains("1 ms old"));

        // Observed market data and modeled estimates are separate steps.
        let displayed = step_with(&e, "the historical book displayed 0.800");
        assert_eq!(displayed.basis, Basis::Observed);
        let assigned = step_with(&e, "The queue model assigned 0.800 ahead");
        assert_eq!(assigned.basis, Basis::Modeled);
        assert_eq!(assigned.event_seqs, vec![queued]);
        let prints = step_with(&e, "2 trade prints totalling 1.250");
        assert_eq!(prints.basis, Basis::Observed);
        assert_eq!(prints.event_seqs, vec![t1, t2]);

        // The mechanism cites the exhausting trade, the fill and its response.
        let mechanism = step_with(&e, "it exhausted the modeled queue ahead");
        assert_eq!(mechanism.basis, Basis::Modeled);
        assert_eq!(mechanism.event_seqs, vec![t2, fill, resp]);
        assert!(mechanism.statement.contains("a trade of 0.750"));
        assert!(mechanism.statement.contains("0.300 modeled ahead"));
        assert!(mechanism.statement.contains("going 0.450 beyond it"));

        assert!(e.steps.iter().any(|s| s.basis == Basis::Unobservable));
        assert_eq!(
            e.latency,
            LatencyBreakdown {
                feed_staleness_ns: Some(MS),
                feed_latency_ns: Some(2 * MS),
                entry_latency_ns: Some(MS),
                response_latency_ns: Some(MS),
                time_to_first_fill_ns: Some(19 * MS),
                resting_time_ns: Some(19 * MS),
                cancel_entry_latency_ns: None,
            }
        );
        let path: Vec<(u64, RelNs, f64)> =
            e.queue_path.iter().map(|p| (p.seq, p.ts, p.ahead)).collect();
        assert_eq!(
            path,
            vec![
                (queued, 11 * MS, 0.8),
                (t1, 20 * MS, 0.3),
                (t2, 30 * MS, -0.45)
            ]
        );
    }

    #[test]
    fn price_crossed_maker_fill() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));
        let depth = t.depth(20 * MS, 256, 2.0, 1.5, 2.0, 1.5);
        let fill = t.fill(25 * MS, 256, 20_000, 1.0, true, 1.0, 0.0);
        let resp = t.resp(25 * MS, 256, OrderStatus::Filled, 0.0);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Filled);
        assert_eq!(order.traded_at_price_qty, 0.0);
        assert_eq!(order.min_queue_ahead, Some(1.5));
        assert_eq!(e.verdict, Verdict::FilledPriceCrossed);
        assert_eq!(
            e.headline,
            "Filled as maker at 10000.0: the market traded through this price or the opposite \
             best price crossed it, and the exchange model fills a resting order then regardless \
             of the modeled 1.500 still queued ahead."
        );

        // The tape cannot tell a trade-through from a crossing, and the step says so.
        let mechanism = step_with(&e, "The tape does not record which of the two occurred");
        assert_eq!(mechanism.basis, Basis::Modeled);
        assert_eq!(mechanism.event_seqs, vec![depth, fill, resp]);

        let none = step_with(&e, "No trade printed at 10000.0");
        assert_eq!(none.basis, Basis::Observed);
        let moved = step_with(&e, "the configured queue model (risk_adverse) decides it");
        assert_eq!(moved.basis, Basis::Modeled);
        assert_eq!(moved.event_seqs, vec![depth]);
        assert!(moved.statement.contains("fell by 0.500 through 1 change"));

        // The data ended before the fill notification reached the strategy.
        step_with(&e, "No fill notification had reached the strategy");
    }

    #[test]
    fn taker_on_arrival() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_002, 1.0, Tif::Gtc);
        t.sent(10 * MS, 256, ReqKind::New, MS);
        let fill = t.fill(11 * MS, 256, 20_002, 1.0, false, 5.0, 0.0);
        let resp = t.resp(11 * MS, 256, OrderStatus::Filled, 0.0);
        t.local(12 * MS, 256, OrderStatus::Filled, 1.0, 0.0, 11 * MS);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Filled);
        assert_eq!(order.taker_qty, 1.0);
        assert_eq!(order.maker_qty, 0.0);
        assert_eq!(order.exch_arrival_ts, Some(11 * MS));
        assert_eq!(order.avg_fill_tick, Some(20_002.0));
        assert_eq!(order.queue_ahead_at_entry, None);
        assert_eq!(e.verdict, Verdict::FilledTakerOnArrival);
        assert_eq!(
            e.headline,
            "Filled as taker on arrival at 10001.0: the order was marketable and the exchange \
             model executed 1.000 immediately."
        );

        let arrival = step_with(&e, "immediately as a taker: 1.000 at 10001.0");
        assert_eq!(arrival.basis, Basis::Modeled);
        assert_eq!(arrival.event_seqs, vec![fill, resp]);
        assert!(arrival.statement.contains("the execution is hypothetical"));
        assert!(e.queue_path.is_empty());
        assert_eq!(e.latency.time_to_first_fill_ns, Some(0));
        assert!(e.steps.iter().any(|s| s.basis == Basis::Unobservable));
    }

    #[test]
    fn mixed_mechanisms_are_reported_as_mixed() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 2.0, Tif::Gtc, Some(0.5));
        t.trade(20 * MS, 256, 1.0, 0.5, -0.5);
        t.fill(20 * MS, 256, 20_000, 0.5, true, 0.5, 1.5);
        t.resp(20 * MS, 256, OrderStatus::PartiallyFilled, 1.5);
        // No trade at the price precedes this one: the price was traded through or crossed.
        t.fill(30 * MS, 256, 20_000, 1.5, true, 1.5, 0.0);
        t.resp(30 * MS, 256, OrderStatus::Filled, 0.0);

        let (order, e) = explain(&t, 256, &partial_fill_config());
        assert_eq!(order.outcome, OrderOutcome::Filled);
        assert_eq!(order.fill_count, 2);
        assert_eq!(e.question, Question::WhyFilled);
        assert_eq!(e.verdict, Verdict::FilledMixed);
        assert_eq!(
            e.headline,
            "Filled 2.000 by more than one mechanism: 0.500 as maker after trades exhausted the \
             modeled queue ahead, 1.500 as maker when the market traded through or crossed the \
             price."
        );
        step_with(&e, "it exhausted the modeled queue ahead");
        step_with(&e, "The tape does not record which of the two occurred");
    }

    #[test]
    fn post_only_would_cross() {
        let mut t = Tape::new();
        let intent = t.intent(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtx);
        t.sent(10 * MS, 256, ReqKind::New, MS);
        let expired = t.resp(11 * MS, 256, OrderStatus::Expired, 1.0);
        t.local(12 * MS, 256, OrderStatus::Expired, 0.0, 1.0, 11 * MS);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Expired);
        assert_eq!(order.exch_arrival_ts, Some(11 * MS));
        assert_eq!(order.terminal_ts, Some(11 * MS));
        assert_eq!(order.filled_qty, 0.0);
        assert_eq!(e.question, Question::WhyNotFilled);
        assert_eq!(e.verdict, Verdict::NotFilledPostOnlyWouldCross);
        assert_eq!(
            e.headline,
            "Not filled: the post-only order would have crossed the book on arrival at 10000.0, \
             so the exchange model expired it."
        );

        let why = step_with(&e, "it would have crossed");
        assert_eq!(why.basis, Basis::Modeled);
        assert_eq!(why.event_seqs, vec![expired]);
        // The strategy saw the ask two ticks away; the book at the exchange was different.
        let view = step_with(&e, "did not cross the book the strategy saw");
        assert_eq!(view.basis, Basis::Derived);
        assert_eq!(view.event_seqs, vec![intent, expired]);
        assert!(view.statement.contains("a best ask of 10001.0, 2 ticks away"));
        assert!(e.queue_path.is_empty());
    }

    #[test]
    fn ioc_finds_no_liquidity() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_002, 1.0, Tif::Ioc);
        t.sent(10 * MS, 256, ReqKind::New, MS);
        let expired = t.resp(11 * MS, 256, OrderStatus::Expired, 1.0);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Expired);
        assert_eq!(e.verdict, Verdict::NotFilledNoLiquidityOnArrival);
        assert_eq!(
            e.headline,
            "Not filled: the IOC order found no displayed liquidity it could execute against at \
             10001.0 or better on arrival and expired."
        );
        let why = step_with(&e, "an IOC order does not rest");
        assert_eq!(why.event_seqs, vec![expired]);
        // The order was marketable against the view the strategy had 1 ms earlier.
        let view = step_with(&e, "marketable against the book the strategy saw");
        assert_eq!(view.basis, Basis::Derived);
        // The response never reached the strategy.
        step_with(&e, "the dataset ended first");
        assert_eq!(e.latency.resting_time_ns, Some(0));
    }

    #[test]
    fn cancelled_behind_the_queue_with_a_trade_during_the_cancel_flight() {
        let mut t = Tape::new();
        let queued = t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(3.0));
        t.trade(20 * MS, 256, 0.5, 3.0, 2.5);
        t.cancel_intent(40 * MS, 256, "requote");
        let sent = t.sent(40 * MS, 256, ReqKind::Cancel, MS);
        let racing = t.trade(40 * MS + 500_000, 256, 1.0, 2.5, 1.5);
        let canceled = t.resp(41 * MS, 256, OrderStatus::Canceled, 1.0);
        t.local(42 * MS, 256, OrderStatus::Canceled, 0.0, 1.0, 41 * MS);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Canceled);
        assert_eq!(order.cancel_sent_ts, Some(40 * MS));
        assert_eq!(order.cancel_reason.as_deref(), Some("requote"));
        assert_eq!(order.traded_at_price_qty, 1.5);
        assert_eq!(order.min_queue_ahead, Some(1.5));
        assert_eq!(order.terminal_ts, Some(41 * MS));
        assert_eq!(e.question, Question::WhyNotFilled);
        // Trades printed at the price, yet a cancelled order keeps the cancel verdict.
        assert_eq!(e.verdict, Verdict::NotFilledCanceled);
        assert_eq!(
            e.headline,
            "Not filled: cancelled with the modeled 1.500 still queued ahead, after 1.500 traded \
             at 10000.0."
        );

        assert!(step_with(&e, "decided to cancel the order").statement.contains("requote"));
        let race = step_with(&e, "Latency race");
        assert_eq!(race.basis, Basis::Modeled);
        assert_eq!(race.event_seqs, vec![sent, racing, canceled]);
        assert!(race.statement.contains("1.000 traded at 10000.0 in 1 trade print"));
        assert!(race.statement.contains("moved from 2.500 to 1.500"));

        let state = step_with(&e, "When the cancel took effect the modeled quantity ahead was 1.500");
        assert_eq!(state.event_seqs, vec![queued, racing, canceled]);
        assert!(state.statement.contains("it was 3.000 at entry"));

        assert_eq!(e.latency.cancel_entry_latency_ns, Some(MS));
        assert_eq!(e.latency.resting_time_ns, Some(30 * MS));
        assert_eq!(e.latency.time_to_first_fill_ns, None);
        assert_eq!(e.queue_path.len(), 3);
    }

    #[test]
    fn cancel_that_loses_the_race_to_a_fill() {
        let mut t = Tape::new();
        t.rest(50 * MS, 512, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(0.5));
        t.cancel_intent(60 * MS, 512, "requote");
        let sent = t.sent(60 * MS, 512, ReqKind::Cancel, MS);
        let trade = t.trade(60 * MS + 500_000, 512, 1.0, 0.5, -0.5);
        let fill = t.fill(60 * MS + 500_000, 512, 20_000, 1.0, true, 1.0, 0.0);
        t.resp(60 * MS + 500_000, 512, OrderStatus::Filled, 0.0);
        let rejected = t.rejected_cancel(61 * MS, 512);

        let (order, e) = explain(&t, 512, &config());
        assert_eq!(order.outcome, OrderOutcome::Filled);
        assert!(order.cancel_rejected);
        assert_eq!(e.verdict, Verdict::FilledQueueExhausted);

        let race = step_with(&e, "Latency race");
        assert_eq!(race.event_seqs, vec![sent, trade, fill, rejected]);
        assert!(race.statement.contains("500 µs after the cancel was sent"));
        assert!(race.statement.contains("the cancel lost the race"));
        let gone = step_with(&e, "the exchange rejected the cancel");
        assert!(gone.statement.contains("it had become filled at t=0.0605s"));
    }

    #[test]
    fn risk_blocked() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc);
        let blocked = t.push(
            10 * MS,
            Some(256),
            FlightBody::RiskBlocked {
                tick_no: 1,
                rule: RiskRule::MaxPosition,
                detail: "position 9.5 + 1 exceeds 10".into(),
            },
        );

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::RiskBlocked);
        assert_eq!(order.exch_arrival_ts, None);
        assert_eq!(e.verdict, Verdict::NotFilledRiskBlocked);
        assert_eq!(
            e.headline,
            "Not filled: the max_position risk rule blocked the order before it was sent."
        );
        let rule = step_with(&e, "The pre-trade risk rule max_position");
        assert_eq!(rule.basis, Basis::Derived);
        assert_eq!(rule.event_seqs, vec![blocked]);
        assert!(rule.statement.contains("position 9.5 + 1 exceeds 10"));
        // Nothing reached an exchange model, so there is no modeled or unobservable claim.
        assert!(e
            .steps
            .iter()
            .all(|s| !matches!(s.basis, Basis::Modeled | Basis::Unobservable)));
        assert!(e.queue_path.is_empty());
        assert_eq!(e.latency.entry_latency_ns, None);
        assert_eq!(e.latency.feed_staleness_ns, Some(MS));
    }

    #[test]
    fn refused_by_the_engine() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc);
        let refused = t.push(
            10 * MS,
            Some(256),
            FlightBody::RequestRefused {
                req: ReqKind::New,
                error: "order id already exists".into(),
            },
        );

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::Refused);
        assert_eq!(e.verdict, Verdict::NotFilledRefused);
        assert_eq!(
            e.headline,
            "Not filled: the engine refused the request and nothing was sent."
        );
        let step = step_with(&e, "order id already exists");
        assert_eq!(step.event_seqs, vec![refused]);
    }

    #[test]
    fn in_flight_at_the_end_of_the_data() {
        let mut t = Tape::new();
        t.intent(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc);
        let sent = t.sent(10 * MS, 256, ReqKind::New, MS);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::InFlightAtEnd);
        assert_eq!(order.exch_arrival_ts, None);
        assert_eq!(e.verdict, Verdict::NotFilledDataEndedInFlight);
        assert_eq!(
            e.headline,
            "Not filled: the dataset ended before the request reached the exchange."
        );
        let request = step_with(&e, "the dataset ended before the request arrived");
        assert_eq!(request.basis, Basis::Modeled);
        assert_eq!(request.event_seqs, vec![sent]);
        assert!(request.statement.contains("constant: 1 ms entry, 1 ms response"));
        assert!(e.steps.iter().any(|s| s.basis == Basis::Unobservable));
        assert_eq!(e.latency.entry_latency_ns, Some(MS));
        assert_eq!(e.latency.response_latency_ns, None);
    }

    #[test]
    fn working_at_end_with_trades_at_the_price() {
        let mut t = Tape::new();
        let queued = t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));
        let trade = t.trade(20 * MS, 256, 0.5, 2.0, 1.5);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::WorkingAtEnd);
        assert_eq!(order.traded_at_price_qty, 0.5);
        assert_eq!(order.terminal_ts, None);
        assert_eq!(e.verdict, Verdict::NotFilledQueueNotReached);
        assert_eq!(
            e.headline,
            "Not filled: 0.500 traded at 10000.0 while the order rested, leaving 1.500 of the \
             modeled 2.000 queued ahead when the dataset ended."
        );
        let end = step_with(&e, "The dataset ended with the order still resting and unfilled");
        assert_eq!(end.basis, Basis::Modeled);
        assert_eq!(end.event_seqs, vec![queued, trade]);
        assert!(end.statement.contains("the estimate went from 2.000 to 1.500"));
        assert_eq!(e.latency.resting_time_ns, None);
    }

    #[test]
    fn working_at_end_without_trades_at_the_price() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.outcome, OrderOutcome::WorkingAtEnd);
        assert_eq!(order.traded_at_price_qty, 0.0);
        assert_eq!(e.verdict, Verdict::NotFilledNoTradeAtPrice);
        assert_eq!(
            e.headline,
            "Not filled: no trade printed at 10000.0 while the order rested, and it was still \
             working when the dataset ended."
        );
        step_with(&e, "The modeled quantity ahead stayed at 2.000");
        assert_eq!(e.queue_path.len(), 1);
    }

    #[test]
    fn unknown_queue_state_is_reported_as_unknown() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, None);

        let (order, e) = explain(&t, 256, &config());
        assert_eq!(order.queue_ahead_at_entry, None);
        assert_eq!(e.verdict, Verdict::NotFilledNoTradeAtPrice);
        step_with(&e, "it is not treated as zero");
        step_with(&e, "The modeled quantity ahead stayed at an unknown quantity");
        assert!(e.queue_path.is_empty());
    }

    #[test]
    fn partial_fill_then_cancel() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Sell, 20_002, 2.0, Tif::Gtc, Some(0.5));
        let trade = t.trade(20 * MS, 256, 1.0, 0.5, -0.5);
        let fill = t.fill(20 * MS, 256, 20_002, 0.5, true, 0.5, 1.5);
        let resp = t.resp(20 * MS, 256, OrderStatus::PartiallyFilled, 1.5);
        t.local(21 * MS, 256, OrderStatus::PartiallyFilled, 0.5, 1.5, 20 * MS);
        t.cancel_intent(40 * MS, 256, "inventory limit");
        t.sent(40 * MS, 256, ReqKind::Cancel, MS);
        let canceled = t.resp(41 * MS, 256, OrderStatus::Canceled, 1.5);

        let cfg = partial_fill_config();
        let (order, e) = explain(&t, 256, &cfg);
        assert_eq!(order.outcome, OrderOutcome::PartiallyFilled);
        assert_eq!(order.filled_qty, 0.5);
        assert_eq!(order.maker_qty, 0.5);
        assert_eq!(order.final_status, OrderStatus::Canceled);
        assert_eq!(order.terminal_ts, Some(41 * MS));
        assert_eq!(e.question, Question::WhyPartiallyFilled);
        // The verdict explains the remainder; the steps cover the filled part too.
        assert_eq!(e.verdict, Verdict::NotFilledCanceled);
        assert_eq!(
            e.headline,
            "Partially filled (0.500 of 2.000): the remaining 1.500 was cancelled while still \
             resting."
        );

        let mechanism = step_with(&e, "it exhausted the modeled queue ahead");
        assert_eq!(mechanism.event_seqs, vec![trade, fill, resp]);
        assert!(mechanism.statement.contains("partial-fill exchange model"));
        assert!(mechanism.statement.contains("1.500 of the order stayed resting"));
        let removed = step_with(&e, "removing the order with 1.500 unfilled");
        assert!(removed.event_seqs.contains(&canceled));
        step_with(&e, "no fill was won or lost to the cancel's latency");
        step_with(&e, "The strategy learned of its first fill at t=0.021s, 1 ms after");
        let fees = step_with(&e, "The fee model charged 0.5");
        assert_eq!(fees.basis, Basis::Modeled);
        assert_eq!(fees.event_seqs, vec![fill]);
        assert_eq!(e.latency.time_to_first_fill_ns, Some(9 * MS));
    }

    #[test]
    fn staleness_basis_follows_the_provenance_of_receipt_times() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));
        let cfg = config();

        let (_, measured) = explain_with(&t, 256, &cfg, LocalTsProvenance::Measured);
        assert_eq!(measured.steps[1].basis, Basis::Observed);
        assert!(measured.steps[1].statement.contains("a feed latency of 2 ms"));

        let (_, synthetic) = explain_with(&t, 256, &cfg, LocalTsProvenance::Synthetic);
        assert_eq!(synthetic.steps[1].basis, Basis::Modeled);
        assert!(synthetic.steps[1].statement.contains("modeled, not measured"));

        let (_, assumed) = explain_with(&t, 256, &cfg, LocalTsProvenance::Assumed);
        assert_eq!(assumed.steps[1].basis, Basis::Modeled);
        assert!(assumed.steps[1].statement.contains("not measurements"));

        // A configured offset turns even recorded receipt times into model output.
        let mut shifted = config();
        shifted.execution.feed_latency_offset_ns = 250_000;
        let (_, offset) = explain_with(&t, 256, &shifted, LocalTsProvenance::Measured);
        assert_eq!(offset.steps[1].basis, Basis::Modeled);
        assert!(offset.steps[1].statement.contains("offset of 250 µs"));
    }

    #[test]
    fn depth_decreases_name_the_configured_queue_model() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));
        t.depth(20 * MS, 256, 2.0, 1.0, 2.0, 1.4);
        t.trade(30 * MS, 256, 0.5, 1.4, 0.9);
        let mut cfg = config();
        cfg.execution.queue_model = QueueModelConfig::PowerProb3 { n: 3.0 };

        let (_, e) = explain(&t, 256, &cfg);
        assert_eq!(e.verdict, Verdict::NotFilledQueueNotReached);
        let moved = step_with(&e, "the configured queue model (power_prob3(n=3)) decides it");
        assert_eq!(moved.basis, Basis::Modeled);
        assert!(moved
            .statement
            .contains("fell by 0.500 through those trades and by 0.600 through 1 change"));
        assert!(moved.statement.contains("1 - (front / (front + back))^3"));
        assert!(moved
            .statement
            .contains("The smallest modeled quantity ahead reached was 0.900"));
    }

    #[test]
    fn truncation_adds_its_own_caveat() {
        let mut t = Tape::new();
        t.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));
        t.push(20 * MS, Some(256), FlightBody::QueueTruncated { dropped: 12 });

        let (_, e) = explain(&t, 256, &config());
        assert_eq!(e.caveats.len(), 5);
        assert!(e.caveats[4].contains("12 updates"));
        // The grammar only truncates depth updates, never evidence of trades.
        assert!(step_with(&e, "No trade printed at 10000.0")
            .statement
            .contains("Trade-driven updates are always retained"));
    }
}
