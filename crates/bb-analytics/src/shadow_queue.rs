//! Shadow orders through a reconstructed L3 FIFO queue versus the Level-2 queue models.
//!
//! The pre-registered experiment of docs/research/REAL_DATA_VALIDATION_PLAN.md §5, documented in
//! docs/research/QUEUE_SHADOW_EXPERIMENT.md. One pass over an ordered ITCH message stream (file
//! order, never re-sorted, also inside a nanosecond) keeps every stock's L3 book in
//! [`OrderBooks`] and, for the selected stocks, places hypothetical 100-share orders at the back
//! of the displayed best queue. Each shadow order is followed twice on the same messages:
//!
//! * **L3 reference.** The displayed orders ahead of it at entry are the live orders at its
//!   price; executions and cancellations of exactly those orders reduce the shares ahead. It fills
//!   when an execution reaches a displayed order at or behind it in priority (queue path or
//!   trade-through), or when the opposite best price reaches its price.
//! * **L2 models.** The L2 projection of the same messages (one trade row per displayed printable
//!   execution, before the depth row it caused; one depth row per changed level, as
//!   `bb_data::itch::l2`) drives the queue formulas of hftbacktest 0.9.4 with the fill rules of its
//!   `NoPartialFillExchange`: `front` starts at the displayed quantity, a trade at the price calls
//!   `trade` then `is_filled`, a better-priced trade or a crossing opposite best fills at once.
//!
//! Shadow orders are not added to the book (no market impact) and nobody reacts to them. Their
//! fills are counterfactual model outputs, never recorded executions. The engine reads only
//! messages already applied: a decision at a message uses nothing later in the stream.
use std::collections::HashMap;

use bb_data::itch::{
    book::{Applied, Change, OrderBooks, PrintKind},
    decode::{Message, Side},
};
use serde::{Deserialize, Serialize};

use crate::{
    Value,
    execution::{ConfidenceBand, SurvivalObservation, kaplan_meier},
};

pub const SCHEMA: &str = "blackbox.queue-shadow";
pub const SCHEMA_VERSION: u32 = 1;
/// Implementation revision, deliberately separate from the frozen experimental design.
pub const ENGINE_REVISION: &str = "inclusive-deadlines-v2";
/// Name of the L3 reference in cell keys.
pub const L3_REFERENCE: &str = "l3_shadow";
/// Two-sided 95% normal quantile.
const Z95: f64 = 1.959_963_984_540_054;
const NS_PER_S: u64 = 1_000_000_000;

/// Rules of the engine, copied into every design (and therefore into its hash).
pub const RULES: &[&str] = &[
    "Messages are processed in stream (file) order; messages sharing a timestamp are never reordered.",
    "Shadow k is scheduled at window_start + k * interval for every such time before window_end; \
     even k is a buy, odd k a sell. Its arrival is the scheduled time plus the latency L.",
    "A shadow arriving at T is sequenced after every message stamped before T and before the first \
     message stamped at or after T; arrivals at or after window_end are not placed.",
    "At arrival it joins the back of the displayed best queue of its side at that price; it is not \
     placed when that side is empty or the book is locked or crossed at that moment.",
    "No impact: the shadow is never added to the book and no message is changed because of it.",
    "L3 reference: shares ahead are the displayed shares at the price at arrival; executions, \
     cancellations, deletes and replaces of those orders reduce them. A replace loses priority: its \
     new order is behind.",
    "L3 fill: an execution (E or C, printable or not) of a displayed order on the shadow's side at \
     its price that was not ahead of it (queue), or at a worse price (trade-through); or the \
     opposite best reaching its price after a message (book cross).",
    "L2 projection: a displayed printable execution emits a trade row (aggressor opposite the \
     resting side) before its depth row; each changed level emits a depth row with the new total; \
     non-printable executions, non-cross trades (P) and crosses (Q) emit no sided row and are \
     ignored, as by hftbacktest's NoPartialFillExchange.",
    "L2 models: hftbacktest 0.9.4 formulas (docs/QUEUE_MODELS.md), lot size 1 share. front starts at \
     the displayed quantity at arrival; a trade at the price calls trade then is_filled \
     (fill when round(-front) > 0); a trade at a better price fills (trade-through); a depth row at \
     the price calls depth(prev, new); the opposite best reaching the price fills (book cross).",
    "A fill is the first counterfactual execution of any part of the shadow (first touch).",
    "Each shadow is followed until every path filled, until its longest horizon or checkpoint \
     passed, or until the end of tracking (window_end, or the last message when the stream ends \
     before it). The L3 cancellation counters stop at the L3 fill.",
    "Fill fraction at horizon tau uses only shadows with arrival + tau <= end of tracking, \
     whatever their outcome; Kaplan-Meier uses every placed shadow with right censoring.",
];

/// The hftbacktest queue model families BlackBox exposes (docs/QUEUE_MODELS.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    RiskAdverse,
    LogProb,
    LogProb2,
    PowerProb,
    PowerProb2,
    PowerProb3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub family: Family,
    /// Exponent of the power families; `None` otherwise.
    pub n: Option<u32>,
}

impl ModelSpec {
    pub const fn new(family: Family, n: Option<u32>) -> Self {
        Self { family, n }
    }

    /// Stable key: the BlackBox config name, with `_n<k>` for the power families.
    pub fn id(&self) -> String {
        let base = match self.family {
            Family::RiskAdverse => "risk_adverse",
            Family::LogProb => "log_prob",
            Family::LogProb2 => "log_prob2",
            Family::PowerProb => "power_prob",
            Family::PowerProb2 => "power_prob2",
            Family::PowerProb3 => "power_prob3",
        };
        match self.n {
            Some(n) => format!("{base}_n{n}"),
            None => base.to_string(),
        }
    }

    fn power(&self) -> f64 {
        f64::from(self.n.unwrap_or(1))
    }

    /// `Probability::prob(front, back)` of the matching hftbacktest function: the share of a
    /// decrease assumed to have happened behind the order.
    pub fn prob(&self, front: f64, back: f64) -> f64 {
        let n = self.power();
        let ln = |x: f64| (1.0 + x).ln();
        match self.family {
            Family::RiskAdverse => 1.0,
            Family::LogProb => ln(back) / (ln(back) + ln(front)),
            Family::LogProb2 => ln(back) / ln(back + front),
            Family::PowerProb => back.powf(n) / (back.powf(n) + front.powf(n)),
            Family::PowerProb2 => back.powf(n) / (back + front).powf(n),
            Family::PowerProb3 => 1.0 - (front / (front + back)).powf(n),
        }
    }

    /// The models of the pre-registered design, in report order.
    pub fn design_set() -> Vec<ModelSpec> {
        let mut out = vec![
            Self::new(Family::RiskAdverse, None),
            Self::new(Family::LogProb, None),
            Self::new(Family::LogProb2, None),
        ];
        for family in [Family::PowerProb, Family::PowerProb2, Family::PowerProb3] {
            for n in [2, 3] {
                out.push(Self::new(family, Some(n)));
            }
        }
        out
    }
}

/// Queue state of one order under one L2 model: hftbacktest's `front_q_qty` and
/// `cum_trade_qty` (the latter unused by RiskAdverse).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueueState {
    pub front: f64,
    pub cum_trade: f64,
}

impl QueueState {
    /// `QueueModel::new_order`: the order joins behind the displayed quantity.
    pub fn new(displayed: f64) -> Self {
        Self {
            front: displayed,
            cum_trade: 0.0,
        }
    }

    /// `QueueModel::trade`.
    pub fn trade(&mut self, model: &ModelSpec, qty: f64) {
        self.front -= qty;
        if model.family != Family::RiskAdverse {
            self.cum_trade += qty;
        }
    }

    /// `QueueModel::depth`.
    pub fn depth(&mut self, model: &ModelSpec, prev: f64, new: f64) {
        if model.family == Family::RiskAdverse {
            self.front = self.front.min(new);
            return;
        }
        let chg = prev - new - self.cum_trade;
        self.cum_trade = 0.0;
        if chg < 0.0 {
            self.front = self.front.min(new);
            return;
        }
        let front = self.front;
        let back = prev - front;
        let mut prob = model.prob(front, back);
        if prob.is_infinite() {
            prob = 1.0;
        }
        let est = front - (1.0 - prob) * chg + (back - prob * chg).min(0.0);
        self.front = est.min(new);
    }

    /// `QueueModel::is_filled` with a lot of `lot`: executable quantity, resetting `front` to 0.
    pub fn is_filled(&mut self, lot: f64) -> f64 {
        let exec = (-self.front / lot).round() as i64;
        if exec > 0 {
            self.front = 0.0;
            exec as f64 * lot
        } else {
            0.0
        }
    }
}

/// The frozen experimental design. Its canonical JSON is hashed into the design SHA-256.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Design {
    /// Exchange clock, nanoseconds since midnight (ITCH timestamps).
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    pub interval_ns: u64,
    pub shares: u32,
    pub lot_size_shares: u32,
    /// Latency assumptions L (swept, never fitted).
    pub latencies_ns: Vec<u64>,
    /// Censoring horizons tau.
    pub horizons_ns: Vec<u64>,
    /// Times after arrival at which `front` and the L3 shares ahead are compared.
    pub checkpoints_ns: Vec<u64>,
    pub models: Vec<ModelSpec>,
    pub rules: Vec<String>,
}

impl Design {
    /// The pre-registered design: 09:30:00–09:39:00 ET, 1 s, 100 shares, L ∈ {0, 50 µs, 500 µs},
    /// τ ∈ {1, 5, 30 s}, checkpoints {0.1, 1, 5, 30 s}, every model of [`ModelSpec::design_set`].
    pub fn preregistered() -> Self {
        let hour = 3_600 * NS_PER_S;
        let minute = 60 * NS_PER_S;
        Self {
            window_start_ns: 9 * hour + 30 * minute,
            window_end_ns: 9 * hour + 39 * minute,
            interval_ns: NS_PER_S,
            shares: 100,
            lot_size_shares: 1,
            latencies_ns: vec![0, 50_000, 500_000],
            horizons_ns: vec![NS_PER_S, 5 * NS_PER_S, 30 * NS_PER_S],
            checkpoints_ns: vec![NS_PER_S / 10, NS_PER_S, 5 * NS_PER_S, 30 * NS_PER_S],
            models: ModelSpec::design_set(),
            rules: RULES.iter().map(|r| r.to_string()).collect(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.window_end_ns <= self.window_start_ns {
            return Err("window_end must be after window_start".into());
        }
        if self.interval_ns == 0 || self.shares == 0 || self.lot_size_shares == 0 {
            return Err("interval, shares and lot size must be positive".into());
        }
        if self.latencies_ns.is_empty() || self.horizons_ns.is_empty() || self.models.is_empty() {
            return Err("latencies, horizons and models must not be empty".into());
        }
        if self.horizons_ns.contains(&0) || self.checkpoints_ns.contains(&0) {
            return Err("horizons and checkpoints must be positive".into());
        }
        Ok(())
    }

    fn longest_ns(&self) -> u64 {
        let h = self.horizons_ns.iter().max().copied().unwrap_or(0);
        let c = self.checkpoints_ns.iter().max().copied().unwrap_or(0);
        h.max(c)
    }
}

/// Why a path filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillCause {
    Queue,
    TradeThrough,
    BookCross,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub elapsed_ns: u64,
    pub cause: FillCause,
}

/// State after all messages stamped at or before the checkpoint; `None` for a filled path.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckpointObs {
    pub l3_ahead: Option<u64>,
    pub fronts: Vec<Option<f64>>,
}

/// Everything recorded about one placed shadow order. Stays in memory or local files only.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowOutcome {
    pub symbol: usize,
    pub k: u64,
    pub latency: usize,
    pub side: Side,
    pub price: u32,
    pub arrival_ns: u64,
    pub initial_ahead: u64,
    pub l3: Option<Fill>,
    pub models: Vec<Option<Fill>>,
    /// Nanoseconds after arrival through which the shadow was observed when unfilled.
    pub observed_ns: u64,
    /// Per checkpoint; `None` when tracking ended before it.
    pub checkpoints: Vec<Option<CheckpointObs>>,
    /// Per horizon: L3 cancelled shares at the shadow's price (ahead, behind) up to
    /// min(L3 fill, horizon, end of tracking).
    pub cancelled: Vec<(u64, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolSummary {
    pub symbol: String,
    /// Locate code from the stock directory; `None` when the symbol never appeared.
    pub locate: Option<u16>,
    pub scheduled: u64,
    pub placed: u64,
    pub skipped_empty_side: u64,
    pub skipped_locked_or_crossed: u64,
    /// Scheduled arrivals at or after the end of tracking (never placed).
    pub not_arrived: u64,
}

#[derive(Debug)]
struct Shadow {
    out: ShadowOutcome,
    ahead: HashMap<u64, u32>,
    ahead_shares: u64,
    level_qty: f64,
    states: Vec<QueueState>,
    milestone: usize,
    cancel_ahead: u64,
    cancel_behind: u64,
}

impl Shadow {
    fn done(&self) -> bool {
        self.out.l3.is_some() && self.out.models.iter().all(Option::is_some)
    }

    fn checkpoint(&self) -> CheckpointObs {
        CheckpointObs {
            l3_ahead: self.out.l3.is_none().then_some(self.ahead_shares),
            fronts: self
                .states
                .iter()
                .zip(&self.out.models)
                .map(|(s, f)| f.is_none().then_some(s.front))
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Milestone {
    Checkpoint(usize),
    Horizon(usize),
}

#[derive(Debug, Default)]
struct SymbolState {
    summary: SymbolSummary,
    /// Next schedule index per latency.
    next_k: Vec<u64>,
    active: Vec<Shadow>,
    /// Earliest time at which an arrival or a milestone of this symbol is due.
    next_due: u64,
}

/// Pre-apply view of the order a message names.
#[derive(Clone, Copy, Debug)]
enum Touch {
    Execution {
        side: Side,
        price: u32,
        order: u64,
        shares: u32,
    },
    Removal {
        side: Side,
        price: u32,
        order: u64,
        shares: u32,
    },
}

/// Streaming engine. Feed [`on_message`](Self::on_message) every message in stream order, then
/// call [`finish`](Self::finish).
#[derive(Debug)]
pub struct ShadowEngine {
    design: Design,
    books: OrderBooks,
    symbols: Vec<SymbolState>,
    locates: HashMap<u16, usize>,
    milestones: Vec<(u64, Milestone)>,
    done: Vec<ShadowOutcome>,
    last_ts: Option<u64>,
    ended: bool,
    messages: u64,
}

impl ShadowEngine {
    pub fn new(design: Design, symbols: &[String]) -> Result<Self, String> {
        design.validate()?;
        let mut milestones: Vec<(u64, Milestone)> = design
            .checkpoints_ns
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, Milestone::Checkpoint(i)))
            .chain(
                design
                    .horizons_ns
                    .iter()
                    .enumerate()
                    .map(|(i, &h)| (h, Milestone::Horizon(i))),
            )
            .collect();
        milestones.sort_by_key(|m| m.0);
        let lat = design.latencies_ns.len();
        Ok(Self {
            symbols: symbols
                .iter()
                .map(|s| SymbolState {
                    summary: SymbolSummary {
                        symbol: s.clone(),
                        ..SymbolSummary::default()
                    },
                    next_k: vec![0; lat],
                    active: Vec::new(),
                    next_due: 0,
                })
                .collect(),
            design,
            books: OrderBooks::new(),
            locates: HashMap::new(),
            milestones,
            done: Vec::new(),
            last_ts: None,
            ended: false,
            messages: 0,
        })
    }

    /// True once a message at or after the window end arrived; later messages are ignored.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Messages applied to the book (those before the window end).
    pub fn messages(&self) -> u64 {
        self.messages
    }

    pub fn books(&self) -> &OrderBooks {
        &self.books
    }

    pub fn on_message(&mut self, msg: &Message<'_>) {
        if self.ended {
            return;
        }
        let Some(header) = msg.header() else {
            return;
        };
        let ts = header.timestamp_ns;
        if ts >= self.design.window_end_ns {
            self.ended = true;
            return;
        }
        self.messages += 1;
        self.last_ts = Some(ts);
        for s in 0..self.symbols.len() {
            self.advance(s, ts, false);
        }
        if let Message::StockDirectory(m) = msg {
            let name = m.stock.trimmed();
            if let Some(i) = self.symbols.iter().position(|s| s.summary.symbol == name) {
                self.locates.insert(m.header.stock_locate, i);
                self.symbols[i].summary.locate = Some(m.header.stock_locate);
            }
        }
        let tracked = self
            .locates
            .get(&header.stock_locate)
            .copied()
            .filter(|&i| !self.symbols[i].active.is_empty());
        let touch = tracked.and_then(|_| self.touch(msg));
        if matches!(self.books.apply(msg), Applied::Quarantined(_)) {
            return;
        }
        if let Some(i) = tracked {
            self.react(i, header.stock_locate, ts, touch);
        }
    }

    /// Ends tracking and returns every placed shadow in (symbol, latency, k) order.
    pub fn finish(mut self) -> ShadowRun {
        let end = if self.ended {
            self.design.window_end_ns
        } else {
            self.last_ts.unwrap_or(self.design.window_start_ns)
        };
        let d = &self.design;
        let per_latency = (d.window_end_ns - d.window_start_ns).div_ceil(d.interval_ns);
        let latencies = d.latencies_ns.len() as u64;
        for s in 0..self.symbols.len() {
            self.advance(s, end, true);
            for shadow in std::mem::take(&mut self.symbols[s].active) {
                let observed = end.saturating_sub(shadow.out.arrival_ns);
                self.retire(shadow, observed, Some(end));
            }
            let state = &mut self.symbols[s];
            state.summary.scheduled = per_latency * latencies;
            state.summary.not_arrived = state
                .next_k
                .iter()
                .map(|&next| per_latency - next.min(per_latency))
                .sum();
        }
        let mut shadows = self.done;
        shadows.sort_by_key(|o| (o.symbol, o.latency, o.k));
        ShadowRun {
            tracking_end_ns: end,
            stream_reached_window_end: self.ended,
            messages: self.messages,
            symbols: self.symbols.into_iter().map(|s| s.summary).collect(),
            shadows,
        }
    }

    /// Arrivals precede messages stamped at their arrival; milestones follow every message
    /// stamped at their boundary. `finishing` flushes milestones at the last observed timestamp.
    /// Does nothing before the symbol's next due time, so idle messages cost one comparison.
    fn advance(&mut self, s: usize, ts: u64, finishing: bool) {
        if !finishing && ts < self.symbols[s].next_due {
            return;
        }
        let longest = self.design.longest_ns();
        for li in 0..self.design.latencies_ns.len() {
            while let Some(arrival) = self.arrival(s, li).filter(|&a| a <= ts) {
                let k = self.symbols[s].next_k[li];
                self.symbols[s].next_k[li] = k + 1;
                let side = if k.is_multiple_of(2) {
                    Side::Buy
                } else {
                    Side::Sell
                };
                self.place(s, k, li, side, arrival);
            }
        }
        let mut i = 0;
        while i < self.symbols[s].active.len() {
            let shadow = &mut self.symbols[s].active[i];
            while let Some(&(offset, m)) = self.milestones.get(shadow.milestone) {
                let boundary = shadow.out.arrival_ns + offset;
                if boundary > ts || (!finishing && boundary == ts) {
                    break;
                }
                record(shadow, m);
                shadow.milestone += 1;
            }
            let deadline = shadow.out.arrival_ns + longest;
            if deadline < ts || (finishing && deadline == ts) {
                let shadow = self.symbols[s].active.swap_remove(i);
                self.retire(shadow, longest, None);
            } else {
                i += 1;
            }
        }
        let next_arrival = (0..self.design.latencies_ns.len())
            .filter_map(|li| self.arrival(s, li))
            .min();
        let next_milestone = self.symbols[s]
            .active
            .iter()
            .map(|sh| {
                let offset = self.milestones.get(sh.milestone).map_or(longest, |m| m.0);
                // Before-message processing is due only after the entire boundary timestamp.
                (sh.out.arrival_ns + offset.min(longest)).saturating_add(1)
            })
            .min();
        self.symbols[s].next_due = next_arrival
            .into_iter()
            .chain(next_milestone)
            .min()
            .unwrap_or(u64::MAX);
    }

    /// Arrival time of the next scheduled shadow of latency `li`, if it falls inside the window.
    fn arrival(&self, s: usize, li: usize) -> Option<u64> {
        let d = &self.design;
        let scheduled = d.window_start_ns + self.symbols[s].next_k[li] * d.interval_ns;
        let arrival = scheduled + d.latencies_ns[li];
        (scheduled < d.window_end_ns && arrival < d.window_end_ns).then_some(arrival)
    }

    fn place(&mut self, s: usize, k: u64, latency: usize, side: Side, arrival: u64) {
        let best = self.symbols[s].summary.locate.and_then(|locate| {
            let best = match side {
                Side::Buy => self.books.best_bid(locate),
                Side::Sell => self.books.best_ask(locate),
            }?;
            Some((locate, best, self.opposite_best(locate, side)))
        });
        let summary = &mut self.symbols[s].summary;
        let Some((locate, best, opposite)) = best else {
            summary.skipped_empty_side += 1;
            return;
        };
        let price = best.price.0;
        if crosses(side, price, opposite) {
            summary.skipped_locked_or_crossed += 1;
            return;
        }
        summary.placed += 1;
        let ahead: HashMap<u64, u32> = self
            .books
            .level_queue(locate, side, best.price)
            .iter()
            .map(|q| (q.order_ref, q.shares))
            .collect();
        let displayed = best.shares as f64;
        let models = self.design.models.len();
        self.symbols[s].active.push(Shadow {
            out: ShadowOutcome {
                symbol: s,
                k,
                latency,
                side,
                price,
                arrival_ns: arrival,
                initial_ahead: best.shares,
                l3: None,
                models: vec![None; models],
                observed_ns: 0,
                checkpoints: vec![None; self.design.checkpoints_ns.len()],
                cancelled: vec![(0, 0); self.design.horizons_ns.len()],
            },
            ahead,
            ahead_shares: best.shares,
            level_qty: displayed,
            states: vec![QueueState::new(displayed); models],
            milestone: 0,
            cancel_ahead: 0,
            cancel_behind: 0,
        });
    }

    fn opposite_best(&self, locate: u16, side: Side) -> Option<u32> {
        match side {
            Side::Buy => self.books.best_ask(locate),
            Side::Sell => self.books.best_bid(locate),
        }
        .map(|l| l.price.0)
    }

    fn touch(&self, msg: &Message<'_>) -> Option<Touch> {
        let (order, shares, exec) = match msg {
            Message::OrderExecuted(m) => (m.order_ref, Some(m.shares), true),
            Message::OrderExecutedWithPrice(m) => (m.order_ref, Some(m.shares), true),
            Message::OrderCancel(m) => (m.order_ref, Some(m.shares), false),
            Message::OrderDelete(m) => (m.order_ref, None, false),
            Message::OrderReplace(m) => (m.original_ref, None, false),
            _ => return None,
        };
        let view = self.books.order(order)?;
        let shares = shares.unwrap_or(view.shares);
        let (side, price) = (view.side, view.price.0);
        Some(if exec {
            Touch::Execution {
                side,
                price,
                order,
                shares,
            }
        } else {
            Touch::Removal {
                side,
                price,
                order,
                shares,
            }
        })
    }

    fn react(&mut self, s: usize, locate: u16, ts: u64, touch: Option<Touch>) {
        let lot = f64::from(self.design.lot_size_shares);
        let bid = self.books.best_bid(locate).map(|l| l.price.0);
        let ask = self.books.best_ask(locate).map(|l| l.price.0);
        let changes = self.books.changes();
        let models = &self.design.models;
        let mut finished = Vec::new();
        let mut i = 0;
        while i < self.symbols[s].active.len() {
            let sh = &mut self.symbols[s].active[i];
            let elapsed = ts - sh.out.arrival_ns;
            let (side, price) = (sh.out.side, sh.out.price);
            // L3 reference.
            if sh.out.l3.is_none() {
                match touch {
                    Some(Touch::Execution {
                        side: o_side,
                        price: o_price,
                        order,
                        shares,
                    }) if o_side == side => {
                        if let Some(left) = sh.ahead.get_mut(&order) {
                            *left -= shares;
                            if *left == 0 {
                                sh.ahead.remove(&order);
                            }
                            sh.ahead_shares -= u64::from(shares);
                        } else if o_price == price {
                            sh.out.l3 = Some(Fill {
                                elapsed_ns: elapsed,
                                cause: FillCause::Queue,
                            });
                        } else if worse(side, o_price, price) {
                            sh.out.l3 = Some(Fill {
                                elapsed_ns: elapsed,
                                cause: FillCause::TradeThrough,
                            });
                        }
                    }
                    Some(Touch::Removal {
                        side: o_side,
                        price: o_price,
                        order,
                        shares,
                    }) if o_side == side && o_price == price => {
                        if let Some(left) = sh.ahead.get_mut(&order) {
                            *left -= shares;
                            if *left == 0 {
                                sh.ahead.remove(&order);
                            }
                            sh.ahead_shares -= u64::from(shares);
                            sh.cancel_ahead += u64::from(shares);
                        } else {
                            sh.cancel_behind += u64::from(shares);
                        }
                    }
                    _ => {}
                }
            }
            // L2 projection through each model, rows in emission order.
            for change in changes {
                match *change {
                    Change::Print(p) => {
                        let PrintKind::Execution {
                            resting,
                            printable: true,
                        } = p.kind
                        else {
                            continue;
                        };
                        if resting != side {
                            continue;
                        }
                        for (m, (state, fill)) in models
                            .iter()
                            .zip(sh.states.iter_mut().zip(&mut sh.out.models))
                        {
                            if fill.is_some() {
                                continue;
                            }
                            if worse(side, p.price.0, price) {
                                *fill = Some(Fill {
                                    elapsed_ns: elapsed,
                                    cause: FillCause::TradeThrough,
                                });
                            } else if p.price.0 == price {
                                state.trade(m, p.shares as f64);
                                if state.is_filled(lot) > 0.0 {
                                    *fill = Some(Fill {
                                        elapsed_ns: elapsed,
                                        cause: FillCause::Queue,
                                    });
                                }
                            }
                        }
                    }
                    Change::Level {
                        side: l_side,
                        price: l_price,
                        shares,
                        ..
                    } if l_side == side && l_price.0 == price => {
                        let new = shares as f64;
                        for (m, (state, fill)) in
                            models.iter().zip(sh.states.iter_mut().zip(&sh.out.models))
                        {
                            if fill.is_none() {
                                state.depth(m, sh.level_qty, new);
                            }
                        }
                        sh.level_qty = new;
                    }
                    Change::Level { .. } => {}
                }
            }
            // Book cross, identical for every path.
            let opposite = match side {
                Side::Buy => ask,
                Side::Sell => bid,
            };
            if crosses(side, price, opposite) {
                let cross = Some(Fill {
                    elapsed_ns: elapsed,
                    cause: FillCause::BookCross,
                });
                if sh.out.l3.is_none() {
                    sh.out.l3 = cross;
                }
                for fill in sh.out.models.iter_mut().filter(|f| f.is_none()) {
                    *fill = cross;
                }
            }
            if sh.done() {
                finished.push(self.symbols[s].active.swap_remove(i));
            } else {
                i += 1;
            }
        }
        let longest = self.design.longest_ns();
        for shadow in finished {
            self.retire(shadow, longest, None);
        }
    }

    /// Moves a shadow to the outcomes. Remaining milestones get the current state when they were
    /// reached or decided (every path filled), and nothing when tracking ended first.
    fn retire(&mut self, mut sh: Shadow, observed_ns: u64, ended_at: Option<u64>) {
        while let Some(&(offset, m)) = self.milestones.get(sh.milestone) {
            let reached = ended_at.is_none_or(|end| sh.out.arrival_ns + offset <= end);
            match m {
                Milestone::Checkpoint(_) if !reached && !sh.done() => {}
                _ => record(&mut sh, m),
            }
            sh.milestone += 1;
        }
        sh.out.observed_ns = observed_ns;
        self.done.push(sh.out);
    }
}

fn record(sh: &mut Shadow, m: Milestone) {
    match m {
        Milestone::Checkpoint(c) => sh.out.checkpoints[c] = Some(sh.checkpoint()),
        Milestone::Horizon(h) => sh.out.cancelled[h] = (sh.cancel_ahead, sh.cancel_behind),
    }
}

/// `price` is worse than the shadow's `own` price for a resting order of `side` (lower bid,
/// higher ask): a trade there passed through the shadow.
fn worse(side: Side, price: u32, own: u32) -> bool {
    match side {
        Side::Buy => price < own,
        Side::Sell => price > own,
    }
}

/// The opposite best has reached the shadow's price.
fn crosses(side: Side, own: u32, opposite: Option<u32>) -> bool {
    opposite.is_some_and(|o| match side {
        Side::Buy => o <= own,
        Side::Sell => o >= own,
    })
}

/// Raw result of one pass, before aggregation.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowRun {
    /// End of tracking on the exchange clock: the window end, or the last message's timestamp
    /// when the stream ended earlier.
    pub tracking_end_ns: u64,
    pub stream_reached_window_end: bool,
    pub messages: u64,
    pub symbols: Vec<SymbolSummary>,
    pub shadows: Vec<ShadowOutcome>,
}

// ---------------------------------------------------------------------------------------------
// Intervals

/// Wilson score interval (no continuity correction) for `x` successes in `n` trials.
pub fn wilson(x: u64, n: u64) -> Option<(f64, f64)> {
    if n == 0 || x > n {
        return None;
    }
    let n = n as f64;
    let p = x as f64 / n;
    let z2 = Z95 * Z95;
    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let half = Z95 * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denom;
    Some(((center - half).max(0.0), (center + half).min(1.0)))
}

/// Newcombe (1998) method 10 for the difference of paired proportions p_A − p_B: Wilson score
/// intervals for each margin combined with the continuity-corrected correlation φ.
/// `e` both, `f` A only, `g` B only, `h` neither.
pub fn newcombe_paired(e: u64, f: u64, g: u64, h: u64) -> Option<(f64, f64, f64)> {
    let n = e + f + g + h;
    let (l2, u2) = wilson(e + f, n)?;
    let (l3, u3) = wilson(e + g, n)?;
    let nf = n as f64;
    let pa = (e + f) as f64 / nf;
    let pb = (e + g) as f64 / nf;
    let theta = pa - pb;
    let (e_, f_, g_, h_) = (e as f64, f as f64, g as f64, h as f64);
    let denom = (e_ + f_) * (g_ + h_) * (e_ + g_) * (f_ + h_);
    let phi = if denom == 0.0 {
        0.0
    } else {
        let mut num = e_ * h_ - f_ * g_;
        if num > 0.0 {
            num = (num - nf / 2.0).max(0.0);
        }
        num / denom.sqrt()
    };
    let (dl2, du2, dl3, du3) = (pa - l2, u2 - pa, pb - l3, u3 - pb);
    let delta = (dl2 * dl2 - 2.0 * phi * dl2 * du3 + du3 * du3)
        .max(0.0)
        .sqrt();
    let eps = (du2 * du2 - 2.0 * phi * du2 * dl3 + dl3 * dl3)
        .max(0.0)
        .sqrt();
    Some((theta, theta - delta, theta + eps))
}

const WILSON: &str = "nominal 95% Wilson score interval (no continuity correction)";
const NEWCOMBE: &str =
    "nominal 95% Newcombe (1998) method 10: paired, Wilson margins, continuity-corrected phi";

fn band(bounds: Option<(f64, f64)>, method: &str, unit: &str, n: u64) -> ConfidenceBand {
    match bounds {
        Some((lo, hi)) => ConfidenceBand {
            lower: Value::measured(lo, unit, n, 0),
            upper: Value::measured(hi, unit, n, 0),
            method: method.into(),
        },
        None => ConfidenceBand {
            lower: Value::unavailable("no eligible shadow orders", unit, 0),
            upper: Value::unavailable("no eligible shadow orders", unit, 0),
            method: method.into(),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Aggregation

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CauseCounts {
    pub queue: u64,
    pub trade_through: u64,
    pub book_cross: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairedDifference {
    /// Shadows filled within τ by both paths, by the model only, by L3 only, by neither.
    pub both: u64,
    pub model_only: u64,
    pub l3_only: u64,
    pub neither: u64,
    /// Model fill fraction minus L3 fill fraction.
    pub difference: Value,
    pub difference_95: ConfidenceBand,
    /// H0 (equal fill fractions) is rejected when the interval excludes 0.
    pub excludes_zero: Option<bool>,
}

/// One model (or the L3 reference) × symbol × latency × horizon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cell {
    pub symbol: String,
    pub latency_ns: u64,
    pub horizon_ns: u64,
    /// `l3_shadow` or a model id such as `power_prob3_n2`.
    pub model: String,
    pub family: Option<Family>,
    pub n: Option<u32>,
    /// Every placed shadow (Kaplan–Meier sample).
    pub placed: u64,
    /// Shadows whose whole horizon lies inside tracking (fill-fraction sample).
    pub eligible: u64,
    pub filled: u64,
    pub fill_fraction: Value,
    pub fill_fraction_95: ConfidenceBand,
    pub fill_causes: CauseCounts,
    /// `None` for the L3 reference itself.
    pub vs_l3: Option<PairedDifference>,
    pub km_median_ns: Value,
    pub km_fill_probability: Value,
    pub km_fill_probability_95: ConfidenceBand,
    pub km_censored: u64,
}

/// L3 cancellations at the shadow's price until min(L3 fill, τ, end of tracking).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CancelledVolume {
    pub symbol: String,
    pub latency_ns: u64,
    pub horizon_ns: u64,
    pub placed: u64,
    pub ahead_shares: u64,
    pub behind_shares: u64,
    /// ahead / (ahead + behind), pooled over shadows.
    pub share_ahead: Value,
}

/// `front − ahead_L3` at one checkpoint, over shadows unfilled under both paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointCell {
    pub symbol: String,
    pub latency_ns: u64,
    pub checkpoint_ns: u64,
    pub model: String,
    pub pairs: u64,
    pub mean_abs_error_shares: Value,
    pub mean_signed_error_shares: Value,
    pub mean_l3_ahead_shares: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aggregates {
    pub tracking_end_ns: u64,
    pub stream_reached_window_end: bool,
    pub messages: u64,
    pub symbols: Vec<SymbolSummary>,
    pub cells: Vec<Cell>,
    pub cancelled_volume: Vec<CancelledVolume>,
    pub checkpoints: Vec<CheckpointCell>,
}

fn within(fill: Option<Fill>, horizon: u64) -> Option<FillCause> {
    fill.filter(|f| f.elapsed_ns <= horizon).map(|f| f.cause)
}

/// Every cell of the design; nothing selected or dropped.
pub fn aggregate(design: &Design, run: &ShadowRun) -> Aggregates {
    let mut cells = Vec::new();
    let mut cancelled_volume = Vec::new();
    let mut checkpoints = Vec::new();
    for (si, summary) in run.symbols.iter().enumerate() {
        for (li, &latency_ns) in design.latencies_ns.iter().enumerate() {
            let group: Vec<&ShadowOutcome> = run
                .shadows
                .iter()
                .filter(|o| o.symbol == si && o.latency == li)
                .collect();
            let placed = group.len() as u64;
            for (hi, &horizon) in design.horizons_ns.iter().enumerate() {
                let eligible: Vec<&&ShadowOutcome> = group
                    .iter()
                    .filter(|o| o.arrival_ns + horizon <= run.tracking_end_ns)
                    .collect();
                let paths = std::iter::once((L3_REFERENCE.to_string(), None, None)).chain(
                    design
                        .models
                        .iter()
                        .enumerate()
                        .map(|(mi, m)| (m.id(), Some(*m), Some(mi))),
                );
                for (id, spec, index) in paths {
                    let path = |o: &ShadowOutcome| index.map_or(o.l3, |mi| o.models[mi]);
                    let mut causes = CauseCounts::default();
                    let (mut both, mut model_only, mut l3_only, mut neither) = (0, 0, 0, 0);
                    for o in &eligible {
                        let mine = within(path(o), horizon);
                        match mine {
                            Some(FillCause::Queue) => causes.queue += 1,
                            Some(FillCause::TradeThrough) => causes.trade_through += 1,
                            Some(FillCause::BookCross) => causes.book_cross += 1,
                            None => {}
                        }
                        match (mine.is_some(), within(o.l3, horizon).is_some()) {
                            (true, true) => both += 1,
                            (true, false) => model_only += 1,
                            (false, true) => l3_only += 1,
                            (false, false) => neither += 1,
                        }
                    }
                    let n = eligible.len() as u64;
                    let filled = both + model_only;
                    let fraction = if n == 0 {
                        Value::unavailable("no eligible shadow orders", "probability", placed)
                    } else {
                        Value::measured(filled as f64 / n as f64, "probability", n, placed - n)
                    };
                    let vs_l3 = spec.map(|_| {
                        let diff = newcombe_paired(both, model_only, l3_only, neither);
                        PairedDifference {
                            both,
                            model_only,
                            l3_only,
                            neither,
                            difference: diff.map_or_else(
                                || {
                                    Value::unavailable(
                                        "no eligible shadow orders",
                                        "probability",
                                        0,
                                    )
                                },
                                |d| Value::measured(d.0, "probability", n, 0),
                            ),
                            difference_95: band(
                                diff.map(|d| (d.1, d.2)),
                                NEWCOMBE,
                                "probability",
                                n,
                            ),
                            excludes_zero: diff.map(|d| d.1 > 0.0 || d.2 < 0.0),
                        }
                    });
                    let obs: Vec<SurvivalObservation> = group
                        .iter()
                        .map(|o| match path(o) {
                            Some(f) => SurvivalObservation {
                                elapsed_ns: f.elapsed_ns as i64,
                                event: true,
                            },
                            None => SurvivalObservation {
                                elapsed_ns: o.observed_ns as i64,
                                event: false,
                            },
                        })
                        .collect();
                    let mut km = kaplan_meier(&obs, horizon as i64);
                    km.probability_95.method.insert_str(0, "nominal ");
                    cells.push(Cell {
                        symbol: summary.symbol.clone(),
                        latency_ns,
                        horizon_ns: horizon,
                        model: id.clone(),
                        family: spec.map(|s| s.family),
                        n: spec.and_then(|s| s.n),
                        placed,
                        eligible: n,
                        filled,
                        fill_fraction: fraction,
                        fill_fraction_95: band(wilson(filled, n), WILSON, "probability", n),
                        fill_causes: causes,
                        vs_l3,
                        km_median_ns: km.median_ns,
                        km_fill_probability: km.fill_probability,
                        km_fill_probability_95: km.probability_95,
                        km_censored: km.censored,
                    });
                }
                let (ahead, behind) = group.iter().fold((0u64, 0u64), |(a, b), o| {
                    (a + o.cancelled[hi].0, b + o.cancelled[hi].1)
                });
                let total = ahead + behind;
                cancelled_volume.push(CancelledVolume {
                    symbol: summary.symbol.clone(),
                    latency_ns,
                    horizon_ns: horizon,
                    placed,
                    ahead_shares: ahead,
                    behind_shares: behind,
                    share_ahead: if total == 0 {
                        Value::unavailable("no cancelled shares at the shadow prices", "share", 0)
                    } else {
                        Value::measured(ahead as f64 / total as f64, "share", total, 0)
                    },
                });
            }
            for (ci, &checkpoint_ns) in design.checkpoints_ns.iter().enumerate() {
                for (mi, m) in design.models.iter().enumerate() {
                    let (mut pairs, mut abs, mut signed, mut ahead) = (0u64, 0.0, 0.0, 0.0);
                    for o in &group {
                        let Some(Some(obs)) = o.checkpoints.get(ci) else {
                            continue;
                        };
                        if let (Some(l3), Some(front)) = (obs.l3_ahead, obs.fronts[mi]) {
                            let err = front - l3 as f64;
                            pairs += 1;
                            abs += err.abs();
                            signed += err;
                            ahead += l3 as f64;
                        }
                    }
                    let mean = |sum: f64| {
                        if pairs == 0 {
                            Value::unavailable("no shadow unfilled under both paths", "shares", 0)
                        } else {
                            Value::measured(sum / pairs as f64, "shares", pairs, 0)
                        }
                    };
                    checkpoints.push(CheckpointCell {
                        symbol: summary.symbol.clone(),
                        latency_ns,
                        checkpoint_ns,
                        model: m.id(),
                        pairs,
                        mean_abs_error_shares: mean(abs),
                        mean_signed_error_shares: mean(signed),
                        mean_l3_ahead_shares: mean(ahead),
                    });
                }
            }
        }
    }
    Aggregates {
        tracking_end_ns: run.tracking_end_ns,
        stream_reached_window_end: run.stream_reached_window_end,
        messages: run.messages,
        symbols: run.symbols.clone(),
        cells,
        cancelled_volume,
        checkpoints,
    }
}

#[cfg(test)]
#[path = "shadow_queue_tests.rs"]
mod tests;
