//! Data-quality report over a TotalView-ITCH 5.0 stream, complementing [`super::ItchManifest`].
//!
//! The manifest says whether the input could be reconstructed; this report characterizes the
//! stream itself: crossed/locked episodes by session state, missing and duplicate references,
//! out-of-range quantities and prices, replace semantics, timestamp ties, the (absent) sequence
//! field and session-state transitions. Every figure is an aggregate count; nothing event-level
//! leaves the process except the trading-state transitions of the selected symbols, which are
//! administrative messages (a few per symbol per day).
//!
//! [`QualityReporter`] keeps its own [`OrderBooks`], so it can run beside a
//! [`super::Reconstructor`] on the same frames without touching it. Crossed and locked books
//! are judged exactly as [`OrderBooks`] judges them (per stock, after each batch of applied
//! messages sharing one timestamp), and [`QualityReport::episodes_agree_with_book`] checks that
//! both judgements count the same entries.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    FrameSink, SystemEventRecord,
    book::{Applied, MAX_PRICE4, OrderBooks, Quarantine},
    decode::{Message, Side, decode, show_kind},
};

/// Market-wide session phase from the System Event messages (§1.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// Before `Q` (start of market hours): pre-market.
    PreMarket,
    /// From `Q` to `M`: regular market hours.
    Regular,
    /// From `M` (end of market hours) on.
    PostMarket,
}

impl Phase {
    fn key(self) -> &'static str {
        match self {
            Phase::PreMarket => "pre_market",
            Phase::Regular => "regular",
            Phase::PostMarket => "post_market",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Top {
    Normal,
    Locked,
    Crossed,
}

#[derive(Copy, Clone, Debug)]
struct LocateState {
    directory: bool,
    trading_state: Option<u8>,
    dirty: bool,
    batch_ts: u64,
    batch_phase: Phase,
    top: Top,
    top_since: u64,
    /// Key of the open episode, set while `top` is not `Normal`.
    episode: Option<(Top, Option<u8>, Phase)>,
    last_book_ts: Option<u64>,
}

impl Default for LocateState {
    fn default() -> Self {
        Self {
            directory: false,
            trading_state: None,
            dirty: false,
            batch_ts: 0,
            batch_phase: Phase::PreMarket,
            top: Top::Normal,
            top_since: 0,
            episode: None,
            last_book_ts: None,
        }
    }
}

/// Entries into one kind of crossed or locked state and the exchange time spent in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EpisodeStats {
    pub entries: u64,
    /// Nanoseconds from entry to exit (or to the last timestamp of the input).
    pub total_ns: u64,
    /// Episodes still open at the end of the input, closed at its last timestamp.
    pub open_at_end: u64,
}

/// How Order Replace (`U`, §1.4.5) messages relate to the order they replace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReplaceSemantics {
    pub messages: u64,
    /// Replaces the book applied (original live, new reference unused, valid values).
    pub applied: u64,
    pub same_price: u64,
    /// Better price for the order's side (higher bid, lower ask).
    pub price_improved: u64,
    pub price_worsened: u64,
    pub shares_increased: u64,
    pub shares_decreased: u64,
    pub shares_unchanged: u64,
    /// Same price and fewer shares: a change that keeps priority on venues that preserve it
    /// for size reductions. ITCH issues a new reference, and the reconstruction (like
    /// itch-book) puts the new order at the back of its level.
    pub same_price_size_decrease: u64,
    /// New reference equal to the original. The specification calls for a new reference; the
    /// book applies it as a replace of the order by itself (back of the queue).
    pub new_ref_equals_original: u64,
}

/// Messages whose timestamp equals the previous message's.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TimestampTies {
    /// Consecutive message pairs compared (all stamped messages, every stock).
    pub compared: u64,
    pub ties: u64,
    pub fraction: Option<f64>,
    /// The same for book messages (A, F, E, C, X, D, U) of one stock, against that stock's
    /// previous book message.
    pub book_compared: u64,
    pub book_ties: u64,
    pub book_fraction: Option<f64>,
    /// Messages whose timestamp is below the previous message's.
    pub regressions: u64,
}

/// What the input can say about message sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SequenceCheck {
    /// Whether any field of the input carries a message sequence number.
    pub supported: bool,
    pub basis: String,
    /// Consecutive frames with identical bytes (a replayed or duplicated frame would show here).
    pub identical_consecutive_frames: u64,
    /// Messages naming a stock locate (other than 0) before any Stock Directory entry for it.
    pub locate_without_directory: u64,
}

/// One trading-state change of a selected symbol (Stock Trading Action, §1.2.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StateTransition {
    pub symbol: String,
    pub timestamp_ns: u64,
    /// Previous state, `none` before the first Trading Action.
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// Session state of the market and of the stocks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionReport {
    pub system_events: Vec<SystemEventRecord>,
    /// Stock Trading Action transitions over all stocks, keyed `from->to` (`none` for the first).
    pub trading_state_transitions: BTreeMap<String, u64>,
    /// Trading Actions repeating the stock's current state.
    pub trading_state_repeats: u64,
    /// Every transition of the selected symbols, in input order.
    pub selected: Vec<StateTransition>,
}

/// One named check with its count; `passed` is `count == 0`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReportCheck {
    pub id: String,
    pub count: u64,
    pub passed: bool,
    pub definition: String,
}

/// The data-quality report. Keys of the per-type maps are ITCH type letters.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QualityReport {
    pub frames: u64,
    pub invalid_frames: u64,
    /// Crossed and locked episodes keyed `crossed|locked/<trading state>/<market phase>`, where
    /// the trading state is the stock's last Trading Action when the batch was judged (`none`
    /// before any) and the phase is the System Event phase when the batch's last message came.
    pub episodes: BTreeMap<String, EpisodeStats>,
    /// The episode entries counted here equal [`OrderBooks::crossed`] and
    /// [`OrderBooks::locked`] counts.
    pub episodes_agree_with_book: bool,
    /// Modify messages (E, C, X, D, U) naming an order reference that is not live.
    pub missing_ref_by_type: BTreeMap<String, u64>,
    /// Modify messages naming a live order of another stock locate.
    pub locate_mismatch_by_type: BTreeMap<String, u64>,
    /// Adds and replaces whose new reference is already live.
    pub duplicate_live_ref_by_type: BTreeMap<String, u64>,
    /// Adds and replaces whose new reference is not above every reference added before it.
    /// Informational: references are day-unique, not required to increase.
    pub new_ref_not_above_previous_max: u64,
    /// Messages carrying zero shares where the specification implies a positive count (A, F,
    /// E, C, X, U, P). Zero-share crosses (Q) are legitimate and counted apart.
    pub zero_shares_by_type: BTreeMap<String, u64>,
    pub zero_share_crosses: u64,
    /// Prices of 0 or above 200,000.0000 on A, F, U, C, P and positive-share Q messages.
    pub price_out_of_range_by_type: BTreeMap<String, u64>,
    /// Executions or cancels larger than the order's remaining shares.
    pub exceeds_remaining_by_type: BTreeMap<String, u64>,
    /// All quarantined messages by [`Quarantine::key`].
    pub quarantined: BTreeMap<String, u64>,
    pub replace: ReplaceSemantics,
    pub timestamps: TimestampTies,
    pub sequence: SequenceCheck,
    pub session: SessionReport,
    pub checks: Vec<ReportCheck>,
}

/// Collects a [`QualityReport`] frame by frame.
#[derive(Debug)]
pub struct QualityReporter {
    max_messages: Option<u64>,
    selected_symbols: Vec<String>,
    books: OrderBooks,
    locates: Vec<LocateState>,
    phase: Phase,
    frames: u64,
    invalid_frames: u64,
    prev_frame: Vec<u8>,
    prev_ts: Option<u64>,
    end_ts: u64,
    max_new_ref: Option<u64>,
    missing_ref: [u64; 256],
    locate_mismatch: [u64; 256],
    duplicate_ref: [u64; 256],
    exceeds_remaining: [u64; 256],
    zero_shares: [u64; 256],
    price_out_of_range: [u64; 256],
    quarantined: BTreeMap<String, u64>,
    new_ref_not_above_previous_max: u64,
    zero_share_crosses: u64,
    episodes: BTreeMap<String, EpisodeStats>,
    replace: ReplaceSemantics,
    ties: TimestampTies,
    identical_consecutive_frames: u64,
    locate_without_directory: u64,
    session: SessionReport,
}

impl QualityReporter {
    /// `selected_symbols` get their trading-state transitions listed individually.
    pub fn new(max_messages: Option<u64>, selected_symbols: &[String]) -> Self {
        Self {
            max_messages,
            selected_symbols: selected_symbols.to_vec(),
            books: OrderBooks::new(),
            locates: Vec::new(),
            phase: Phase::PreMarket,
            frames: 0,
            invalid_frames: 0,
            prev_frame: Vec::new(),
            prev_ts: None,
            end_ts: 0,
            max_new_ref: None,
            missing_ref: [0; 256],
            locate_mismatch: [0; 256],
            duplicate_ref: [0; 256],
            exceeds_remaining: [0; 256],
            zero_shares: [0; 256],
            price_out_of_range: [0; 256],
            quarantined: BTreeMap::new(),
            new_ref_not_above_previous_max: 0,
            zero_share_crosses: 0,
            episodes: BTreeMap::new(),
            replace: ReplaceSemantics::default(),
            ties: TimestampTies::default(),
            identical_consecutive_frames: 0,
            locate_without_directory: 0,
            session: SessionReport::default(),
        }
    }

    /// The reporter's own books, as of the last frame.
    pub fn books(&self) -> &OrderBooks {
        &self.books
    }

    fn state(&mut self, locate: u16) -> &mut LocateState {
        let i = usize::from(locate);
        if self.locates.len() <= i {
            self.locates.resize(i + 1, LocateState::default());
        }
        &mut self.locates[i]
    }

    /// Judges `locate`'s book as of its last batch, exactly like `OrderBooks` does.
    fn judge(&mut self, locate: u16) {
        let bid = self.books.best_bid(locate).map(|l| l.price.0);
        let ask = self.books.best_ask(locate).map(|l| l.price.0);
        let top = match (bid, ask) {
            (Some(b), Some(a)) if b > a => Top::Crossed,
            (Some(b), Some(a)) if b == a => Top::Locked,
            _ => Top::Normal,
        };
        let st = self.state(locate);
        st.dirty = false;
        if top == st.top {
            return;
        }
        let ts = st.batch_ts;
        let closed = st
            .episode
            .take()
            .map(|key| (key, ts.saturating_sub(st.top_since)));
        st.top = top;
        st.top_since = ts;
        if top != Top::Normal {
            st.episode = Some((top, st.trading_state, st.batch_phase));
        }
        let opened = st.episode;
        if let Some((key, spent)) = closed {
            self.episode_mut(key).total_ns += spent;
        }
        if let Some(key) = opened {
            self.episode_mut(key).entries += 1;
        }
    }

    fn episode_mut(&mut self, (top, state, phase): (Top, Option<u8>, Phase)) -> &mut EpisodeStats {
        let kind = if top == Top::Crossed {
            "crossed"
        } else {
            "locked"
        };
        let state = state.map_or_else(|| "none".to_string(), show_kind);
        self.episodes
            .entry(format!("{kind}/{state}/{}", phase.key()))
            .or_default()
    }

    /// Processes one message body (the bytes after its length prefix).
    pub fn push(&mut self, frame: &[u8]) {
        self.frames += 1;
        if !self.prev_frame.is_empty() && self.prev_frame == frame {
            self.identical_consecutive_frames += 1;
        }
        self.prev_frame.clear();
        self.prev_frame.extend_from_slice(frame);
        let Ok(msg) = decode(frame) else {
            self.invalid_frames += 1;
            return;
        };
        let Some(header) = msg.header() else {
            return;
        };
        let kind = frame[0];
        let k = usize::from(kind);
        let ts = header.timestamp_ns;
        let locate = header.stock_locate;
        self.end_ts = ts;
        if let Some(prev) = self.prev_ts {
            self.ties.compared += 1;
            self.ties.ties += u64::from(ts == prev);
            self.ties.regressions += u64::from(ts < prev);
        }
        self.prev_ts = Some(ts);
        if locate != 0
            && !matches!(msg, Message::StockDirectory(_))
            && !self.state(locate).directory
        {
            self.locate_without_directory += 1;
        }
        let book_message = matches!(kind, b'A' | b'F' | b'E' | b'C' | b'X' | b'D' | b'U');
        if book_message {
            let st = self.state(locate);
            if let Some(prev) = st.last_book_ts {
                self.ties.book_compared += 1;
                self.ties.book_ties += u64::from(prev == ts);
            }
            self.state(locate).last_book_ts = Some(ts);
        }

        // Value checks, read from the message itself so a quarantine reason taking precedence
        // cannot hide them.
        let out_of_range = |p: u32| p == 0 || p > MAX_PRICE4;
        let (zero, bad_price) = match msg {
            Message::AddOrder(m) => (m.shares == 0, out_of_range(m.price.0)),
            Message::OrderExecuted(m) => (m.shares == 0, false),
            Message::OrderExecutedWithPrice(m) => (m.shares == 0, out_of_range(m.price.0)),
            Message::OrderCancel(m) => (m.shares == 0, false),
            Message::OrderReplace(m) => (m.shares == 0, out_of_range(m.price.0)),
            Message::Trade(m) => (m.shares == 0, out_of_range(m.price.0)),
            Message::CrossTrade(m) => {
                self.zero_share_crosses += u64::from(m.shares == 0);
                (false, m.shares > 0 && out_of_range(m.price.0))
            }
            _ => (false, false),
        };
        self.zero_shares[k] += u64::from(zero);
        self.price_out_of_range[k] += u64::from(bad_price);

        let new_ref = match msg {
            Message::AddOrder(m) => Some(m.order_ref),
            Message::OrderReplace(m) => Some(m.new_ref),
            _ => None,
        };
        if let Some(r) = new_ref {
            if self.max_new_ref.is_some_and(|max| r <= max) {
                self.new_ref_not_above_previous_max += 1;
            }
            self.max_new_ref = Some(self.max_new_ref.map_or(r, |max| max.max(r)));
        }
        let original = match msg {
            Message::OrderReplace(m) => {
                self.replace.messages += 1;
                self.replace.new_ref_equals_original += u64::from(m.new_ref == m.original_ref);
                self.books.order(m.original_ref).map(|o| (o, m))
            }
            _ => None,
        };

        // Close the stock's previous batch before a message at a new timestamp changes it, as
        // `OrderBooks` does for book messages and Trading Actions.
        if book_message || matches!(msg, Message::TradingAction(_)) {
            let st = *self.state(locate);
            if st.dirty && st.batch_ts != ts {
                self.judge(locate);
            }
        }
        match msg {
            Message::SystemEvent(e) => {
                self.session.system_events.push(SystemEventRecord {
                    code: show_kind(e.event_code),
                    timestamp_ns: ts,
                });
                match e.event_code {
                    b'Q' => self.phase = Phase::Regular,
                    b'M' => self.phase = Phase::PostMarket,
                    _ => {}
                }
            }
            Message::StockDirectory(_) => self.state(locate).directory = true,
            Message::TradingAction(m) => {
                let from = self.state(locate).trading_state;
                if from == Some(m.trading_state) {
                    self.session.trading_state_repeats += 1;
                } else {
                    let name = |s: Option<u8>| s.map_or_else(|| "none".to_string(), show_kind);
                    *self
                        .session
                        .trading_state_transitions
                        .entry(format!("{}->{}", name(from), name(Some(m.trading_state))))
                        .or_default() += 1;
                    let symbol = m.stock.trimmed();
                    if self.selected_symbols.contains(&symbol) {
                        self.session.selected.push(StateTransition {
                            symbol,
                            timestamp_ns: ts,
                            from: name(from),
                            to: name(Some(m.trading_state)),
                            reason: String::from_utf8_lossy(m.reason).trim().to_string(),
                        });
                    }
                }
                self.state(locate).trading_state = Some(m.trading_state);
            }
            _ => {}
        }

        let applied = self.books.apply(&msg);
        match applied {
            Applied::Book => {
                let phase = self.phase;
                let st = self.state(locate);
                st.dirty = true;
                st.batch_ts = ts;
                st.batch_phase = phase;
                if let Some((o, m)) = original {
                    let r = &mut self.replace;
                    r.applied += 1;
                    let better = match o.side {
                        Side::Buy => m.price.0 > o.price.0,
                        Side::Sell => m.price.0 < o.price.0,
                    };
                    if m.price == o.price {
                        r.same_price += 1;
                    } else if better {
                        r.price_improved += 1;
                    } else {
                        r.price_worsened += 1;
                    }
                    match m.shares.cmp(&o.shares) {
                        std::cmp::Ordering::Greater => r.shares_increased += 1,
                        std::cmp::Ordering::Less => {
                            r.shares_decreased += 1;
                            r.same_price_size_decrease += u64::from(m.price == o.price);
                        }
                        std::cmp::Ordering::Equal => r.shares_unchanged += 1,
                    }
                }
            }
            Applied::Quarantined(q) => {
                *self.quarantined.entry(q.key().to_string()).or_default() += 1;
                match q {
                    Quarantine::UnknownOrder => self.missing_ref[k] += 1,
                    Quarantine::LocateMismatch => self.locate_mismatch[k] += 1,
                    Quarantine::DuplicateOrderRef => self.duplicate_ref[k] += 1,
                    Quarantine::ExceedsRemaining => self.exceeds_remaining[k] += 1,
                    Quarantine::ZeroShares
                    | Quarantine::InvalidPrice
                    | Quarantine::StockMismatch => {}
                }
            }
            Applied::Print | Applied::NoEffect => {}
        }
    }

    /// Judges open batches, closes open episodes at the last timestamp and assembles the report.
    pub fn finish(mut self) -> QualityReport {
        let end = self.end_ts;
        self.books.finish(end);
        for locate in 0..self.locates.len() {
            let locate = u16::try_from(locate).expect("locates are u16");
            if self.locates[usize::from(locate)].dirty {
                self.judge(locate);
            }
            let st = &mut self.locates[usize::from(locate)];
            if let Some(key) = st.episode {
                let open = end.saturating_sub(st.top_since);
                let e = self.episode_mut(key);
                e.total_ns += open;
                e.open_at_end += 1;
            }
        }
        let (mut crossed, mut locked) = (0, 0);
        for (key, e) in &self.episodes {
            if key.starts_with("crossed/") {
                crossed += e.entries;
            } else {
                locked += e.entries;
            }
        }
        let episodes_agree_with_book =
            crossed == self.books.crossed().count && locked == self.books.locked().count;
        let by_type = |a: &[u64; 256]| -> BTreeMap<String, u64> {
            (0..=255u8)
                .filter(|&k| a[usize::from(k)] > 0)
                .map(|k| (show_kind(k), a[usize::from(k)]))
                .collect()
        };
        let fraction = |n: u64, d: u64| (d > 0).then(|| n as f64 / d as f64);
        self.ties.fraction = fraction(self.ties.ties, self.ties.compared);
        self.ties.book_fraction = fraction(self.ties.book_ties, self.ties.book_compared);
        let total = |a: &[u64; 256]| a.iter().sum::<u64>();
        let episodes_while_trading = self
            .episodes
            .iter()
            .filter(|(key, _)| key.contains("/T/"))
            .map(|(_, e)| e.entries)
            .sum();
        let check = |id: &str, count: u64, definition: &str| ReportCheck {
            id: id.to_string(),
            count,
            passed: count == 0,
            definition: definition.to_string(),
        };
        let checks = vec![
            check(
                "frames.invalid",
                self.invalid_frames,
                "Frames whose length or coded fields contradict the specification.",
            ),
            check(
                "time.regression",
                self.ties.regressions,
                "Messages stamped below the previous message.",
            ),
            check(
                "sequence.identical_consecutive_frames",
                self.identical_consecutive_frames,
                "Frames byte-identical to the previous frame.",
            ),
            check(
                "sequence.locate_without_directory",
                self.locate_without_directory,
                "Stock messages whose locate had no Stock Directory entry yet.",
            ),
            check(
                "refs.missing",
                total(&self.missing_ref),
                "Modify messages naming an order reference that is not live.",
            ),
            check(
                "refs.locate_mismatch",
                total(&self.locate_mismatch),
                "Modify messages naming a live order of another stock.",
            ),
            check(
                "refs.duplicate_live",
                total(&self.duplicate_ref),
                "Adds or replaces reusing a live order reference.",
            ),
            check(
                "values.zero_shares",
                total(&self.zero_shares),
                "A, F, E, C, X, U or P messages for zero shares.",
            ),
            check(
                "values.price_out_of_range",
                total(&self.price_out_of_range),
                "Prices of 0 or above 200,000.0000 (A, F, U, C, P, Q with shares).",
            ),
            check(
                "values.exceeds_remaining",
                total(&self.exceeds_remaining),
                "Executions or cancels above the order's remaining shares.",
            ),
            check(
                "book.crossed_or_locked_while_trading",
                episodes_while_trading,
                "Crossed or locked entries while the stock's trading state was T.",
            ),
        ];
        QualityReport {
            frames: self.frames,
            invalid_frames: self.invalid_frames,
            episodes: self.episodes,
            episodes_agree_with_book,
            missing_ref_by_type: by_type(&self.missing_ref),
            locate_mismatch_by_type: by_type(&self.locate_mismatch),
            duplicate_live_ref_by_type: by_type(&self.duplicate_ref),
            new_ref_not_above_previous_max: self.new_ref_not_above_previous_max,
            zero_shares_by_type: by_type(&self.zero_shares),
            zero_share_crosses: self.zero_share_crosses,
            price_out_of_range_by_type: by_type(&self.price_out_of_range),
            exceeds_remaining_by_type: by_type(&self.exceeds_remaining),
            quarantined: self.quarantined,
            replace: self.replace,
            timestamps: self.ties,
            sequence: SequenceCheck {
                supported: false,
                basis: "Nasdaq BinaryFILE carries only a 2-byte length per message; the \
                        MoldUDP64 sequence numbers of the live feed are not in the file and the \
                        2-byte tracking number is an internal value, not a sequence. Gaps cannot \
                        be detected; framing, identical consecutive frames and locates without a \
                        directory entry are checked instead."
                    .to_string(),
                identical_consecutive_frames: self.identical_consecutive_frames,
                locate_without_directory: self.locate_without_directory,
            },
            session: self.session,
            checks,
        }
    }
}

impl FrameSink for QualityReporter {
    fn limit_reached(&self) -> bool {
        self.max_messages.is_some_and(|max| self.frames >= max)
    }

    fn push(&mut self, frame: &[u8]) {
        QualityReporter::push(self, frame);
    }
}
