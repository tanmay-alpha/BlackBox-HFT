//! Source-order L3 observations alongside the existing completed-batch L2 analytics.
//! Native summaries use every observation; display reduction never feeds a summary.
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

use bb_core::dataset::InstrumentMeta;
use bb_data::l3workbench::{EventRecord, Observation, Workbench};
use serde::{Deserialize, Serialize};

use crate::market::{MarketConfig, MarketReport, compute_market};

const BASIS: &str = "Selected-symbol ITCH source-order observations; reconstructed FIFO is not proof of exchange matching priority";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct L3Report {
    pub comparison: Comparison,
    pub metrics: Metrics,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = L3Comparison))]
pub struct Comparison {
    pub basis: String,
    pub events: u64,
    pub order_identity_observations: u64,
    pub touch_queue_observations: u64,
    pub execution_messages: u64,
    pub executions_with_order_identity: u64,
    pub cancel_messages: u64,
    pub cancels_ahead_of_reference: u64,
    pub cancels_at_reference: u64,
    pub cancels_behind_reference: u64,
    pub same_timestamp_add_cancel_level_groups: u64,
    pub l2_unobservable: Vec<String>,
    pub definitions: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = L3Metrics))]
pub struct Metrics {
    pub basis: String,
    pub definitions: BTreeMap<String, String>,
    pub counts: BTreeMap<String, u64>,
    pub rates_per_second: BTreeMap<String, Option<f64>>,
    pub order_lifetime_ns: LifetimeDistribution,
    pub queue_position_at_execution: QueuePositionDistribution,
    pub series: Vec<MetricPoint>,
    pub l2: MarketReport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LifetimeDistribution {
    pub n: u64,
    pub min: Option<String>,
    pub p50: Option<String>,
    pub p90: Option<String>,
    pub max: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QueuePositionDistribution {
    pub n: u64,
    pub min: Option<u32>,
    pub p50: Option<u32>,
    pub p90: Option<u32>,
    pub max: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MetricPoint {
    pub event_index: u64,
    pub ts_ns: Option<String>,
    pub best_bid_price4: Option<u32>,
    pub best_ask_price4: Option<u32>,
    pub live_orders: u64,
    pub bid_shares: u64,
    pub ask_shares: u64,
    pub order_count_imbalance: Option<f64>,
    pub signed_added_shares: i64,
    pub signed_cancelled_shares: i64,
    pub signed_executed_shares: i64,
    pub touch_depleted_shares: u64,
    pub touch_replenished_shares: u64,
}

impl From<&Observation> for MetricPoint {
    fn from(o: &Observation) -> Self {
        Self {
            event_index: o.event_index,
            ts_ns: o.ts_ns.clone(),
            best_bid_price4: o.best_bid_price4,
            best_ask_price4: o.best_ask_price4,
            live_orders: o.live_orders,
            bid_shares: o.bid_shares,
            ask_shares: o.ask_shares,
            order_count_imbalance: o.order_count_imbalance,
            signed_added_shares: o.signed_added_shares,
            signed_cancelled_shares: o.signed_cancelled_shares,
            signed_executed_shares: o.signed_executed_shares,
            touch_depleted_shares: o.touch_depleted_shares,
            touch_replenished_shares: o.touch_replenished_shares,
        }
    }
}

/// The points kept when `series` is reduced to at most `max_points`: the last source event in
/// each equal-width event-index bucket over [first_index, last_index + 1), never backdated.
/// Zero keeps none; one keeps only the final point. Borrowing, so a server can answer from a
/// shared report without copying it; [`Metrics::downsample`] is the in-place form.
pub fn downsampled(series: &[MetricPoint], max_points: usize) -> Vec<&MetricPoint> {
    if max_points == 0 {
        return Vec::new();
    }
    if series.len() <= max_points {
        return series.iter().collect();
    }
    let first = series[0].event_index;
    let span = u128::from(series[series.len() - 1].event_index - first) + 1;
    let bucket = |index: u64| u128::from(index - first) * max_points as u128 / span;
    series
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            series
                .get(i + 1)
                .is_none_or(|next| bucket(next.event_index) != bucket(p.event_index))
        })
        .map(|(_, p)| p)
        .collect()
}

impl Metrics {
    /// In-place [`downsampled`]: same points, same order.
    pub fn downsample(&mut self, max_points: usize) {
        let keep: Vec<MetricPoint> = downsampled(&self.series, max_points)
            .into_iter()
            .cloned()
            .collect();
        self.series = keep;
    }
}

/// Compute L2 analytics without fitting M6 to the evaluated session or inventing M10 windows.
/// An absent L2 book propagates the existing compute_market error, not a fabricated report.
pub fn compute(workbench: &Workbench) -> Result<L3Report, String> {
    let instrument = InstrumentMeta {
        tick_size: 0.0001,
        lot_size: 1.0,
        contract_size: 1.0,
    };
    let l2 = compute_market(
        &workbench.l2,
        &instrument,
        workbench.manifest.normalization.midnight_epoch_ns,
        &MarketConfig::default(),
    )
    .map_err(|error| error.to_string())?;
    aggregate(&workbench.events, &workbench.observations, l2)
}

fn definitions(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

fn timestamp(value: &Option<String>) -> Result<Option<u64>, String> {
    value
        .as_deref()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| "invalid session nanosecond timestamp".to_owned())
        })
        .transpose()
}

fn nearest_rank<T: Copy>(sorted: &[T], percentile: usize) -> Option<T> {
    if sorted.is_empty() {
        None
    } else {
        Some(sorted[(sorted.len() * percentile).div_ceil(100) - 1])
    }
}

fn lifetime_distribution(mut samples: Vec<u64>) -> LifetimeDistribution {
    samples.sort_unstable();
    LifetimeDistribution {
        n: samples.len() as u64,
        min: samples.first().map(u64::to_string),
        p50: nearest_rank(&samples, 50).map(|value| value.to_string()),
        p90: nearest_rank(&samples, 90).map(|value| value.to_string()),
        max: samples.last().map(u64::to_string),
    }
}

fn queue_distribution(mut samples: Vec<u32>) -> QueuePositionDistribution {
    samples.sort_unstable();
    QueuePositionDistribution {
        n: samples.len() as u64,
        min: samples.first().copied(),
        p50: nearest_rank(&samples, 50),
        p90: nearest_rank(&samples, 90),
        max: samples.last().copied(),
    }
}

/// Aggregate native source observations independently of L2 replay. Both slices are in source
/// event-index order; observations may omit events, but are never aligned by row position.
pub fn aggregate(
    events: &[EventRecord],
    observations: &[Observation],
    l2_report: MarketReport,
) -> Result<L3Report, String> {
    let mut comparison = Comparison {
        basis: BASIS.into(),
        events: events.len() as u64,
        order_identity_observations: 0,
        touch_queue_observations: 0,
        execution_messages: 0,
        executions_with_order_identity: 0,
        cancel_messages: 0,
        cancels_ahead_of_reference: 0,
        cancels_at_reference: 0,
        cancels_behind_reference: 0,
        same_timestamp_add_cancel_level_groups: 0,
        l2_unobservable: vec![
            "Resting order references and individual remaining shares".into(),
            "Reconstructed per-price FIFO position and cancellation location".into(),
            "Individual terminal order lifetimes and right censoring".into(),
            "Gross add/cancel activity netted within a timestamp-side-price batch".into(),
        ],
        definitions: definitions(&[
            (
                "events",
                "All selected source records, including non-book, invalid and quarantined records; physical frame indices may have gaps.",
            ),
            (
                "order_identity_observations",
                "Sum of engine-observed order identities across events; repeated observations of one order count again, not unique orders.",
            ),
            (
                "touch_queue_observations",
                "Sum of engine-observed post-event touch queue orders across events; repeated orders count again.",
            ),
            (
                "execution_messages",
                "Accepted book-effect E/C messages only; rejected messages and non-cross/cross prints are excluded.",
            ),
            (
                "executions_with_order_identity",
                "Accepted E/C messages with a resolved resting order reference; not shares or unique orders.",
            ),
            (
                "cancel_messages",
                "Accepted book-effect X/D messages only, including partial X; replacements U are excluded.",
            ),
            (
                "cancel_location",
                "For accepted X/D, compare the cancelled order's pre-event zero-based FIFO index with floor((queue_length-1)/2) at its own price: -1 ahead, 0 reference, +1 behind.",
            ),
            (
                "same_timestamp_add_cancel_level_groups",
                "Distinct (numeric session timestamp, side, price4) groups containing both accepted A/F and accepted X/D; count groups once, not messages. This is ambiguity of timestamp-batched L2 netting, not a claim that event-by-event L2 hides gross level changes.",
            ),
            (
                "queue_priority",
                "FIFO means reconstruction arrival order at one price, not evidence of the venue's matching rules.",
            ),
        ]),
    };
    let mut counts = BTreeMap::new();
    for key in [
        "events",
        "add_messages",
        "cancel_messages",
        "execution_messages",
        "replace_messages",
        "accepted_add_messages",
        "accepted_cancel_messages",
        "accepted_execution_messages",
        "accepted_replace_messages",
        "quarantined_messages",
        "invalid_messages",
        "added_shares",
        "cancelled_shares",
        "executed_shares",
        "touch_depleted_shares",
        "touch_replenished_shares",
        "terminal_orders",
        "right_censored_orders",
    ] {
        counts.insert(key.to_owned(), 0u64);
    }
    counts.insert("events".into(), events.len() as u64);
    let mut first_ns = None;
    let mut last_ns = None;
    let mut source_counts = BTreeMap::<&str, u64>::new();
    for event in events {
        if let Some(ts) = timestamp(&event.ts_ns)? {
            first_ns.get_or_insert(ts);
            last_ns = Some(ts);
        }
        *source_counts
            .entry(event.message_type.as_str())
            .or_default() += 1;
        let accepted = event.applied == "book";
        let category = match event.message_type.as_str() {
            "A" | "F" => Some(("add_messages", "accepted_add_messages")),
            "X" | "D" => Some(("cancel_messages", "accepted_cancel_messages")),
            "E" | "C" => Some(("execution_messages", "accepted_execution_messages")),
            "U" => Some(("replace_messages", "accepted_replace_messages")),
            _ => None,
        };
        if let Some((all, accepted_count)) = category {
            *counts.get_mut(all).unwrap() += 1;
            if accepted {
                *counts.get_mut(accepted_count).unwrap() += 1;
            }
        }
        if accepted && matches!(event.message_type.as_str(), "E" | "C") {
            comparison.execution_messages += 1;
            comparison.executions_with_order_identity += u64::from(event.order_ref.is_some());
        }
        if accepted && matches!(event.message_type.as_str(), "X" | "D") {
            comparison.cancel_messages += 1;
        }
        match event.applied.as_str() {
            "quarantined" => *counts.get_mut("quarantined_messages").unwrap() += 1,
            "invalid" => *counts.get_mut("invalid_messages").unwrap() += 1,
            _ => {}
        }
    }
    for (kind, count) in source_counts {
        counts.insert(format!("messages_{kind}"), count);
    }

    let mut lifetimes = Vec::new();
    let mut execution_positions = Vec::new();
    let mut groups = BTreeMap::<(u64, u8, u32), u8>::new();
    let mut source = events.iter().peekable();
    for observation in observations {
        comparison.order_identity_observations += observation.order_identity_observations;
        comparison.touch_queue_observations += observation.touch_queue_observations;
        for (key, shares) in [
            (
                "added_shares",
                observation.signed_added_shares.unsigned_abs(),
            ),
            (
                "cancelled_shares",
                observation.signed_cancelled_shares.unsigned_abs(),
            ),
            (
                "executed_shares",
                observation.signed_executed_shares.unsigned_abs(),
            ),
            ("touch_depleted_shares", observation.touch_depleted_shares),
            (
                "touch_replenished_shares",
                observation.touch_replenished_shares,
            ),
        ] {
            *counts.get_mut(key).unwrap() += shares;
        }
        if let Some(lifetime) = observation.lifetime_ns {
            lifetimes.push(lifetime);
        }
        while source
            .peek()
            .is_some_and(|event| event.event_index < observation.event_index)
        {
            source.next();
        }
        if let Some(event) = source
            .peek()
            .filter(|event| event.event_index == observation.event_index)
            && event.applied == "book"
        {
            if matches!(event.message_type.as_str(), "E" | "C")
                && let Some(position) = observation.executed_queue_position
            {
                execution_positions.push(position);
            }
            if matches!(event.message_type.as_str(), "X" | "D") {
                match observation.cancel_location {
                    Some(-1) => comparison.cancels_ahead_of_reference += 1,
                    Some(0) => comparison.cancels_at_reference += 1,
                    Some(1) => comparison.cancels_behind_reference += 1,
                    _ => {}
                }
            }
        }
        if (observation.accepted_add || observation.accepted_cancel)
            && let (Some(ts), Some((side, price))) =
                (timestamp(&observation.ts_ns)?, observation.level_key)
        {
            let flags =
                u8::from(observation.accepted_add) | (u8::from(observation.accepted_cancel) << 1);
            *groups.entry((ts, side, price)).or_default() |= flags;
        }
    }
    comparison.same_timestamp_add_cancel_level_groups =
        groups.values().filter(|flags| **flags == 3).count() as u64;
    counts.insert("terminal_orders".into(), lifetimes.len() as u64);
    counts.insert(
        "right_censored_orders".into(),
        observations.last().map_or(0, |o| o.live_orders),
    );
    let seconds = first_ns
        .zip(last_ns)
        .and_then(|(first, last)| last.checked_sub(first))
        .filter(|span| *span > 0)
        .map(|span| span as f64 / 1_000_000_000.0);
    let rates_per_second = counts
        .iter()
        .map(|(key, count)| (key.clone(), seconds.map(|seconds| *count as f64 / seconds)))
        .collect();
    Ok(L3Report {
        comparison,
        metrics: Metrics {
            basis: BASIS.into(),
            definitions: definitions(&[
                (
                    "counts",
                    "events and messages_<ITCH type> count all source records regardless of acceptance. add/cancel/execution/replace_messages group A/F, X/D, E/C, U; accepted_* require applied=book. quarantined_messages and invalid_messages count source outcomes.",
                ),
                (
                    "rates_per_second",
                    "Each count divided by (last timestamp - first timestamp)/1e9, using non-null numeric session timestamps in source order, not epoch or min/max timestamps. Null if absent, zero or negative span. terminal_orders and right_censored_orders rates are descriptive count/span, not survival hazards.",
                ),
                (
                    "order_lifetime_ns",
                    "Fully removed order identities only, from accepted add/replace entry to terminal execute/cancel/delete/replace event. Nanoseconds are decimal strings; nearest-rank p50/p90 use sorted sample ceil(p*n), no interpolation. Live orders are excluded and counted as right_censored_orders; unknown/regressing lifetimes are not fabricated.",
                ),
                (
                    "queue_position_at_execution",
                    "Pre-event 1-based reconstructed FIFO position at its own price for accepted E/C messages, including partial executions and non-printable C. One sample per message, not share-weighted; nearest-rank p50/p90. No sample from rejected messages or P/Q prints.",
                ),
                (
                    "series",
                    "Post-event whole-book state and per-event flows in source order, never cumulative flows. Display downsampling keeps the final observation per equal event-index bucket across [first,last+1), includes final, and never backdates a later bucket's state. Exact summaries use unreduced observations.",
                ),
                (
                    "order_count_imbalance",
                    "(live bid orders - live ask orders)/(live bid orders + live ask orders) across the whole selected-symbol book; null when no live orders.",
                ),
                (
                    "bid_shares",
                    "Remaining shares across every live selected-symbol bid level, not just the touch; ask_shares is the analogous ask total.",
                ),
                (
                    "signed_flows",
                    "Accepted observed share effects per event, buy positive/sell negative. added_shares/cancelled_shares/executed_shares counts sum the absolute signed per-event flows, never the absolute net session flow. Replacements contribute their observed removal and insertion, not an inferred trade.",
                ),
                (
                    "touch_flows",
                    "touch_depleted_shares/touch_replenished_shares sum loss/gain at pre-event best bid/ask prices. A new touch after a price move is not treated as replenishment of the old touch.",
                ),
                (
                    "l2",
                    "Unmodified compute_market on the normalized L2 rows, tick_size=0.0001, lot_size=1, contract_size=1 and default MarketConfig. Completed exchange-timestamp batches; M6 unavailable without independent calibration; no M10 recovery inferred without explicit shock windows.",
                ),
            ]),
            counts,
            rates_per_second,
            order_lifetime_ns: lifetime_distribution(lifetimes),
            queue_position_at_execution: queue_distribution(execution_positions),
            series: observations.iter().map(MetricPoint::from).collect(),
            l2: l2_report,
        },
    })
}

// ---- Touch metrics from an L3 (ITCH) book --------------------------------------------------
//
// Metrics that need the order-by-order book: the exact touch before and after every book
// message, the message kind and the executed resting side. Prices stay ITCH Price(4) integers
// (1/10,000 USD; the Nasdaq tick of $0.01 is 100 units) and timestamps are exchange-clock
// nanoseconds (ITCH: since midnight Eastern) as i64. Ratios are computed from exact integers and
// only then converted to f64. docs/research/ITCH_VALIDATION_PIPELINE.md gives the formulas.

const TOUCH_BASIS: &str = "One symbol's reconstructed L3 book, touch before and after each accepted book message (A, F, E, C, X, D, U) in source order; ITCH exchange clock; Price(4) integers";

/// Best displayed price (ITCH Price(4) integer) and the shares displayed at it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    pub price4: u32,
    pub shares: u64,
}

/// Best bid and ask of one book; a side is `None` when empty.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Touch {
    pub bid: Option<Quote>,
    pub ask: Option<Quote>,
}

impl Touch {
    /// Both sides, only when the book is two-sided and neither locked nor crossed (bid < ask).
    pub fn uncrossed(&self) -> Option<(Quote, Quote)> {
        let (bid, ask) = (self.bid?, self.ask?);
        (bid.price4 < ask.price4).then_some((bid, ask))
    }

    /// Twice the midprice, `bid + ask`, in Price(4) units: exact for any pair of prices.
    pub fn mid2(&self) -> Option<i64> {
        self.uncrossed()
            .map(|(b, a)| i64::from(b.price4) + i64::from(a.price4))
    }

    /// Quoted spread `ask - bid` in Price(4) units.
    pub fn spread_price4(&self) -> Option<u32> {
        self.uncrossed().map(|(b, a)| a.price4 - b.price4)
    }

    /// Relative spread `(ask - bid) / mid` in basis points: `20,000 (a - b) / (a + b)`.
    pub fn relative_spread_bps(&self) -> Option<f64> {
        self.uncrossed().map(|(b, a)| {
            20_000.0 * f64::from(a.price4 - b.price4) / (f64::from(b.price4) + f64::from(a.price4))
        })
    }

    /// Microprice `(b Qa + a Qb) / (Qa + Qb)` as an exact fraction (numerator, denominator) in
    /// Price(4) units. Levels in a book always hold shares, so the denominator is positive.
    pub fn microprice(&self) -> Option<(u128, u128)> {
        let (b, a) = self.uncrossed()?;
        let den = u128::from(b.shares) + u128::from(a.shares);
        (den > 0).then(|| {
            (
                u128::from(b.price4) * u128::from(a.shares)
                    + u128::from(a.price4) * u128::from(b.shares),
                den,
            )
        })
    }

    /// Microprice minus midprice in Price(4) units: `(a - b)(Qb - Qa) / (2 (Qa + Qb))`.
    pub fn microprice_offset_price4(&self) -> Option<f64> {
        let (b, a) = self.uncrossed()?;
        let den = 2 * (i128::from(b.shares) + i128::from(a.shares));
        (den > 0).then(|| {
            (i128::from(a.price4 - b.price4) * (i128::from(b.shares) - i128::from(a.shares))) as f64
                / den as f64
        })
    }
}

/// Order-flow imbalance contribution of one touch change (Cont, Kukanov and Stoikov 2014,
/// "The price impact of order book events", eq. 2), in shares:
/// `e = 1{Pb' >= Pb} qb' - 1{Pb' <= Pb} qb - 1{Pa' <= Pa} qa' + 1{Pa' >= Pa} qa`,
/// unprimed before and primed after. `None` unless both states are uncrossed.
pub fn order_flow_imbalance(before: &Touch, after: &Touch) -> Option<i64> {
    let (b0, a0) = before.uncrossed()?;
    let (b1, a1) = after.uncrossed()?;
    let q = |x: u64| i64::try_from(x).unwrap_or(i64::MAX);
    let mut e = 0i64;
    if b1.price4 >= b0.price4 {
        e += q(b1.shares);
    }
    if b1.price4 <= b0.price4 {
        e -= q(b0.shares);
    }
    if a1.price4 <= a0.price4 {
        e -= q(a1.shares);
    }
    if a1.price4 >= a0.price4 {
        e += q(a0.shares);
    }
    Some(e)
}

/// The kind of an accepted book message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TouchEventKind {
    /// A or F.
    Add,
    /// E or C (printable or not: both remove displayed shares).
    Execute,
    /// X.
    PartialCancel,
    /// D.
    Delete,
    /// U.
    Replace,
}

impl TouchEventKind {
    const ALL: [TouchEventKind; 5] = [
        TouchEventKind::Add,
        TouchEventKind::Execute,
        TouchEventKind::PartialCancel,
        TouchEventKind::Delete,
        TouchEventKind::Replace,
    ];

    pub fn key(self) -> &'static str {
        match self {
            TouchEventKind::Add => "add",
            TouchEventKind::Execute => "execute",
            TouchEventKind::PartialCancel => "partial_cancel",
            TouchEventKind::Delete => "delete",
            TouchEventKind::Replace => "replace",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One accepted book message of the measured symbol.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TouchEvent {
    /// Exchange timestamp, nanoseconds.
    pub ts_ns: i64,
    pub kind: TouchEventKind,
    /// Side of the resting order an execution hit; the aggressor is the other side.
    pub resting_side: Option<bb_data::itch::decode::Side>,
    /// Shares executed (Execute), cancelled (PartialCancel) or deleted (Delete); new shares for
    /// Add and Replace.
    pub shares: u64,
    pub before: Touch,
    pub after: Touch,
    /// Displayed shares left after the message at the pre-message best bid price (0 if that
    /// level is gone); equals the pre-message touch shares when the level was not touched.
    pub bid_after_at_before_price: u64,
    pub ask_after_at_before_price: u64,
}

/// What is measured and over which exchange-time window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TouchMetricsConfig {
    /// `[start, end)` in exchange nanoseconds; only messages inside it are measured.
    pub window_start_ns: i64,
    pub window_end_ns: i64,
    /// Markout horizons after each execution, exchange nanoseconds.
    pub markout_horizons_ns: Vec<i64>,
    /// Bucket length of the order-flow-imbalance regression.
    pub ofi_bucket_ns: i64,
}

impl TouchMetricsConfig {
    /// 100 ms, 1 s and 5 s markouts; 1 s OFI buckets.
    pub fn new(window_start_ns: i64, window_end_ns: i64) -> Self {
        Self {
            window_start_ns,
            window_end_ns,
            markout_horizons_ns: vec![100_000_000, 1_000_000_000, 5_000_000_000],
            ofi_bucket_ns: 1_000_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ActivityRates {
    /// Accepted messages per kind inside the window.
    pub messages: BTreeMap<String, u64>,
    /// Shares per kind (executed, cancelled, deleted; added and replaced count new shares).
    pub shares: BTreeMap<String, u64>,
    /// `messages / (span_ns / 1e9)`; null when the span is not positive.
    pub messages_per_second: BTreeMap<String, Option<f64>>,
    /// `(partial_cancel + delete) / execute` messages; null without executions.
    pub cancel_to_execution_ratio: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpreadStats {
    /// Post-message states with an uncrossed two-sided touch.
    pub n: u64,
    pub one_sided: u64,
    pub locked_or_crossed: u64,
    pub event_mean: Option<f64>,
    /// Weighted by exchange time each state lasted inside the window.
    pub time_weighted_mean: Option<f64>,
    /// Exchange time inside the window with no uncrossed two-sided touch.
    pub undefined_ns: i64,
    pub p10: Option<f64>,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotedSpread {
    pub n: u64,
    pub mean_price4: Option<f64>,
    pub p10_price4: Option<u32>,
    pub p50_price4: Option<u32>,
    pub p90_price4: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MicropriceStats {
    pub n: u64,
    /// Mean of microprice minus midprice, Price(4) units.
    pub mean_offset_price4: Option<f64>,
    pub mean_abs_offset_price4: Option<f64>,
    /// States with the microprice above, at and below the midprice.
    pub above_mid: u64,
    pub at_mid: u64,
    pub below_mid: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OfiStats {
    pub events_used: u64,
    /// Messages where the touch before or after was one-sided, locked or crossed.
    pub events_skipped: u64,
    pub total_shares: i64,
    pub bucket_ns: i64,
    /// Buckets holding at least one used message.
    pub buckets: u64,
    /// OLS of the bucket's summed midprice change (Price(4)) on its OFI (shares), with
    /// intercept: slope and Pearson correlation. Null with fewer than two buckets or no OFI
    /// variance.
    pub slope_price4_per_share: Option<f64>,
    pub intercept_price4: Option<f64>,
    pub correlation: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TouchFlows {
    /// Shares leaving the pre-message best level, by message kind.
    pub depleted_shares: BTreeMap<String, u64>,
    pub depleted_total: u64,
    /// Shares joining the pre-message best level (a new better level is not replenishment).
    pub replenished_shares: BTreeMap<String, u64>,
    pub replenished_total: u64,
    /// Messages that removed shares from the touch, by kind.
    pub depleting_messages: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MarkoutStats {
    pub horizon_ns: i64,
    /// Executions with a defined midprice before the execution and at the horizon.
    pub n: u64,
    /// Mean of `s (mid(t+h) - mid(t-))`, s = +1 when the aggressor bought (resting sell).
    pub mean_price4: Option<f64>,
    /// Same, divided by `mid(t-)`, basis points.
    pub mean_bps: Option<f64>,
    /// Weighted by executed shares.
    pub share_weighted_mean_price4: Option<f64>,
    pub positive: u64,
    pub zero: u64,
    pub negative: u64,
    pub undefined_at_execution: u64,
    pub undefined_at_horizon: u64,
    /// `t + h` beyond the last observed exchange time.
    pub censored: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TouchMetricsReport {
    pub basis: String,
    pub definitions: BTreeMap<String, String>,
    pub window_ns: [i64; 2],
    pub observed_until_ns: i64,
    /// `min(window_end, observed_until) - window_start`.
    pub span_ns: i64,
    pub events_in_window: u64,
    pub activity: ActivityRates,
    pub relative_spread_bps: SpreadStats,
    pub quoted_spread: QuotedSpread,
    pub microprice: MicropriceStats,
    pub order_flow_imbalance: OfiStats,
    pub touch_flows: TouchFlows,
    pub markouts: Vec<MarkoutStats>,
}

#[derive(Copy, Clone, Debug)]
struct PendingMarkout {
    deadline: i64,
    sign: i64,
    mid2: i64,
    shares: u64,
}

#[derive(Clone, Debug, Default)]
struct MarkoutSums {
    n: u64,
    sum_half: i128,
    sum_bps: f64,
    sum_share_half: i128,
    shares: u128,
    positive: u64,
    zero: u64,
    negative: u64,
    undefined_at_execution: u64,
    undefined_at_horizon: u64,
    censored: u64,
}

/// Streaming accumulator of [`TouchMetricsReport`]. Push every accepted book message of the
/// symbol in source order (including those before the window, so the touch is right when it
/// opens, and after it, so markouts can resolve), then [`finish`](Self::finish).
#[derive(Clone, Debug)]
pub struct TouchMetrics {
    config: TouchMetricsConfig,
    current: Touch,
    current_since: Option<i64>,
    events_in_window: u64,
    messages: [u64; 5],
    shares: [u64; 5],
    rel_spreads: Vec<f64>,
    spreads: Vec<u32>,
    one_sided: u64,
    locked_or_crossed: u64,
    tw_sum: f64,
    tw_ns: i64,
    tw_undefined_ns: i64,
    micro_n: u64,
    micro_sum: f64,
    micro_abs: f64,
    micro_sign: [u64; 3],
    ofi_used: u64,
    ofi_skipped: u64,
    ofi_total: i64,
    ofi_buckets: BTreeMap<i64, (i64, i64)>,
    depleted: [u64; 5],
    replenished: [u64; 5],
    depleting: [u64; 5],
    pending: Vec<VecDeque<PendingMarkout>>,
    markouts: Vec<MarkoutSums>,
}

impl TouchMetrics {
    pub fn new(config: TouchMetricsConfig) -> Self {
        let horizons = config.markout_horizons_ns.len();
        Self {
            config,
            current: Touch::default(),
            current_since: None,
            events_in_window: 0,
            messages: [0; 5],
            shares: [0; 5],
            rel_spreads: Vec::new(),
            spreads: Vec::new(),
            one_sided: 0,
            locked_or_crossed: 0,
            tw_sum: 0.0,
            tw_ns: 0,
            tw_undefined_ns: 0,
            micro_n: 0,
            micro_sum: 0.0,
            micro_abs: 0.0,
            micro_sign: [0; 3],
            ofi_used: 0,
            ofi_skipped: 0,
            ofi_total: 0,
            ofi_buckets: BTreeMap::new(),
            depleted: [0; 5],
            replenished: [0; 5],
            depleting: [0; 5],
            pending: vec![VecDeque::new(); horizons],
            markouts: vec![MarkoutSums::default(); horizons],
        }
    }

    fn in_window(&self, ts: i64) -> bool {
        (self.config.window_start_ns..self.config.window_end_ns).contains(&ts)
    }

    /// Weights the current state over `[current_since, until)` clipped to the window.
    fn weigh_until(&mut self, until: i64) {
        // Before the first symbol event the book is empty, not outside the observed window.
        let since = self.current_since.unwrap_or(self.config.window_start_ns);
        let from = since.max(self.config.window_start_ns);
        let to = until.min(self.config.window_end_ns);
        if to <= from {
            return;
        }
        let dt = to - from;
        match self.current.relative_spread_bps() {
            Some(bps) => {
                self.tw_sum += bps * dt as f64;
                self.tw_ns += dt;
            }
            None => self.tw_undefined_ns += dt,
        }
    }

    /// Resolves markouts whose horizon `t + h` lies before `ts` (or at it, when `inclusive`):
    /// the current state is then the book after every message stamped at most `t + h`.
    fn resolve(&mut self, ts: i64, inclusive: bool) {
        let mid2 = self.current.mid2();
        for (queue, sums) in self.pending.iter_mut().zip(&mut self.markouts) {
            while let Some(p) = queue.front().copied() {
                if p.deadline > ts || (p.deadline == ts && !inclusive) {
                    break;
                }
                queue.pop_front();
                let Some(m2) = mid2 else {
                    sums.undefined_at_horizon += 1;
                    continue;
                };
                let half = p.sign * (m2 - p.mid2);
                sums.n += 1;
                sums.sum_half += i128::from(half);
                sums.sum_bps += 10_000.0 * half as f64 / p.mid2 as f64;
                sums.sum_share_half += i128::from(half) * i128::from(p.shares);
                sums.shares += u128::from(p.shares);
                match half.signum() {
                    1 => sums.positive += 1,
                    0 => sums.zero += 1,
                    _ => sums.negative += 1,
                }
            }
        }
    }

    pub fn push(&mut self, e: &TouchEvent) {
        self.resolve(e.ts_ns, false);
        self.weigh_until(e.ts_ns);
        if self.in_window(e.ts_ns) {
            self.measure(e);
        }
        self.current = e.after;
        self.current_since = Some(e.ts_ns);
    }

    fn measure(&mut self, e: &TouchEvent) {
        let k = e.kind.index();
        self.events_in_window += 1;
        self.messages[k] += 1;
        self.shares[k] += e.shares;

        if e.after.bid.is_none() || e.after.ask.is_none() {
            self.one_sided += 1;
        } else if let Some((b, a)) = e.after.uncrossed() {
            self.spreads.push(a.price4 - b.price4);
            self.rel_spreads.extend(e.after.relative_spread_bps());
            if let Some(offset) = e.after.microprice_offset_price4() {
                self.micro_n += 1;
                self.micro_sum += offset;
                self.micro_abs += offset.abs();
                self.micro_sign[match b.shares.cmp(&a.shares) {
                    Ordering::Greater => 0,
                    Ordering::Equal => 1,
                    Ordering::Less => 2,
                }] += 1;
            }
        } else {
            self.locked_or_crossed += 1;
        }

        match (
            order_flow_imbalance(&e.before, &e.after),
            e.before.mid2(),
            e.after.mid2(),
        ) {
            (Some(ofi), Some(m0), Some(m1)) => {
                self.ofi_used += 1;
                self.ofi_total += ofi;
                let bucket = self
                    .ofi_buckets
                    .entry(e.ts_ns.div_euclid(self.config.ofi_bucket_ns))
                    .or_default();
                bucket.0 += ofi;
                bucket.1 += m1 - m0;
            }
            _ => self.ofi_skipped += 1,
        }

        let mut depleted = false;
        for (quote, left) in [
            (e.before.bid, e.bid_after_at_before_price),
            (e.before.ask, e.ask_after_at_before_price),
        ] {
            if let Some(q) = quote {
                if left < q.shares {
                    self.depleted[k] += q.shares - left;
                    depleted = true;
                } else {
                    self.replenished[k] += left - q.shares;
                }
            }
        }
        self.depleting[k] += u64::from(depleted);

        if e.kind == TouchEventKind::Execute
            && let Some(resting) = e.resting_side
        {
            let sign = match resting {
                bb_data::itch::decode::Side::Sell => 1,
                bb_data::itch::decode::Side::Buy => -1,
            };
            let mid2 = e.before.mid2();
            for (i, &h) in self.config.markout_horizons_ns.iter().enumerate() {
                match mid2 {
                    Some(mid2) => self.pending[i].push_back(PendingMarkout {
                        deadline: e.ts_ns.saturating_add(h),
                        sign,
                        mid2,
                        shares: e.shares,
                    }),
                    None => self.markouts[i].undefined_at_execution += 1,
                }
            }
        }
    }

    /// `observed_until_ns`: the last exchange time the input covers (for a whole feed, the
    /// last message of any stock); the symbol's book is known up to it. Later horizons are
    /// censored.
    pub fn finish(mut self, observed_until_ns: i64) -> TouchMetricsReport {
        let until = observed_until_ns.max(self.current_since.unwrap_or(observed_until_ns));
        self.resolve(until, true);
        self.weigh_until(until);
        for (queue, sums) in self.pending.iter().zip(&mut self.markouts) {
            sums.censored += queue.len() as u64;
        }
        let c = &self.config;
        let span_ns = until.min(c.window_end_ns) - c.window_start_ns;
        let seconds = (span_ns > 0).then(|| span_ns as f64 / 1e9);
        let mean = |sum: f64, n: u64| (n > 0).then(|| sum / n as f64);
        let keyed = |a: &[u64; 5]| -> BTreeMap<String, u64> {
            TouchEventKind::ALL
                .iter()
                .map(|k| (k.key().to_string(), a[k.index()]))
                .collect()
        };

        let m = &self.messages;
        let executions = m[TouchEventKind::Execute.index()];
        let cancels = m[TouchEventKind::PartialCancel.index()] + m[TouchEventKind::Delete.index()];
        let activity = ActivityRates {
            messages: keyed(m),
            shares: keyed(&self.shares),
            messages_per_second: TouchEventKind::ALL
                .iter()
                .map(|k| {
                    (
                        k.key().to_string(),
                        seconds.map(|s| m[k.index()] as f64 / s),
                    )
                })
                .collect(),
            cancel_to_execution_ratio: (executions > 0).then(|| cancels as f64 / executions as f64),
        };

        let mut rel = std::mem::take(&mut self.rel_spreads);
        rel.sort_by(f64::total_cmp);
        let relative_spread_bps = SpreadStats {
            n: rel.len() as u64,
            one_sided: self.one_sided,
            locked_or_crossed: self.locked_or_crossed,
            event_mean: mean(rel.iter().sum(), rel.len() as u64),
            time_weighted_mean: (self.tw_ns > 0).then(|| self.tw_sum / self.tw_ns as f64),
            undefined_ns: self.tw_undefined_ns,
            p10: nearest_rank(&rel, 10),
            p50: nearest_rank(&rel, 50),
            p90: nearest_rank(&rel, 90),
        };
        let mut spreads = std::mem::take(&mut self.spreads);
        spreads.sort_unstable();
        let quoted_spread = QuotedSpread {
            n: spreads.len() as u64,
            mean_price4: mean(
                spreads.iter().map(|&s| f64::from(s)).sum(),
                spreads.len() as u64,
            ),
            p10_price4: nearest_rank(&spreads, 10),
            p50_price4: nearest_rank(&spreads, 50),
            p90_price4: nearest_rank(&spreads, 90),
        };
        let microprice = MicropriceStats {
            n: self.micro_n,
            mean_offset_price4: mean(self.micro_sum, self.micro_n),
            mean_abs_offset_price4: mean(self.micro_abs, self.micro_n),
            above_mid: self.micro_sign[0],
            at_mid: self.micro_sign[1],
            below_mid: self.micro_sign[2],
        };

        let points: Vec<(f64, f64)> = self
            .ofi_buckets
            .values()
            .map(|&(ofi, dmid2)| (ofi as f64, dmid2 as f64 / 2.0))
            .collect();
        let (slope, intercept, correlation) = ols(&points);
        let order_flow_imbalance = OfiStats {
            events_used: self.ofi_used,
            events_skipped: self.ofi_skipped,
            total_shares: self.ofi_total,
            bucket_ns: c.ofi_bucket_ns,
            buckets: points.len() as u64,
            slope_price4_per_share: slope,
            intercept_price4: intercept,
            correlation,
        };
        let touch_flows = TouchFlows {
            depleted_shares: keyed(&self.depleted),
            depleted_total: self.depleted.iter().sum(),
            replenished_shares: keyed(&self.replenished),
            replenished_total: self.replenished.iter().sum(),
            depleting_messages: keyed(&self.depleting),
        };
        let markouts = c
            .markout_horizons_ns
            .iter()
            .zip(&self.markouts)
            .map(|(&horizon_ns, s)| MarkoutStats {
                horizon_ns,
                n: s.n,
                mean_price4: (s.n > 0).then(|| s.sum_half as f64 / (2.0 * s.n as f64)),
                mean_bps: mean(s.sum_bps, s.n),
                share_weighted_mean_price4: (s.shares > 0)
                    .then(|| s.sum_share_half as f64 / (2.0 * s.shares as f64)),
                positive: s.positive,
                zero: s.zero,
                negative: s.negative,
                undefined_at_execution: s.undefined_at_execution,
                undefined_at_horizon: s.undefined_at_horizon,
                censored: s.censored,
            })
            .collect();

        TouchMetricsReport {
            basis: TOUCH_BASIS.into(),
            definitions: definitions(&[
                (
                    "relative_spread_bps",
                    "20000 (ask - bid) / (ask + bid) after each in-window message with an uncrossed two-sided touch; event mean, nearest-rank percentiles, and the time-weighted mean over exchange time inside [window_start, min(window_end, observed_until)).",
                ),
                (
                    "quoted_spread",
                    "ask - bid in Price(4) units (100 = $0.01) after each in-window message with an uncrossed touch.",
                ),
                (
                    "microprice",
                    "(bid Qask + ask Qbid)/(Qbid + Qask); reported as its offset from the midprice, (ask - bid)(Qbid - Qask)/(2(Qbid + Qask)), Price(4) units, after each in-window message with an uncrossed touch.",
                ),
                (
                    "order_flow_imbalance",
                    "Cont-Kukanov-Stoikov e = 1{Pb'>=Pb}qb' - 1{Pb'<=Pb}qb - 1{Pa'<=Pa}qa' + 1{Pa'>=Pa}qa per message, shares, when the touch before and after is uncrossed; summed per bucket_ns exchange-time bucket and regressed (OLS with intercept) against the bucket's summed midprice change in Price(4).",
                ),
                (
                    "activity",
                    "Accepted book messages and shares per kind inside the window; per-second rates divide by span_ns/1e9.",
                ),
                (
                    "touch_flows",
                    "Per message, shares lost (depleted) or gained (replenished) at the pre-message best bid and ask prices, attributed to the message kind; a better new level is not replenishment of the old one.",
                ),
                (
                    "markouts",
                    "Per in-window execution (E or C) with an uncrossed touch just before it: s (mid(t+h) - mid(t-)), s = +1 when the resting order was a sell (aggressor bought), -1 otherwise; mid(t+h) is the touch after every message stamped at most t+h. Undefined midprices are counted, not filled; horizons beyond observed_until are censored.",
                ),
            ]),
            window_ns: [c.window_start_ns, c.window_end_ns],
            observed_until_ns: until,
            span_ns,
            events_in_window: self.events_in_window,
            activity,
            relative_spread_bps,
            quoted_spread,
            microprice,
            order_flow_imbalance,
            touch_flows,
            markouts,
        }
    }
}

/// OLS of y on x with intercept: (slope, intercept, Pearson correlation). Null with fewer than
/// two points or no variance in x; correlation null without variance in y.
fn ols(points: &[(f64, f64)]) -> (Option<f64>, Option<f64>, Option<f64>) {
    if points.len() < 2 {
        return (None, None, None);
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &(x, y) in points {
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
        sxy += (x - mx) * (y - my);
    }
    if sxx == 0.0 {
        return (None, None, None);
    }
    let slope = sxy / sxx;
    let r = (syy > 0.0).then(|| sxy / (sxx * syy).sqrt());
    (Some(slope), Some(my - slope * mx), r)
}

#[cfg(test)]
mod touch_tests {
    use super::*;
    use bb_data::itch::decode::Side;

    fn touch(bid: Option<(u32, u64)>, ask: Option<(u32, u64)>) -> Touch {
        let q = |(price4, shares)| Quote { price4, shares };
        Touch {
            bid: bid.map(q),
            ask: ask.map(q),
        }
    }

    fn event(ts_ns: i64, kind: TouchEventKind, before: Touch, after: Touch) -> TouchEvent {
        TouchEvent {
            ts_ns,
            kind,
            resting_side: None,
            shares: 0,
            before,
            after,
            bid_after_at_before_price: before.bid.map_or(0, |q| q.shares),
            ask_after_at_before_price: before.ask.map_or(0, |q| q.shares),
        }
    }

    #[test]
    fn touch_values_are_exact_and_undefined_when_one_sided_locked_or_crossed() {
        // $10.00 x $10.02, 300 x 100 shares.
        let t = touch(Some((100_000, 300)), Some((100_200, 100)));
        assert_eq!(t.mid2(), Some(200_200));
        assert_eq!(t.spread_price4(), Some(200));
        // 20000 * 200 / 200200 = 19.98001998... bps
        assert!((t.relative_spread_bps().unwrap() - 4_000_000.0 / 200_200.0).abs() < 1e-12);
        // (100000*100 + 100200*300) / 400 = 100150 exactly.
        assert_eq!(t.microprice(), Some((40_060_000, 400)));
        // (200)(300-100)/(2*400) = 50 = 100150 - 100100.
        assert_eq!(t.microprice_offset_price4(), Some(50.0));
        for undefined in [
            touch(Some((100_000, 1)), None),
            touch(None, Some((100_000, 1))),
            touch(None, None),
            touch(Some((100_000, 1)), Some((100_000, 1))),
            touch(Some((100_100, 1)), Some((100_000, 1))),
        ] {
            assert_eq!(undefined.mid2(), None);
            assert_eq!(undefined.relative_spread_bps(), None);
            assert_eq!(undefined.microprice(), None);
            assert_eq!(undefined.microprice_offset_price4(), None);
        }
    }

    #[test]
    fn order_flow_imbalance_follows_cont_kukanov_stoikov() {
        let base = touch(Some((100, 10)), Some((102, 20)));
        // Bid size up 10 -> 15: +15 - 10 = +5; ask unchanged: -20 + 20 = 0.
        assert_eq!(
            order_flow_imbalance(&base, &touch(Some((100, 15)), Some((102, 20)))),
            Some(5)
        );
        // Bid improves to 101 x 7: +7 only. Ask unchanged: 0.
        assert_eq!(
            order_flow_imbalance(&base, &touch(Some((101, 7)), Some((102, 20)))),
            Some(7)
        );
        // Bid level gone, new best 99 x 50: -10. Ask improves to 101 x 4: -4.
        assert_eq!(
            order_flow_imbalance(&base, &touch(Some((99, 50)), Some((101, 4)))),
            Some(-14)
        );
        // Ask depleted 20 -> 5 by an execution: -5 + 20 = +15.
        assert_eq!(
            order_flow_imbalance(&base, &touch(Some((100, 10)), Some((102, 5)))),
            Some(15)
        );
        // Ask level gone, new best 103 x 9: +20.
        assert_eq!(
            order_flow_imbalance(&base, &touch(Some((100, 10)), Some((103, 9)))),
            Some(20)
        );
        assert_eq!(
            order_flow_imbalance(&base, &touch(Some((100, 10)), None)),
            None
        );
    }

    #[test]
    fn coverage_includes_the_initial_empty_book_and_clips_to_observation() {
        for (start, end, observed, expected) in [
            (0, 200, 200, 200),
            (100, 300, 250, 150),
            (100, 300, 400, 200),
            (100, 300, 50, 0),
        ] {
            let r = TouchMetrics::new(TouchMetricsConfig::new(start, end)).finish(observed);
            assert_eq!(r.relative_spread_bps.undefined_ns, expected);
            assert_eq!(r.relative_spread_bps.time_weighted_mean, None);
            assert_eq!(r.events_in_window, 0);
        }
    }

    #[test]
    fn first_event_inside_or_after_window_preserves_all_time_weights() {
        let a = touch(Some((100, 10)), Some((102, 20)));
        let b = touch(Some((100, 10)), Some((104, 20)));
        let mut m = TouchMetrics::new(TouchMetricsConfig::new(0, 200));
        m.push(&event(50, TouchEventKind::Add, Touch::default(), a));
        m.push(&event(100, TouchEventKind::Add, a, b));
        let r = m.finish(200);
        assert_eq!(r.span_ns, 200);
        assert_eq!(r.relative_spread_bps.undefined_ns, 50);
        let expected = (a.relative_spread_bps().unwrap() * 50.0
            + b.relative_spread_bps().unwrap() * 100.0)
            / 150.0;
        assert!((r.relative_spread_bps.time_weighted_mean.unwrap() - expected).abs() < 1e-12);
        assert_eq!(r.activity.messages_per_second["add"], Some(10_000_000.0));
        assert_eq!(r.relative_spread_bps.n, 2);

        let mut m = TouchMetrics::new(TouchMetricsConfig::new(0, 200));
        m.push(&event(300, TouchEventKind::Add, Touch::default(), a));
        let r = m.finish(400);
        assert_eq!(r.relative_spread_bps.undefined_ns, 200);
        assert_eq!(r.relative_spread_bps.time_weighted_mean, None);
        assert_eq!(r.events_in_window, 0);
    }

    #[test]
    fn accumulator_hand_computed_window() {
        let a = touch(Some((100_000, 300)), Some((100_200, 100)));
        let b = touch(Some((100_000, 300)), Some((100_100, 50)));
        let c = touch(Some((100_000, 300)), Some((100_200, 100)));
        let mut config = TouchMetricsConfig::new(1_000, 3_000);
        config.markout_horizons_ns = vec![500, 5_000];
        let mut m = TouchMetrics::new(config);
        // Before the window: sets the touch, not measured.
        m.push(&event(500, TouchEventKind::Add, Touch::default(), a));
        // t=1000: add improves the ask to 100100 x 50.
        let mut add = event(1_000, TouchEventKind::Add, a, b);
        add.shares = 50;
        m.push(&add);
        // t=2000: buyer lifts the 50 at 100100; ask back to 100200 x 100.
        m.push(&TouchEvent {
            resting_side: Some(Side::Sell),
            shares: 50,
            ask_after_at_before_price: 0,
            ..event(2_000, TouchEventKind::Execute, b, c)
        });
        // t=2200: one-sided (bid deleted).
        let gone = touch(None, Some((100_200, 100)));
        m.push(&TouchEvent {
            shares: 300,
            bid_after_at_before_price: 0,
            ..event(2_200, TouchEventKind::Delete, c, gone)
        });
        // t=2400: bid back.
        m.push(&event(2_400, TouchEventKind::Add, gone, c));
        let r = m.finish(4_000);

        assert_eq!(r.events_in_window, 4);
        assert_eq!(r.span_ns, 2_000);
        assert_eq!(r.activity.messages["add"], 2);
        assert_eq!(r.activity.messages["execute"], 1);
        assert_eq!(r.activity.messages_per_second["add"], Some(1e6));
        assert_eq!(r.activity.cancel_to_execution_ratio, Some(1.0));

        // Spreads after t=1000 (100), 2000 (200), 2400 (200); 2200 one-sided.
        assert_eq!(r.quoted_spread.n, 3);
        assert_eq!(r.quoted_spread.p50_price4, Some(200));
        assert_eq!(r.relative_spread_bps.one_sided, 1);
        let s100 = 20_000.0 * 100.0 / 200_100.0;
        let s200 = 20_000.0 * 200.0 / 200_200.0;
        let em = r.relative_spread_bps.event_mean.unwrap();
        assert!((em - (s100 + 2.0 * s200) / 3.0).abs() < 1e-12);
        // Time weights in [1000, 3000): s100 for 1000 ns, s200 for 200 + 600 ns, 200 undefined.
        let tw = r.relative_spread_bps.time_weighted_mean.unwrap();
        assert!((tw - (s100 * 1_000.0 + s200 * 800.0) / 1_800.0).abs() < 1e-9);
        assert_eq!(r.relative_spread_bps.undefined_ns, 200);

        // Microprice offsets: b: 100*(300-50)/(2*350) = 35.714...; c twice: 50.
        assert_eq!(r.microprice.n, 3);
        let mo = r.microprice.mean_offset_price4.unwrap();
        assert!((mo - (25_000.0 / 700.0 + 100.0) / 3.0).abs() < 1e-9);
        assert_eq!(r.microprice.above_mid, 3);

        // OFI: add a->b: bid 300-300 = 0; ask improves: -50 => -50, dmid2 -100.
        // exec b->c: bid 0; ask worsens: +50 => +50, dmid2 +100. Delete and re-add skipped.
        assert_eq!(r.order_flow_imbalance.events_used, 2);
        assert_eq!(r.order_flow_imbalance.events_skipped, 2);
        assert_eq!(r.order_flow_imbalance.total_shares, 0);
        assert_eq!(r.order_flow_imbalance.buckets, 1, "one 1 s bucket");

        // Depletion: ask 50 by the execution, bid 300 by the delete. Replenishment: the add
        // at 2400 has no pre-message bid; the add at 1000 left the old ask level untouched.
        assert_eq!(r.touch_flows.depleted_shares["execute"], 50);
        assert_eq!(r.touch_flows.depleted_shares["delete"], 300);
        assert_eq!(r.touch_flows.depleted_total, 350);
        assert_eq!(r.touch_flows.depleting_messages["execute"], 1);

        // Markout of the buy at t=2000, mid2 before = 200100.
        // h=500: at 2500 the touch is c, mid2 200200: +100/2 = +50 price4.
        let h500 = &r.markouts[0];
        assert_eq!((h500.n, h500.positive), (1, 1));
        assert_eq!(h500.mean_price4, Some(50.0));
        assert!((h500.mean_bps.unwrap() - 10_000.0 * 100.0 / 200_100.0).abs() < 1e-12);
        assert_eq!(h500.share_weighted_mean_price4, Some(50.0));
        // h=5000: 7000 is beyond observed_until 4000: censored.
        assert_eq!((r.markouts[1].n, r.markouts[1].censored), (0, 1));
    }

    #[test]
    fn markout_at_horizon_includes_messages_stamped_exactly_then_and_counts_undefined() {
        let a = touch(Some((100, 10)), Some((102, 10)));
        let up = touch(Some((102, 10)), Some((104, 10)));
        let mut config = TouchMetricsConfig::new(0, 10_000);
        config.markout_horizons_ns = vec![100];
        let mut m = TouchMetrics::new(config);
        m.push(&event(0, TouchEventKind::Add, Touch::default(), a));
        // Seller hits the bid at t=10: s = -1.
        m.push(&TouchEvent {
            resting_side: Some(Side::Buy),
            shares: 5,
            ..event(10, TouchEventKind::Execute, a, a)
        });
        // Exactly at t+h = 110 the book moves up by 2: markout = -1 * (206 - 202)/2 = -2.
        m.push(&event(110, TouchEventKind::Add, a, up));
        // An execution with a crossed book before it is undefined at execution.
        let crossed = touch(Some((105, 1)), Some((104, 1)));
        m.push(&event(200, TouchEventKind::Add, up, crossed));
        m.push(&TouchEvent {
            resting_side: Some(Side::Sell),
            shares: 1,
            ..event(300, TouchEventKind::Execute, crossed, crossed)
        });
        let r = m.finish(1_000);
        let h = &r.markouts[0];
        assert_eq!((h.n, h.negative, h.undefined_at_execution), (1, 1, 1));
        assert_eq!(h.mean_price4, Some(-2.0));
        assert_eq!(r.relative_spread_bps.locked_or_crossed, 2);
    }

    #[test]
    fn ols_recovers_an_exact_line() {
        let (slope, intercept, r) = ols(&[(0.0, 1.0), (1.0, 3.0), (2.0, 5.0)]);
        assert_eq!((slope, intercept, r), (Some(2.0), Some(1.0), Some(1.0)));
        assert_eq!(ols(&[(1.0, 1.0)]), (None, None, None));
        assert_eq!(ols(&[(1.0, 1.0), (1.0, 2.0)]), (None, None, None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bb_data::Event;

    // Real L2 replay, not a mock MarketReport; native fixtures are hand-computed below.
    fn l2_report() -> MarketReport {
        let events = [(1 << 29, 100), (1 << 28, 102)].map(|(side, price4)| Event {
            ev: (1 << 31) | side | 1,
            exch_ts: 0,
            local_ts: 0,
            px: price4 as f64 * 0.0001,
            qty: 10.0,
            order_id: 0,
            ival: 0,
            fval: 0.0,
        });
        compute_market(
            &events,
            &InstrumentMeta {
                tick_size: 0.0001,
                lot_size: 1.0,
                contract_size: 1.0,
            },
            0,
            &MarketConfig::default(),
        )
        .unwrap()
    }

    fn event(index: u64, kind: &str, applied: &str, ts: Option<u64>) -> EventRecord {
        EventRecord {
            event_index: index,
            message_type: kind.into(),
            ts_ns: ts.map(|ts| ts.to_string()),
            order_ref: (applied == "book").then(|| (index + 1).to_string()),
            new_order_ref: None,
            side: None,
            price4: None,
            shares: None,
            best_bid_price4: None,
            best_ask_price4: None,
            applied: applied.into(),
        }
    }

    fn observation(index: u64, ts: Option<u64>) -> Observation {
        Observation {
            event_index: index,
            ts_ns: ts.map(|ts| ts.to_string()),
            best_bid_price4: Some(100),
            best_ask_price4: Some(102),
            live_orders: 0,
            bid_shares: 0,
            ask_shares: 0,
            order_count_imbalance: None,
            signed_added_shares: 0,
            signed_cancelled_shares: 0,
            signed_executed_shares: 0,
            touch_depleted_shares: 0,
            touch_replenished_shares: 0,
            lifetime_ns: None,
            executed_queue_position: None,
            cancel_location: None,
            order_identity_observations: 0,
            touch_queue_observations: 0,
            accepted_add: false,
            accepted_cancel: false,
            level_key: None,
        }
    }

    fn fixture() -> (Vec<EventRecord>, Vec<Observation>) {
        let kinds = [
            ("S", "no_effect", 0),
            ("A", "book", 1_000_000_000),
            ("F", "book", 1_000_000_000),
            ("X", "book", 1_000_000_000),
            ("D", "book", 1_000_000_000),
            ("A", "book", 1_000_000_000),
            ("X", "book", 1_000_000_000),
            ("E", "book", 1_500_000_000),
            ("C", "book", 1_600_000_000),
            ("E", "quarantined", 1_700_000_000),
            ("U", "book", 1_800_000_000),
            ("P", "print", 1_900_000_000),
            ("A", "quarantined", 1_900_000_000),
            ("D", "invalid", 1_900_000_000),
            ("H", "no_effect", 2_000_000_000),
        ];
        let events: Vec<_> = kinds
            .iter()
            .enumerate()
            .map(|(index, (kind, applied, ts))| event(index as u64, kind, applied, Some(*ts)))
            .collect();
        let mut observations: Vec<_> = events
            .iter()
            .map(|event| {
                observation(
                    event.event_index,
                    event.ts_ns.as_deref().map(|ts| ts.parse().unwrap()),
                )
            })
            .collect();
        for (index, added, cancelled, executed) in [
            (1, 100, 0, 0),
            (2, -40, 0, 0),
            (3, 0, 10, 0),
            (4, 0, -40, 0),
            (5, 50, 0, 0),
            (6, 0, 5, 0),
            (7, 0, 0, 30),
            (8, 0, 0, -20),
            (10, 25, 20, 0),
        ] {
            let o = &mut observations[index];
            o.signed_added_shares = added;
            o.signed_cancelled_shares = cancelled;
            o.signed_executed_shares = executed;
            o.order_identity_observations = if index == 10 { 2 } else { 1 };
        }
        for index in [1, 2, 5] {
            observations[index].accepted_add = true;
            observations[index].level_key = Some(if index == 2 { (1, 102) } else { (0, 100) });
        }
        for (index, location) in [(3, -1), (4, 1), (6, 0)] {
            observations[index].accepted_cancel = true;
            observations[index].cancel_location = Some(location);
            observations[index].level_key = Some(if index == 4 { (1, 102) } else { (0, 100) });
        }
        for (index, lifetime) in [(4, 20), (7, 30), (8, 10), (10, 40)] {
            observations[index].lifetime_ns = Some(lifetime);
        }
        observations[7].executed_queue_position = Some(3);
        observations[8].executed_queue_position = Some(1);
        // Even a provided diagnostic on a rejected event must not become an execution sample.
        observations[9].executed_queue_position = Some(99);
        observations[13].cancel_location = Some(-1);
        observations[3].touch_depleted_shares = 10;
        observations[4].touch_depleted_shares = 40;
        observations[5].touch_replenished_shares = 50;
        observations[3].touch_queue_observations = 3;
        observations[4].touch_queue_observations = 2;
        observations[14].live_orders = 2;
        observations[14].bid_shares = 70;
        observations[14].ask_shares = 30;
        observations[14].order_count_imbalance = Some(0.0);
        (events, observations)
    }

    #[test]
    fn native_distribution_counts_rates_flows_and_ambiguity_are_hand_computed() {
        let (events, observations) = fixture();
        let report = aggregate(&events, &observations, l2_report()).unwrap();
        let c = &report.comparison;
        assert_eq!(c.events, 15);
        assert_eq!(c.order_identity_observations, 10);
        assert_eq!(c.touch_queue_observations, 5);
        assert_eq!(c.execution_messages, 2);
        assert_eq!(c.executions_with_order_identity, 2);
        assert_eq!(c.cancel_messages, 3);
        assert_eq!(c.cancels_ahead_of_reference, 1);
        assert_eq!(c.cancels_at_reference, 1);
        assert_eq!(c.cancels_behind_reference, 1);
        assert_eq!(c.same_timestamp_add_cancel_level_groups, 2);
        let m = &report.metrics;
        for (key, expected) in [
            ("events", 15),
            ("messages_A", 3),
            ("messages_F", 1),
            ("messages_P", 1),
            ("messages_H", 1),
            ("add_messages", 4),
            ("accepted_add_messages", 3),
            ("cancel_messages", 4),
            ("accepted_cancel_messages", 3),
            ("execution_messages", 3),
            ("accepted_execution_messages", 2),
            ("replace_messages", 1),
            ("accepted_replace_messages", 1),
            ("quarantined_messages", 2),
            ("invalid_messages", 1),
            ("added_shares", 215),
            ("cancelled_shares", 75),
            ("executed_shares", 50),
            ("touch_depleted_shares", 50),
            ("touch_replenished_shares", 50),
            ("terminal_orders", 4),
            ("right_censored_orders", 2),
        ] {
            assert_eq!(m.counts[key], expected, "{key}");
            assert_eq!(
                m.rates_per_second[key],
                Some(expected as f64 / 2.0),
                "{key}"
            );
        }
        let life = &m.order_lifetime_ns;
        assert_eq!(life.n, 4);
        assert_eq!(life.min.as_deref(), Some("10"));
        assert_eq!(life.p50.as_deref(), Some("20"));
        assert_eq!(life.p90.as_deref(), Some("40"));
        assert_eq!(life.max.as_deref(), Some("40"));
        let queue = &m.queue_position_at_execution;
        assert_eq!(queue.n, 2);
        assert_eq!(
            (queue.min, queue.p50, queue.p90, queue.max),
            (Some(1), Some(1), Some(3), Some(3))
        );
        assert_eq!(m.series[2].signed_added_shares, -40);
        assert_eq!(m.series[8].signed_executed_shares, -20);
        assert_eq!(m.series[14].bid_shares, 70);
        assert_eq!(m.series[14].ask_shares, 30);
        assert_eq!(m.series[14].best_bid_price4, Some(100));
        assert_eq!(m.series[14].best_ask_price4, Some(102));
        assert!(m.l2.stoikov.is_none());
        assert!(m.l2.liquidity.is_empty());
        assert!(
            m.l2.metrics
                .iter()
                .find(|metric| metric.name == "stoikov_microprice")
                .unwrap()
                .event_mean
                .get()
                .is_none()
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["metrics"]["order_lifetime_ns"]["p50"], "20");
        let decoded: L3Report = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.metrics.counts["added_shares"], 215);
    }

    #[test]
    fn empty_native_observations_have_null_distributions_and_rates() {
        let report = aggregate(&[], &[], l2_report()).unwrap();
        assert_eq!(report.comparison.events, 0);
        assert_eq!(report.comparison.same_timestamp_add_cancel_level_groups, 0);
        let m = report.metrics;
        assert!(m.series.is_empty());
        assert!(m.counts.values().all(|count| *count == 0));
        assert!(m.rates_per_second.values().all(Option::is_none));
        assert_eq!(m.order_lifetime_ns.n, 0);
        assert!(m.order_lifetime_ns.min.is_none());
        assert!(m.order_lifetime_ns.p50.is_none());
        assert!(m.order_lifetime_ns.p90.is_none());
        assert!(m.order_lifetime_ns.max.is_none());
        assert_eq!(m.queue_position_at_execution.n, 0);
        assert!(m.queue_position_at_execution.min.is_none());
        assert!(m.queue_position_at_execution.p50.is_none());
        assert!(m.queue_position_at_execution.p90.is_none());
        assert!(m.queue_position_at_execution.max.is_none());
    }

    #[test]
    fn zero_missing_and_regressing_session_spans_have_no_rate() {
        for timestamps in [[Some(7), Some(7)], [None, None], [Some(9), Some(7)]] {
            let events = [
                event(0, "S", "no_effect", timestamps[0]),
                event(1, "H", "no_effect", timestamps[1]),
            ];
            let report = aggregate(&events, &[], l2_report()).unwrap();
            assert_eq!(report.metrics.counts["events"], 2);
            assert!(
                report
                    .metrics
                    .rates_per_second
                    .values()
                    .all(Option::is_none)
            );
        }
        let mut invalid = event(0, "S", "invalid", None);
        invalid.ts_ns = Some("not-nanoseconds".into());
        assert!(aggregate(&[invalid], &[], l2_report()).is_err());
    }

    #[test]
    fn ambiguity_keys_require_numeric_timestamp_side_and_price_and_count_groups() {
        let mut observations = Vec::new();
        for (index, ts, side, price, add, cancel) in [
            (0, 10, 0, 100, true, false),
            (1, 10, 0, 100, true, false),
            (2, 10, 0, 100, false, true),
            (3, 10, 0, 100, false, true),
            (4, 11, 0, 100, false, true),
            (5, 10, 1, 100, false, true),
            (6, 10, 0, 101, false, true),
        ] {
            let mut o = observation(index, Some(ts));
            o.level_key = Some((side, price));
            o.accepted_add = add;
            o.accepted_cancel = cancel;
            observations.push(o);
        }
        observations[2].ts_ns = Some("00010".into());
        let mut missing_time = observation(7, None);
        missing_time.accepted_add = true;
        missing_time.level_key = Some((0, 101));
        observations.push(missing_time);
        let mut missing_level = observation(8, Some(11));
        missing_level.accepted_add = true;
        observations.push(missing_level);
        let report = aggregate(&[], &observations, l2_report()).unwrap();
        assert_eq!(report.comparison.same_timestamp_add_cancel_level_groups, 1);
    }

    #[test]
    fn execution_samples_join_physical_indices_instead_of_slice_positions() {
        let events = [
            event(1, "H", "no_effect", Some(0)),
            event(8, "E", "book", Some(1)),
            event(12, "E", "quarantined", Some(2)),
            event(21, "C", "book", Some(3)),
        ];
        let mut observations: Vec<_> = [8, 12, 21]
            .into_iter()
            .map(|index| observation(index, None))
            .collect();
        observations[0].executed_queue_position = Some(2);
        observations[1].executed_queue_position = Some(99);
        observations[2].executed_queue_position = Some(4);
        let report = aggregate(&events, &observations, l2_report()).unwrap();
        let queue = report.metrics.queue_position_at_execution;
        assert_eq!(queue.n, 2);
        assert_eq!(queue.p50, Some(2));
        assert_eq!(queue.p90, Some(4));
    }

    fn metrics_for_indices(indices: &[u64]) -> Metrics {
        let observations: Vec<_> = indices
            .iter()
            .map(|index| observation(*index, Some(*index)))
            .collect();
        aggregate(&[], &observations, l2_report()).unwrap().metrics
    }

    #[test]
    fn reduction_keeps_last_in_equal_index_buckets_not_row_rank_buckets() {
        let mut metrics = metrics_for_indices(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
        metrics.downsample(3);
        assert_eq!(
            metrics
                .series
                .iter()
                .map(|p| p.event_index)
                .collect::<Vec<_>>(),
            [13, 16, 19]
        );
        assert_eq!(
            metrics
                .series
                .iter()
                .map(|p| p.ts_ns.as_deref())
                .collect::<Vec<_>>(),
            [Some("13"), Some("16"), Some("19")]
        );
        let mut sparse = metrics_for_indices(&[10, 11, 12, 90, 99]);
        sparse.downsample(3);
        assert_eq!(
            sparse
                .series
                .iter()
                .map(|p| p.event_index)
                .collect::<Vec<_>>(),
            [12, 99]
        );
        let mut empty = metrics_for_indices(&[]);
        empty.downsample(2);
        assert!(empty.series.is_empty());
        let mut single = metrics_for_indices(&[7]);
        single.downsample(1);
        assert_eq!(single.series[0].event_index, 7);
        let mut one = metrics_for_indices(&[0, 9, 100]);
        one.downsample(1);
        assert_eq!(one.series.len(), 1);
        assert_eq!(one.series[0].event_index, 100);
        one.downsample(0);
        assert!(one.series.is_empty());
        let mut extreme = metrics_for_indices(&[0, 1, u64::MAX - 1, u64::MAX]);
        extreme.downsample(2);
        assert_eq!(
            extreme
                .series
                .iter()
                .map(|p| p.event_index)
                .collect::<Vec<_>>(),
            [1, u64::MAX]
        );
    }

    #[test]
    fn reduction_is_bounded_causal_final_and_does_not_change_summaries() {
        let (events, observations) = fixture();
        let mut metrics = aggregate(&events, &observations, l2_report())
            .unwrap()
            .metrics;
        let counts = metrics.counts.clone();
        let rates = metrics.rates_per_second.clone();
        let lifetime_n = metrics.order_lifetime_ns.n;
        metrics.downsample(3);
        assert_eq!(metrics.counts, counts);
        assert_eq!(metrics.rates_per_second, rates);
        assert_eq!(metrics.order_lifetime_ns.n, lifetime_n);
        assert_eq!(metrics.series.last().unwrap().event_index, 14);

        let indices = [3, 4, 5, 19, 20, 21, 22, 27, 40, 90, 91, 102, 501];
        let original = metrics_for_indices(&indices);
        for max in 1..=indices.len() + 1 {
            let mut reduced = original.clone();
            reduced.downsample(max);
            assert!(reduced.series.len() <= max);
            assert_eq!(reduced.series.last().unwrap().event_index, 501);
            if max >= indices.len() {
                assert_eq!(reduced.series.len(), indices.len());
                continue;
            }
            let span = 501u128 - 3 + 1;
            let bucket = |index: u64| u128::from(index - 3) * max as u128 / span;
            for point in &reduced.series {
                let last = indices
                    .iter()
                    .rfind(|index| bucket(**index) == bucket(point.event_index))
                    .unwrap();
                assert_eq!(point.event_index, *last);
                assert_eq!(point.ts_ns, Some(last.to_string()));
            }
            assert!(
                reduced
                    .series
                    .windows(2)
                    .all(|pair| bucket(pair[0].event_index) < bucket(pair[1].event_index))
            );
        }
    }
}
