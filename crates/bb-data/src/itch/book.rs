//! Order-by-order (L3) book reconstruction from decoded ITCH messages.
//!
//! One [`OrderBooks`] holds every stock of a file: order reference numbers are unique for the
//! day across all stocks (§1.3), and each stock's book is indexed by its locate code.
//!
//! Invariants, all enforced before a message changes anything:
//!
//! * A modify message (E, C, X, D, U) must name a live order of the same stock locate. Otherwise
//!   it is quarantined: counted, kept as an example and not applied. Nothing is ever invented;
//!   a replace of an unknown order adds no new order, because its side would have to be guessed.
//! * Executed or cancelled shares are positive and at most the order's remaining shares, so no
//!   order's remaining shares can become negative. An order is removed when they reach zero
//!   (§1.4: "When the number of display shares for an order reaches zero, the order is dead").
//! * A replace (§1.4.5) removes the original and adds the new reference number on the same side
//!   at the back of its price's queue.
//! * An add needs a new reference number, positive shares, a price in `1..=200,000.0000` (the
//!   largest `Price (4)` the spec allows) and, once the directory named the locate, its symbol.
//! * Prints (P, Q) never change the book: P executes non-displayed interest and Q is the bulk
//!   print of a cross whose resting orders were already executed through C messages.
//!
//! Crossed (best bid above best ask) and locked (equal) books are detected after each complete
//! per-stock batch of messages sharing one timestamp, and reported, never repaired.
//!
//! The book keeps each price's displayed orders in ITCH arrival order, so
//! [`OrderBooks::observed_queue`] gives the actual orders ahead of a given order. That is an
//! observation of the displayed queue only (hidden and reserve interest never appears in ITCH),
//! and it is distinct from the simulator's modeled queue position (`front` in
//! docs/QUEUE_MODELS.md), which is an estimate from Level-2 data.

use std::{
    collections::{BTreeMap, HashMap, HashSet, btree_map::Entry},
    hash::{BuildHasherDefault, Hasher},
};

use super::decode::{AddOrder, Header, Message, OrderReplace, Price4, Side, show_kind};
use crate::quality::Finding;

/// Largest `Price (4)` value in TotalView-ITCH (spec "Data Types": 200,000.0000, `0x77359400`).
pub const MAX_PRICE4: u32 = 2_000_000_000;

/// Hasher for integer ids (order reference and match numbers). Its maps are only probed and
/// never iterated into any output, so the layout cannot affect results.
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u64(&mut self, n: u64) {
        let x = (self.0 ^ n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = x ^ (x >> 32);
    }
}

type IdMap<V> = HashMap<u64, V, BuildHasherDefault<IdHasher>>;
type IdSet = HashSet<u64, BuildHasherDefault<IdHasher>>;

/// Why a message was quarantined (not applied).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Quarantine {
    /// The order reference is not live on the book.
    UnknownOrder,
    /// The order is live but on another stock locate than the message names.
    LocateMismatch,
    /// More shares executed or cancelled than the order has left.
    ExceedsRemaining,
    /// An add, execution, cancel or replace for zero shares.
    ZeroShares,
    /// An add or replace whose new reference number is already live.
    DuplicateOrderRef,
    /// An add or replace priced 0 or above 200,000.0000.
    InvalidPrice,
    /// An add whose symbol differs from the Stock Directory entry of its locate.
    StockMismatch,
}

impl Quarantine {
    pub const ALL: [Quarantine; 7] = [
        Quarantine::UnknownOrder,
        Quarantine::LocateMismatch,
        Quarantine::ExceedsRemaining,
        Quarantine::ZeroShares,
        Quarantine::DuplicateOrderRef,
        Quarantine::InvalidPrice,
        Quarantine::StockMismatch,
    ];

    /// Stable key, used as the check id suffix and in the manifest.
    pub fn key(self) -> &'static str {
        match self {
            Quarantine::UnknownOrder => "unknown_order",
            Quarantine::LocateMismatch => "locate_mismatch",
            Quarantine::ExceedsRemaining => "exceeds_remaining",
            Quarantine::ZeroShares => "zero_shares",
            Quarantine::DuplicateOrderRef => "duplicate_order_ref",
            Quarantine::InvalidPrice => "invalid_price",
            Quarantine::StockMismatch => "stock_mismatch",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Quarantine::UnknownOrder => "Modify messages naming an order that is not on the book",
            Quarantine::LocateMismatch => "Modify messages naming an order of another stock",
            Quarantine::ExceedsRemaining => {
                "Executions or cancels larger than the order's remaining shares"
            }
            Quarantine::ZeroShares => "Adds, executions, cancels or replaces for zero shares",
            Quarantine::DuplicateOrderRef => "Adds or replaces reusing a live order reference",
            Quarantine::InvalidPrice => "Adds or replaces priced 0 or above 200,000.0000",
            Quarantine::StockMismatch => "Adds whose symbol differs from the stock directory",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// What [`OrderBooks::apply`] did with a message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The book of one stock changed (and possibly a print was recorded).
    Book,
    /// A print was recorded; no book changed.
    Print,
    /// Nothing to apply (administrative or uninterpreted message, broken trade).
    NoEffect,
    Quarantined(Quarantine),
}

/// How a print came about.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PrintKind {
    /// §1.4.1 / §1.4.2: execution of a displayed order resting on `resting`. A non-printable
    /// execution (§1.4.2) is included in a later bulk print.
    Execution { resting: Side, printable: bool },
    /// §1.5.1: execution of non-displayed interest; the resting side is not disseminated.
    NonCross,
    /// §1.5.2: bulk print of a cross (`cross_type` as in the message).
    Cross { cross_type: u8 },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Print {
    pub locate: u16,
    pub timestamp_ns: u64,
    pub price: Price4,
    pub shares: u64,
    pub match_number: u64,
    pub kind: PrintKind,
}

/// One effect of the last applied message, in the order it happened.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The displayed total at a price is now `shares` over `orders` orders (0: level gone).
    Level {
        locate: u16,
        side: Side,
        price: Price4,
        shares: u64,
        orders: u32,
    },
    Print(Print),
}

/// A live order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderView {
    pub order_ref: u64,
    pub locate: u16,
    pub side: Side,
    pub price: Price4,
    pub shares: u32,
}

/// Aggregate of one price level.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LevelView {
    pub price: Price4,
    pub shares: u64,
    pub orders: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct QueuedOrder {
    pub order_ref: u64,
    pub shares: u32,
}

/// The displayed orders ahead of one order at its price, observed from ITCH: the orders that
/// arrived earlier at that price and are still live, in arrival order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedQueue {
    pub order: OrderView,
    pub ahead: Vec<QueuedOrder>,
    pub shares_ahead: u64,
    pub level_shares: u64,
    pub level_orders: u32,
}

#[derive(Copy, Clone, Debug)]
struct Order {
    locate: u16,
    side: Side,
    price: u32,
    shares: u32,
    prev: Option<u64>,
    next: Option<u64>,
}

/// A price level: totals plus the first and last order of its arrival-ordered list.
#[derive(Copy, Clone, Debug)]
struct Level {
    shares: u64,
    orders: u32,
    head: u64,
    tail: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Top {
    Normal,
    Locked,
    Crossed,
}

#[derive(Debug)]
struct StockBook {
    symbol: Option<[u8; 8]>,
    bids: BTreeMap<u32, Level>,
    asks: BTreeMap<u32, Level>,
    had_orders: bool,
    /// Last Stock Trading Action state (§1.2.2); `None` until one arrives.
    trading_state: Option<u8>,
    /// Timestamp of the batch being applied and whether it changed the book.
    batch_ts: u64,
    dirty: bool,
    top: Top,
    top_since: u64,
}

impl Default for StockBook {
    fn default() -> Self {
        Self {
            symbol: None,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            had_orders: false,
            trading_state: None,
            batch_ts: 0,
            dirty: false,
            top: Top::Normal,
            top_since: 0,
        }
    }
}

impl StockBook {
    fn levels(&self, side: Side) -> &BTreeMap<u32, Level> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    fn levels_mut(&mut self, side: Side) -> &mut BTreeMap<u32, Level> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }
}

/// Counters of what the books did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BookStats {
    pub orders_added: u64,
    pub orders_replaced: u64,
    pub orders_deleted: u64,
    pub orders_fully_executed: u64,
    pub orders_cancelled_to_zero: u64,
    pub executions: u64,
    pub executed_shares: u64,
    pub non_printable_executions: u64,
    pub non_cross_trades: u64,
    pub non_cross_shares: u64,
    pub cross_trades: u64,
    pub cross_shares: u64,
    pub zero_share_crosses: u64,
    pub broken_trades: u64,
    /// Exchange-clock time spent crossed or locked, summed over stocks.
    pub crossed_ns: u64,
    pub locked_ns: u64,
    /// Entries into a crossed or locked state while the stock's last Trading Action said `T`
    /// (trading on Nasdaq), where displayed orders would normally have matched.
    pub crossed_entries_while_trading: u64,
    pub locked_entries_while_trading: u64,
}

/// Every stock's L3 book.
#[derive(Debug, Default)]
pub struct OrderBooks {
    orders: IdMap<Order>,
    stocks: Vec<StockBook>,
    match_numbers: IdSet,
    changes: Vec<Change>,
    stats: BookStats,
    quarantined: [Finding; Quarantine::ALL.len()],
    crossed: Finding,
    locked: Finding,
    broken_unknown_match: Finding,
}

impl OrderBooks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Effects of the last [`apply`](Self::apply) call (empty when it changed nothing).
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn stats(&self) -> &BookStats {
        &self.stats
    }

    pub fn quarantined(&self, reason: Quarantine) -> &Finding {
        &self.quarantined[reason.index()]
    }

    pub fn quarantined_total(&self) -> u64 {
        self.quarantined.iter().map(|f| f.count).sum()
    }

    /// Entries into a crossed state (one per stock and episode).
    pub fn crossed(&self) -> &Finding {
        &self.crossed
    }

    /// Entries into a locked state.
    pub fn locked(&self) -> &Finding {
        &self.locked
    }

    pub fn broken_unknown_match(&self) -> &Finding {
        &self.broken_unknown_match
    }

    pub fn live_orders(&self) -> usize {
        self.orders.len()
    }

    /// Locates named by a Stock Directory message.
    pub fn stocks_in_directory(&self) -> usize {
        self.stocks.iter().filter(|s| s.symbol.is_some()).count()
    }

    /// Locates that ever held an order.
    pub fn stocks_with_orders(&self) -> usize {
        self.stocks.iter().filter(|s| s.had_orders).count()
    }

    /// Symbol of a locate from the Stock Directory, without padding.
    pub fn symbol(&self, locate: u16) -> Option<String> {
        let raw = self.stocks.get(usize::from(locate))?.symbol?;
        Some(super::decode::Stock(&raw).trimmed())
    }

    pub fn order(&self, order_ref: u64) -> Option<OrderView> {
        self.orders.get(&order_ref).map(|o| view(order_ref, o))
    }

    /// Levels of one side, best first.
    pub fn levels(&self, locate: u16, side: Side) -> Vec<LevelView> {
        let Some(book) = self.stocks.get(usize::from(locate)) else {
            return Vec::new();
        };
        let map = book.levels(side).iter().map(|(&price, l)| LevelView {
            price: Price4(price),
            shares: l.shares,
            orders: l.orders,
        });
        match side {
            Side::Buy => map.rev().collect(),
            Side::Sell => map.collect(),
        }
    }

    pub fn best_bid(&self, locate: u16) -> Option<LevelView> {
        self.best(locate, Side::Buy)
    }

    pub fn best_ask(&self, locate: u16) -> Option<LevelView> {
        self.best(locate, Side::Sell)
    }

    fn best(&self, locate: u16, side: Side) -> Option<LevelView> {
        let book = self.stocks.get(usize::from(locate))?;
        let (&price, l) = match side {
            Side::Buy => book.bids.last_key_value()?,
            Side::Sell => book.asks.first_key_value()?,
        };
        Some(LevelView {
            price: Price4(price),
            shares: l.shares,
            orders: l.orders,
        })
    }

    /// Whole-side shares and order count without allocating a depth snapshot.
    pub fn side_totals(&self, locate: u16, side: Side) -> (u64, u64) {
        let Some(book) = self.stocks.get(usize::from(locate)) else {
            return (0, 0);
        };
        book.levels(side)
            .values()
            .fold((0, 0), |(shares, orders), level| {
                (shares + level.shares, orders + u64::from(level.orders))
            })
    }

    /// Best-first depth limited before allocation (for bounded replay responses).
    pub fn top_levels(&self, locate: u16, side: Side, limit: usize) -> Vec<LevelView> {
        let Some(book) = self.stocks.get(usize::from(locate)) else {
            return Vec::new();
        };
        let row = |(&price, level): (&u32, &Level)| LevelView {
            price: Price4(price),
            shares: level.shares,
            orders: level.orders,
        };
        match side {
            Side::Buy => book
                .levels(side)
                .iter()
                .rev()
                .take(limit)
                .map(row)
                .collect(),
            Side::Sell => book.levels(side).iter().take(limit).map(row).collect(),
        }
    }

    /// FIFO prefix limited before allocation; totals remain available from the level view.
    pub fn level_queue_prefix(
        &self,
        locate: u16,
        side: Side,
        price: Price4,
        limit: usize,
    ) -> Vec<QueuedOrder> {
        let Some(level) = self
            .stocks
            .get(usize::from(locate))
            .and_then(|b| b.levels(side).get(&price.0))
        else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity((level.orders as usize).min(limit));
        let mut cursor = Some(level.head);
        while let Some(order_ref) = cursor.filter(|_| out.len() < limit) {
            let order = &self.orders[&order_ref];
            out.push(QueuedOrder {
                order_ref,
                shares: order.shares,
            });
            cursor = order.next;
        }
        out
    }

    /// One-based FIFO position and total orders at the resting order's price.
    pub fn queue_position(&self, order_ref: u64) -> Option<(u32, u32)> {
        let order = self.orders.get(&order_ref)?;
        let level = self.stocks[usize::from(order.locate)]
            .levels(order.side)
            .get(&order.price)?;
        let mut cursor = level.head;
        let mut position = 1;
        while cursor != order_ref {
            cursor = self.orders[&cursor].next?;
            position += 1;
        }
        Some((position, level.orders))
    }

    /// Live orders at one price in arrival (priority) order.
    pub fn level_queue(&self, locate: u16, side: Side, price: Price4) -> Vec<QueuedOrder> {
        let Some(level) = self
            .stocks
            .get(usize::from(locate))
            .and_then(|b| b.levels(side).get(&price.0))
        else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(level.orders as usize);
        let mut cursor = Some(level.head);
        while let Some(order_ref) = cursor {
            let order = &self.orders[&order_ref];
            out.push(QueuedOrder {
                order_ref,
                shares: order.shares,
            });
            cursor = order.next;
        }
        out
    }

    /// The observed displayed queue ahead of `order_ref`, or `None` when it is not live.
    pub fn observed_queue(&self, order_ref: u64) -> Option<ObservedQueue> {
        let order = self.orders.get(&order_ref)?;
        let level = self.stocks[usize::from(order.locate)]
            .levels(order.side)
            .get(&order.price)
            .expect("a live order has its level");
        let mut ahead = Vec::new();
        let mut cursor = level.head;
        while cursor != order_ref {
            let o = &self.orders[&cursor];
            ahead.push(QueuedOrder {
                order_ref: cursor,
                shares: o.shares,
            });
            cursor = o.next.expect("the order is in its level's list");
        }
        Some(ObservedQueue {
            order: view(order_ref, order),
            shares_ahead: ahead.iter().map(|q| u64::from(q.shares)).sum(),
            ahead,
            level_shares: level.shares,
            level_orders: level.orders,
        })
    }

    /// Applies one message. Its effects are available from [`changes`](Self::changes) until the
    /// next call.
    pub fn apply(&mut self, msg: &Message<'_>) -> Applied {
        self.changes.clear();
        match msg {
            Message::StockDirectory(m) => {
                self.stock_mut(m.header.stock_locate).symbol = Some(*m.stock.0);
                Applied::NoEffect
            }
            Message::AddOrder(m) => self.add(m),
            Message::OrderExecuted(m) => {
                self.execute(b'E', m.header, m.order_ref, m.shares, m.match_number, None)
            }
            Message::OrderExecutedWithPrice(m) => self.execute(
                b'C',
                m.header,
                m.order_ref,
                m.shares,
                m.match_number,
                Some((m.price, m.printable)),
            ),
            Message::OrderCancel(m) => self.cancel(m.header, m.order_ref, m.shares),
            Message::OrderDelete(m) => self.delete(m.header, m.order_ref),
            Message::OrderReplace(m) => self.replace(m),
            Message::Trade(m) => {
                self.stats.non_cross_trades += 1;
                self.stats.non_cross_shares += u64::from(m.shares);
                self.print(
                    m.header,
                    m.price,
                    u64::from(m.shares),
                    m.match_number,
                    PrintKind::NonCross,
                )
            }
            Message::CrossTrade(m) => {
                self.stats.cross_trades += 1;
                self.stats.cross_shares += m.shares;
                if m.shares == 0 {
                    self.stats.zero_share_crosses += 1;
                }
                self.print(
                    m.header,
                    m.price,
                    m.shares,
                    m.match_number,
                    PrintKind::Cross {
                        cross_type: m.cross_type,
                    },
                )
            }
            Message::BrokenTrade(m) => {
                self.stats.broken_trades += 1;
                if !self.match_numbers.contains(&m.match_number) {
                    let ts = m.header.timestamp_ns;
                    let match_number = m.match_number;
                    self.broken_unknown_match.hit(|| {
                        format!("B at {ts} ns: match number {match_number} was never printed")
                    });
                }
                Applied::NoEffect
            }
            Message::TradingAction(m) => {
                // A state change at a new timestamp closes the open batch first, so each batch is
                // judged under the state in force when it ended.
                let locate = m.header.stock_locate;
                let book = self.stock_mut(locate);
                if book.dirty && book.batch_ts != m.header.timestamp_ns {
                    self.judge(locate);
                }
                self.stocks[usize::from(locate)].trading_state = Some(m.trading_state);
                Applied::NoEffect
            }
            Message::SystemEvent(_) | Message::Skipped { .. } | Message::Unknown { .. } => {
                Applied::NoEffect
            }
        }
    }

    /// Judges every book with an open batch and closes open crossed/locked intervals at
    /// `end_ns` (the last timestamp of the input).
    pub fn finish(&mut self, end_ns: u64) {
        for locate in 0..self.stocks.len() {
            let locate = u16::try_from(locate).expect("locates are u16");
            if self.stocks[usize::from(locate)].dirty {
                self.judge(locate);
            }
            let book = &mut self.stocks[usize::from(locate)];
            let open = end_ns.saturating_sub(book.top_since);
            match book.top {
                Top::Crossed => self.stats.crossed_ns += open,
                Top::Locked => self.stats.locked_ns += open,
                Top::Normal => {}
            }
            book.top_since = end_ns.max(book.top_since);
        }
    }

    /// Checks the structural invariants of every book: each level's totals equal the sum over
    /// its linked orders, links are consistent both ways, every order sits on the level its own
    /// fields name with positive shares, and no live order is outside a level.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut linked = 0usize;
        for (locate, book) in self.stocks.iter().enumerate() {
            for side in [Side::Buy, Side::Sell] {
                for (&price, level) in book.levels(side) {
                    let mut shares = 0u64;
                    let mut count = 0u32;
                    let mut prev: Option<u64> = None;
                    let mut cursor = Some(level.head);
                    while let Some(order_ref) = cursor {
                        let Some(o) = self.orders.get(&order_ref) else {
                            return Err(format!("level {price} links unknown order {order_ref}"));
                        };
                        if usize::from(o.locate) != locate || o.side != side || o.price != price {
                            return Err(format!("order {order_ref} is linked at the wrong level"));
                        }
                        if o.shares == 0 {
                            return Err(format!("order {order_ref} is live with zero shares"));
                        }
                        if o.prev != prev {
                            return Err(format!("order {order_ref} has a broken back link"));
                        }
                        shares += u64::from(o.shares);
                        count += 1;
                        if count > level.orders {
                            return Err(format!("level {price} links more orders than it counts"));
                        }
                        prev = Some(order_ref);
                        cursor = o.next;
                    }
                    if prev != Some(level.tail) {
                        return Err(format!("level {price} has a stale tail"));
                    }
                    if shares != level.shares || count != level.orders {
                        return Err(format!(
                            "level {price} of locate {locate} says {} shares in {} orders, its \
                             orders hold {shares} in {count}",
                            level.shares, level.orders
                        ));
                    }
                    linked += count as usize;
                }
            }
        }
        if linked != self.orders.len() {
            return Err(format!(
                "{} live orders but {linked} linked into levels",
                self.orders.len()
            ));
        }
        Ok(())
    }

    fn stock_mut(&mut self, locate: u16) -> &mut StockBook {
        let i = usize::from(locate);
        if self.stocks.len() <= i {
            self.stocks.resize_with(i + 1, StockBook::default);
        }
        &mut self.stocks[i]
    }

    fn quarantine(&mut self, reason: Quarantine, example: impl FnOnce() -> String) -> Applied {
        self.quarantined[reason.index()].hit(example);
        Applied::Quarantined(reason)
    }

    /// The live order `order_ref` of `locate`, or why a message naming it must be quarantined.
    fn lookup(&self, locate: u16, order_ref: u64) -> Result<Order, Quarantine> {
        match self.orders.get(&order_ref) {
            None => Err(Quarantine::UnknownOrder),
            Some(o) if o.locate != locate => Err(Quarantine::LocateMismatch),
            Some(o) => Ok(*o),
        }
    }

    /// Starts applying a message at `ts` to `locate`'s book; a different timestamp closes the
    /// previous batch first.
    fn begin(&mut self, locate: u16, ts: u64) {
        let book = self.stock_mut(locate);
        if book.dirty && book.batch_ts != ts {
            self.judge(locate);
        }
        let book = &mut self.stocks[usize::from(locate)];
        book.batch_ts = ts;
        book.dirty = true;
        book.had_orders = true;
    }

    /// Classifies the top of `locate`'s book after its batch and records state changes.
    fn judge(&mut self, locate: u16) {
        let book = &mut self.stocks[usize::from(locate)];
        book.dirty = false;
        let bid = book.bids.last_key_value().map(|(&p, _)| p);
        let ask = book.asks.first_key_value().map(|(&p, _)| p);
        let top = match (bid, ask) {
            (Some(b), Some(a)) if b > a => Top::Crossed,
            (Some(b), Some(a)) if b == a => Top::Locked,
            _ => Top::Normal,
        };
        if top == book.top {
            return;
        }
        let ts = book.batch_ts;
        let spent = ts.saturating_sub(book.top_since);
        match book.top {
            Top::Crossed => self.stats.crossed_ns += spent,
            Top::Locked => self.stats.locked_ns += spent,
            Top::Normal => {}
        }
        book.top = top;
        book.top_since = ts;
        let symbol = book
            .symbol
            .map_or_else(|| "?".to_string(), |s| super::decode::Stock(&s).trimmed());
        let trading = book.trading_state == Some(b'T');
        // §1.2.2: a security absent from the pre-opening Trading Action spin is treated as
        // halted.
        let state = book.trading_state.map_or_else(
            || "no trading action yet, treated as halted".to_string(),
            |s| format!("trading state {}", show_kind(s)),
        );
        let example = || {
            format!(
                "{symbol} (locate {locate}) at {ts} ns: best bid {} vs best ask {} ({state})",
                Price4(bid.unwrap_or(0)),
                Price4(ask.unwrap_or(0))
            )
        };
        match top {
            Top::Crossed => {
                self.stats.crossed_entries_while_trading += u64::from(trading);
                self.crossed.hit(example);
            }
            Top::Locked => {
                self.stats.locked_entries_while_trading += u64::from(trading);
                self.locked.hit(example);
            }
            Top::Normal => {}
        }
    }

    /// Appends an order at the back of its price's queue.
    fn link(&mut self, order_ref: u64, mut order: Order) {
        let levels = self.stock_mut(order.locate).levels_mut(order.side);
        let prev = match levels.entry(order.price) {
            Entry::Vacant(v) => {
                v.insert(Level {
                    shares: u64::from(order.shares),
                    orders: 1,
                    head: order_ref,
                    tail: order_ref,
                });
                None
            }
            Entry::Occupied(o) => {
                let level = o.into_mut();
                let prev = level.tail;
                level.tail = order_ref;
                level.shares += u64::from(order.shares);
                level.orders += 1;
                Some(prev)
            }
        };
        if let Some(p) = prev {
            self.orders
                .get_mut(&p)
                .expect("a level's tail is live")
                .next = Some(order_ref);
        }
        order.prev = prev;
        order.next = None;
        self.orders.insert(order_ref, order);
    }

    /// Removes a live order from its level and returns it.
    fn unlink(&mut self, order_ref: u64) -> Order {
        let order = self.orders.remove(&order_ref).expect("caller checked");
        if let Some(p) = order.prev {
            self.orders.get_mut(&p).expect("linked order").next = order.next;
        }
        if let Some(n) = order.next {
            self.orders.get_mut(&n).expect("linked order").prev = order.prev;
        }
        let levels = self.stocks[usize::from(order.locate)].levels_mut(order.side);
        let Entry::Occupied(mut entry) = levels.entry(order.price) else {
            unreachable!("a live order has its level");
        };
        let level = entry.get_mut();
        level.shares -= u64::from(order.shares);
        level.orders -= 1;
        if level.orders == 0 {
            entry.remove();
        } else {
            if level.head == order_ref {
                level.head = order.next.expect("a non-empty level has a next order");
            }
            if level.tail == order_ref {
                level.tail = order.prev.expect("a non-empty level has a previous order");
            }
        }
        order
    }

    /// Takes `by` shares (already checked to be `1..=remaining`) off an order, removing it at
    /// zero. Returns whether it was removed.
    fn reduce(&mut self, order_ref: u64, by: u32) -> bool {
        let order = self.orders.get_mut(&order_ref).expect("caller checked");
        order.shares -= by;
        let (locate, side, price, left) = (order.locate, order.side, order.price, order.shares);
        let level = self.stocks[usize::from(locate)]
            .levels_mut(side)
            .get_mut(&price)
            .expect("a live order has its level");
        level.shares -= u64::from(by);
        if left == 0 {
            self.unlink(order_ref);
            true
        } else {
            false
        }
    }

    fn record_level(&mut self, locate: u16, side: Side, price: u32) {
        let (shares, orders) = self.stocks[usize::from(locate)]
            .levels(side)
            .get(&price)
            .map_or((0, 0), |l| (l.shares, l.orders));
        self.changes.push(Change::Level {
            locate,
            side,
            price: Price4(price),
            shares,
            orders,
        });
    }

    /// Checks the new order of an add or replace. `replaced` is the reference number the
    /// replace removes, which the new order may reuse.
    fn check_new_order(
        &self,
        order_ref: u64,
        shares: u32,
        price: Price4,
        replaced: Option<u64>,
    ) -> Result<(), Quarantine> {
        if shares == 0 {
            return Err(Quarantine::ZeroShares);
        }
        if price.0 == 0 || price.0 > MAX_PRICE4 {
            return Err(Quarantine::InvalidPrice);
        }
        if replaced != Some(order_ref) && self.orders.contains_key(&order_ref) {
            return Err(Quarantine::DuplicateOrderRef);
        }
        Ok(())
    }

    fn add(&mut self, m: &AddOrder<'_>) -> Applied {
        let locate = m.header.stock_locate;
        let ts = m.header.timestamp_ns;
        let mut check = self.check_new_order(m.order_ref, m.shares, m.price, None);
        if check.is_ok()
            && let Some(Some(symbol)) = self.stocks.get(usize::from(locate)).map(|b| b.symbol)
            && symbol != *m.stock.0
        {
            check = Err(Quarantine::StockMismatch);
        }
        if let Err(reason) = check {
            let kind = if m.attribution.is_some() { 'F' } else { 'A' };
            let (order_ref, shares, price, stock) = (m.order_ref, m.shares, m.price, m.stock);
            return self.quarantine(reason, || {
                format!(
                    "{kind} at {ts} ns: order {order_ref}, {shares} shares of {} at {price} \
                     (locate {locate})",
                    stock.trimmed()
                )
            });
        }
        self.begin(locate, ts);
        self.link(
            m.order_ref,
            Order {
                locate,
                side: m.side,
                price: m.price.0,
                shares: m.shares,
                prev: None,
                next: None,
            },
        );
        self.stats.orders_added += 1;
        self.record_level(locate, m.side, m.price.0);
        Applied::Book
    }

    fn execute(
        &mut self,
        kind: u8,
        h: Header,
        order_ref: u64,
        shares: u32,
        match_number: u64,
        with_price: Option<(Price4, bool)>,
    ) -> Applied {
        let locate = h.stock_locate;
        let ts = h.timestamp_ns;
        let order = self.lookup(locate, order_ref).and_then(|o| {
            if shares == 0 {
                Err(Quarantine::ZeroShares)
            } else if shares > o.shares {
                Err(Quarantine::ExceedsRemaining)
            } else {
                Ok(o)
            }
        });
        let order = match order {
            Ok(o) => o,
            Err(reason) => {
                let left = self.orders.get(&order_ref).map(|o| o.shares);
                return self.quarantine(reason, || {
                    format!(
                        "{} at {ts} ns: {shares} shares of order {order_ref} (locate {locate}, \
                         remaining {})",
                        show_kind(kind),
                        left.map_or_else(|| "none".to_string(), |s| s.to_string())
                    )
                });
            }
        };
        self.begin(locate, ts);
        let (price, printable) = with_price.unwrap_or((Price4(order.price), true));
        self.stats.executions += 1;
        self.stats.executed_shares += u64::from(shares);
        if !printable {
            self.stats.non_printable_executions += 1;
        }
        self.match_numbers.insert(match_number);
        self.changes.push(Change::Print(Print {
            locate,
            timestamp_ns: ts,
            price,
            shares: u64::from(shares),
            match_number,
            kind: PrintKind::Execution {
                resting: order.side,
                printable,
            },
        }));
        if self.reduce(order_ref, shares) {
            self.stats.orders_fully_executed += 1;
        }
        self.record_level(locate, order.side, order.price);
        Applied::Book
    }

    fn cancel(&mut self, h: Header, order_ref: u64, shares: u32) -> Applied {
        let locate = h.stock_locate;
        let ts = h.timestamp_ns;
        let order = self.lookup(locate, order_ref).and_then(|o| {
            if shares == 0 {
                Err(Quarantine::ZeroShares)
            } else if shares > o.shares {
                Err(Quarantine::ExceedsRemaining)
            } else {
                Ok(o)
            }
        });
        let order = match order {
            Ok(o) => o,
            Err(reason) => {
                let left = self.orders.get(&order_ref).map(|o| o.shares);
                return self.quarantine(reason, || {
                    format!(
                        "X at {ts} ns: cancel {shares} shares of order {order_ref} (locate \
                         {locate}, remaining {})",
                        left.map_or_else(|| "none".to_string(), |s| s.to_string())
                    )
                });
            }
        };
        self.begin(locate, ts);
        if self.reduce(order_ref, shares) {
            self.stats.orders_cancelled_to_zero += 1;
        }
        self.record_level(locate, order.side, order.price);
        Applied::Book
    }

    fn delete(&mut self, h: Header, order_ref: u64) -> Applied {
        let locate = h.stock_locate;
        let ts = h.timestamp_ns;
        if let Err(reason) = self.lookup(locate, order_ref) {
            return self.quarantine(reason, || {
                format!("D at {ts} ns: delete order {order_ref} (locate {locate})")
            });
        }
        self.begin(locate, ts);
        let order = self.unlink(order_ref);
        self.stats.orders_deleted += 1;
        self.record_level(locate, order.side, order.price);
        Applied::Book
    }

    fn replace(&mut self, m: &OrderReplace) -> Applied {
        let locate = m.header.stock_locate;
        let ts = m.header.timestamp_ns;
        let checked = self.lookup(locate, m.original_ref).and_then(|o| {
            self.check_new_order(m.new_ref, m.shares, m.price, Some(m.original_ref))
                .map(|()| o)
        });
        let old = match checked {
            Ok(o) => o,
            Err(reason) => {
                let (original, new, shares, price) = (m.original_ref, m.new_ref, m.shares, m.price);
                return self.quarantine(reason, || {
                    format!(
                        "U at {ts} ns: replace order {original} by {new}, {shares} shares at \
                         {price} (locate {locate})"
                    )
                });
            }
        };
        self.begin(locate, ts);
        self.unlink(m.original_ref);
        self.link(
            m.new_ref,
            Order {
                locate,
                side: old.side,
                price: m.price.0,
                shares: m.shares,
                prev: None,
                next: None,
            },
        );
        self.stats.orders_replaced += 1;
        self.record_level(locate, old.side, old.price);
        if m.price.0 != old.price {
            self.record_level(locate, old.side, m.price.0);
        }
        Applied::Book
    }

    fn print(
        &mut self,
        h: Header,
        price: Price4,
        shares: u64,
        match_number: u64,
        kind: PrintKind,
    ) -> Applied {
        self.match_numbers.insert(match_number);
        self.changes.push(Change::Print(Print {
            locate: h.stock_locate,
            timestamp_ns: h.timestamp_ns,
            price,
            shares,
            match_number,
            kind,
        }));
        Applied::Print
    }
}

fn view(order_ref: u64, o: &Order) -> OrderView {
    OrderView {
        order_ref,
        locate: o.locate,
        side: o.side,
        price: Price4(o.price),
        shares: o.shares,
    }
}
