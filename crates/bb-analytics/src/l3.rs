//! Source-order L3 observations alongside the existing completed-batch L2 analytics.
//! Native summaries use every observation; display reduction never feeds a summary.
use std::collections::BTreeMap;

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
