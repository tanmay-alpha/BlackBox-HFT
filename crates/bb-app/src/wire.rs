//! Wire encoding of `/api/v1` (API_CONTRACT §1, TARGET_ARCHITECTURE §10): every nanosecond
//! quantity and every seed travels as a JSON **decimal string**, every other integer must be
//! exactly representable in a JavaScript number (≤ 2^53 − 1).
//!
//! bb-core and bb-analytics serialize nanoseconds as JSON numbers, and their canonical JSON is
//! what `config_sha256`/`result_sha256` hash, so the conversion happens here at the HTTP boundary
//! and the hashes and `CONTRACT_VERSION` stay untouched. The converted fields are an explicit list
//! ([`NS_KEYS`] plus the contextual rules in [`to_wire`]); it mirrors the `openapi` feature's
//! `schema(value_type = String)` annotations in bb-core and bb-analytics, and the test
//! `ns_key_list_matches_the_openapi_annotations` checks the two agree.
//!
//! Requests accept both forms: a decimal string or a JSON number ([`from_wire`]).

use serde_json::{Map, Number, Value};

/// Largest integer a JavaScript number holds exactly.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Object keys whose integer values are nanoseconds (or seeds) wherever they occur.
pub const NS_KEYS: &[&str] = &[
    // relative timestamps (RelNs)
    "ts",
    "decision_ts",
    "exch_arrival_ts",
    "first_response_ts",
    "first_fill_ts",
    "last_fill_ts",
    "cancel_sent_ts",
    "terminal_ts",
    "local_arrival_ts",
    "exch_ts",
    "local_ts",
    "feed_exch_ts",
    "feed_local_ts",
    "t0",
    "t1",
    "start",
    "end",
    // durations, latencies, windows (`*_ns`)
    "interval_ns",
    "feed_latency_offset_ns",
    "entry_ns",
    "response_ns",
    "jitter_ns",
    "decision_ns",
    "exch_arrival_ns",
    "first_response_ns",
    "first_fill_ns",
    "completion_ns",
    "local_ts_shift_ns",
    "feed_staleness_ns",
    "feed_latency_ns",
    "entry_latency_ns",
    "response_latency_ns",
    "time_to_first_fill_ns",
    "resting_time_ns",
    "cancel_entry_latency_ns",
    "horizon_ns",
    "step_ns",
    "horizons_ns",
    "queue_horizon_ns",
    "bootstrap_block_ns",
    "observation_end_ns",
    "sent_ns",
    "elapsed_ns",
    "start_ns",
    "end_ns",
    "clock_step_ns",
    "ts_ns",
    "train_start_ns",
    "train_end_ns",
    "test_start_ns",
    "test_end_ns",
    "trade_match_lag_ns",
    "bucket_ns",
    // seeds (u64)
    // synthetic generator parameters (`DatasetManifest.generator`)
    "duration_ns",
    "mean_event_gap_ns",
    "feed_jitter_ns",
    "seed",
    "bootstrap_seed",
];

/// `GridAxis` variants whose `values` are nanoseconds.
const NS_AXES: &[&str] = &[
    "order_latency_ns",
    "entry_latency_ns",
    "feed_latency_offset_ns",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error("integer {value} at `{path}` exceeds 2^53 - 1 and is not a nanosecond field")]
    UnsafeInteger { path: String, value: String },
}

fn is_ns_key(key: &str) -> bool {
    NS_KEYS.contains(&key)
}

/// Contextual ns keys: `DeltaStats.min/max` (an object that also has `nonzero`) and the
/// `values` of an ns grid axis.
fn is_contextual_ns(object: &Map<String, Value>, key: &str) -> bool {
    match key {
        "min" | "max" => object.contains_key("nonzero") && object.contains_key("count"),
        "values" => object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| NS_AXES.contains(&t)),
        _ => false,
    }
}

fn integer_to_string(value: &mut Value) {
    if let Value::Number(n) = value
        && (n.is_i64() || n.is_u64())
    {
        *value = Value::String(n.to_string());
    }
}

/// Integers (or arrays of integers) become decimal strings.
fn ns_to_string(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(integer_to_string),
        other => integer_to_string(other),
    }
}

/// Converts a serialized response in place and asserts that no unsafe integer remains.
pub fn to_wire(value: &mut Value) -> Result<(), WireError> {
    convert_out(value, "$")
}

fn convert_out(value: &mut Value, path: &str) -> Result<(), WireError> {
    match value {
        Value::Object(map) => {
            let ns_keys: Vec<String> = map
                .keys()
                .filter(|k| is_ns_key(k) || is_contextual_ns(map, k))
                .cloned()
                .collect();
            for key in ns_keys {
                if let Some(v) = map.get_mut(&key) {
                    ns_to_string(v);
                }
            }
            if let Some(Value::Array(pairs)) = map.get_mut("opportunities") {
                for pair in pairs {
                    if let Value::Array(items) = pair
                        && let Some(first) = items.first_mut()
                    {
                        integer_to_string(first);
                    }
                }
            }
            for (key, v) in map.iter_mut() {
                convert_out(v, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (i, v) in items.iter_mut().enumerate() {
                convert_out(v, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        Value::Number(n) => {
            let unsafe_integer = n.as_u64().is_some_and(|v| v > MAX_SAFE_INTEGER)
                || n.as_i64()
                    .is_some_and(|v| v.unsigned_abs() > MAX_SAFE_INTEGER);
            if unsafe_integer {
                return Err(WireError::UnsafeInteger {
                    path: path.to_string(),
                    value: n.to_string(),
                });
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn string_to_integer(value: &mut Value) {
    if let Value::String(s) = value {
        let trimmed = s.as_str();
        let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
        if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        if let Ok(v) = trimmed.parse::<i64>() {
            *value = Value::Number(Number::from(v));
        } else if let Ok(v) = trimmed.parse::<u64>() {
            *value = Value::Number(Number::from(v));
        }
    }
}

fn ns_from_string(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(string_to_integer),
        other => string_to_integer(other),
    }
}

/// Converts a request body in place: decimal strings in ns/seed fields become integers, so the
/// bb-core types deserialize them. Numbers are accepted unchanged.
pub fn from_wire(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let ns_keys: Vec<String> = map
                .keys()
                .filter(|k| is_ns_key(k) || is_contextual_ns(map, k))
                .cloned()
                .collect();
            for key in ns_keys {
                if let Some(v) = map.get_mut(&key) {
                    ns_from_string(v);
                }
            }
            map.values_mut().for_each(from_wire);
        }
        Value::Array(items) => items.iter_mut().for_each(from_wire),
        _ => {}
    }
}

/// Formats an i64 nanosecond value for DTOs built by hand.
pub fn ns(value: i64) -> String {
    value.to_string()
}

/// Epoch nanoseconds of a timestamp, as a wire string.
pub fn at_ns(at: chrono::DateTime<chrono::Utc>) -> String {
    at.timestamp_nanos_opt().unwrap_or(i64::MAX).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ns_fields_become_strings_and_counts_stay_numbers() {
        let mut v = json!({
            "seq": 3, "ts": 1_700_000_000_000_000_000i64, "order_id": 9,
            "decision_ts": -5, "first_fill_ts": null,
            "timing": {"exch_arrival": {"count": 2, "nonzero": 1, "min": 5, "max": 9, "median": 7.0}},
            "x": {"type": "order_latency_ns", "values": [1_000_000, 5_000_000]},
            "y": {"type": "maker_fee", "values": [1, 2]},
            "bbo": {"ts": [1, 2, 3]},
            "opportunities": [[100_000_000, {"total": 1}]],
            "seed": 18_446_744_073_709_551_615u64,
            "quantiles": {"min": 1, "max": 2}
        });
        to_wire(&mut v).unwrap();
        assert_eq!(v["seq"], 3);
        assert_eq!(v["ts"], "1700000000000000000");
        assert_eq!(v["decision_ts"], "-5");
        assert_eq!(v["first_fill_ts"], Value::Null);
        assert_eq!(v["timing"]["exch_arrival"]["min"], "5");
        assert_eq!(v["timing"]["exch_arrival"]["count"], 2);
        assert_eq!(v["x"]["values"], json!(["1000000", "5000000"]));
        assert_eq!(v["y"]["values"], json!([1, 2]));
        assert_eq!(v["bbo"]["ts"], json!(["1", "2", "3"]));
        assert_eq!(v["opportunities"][0][0], "100000000");
        assert_eq!(v["seed"], "18446744073709551615");
        assert_eq!(v["quantiles"]["min"], 1);
    }

    #[test]
    fn unsafe_integers_outside_the_list_are_refused() {
        let mut v = json!({"count": 9_007_199_254_740_992u64});
        assert!(matches!(
            to_wire(&mut v),
            Err(WireError::UnsafeInteger { .. })
        ));
        let mut ok = json!({"count": 9_007_199_254_740_991u64});
        to_wire(&mut ok).unwrap();
    }

    #[test]
    fn requests_accept_strings_and_numbers_without_precision_loss() {
        let mut v = json!({
            "seed": "18446744073709551615",
            "strategy": {"interval_ns": "100000000"},
            "execution": {"feed_latency_offset_ns": 5, "order_latency": {"type": "constant", "entry_ns": "9007199254740993", "response_ns": "0"}},
            "label": "12345"
        });
        from_wire(&mut v);
        assert_eq!(v["seed"].as_u64(), Some(u64::MAX));
        assert_eq!(v["strategy"]["interval_ns"].as_i64(), Some(100_000_000));
        assert_eq!(
            v["execution"]["order_latency"]["entry_ns"].as_i64(),
            Some(9_007_199_254_740_993)
        );
        assert_eq!(v["label"], "12345", "non-ns strings are untouched");
        // Round trip: string in -> i64 -> string out is exact.
        to_wire(&mut v).unwrap();
        assert_eq!(
            v["execution"]["order_latency"]["entry_ns"],
            "9007199254740993"
        );
        assert_eq!(v["seed"], "18446744073709551615");
    }
}
