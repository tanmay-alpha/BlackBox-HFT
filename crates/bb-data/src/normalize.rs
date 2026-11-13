//! Timestamp and event-order normalization shared by every data pipeline.
//!
//! This module is a Rust port of `correct_local_timestamp`, `correct_event_order` and
//! `validate_event_order` from py-hftbacktest (`hftbacktest/data/utils/validation.py`,
//! Copyright (c) nkaz001, MIT License). The control flow of the event-order merge, including how
//! ties are broken, follows the reference exactly; the differences are that it never reads past
//! the end of an index array (the reference does before checking bounds) and that the local
//! timestamp correction can additionally enforce a strictly positive minimum feed latency.
//!
//! Input rows carry absolute-nanosecond `exch_ts`/`local_ts` and an `ev` holding only the kind
//! and side bits, in source order. Source order is the tie-break for equal timestamps, so callers
//! must place a trade before the depth update it caused (all trade rows first for file sources):
//! a queue model that sees the depth reduction first and the trade second counts the same volume
//! twice.

use hftbacktest::types::{EXCH_EVENT, LOCAL_EVENT};

use crate::{DataError, Event, has};

/// The engine's feed-latency adjustment rejects rows with `local_ts <= exch_ts`, so by default
/// every row is given at least one nanosecond of feed latency.
pub const DEFAULT_MIN_FEED_LATENCY_NS: i64 = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NormalizeOptions {
    /// Added to the shift when the source contained negative feed latency (the reference
    /// converter's `base_latency`).
    pub base_latency_ns: i64,
    /// Smallest `local_ts - exch_ts` any row may have afterwards. `None` reproduces the reference
    /// converter exactly, which can leave zero-latency rows.
    pub min_feed_latency_ns: Option<i64>,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            base_latency_ns: 0,
            min_feed_latency_ns: Some(DEFAULT_MIN_FEED_LATENCY_NS),
        }
    }
}

/// Result of [`normalize`].
#[derive(Clone, Debug)]
pub struct Normalized {
    /// Engine-ready rows carrying `EXCH_EVENT` and/or `LOCAL_EVENT`.
    pub events: Vec<Event>,
    /// Constant that was added to every local timestamp.
    pub local_ts_shift_ns: i64,
    /// Number of source rows that went in.
    pub source_rows: u64,
}

/// Shifts every `local_ts` by one constant when the source has negative feed latency, and returns
/// the shift (0 when none was applied).
///
/// Reference behaviour: with `min = min(local_ts - exch_ts)` over all rows, if `min < 0` every
/// local timestamp is moved by `-min + base_latency_ns`. Extension: when `min_feed_latency_ns` is
/// given, the shift is increased as far as needed for the smallest latency to reach it.
pub fn correct_local_timestamp(
    rows: &mut [Event],
    base_latency_ns: i64,
    min_feed_latency_ns: Option<i64>,
) -> i64 {
    let Some(min_latency) = rows.iter().map(|r| r.local_ts - r.exch_ts).min() else {
        return 0;
    };
    let mut shift = 0i64;
    if min_latency < 0 {
        shift = base_latency_ns - min_latency;
    }
    if let Some(floor) = min_feed_latency_ns {
        let after = min_latency + shift;
        if after < floor {
            shift += floor - after;
        }
    }
    if shift != 0 {
        for row in rows.iter_mut() {
            row.local_ts += shift;
        }
    }
    shift
}

/// Produces the engine's dual-ordered array: rows flagged `EXCH_EVENT` are non-decreasing in
/// `exch_ts` and rows flagged `LOCAL_EVENT` are non-decreasing in `local_ts`.
///
/// The rows are merged from a stable sort by `exch_ts` and a stable sort by `local_ts`. A row at
/// the head of both orders is emitted once with both flags; otherwise it is emitted twice, once
/// per flag, at the position each clock requires.
pub fn correct_event_order(rows: &[Event]) -> Vec<Event> {
    let n = rows.len();
    let mut by_exch: Vec<usize> = (0..n).collect();
    by_exch.sort_by_key(|&i| rows[i].exch_ts);
    let mut by_local: Vec<usize> = (0..n).collect();
    by_local.sort_by_key(|&i| rows[i].local_ts);

    let flagged = |index: usize, flags: u64| {
        let mut ev = rows[index].clone();
        ev.ev |= flags;
        ev
    };

    let mut out = Vec::with_capacity(n + n / 4);
    let mut exch_rn = 0usize;
    let mut local_rn = 0usize;
    loop {
        match (by_exch.get(exch_rn).copied(), by_local.get(local_rn).copied()) {
            (Some(ei), Some(li)) => {
                let sorted_exch = &rows[ei];
                let sorted_local = &rows[li];
                if sorted_exch.exch_ts == sorted_local.exch_ts
                    && sorted_exch.local_ts == sorted_local.local_ts
                {
                    // The reference asserts the two heads hold the same values. With stable
                    // sorts, heads that agree on both timestamps are always the same source row;
                    // anything else would drop one row and duplicate another.
                    assert!(
                        ei == li,
                        "correct_event_order: heads agree on both timestamps but are different \
                         rows ({ei} and {li})"
                    );
                    out.push(flagged(ei, EXCH_EVENT | LOCAL_EVENT));
                    exch_rn += 1;
                    local_rn += 1;
                } else if (sorted_exch.exch_ts == sorted_local.exch_ts
                    && sorted_exch.local_ts < sorted_local.local_ts)
                    || sorted_exch.exch_ts < sorted_local.exch_ts
                {
                    out.push(flagged(ei, EXCH_EVENT));
                    exch_rn += 1;
                } else {
                    // The reference's third branch ends in `or local_rn < len(data)`, so every
                    // remaining case with a pending local row emits that row.
                    out.push(flagged(li, LOCAL_EVENT));
                    local_rn += 1;
                }
            }
            // Local order exhausted: the reference reaches its exchange branch here (after
            // comparing against an out-of-bounds row, which this port does not do).
            (Some(ei), None) => {
                out.push(flagged(ei, EXCH_EVENT));
                exch_rn += 1;
            }
            (None, Some(li)) => {
                out.push(flagged(li, LOCAL_EVENT));
                local_rn += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// Checks the ordering the engine requires: exchange-flagged rows non-decreasing in `exch_ts`,
/// local-flagged rows non-decreasing in `local_ts`.
pub fn validate_event_order(events: &[Event]) -> Result<(), DataError> {
    let mut last_exch: Option<i64> = None;
    let mut last_local: Option<i64> = None;
    for (i, ev) in events.iter().enumerate() {
        if has(ev, EXCH_EVENT) {
            if let Some(prev) = last_exch
                && ev.exch_ts < prev
            {
                return Err(DataError::Order(format!(
                    "exchange events are out of order at row {i}: exch_ts {} follows {prev}",
                    ev.exch_ts
                )));
            }
            last_exch = Some(ev.exch_ts);
        }
        if has(ev, LOCAL_EVENT) {
            if let Some(prev) = last_local
                && ev.local_ts < prev
            {
                return Err(DataError::Order(format!(
                    "local events are out of order at row {i}: local_ts {} follows {prev}",
                    ev.local_ts
                )));
            }
            last_local = Some(ev.local_ts);
        }
    }
    Ok(())
}

/// Runs the two corrections in the reference converter's order.
pub fn normalize(mut rows: Vec<Event>, opts: &NormalizeOptions) -> Normalized {
    let local_ts_shift_ns =
        correct_local_timestamp(&mut rows, opts.base_latency_ns, opts.min_feed_latency_ns);
    let events = correct_event_order(&rows);
    Normalized {
        events,
        local_ts_shift_ns,
        source_rows: rows.len() as u64,
    }
}

#[cfg(test)]
mod tests {
    use hftbacktest::types::{BUY_EVENT, DEPTH_EVENT, SELL_EVENT, TRADE_EVENT};

    use super::*;
    use crate::make_event;

    fn row(ev: u64, exch_ts: i64, local_ts: i64) -> Event {
        make_event(ev, exch_ts, local_ts, 100.0, 1.0)
    }

    fn only(events: &[Event], flag: u64) -> Vec<(i64, i64)> {
        events
            .iter()
            .filter(|e| has(e, flag))
            .map(|e| (e.exch_ts, e.local_ts))
            .collect()
    }

    #[test]
    fn rows_in_agreeing_order_are_emitted_once_with_both_flags() {
        let rows = vec![
            row(TRADE_EVENT | BUY_EVENT, 10, 15),
            row(DEPTH_EVENT | SELL_EVENT, 10, 15),
            row(DEPTH_EVENT | BUY_EVENT, 20, 26),
        ];
        let out = correct_event_order(&rows);
        assert_eq!(out.len(), 3);
        for (src, ev) in rows.iter().zip(&out) {
            assert_eq!(ev.ev, src.ev | EXCH_EVENT | LOCAL_EVENT);
            assert_eq!((ev.exch_ts, ev.local_ts), (src.exch_ts, src.local_ts));
        }
        validate_event_order(&out).unwrap();
    }

    #[test]
    fn differing_local_order_splits_rows_and_passes_validation() {
        // Exchange order is A, B, C; B arrives locally before A.
        let a = row(DEPTH_EVENT | BUY_EVENT, 100, 190);
        let b = row(TRADE_EVENT | SELL_EVENT, 110, 150);
        let c = row(DEPTH_EVENT | SELL_EVENT, 120, 200);
        let rows = vec![a.clone(), b.clone(), c.clone()];
        let out = correct_event_order(&rows);

        // A is split into an exchange row (before B) and a local row (after B); B and C sit at
        // the head of both orders when their turn comes and keep both flags.
        assert_eq!(out.len(), 4);
        assert_eq!(
            only(&out, EXCH_EVENT),
            vec![(100, 190), (110, 150), (120, 200)]
        );
        assert_eq!(
            only(&out, LOCAL_EVENT),
            vec![(110, 150), (100, 190), (120, 200)]
        );
        let both = out
            .iter()
            .filter(|e| has(e, EXCH_EVENT | LOCAL_EVENT))
            .count();
        assert_eq!(both, 2);
        // Kind and side bits survive on both halves.
        let a_halves: Vec<u64> = out
            .iter()
            .filter(|e| e.exch_ts == 100)
            .map(|e| e.ev)
            .collect();
        assert_eq!(
            a_halves,
            vec![a.ev | EXCH_EVENT, a.ev | LOCAL_EVENT]
        );
        validate_event_order(&out).unwrap();
    }

    #[test]
    fn exchange_rows_may_outlast_local_rows() {
        // Same exchange time, reversed local time: the local order runs out first, which is the
        // case where the reference reads past the end of its index array.
        let y = row(TRADE_EVENT, 1, 9);
        let x = row(DEPTH_EVENT | BUY_EVENT, 1, 3);
        let out = correct_event_order(&[y, x]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].ev, DEPTH_EVENT | BUY_EVENT | LOCAL_EVENT);
        assert_eq!(out[1].ev, TRADE_EVENT | EXCH_EVENT | LOCAL_EVENT);
        assert_eq!(out[2].ev, DEPTH_EVENT | BUY_EVENT | EXCH_EVENT);
        validate_event_order(&out).unwrap();
    }

    #[test]
    fn equal_exchange_time_keeps_source_order_on_the_exchange_clock() {
        // A trade and the depth update it caused share an exchange timestamp; the trade is first
        // in source order and must stay first for the exchange side.
        let trade = row(TRADE_EVENT | BUY_EVENT, 50, 70);
        let depth = row(DEPTH_EVENT | SELL_EVENT, 50, 60);
        let out = correct_event_order(&[trade, depth]);
        let exch: Vec<u64> = out
            .iter()
            .filter(|e| has(e, EXCH_EVENT))
            .map(|e| e.ev & 0xff)
            .collect();
        assert_eq!(exch, vec![TRADE_EVENT, DEPTH_EVENT]);
        validate_event_order(&out).unwrap();
    }

    #[test]
    fn negative_latency_is_shifted_out() {
        let mut rows = vec![
            row(DEPTH_EVENT | BUY_EVENT, 1_000, 1_200),
            row(DEPTH_EVENT | BUY_EVENT, 2_000, 1_700),
            row(DEPTH_EVENT | SELL_EVENT, 3_000, 3_050),
        ];
        // Reference behaviour: shift by the worst negative latency plus the base latency.
        let mut reference = rows.clone();
        assert_eq!(correct_local_timestamp(&mut reference, 40, None), 340);
        assert_eq!(reference[1].local_ts, 2_040);

        // Default: additionally at least 1 ns of latency everywhere.
        let shift = correct_local_timestamp(&mut rows, 0, Some(DEFAULT_MIN_FEED_LATENCY_NS));
        assert_eq!(shift, 301);
        assert_eq!(rows[0].local_ts, 1_501);
        assert!(rows.iter().all(|r| r.local_ts > r.exch_ts));
    }

    #[test]
    fn positive_latency_is_left_alone_and_zero_latency_is_lifted_on_request() {
        let mut rows = vec![row(DEPTH_EVENT, 10, 12), row(DEPTH_EVENT, 20, 25)];
        assert_eq!(correct_local_timestamp(&mut rows, 500, Some(1)), 0);
        assert_eq!(rows[0].local_ts, 12);

        let mut zero = vec![row(DEPTH_EVENT, 10, 10), row(DEPTH_EVENT, 20, 25)];
        assert_eq!(correct_local_timestamp(&mut zero, 0, None), 0);
        assert_eq!(correct_local_timestamp(&mut zero, 0, Some(1)), 1);
        assert_eq!(zero[0].local_ts, 11);
        assert_eq!(correct_local_timestamp(&mut [], 0, Some(1)), 0);
    }

    #[test]
    fn validation_rejects_disorder_on_either_clock() {
        let ok = vec![
            row(DEPTH_EVENT | EXCH_EVENT | LOCAL_EVENT, 1, 2),
            row(DEPTH_EVENT | EXCH_EVENT | LOCAL_EVENT, 1, 2),
        ];
        validate_event_order(&ok).unwrap();

        let bad_exch = vec![
            row(DEPTH_EVENT | EXCH_EVENT, 5, 6),
            row(DEPTH_EVENT | EXCH_EVENT, 4, 7),
        ];
        assert!(matches!(
            validate_event_order(&bad_exch),
            Err(DataError::Order(_))
        ));

        let bad_local = vec![
            row(DEPTH_EVENT | LOCAL_EVENT, 5, 9),
            row(DEPTH_EVENT | LOCAL_EVENT, 6, 8),
        ];
        assert!(matches!(
            validate_event_order(&bad_local),
            Err(DataError::Order(_))
        ));

        // A row without the exchange flag does not take part in the exchange ordering.
        let mixed = vec![
            row(DEPTH_EVENT | EXCH_EVENT, 5, 9),
            row(DEPTH_EVENT | LOCAL_EVENT, 1, 10),
            row(DEPTH_EVENT | EXCH_EVENT, 6, 3),
        ];
        validate_event_order(&mixed).unwrap();
    }

    #[test]
    fn normalize_reports_shift_and_source_rows() {
        let rows = vec![
            row(TRADE_EVENT | BUY_EVENT, 100, 90),
            row(DEPTH_EVENT | SELL_EVENT, 100, 140),
        ];
        let n = normalize(rows, &NormalizeOptions::default());
        assert_eq!(n.local_ts_shift_ns, 11);
        assert_eq!(n.source_rows, 2);
        assert_eq!(n.events.len(), 2);
        validate_event_order(&n.events).unwrap();
    }
}
