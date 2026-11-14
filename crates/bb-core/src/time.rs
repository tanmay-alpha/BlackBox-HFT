//! Explicit timestamp units. Milliseconds, microseconds and nanoseconds are never mixed silently:
//! every conversion goes through [`TimeUnit`] and is range-checked.

use serde::{Deserialize, Serialize};

/// Absolute nanoseconds since the Unix epoch. Engine-internal only; never serialized as a number.
pub type Ns = i64;

/// Nanoseconds relative to a dataset's `origin_ns`. Safe as a JSON number.
pub type RelNs = i64;

pub const NS_PER_US: i64 = 1_000;
pub const NS_PER_MS: i64 = 1_000_000;
pub const NS_PER_S: i64 = 1_000_000_000;

/// 2010-01-01T00:00:00Z in epoch nanoseconds. Lower plausibility bound for market data.
pub const MIN_PLAUSIBLE_NS: Ns = 1_262_304_000 * NS_PER_S;
/// 2100-01-01T00:00:00Z in epoch nanoseconds. Upper plausibility bound for market data.
pub const MAX_PLAUSIBLE_NS: Ns = 4_102_444_800 * NS_PER_S;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    Seconds,
    Millis,
    Micros,
    Nanos,
}

impl TimeUnit {
    /// Nanoseconds per one unit.
    pub const fn ns_per_unit(self) -> i64 {
        match self {
            TimeUnit::Seconds => NS_PER_S,
            TimeUnit::Millis => NS_PER_MS,
            TimeUnit::Micros => NS_PER_US,
            TimeUnit::Nanos => 1,
        }
    }

    /// Converts `value` in this unit to nanoseconds. `None` on overflow.
    pub fn to_ns(self, value: i64) -> Option<Ns> {
        value.checked_mul(self.ns_per_unit())
    }

    /// Converts and additionally requires the result to be a plausible market-data epoch time.
    /// A value expressed in the wrong unit is off by at least a factor of 1000 and fails this.
    pub fn to_plausible_ns(self, value: i64) -> Option<Ns> {
        self.to_ns(value).filter(|ns| is_plausible_epoch_ns(*ns))
    }

    /// The unique unit under which `value` is a plausible epoch time, if exactly one exists.
    pub fn infer(value: i64) -> Option<TimeUnit> {
        let mut found = None;
        for unit in [
            TimeUnit::Seconds,
            TimeUnit::Millis,
            TimeUnit::Micros,
            TimeUnit::Nanos,
        ] {
            if unit.to_plausible_ns(value).is_some() {
                if found.is_some() {
                    return None;
                }
                found = Some(unit);
            }
        }
        found
    }
}

pub fn is_plausible_epoch_ns(ns: Ns) -> bool {
    (MIN_PLAUSIBLE_NS..MAX_PLAUSIBLE_NS).contains(&ns)
}

/// Largest integer a JSON reader using IEEE-754 doubles (JavaScript's `Number`) represents
/// exactly: 2^53 − 1 (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// Longest dataset span, in nanoseconds, whose relative timestamps all stay exact as JSON
/// numbers: 2^53 − 1 ns, about 104.2 days. Ingest and generation refuse anything longer.
pub const MAX_SPAN_NS: i64 = MAX_SAFE_INTEGER;

/// Serde adapter for absolute epoch nanoseconds (or any integer that may exceed 2^53 − 1).
/// Writes a decimal string; reads a decimal string or, for files written before this adapter
/// existed, a JSON integer.
///
/// ```ignore
/// #[serde(with = "bb_core::time::int_string")]
/// pub origin_ns: i64,
/// ```
pub mod int_string {
    use std::{fmt::Display, str::FromStr};

    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<T: Display, S: Serializer>(value: &T, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(value)
    }

    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        T: FromStr + TryFrom<i64> + TryFrom<u64>,
        <T as FromStr>::Err: Display,
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Str(String),
            Signed(i64),
            Unsigned(u64),
        }
        let out_of_range = || D::Error::custom("integer out of range");
        match Repr::deserialize(d)? {
            Repr::Str(s) => s.parse().map_err(D::Error::custom),
            Repr::Signed(v) => T::try_from(v).map_err(|_| out_of_range()),
            Repr::Unsigned(v) => T::try_from(v).map_err(|_| out_of_range()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2020-02-01T00:00:02.342Z, the first trade in the Tardis sample file.
    const SAMPLE_US: i64 = 1_580_515_202_342_000;

    #[test]
    fn unit_is_inferred_only_when_unambiguous() {
        assert_eq!(TimeUnit::infer(SAMPLE_US), Some(TimeUnit::Micros));
        assert_eq!(TimeUnit::infer(SAMPLE_US / 1000), Some(TimeUnit::Millis));
        assert_eq!(TimeUnit::infer(SAMPLE_US * 1000), Some(TimeUnit::Nanos));
        assert_eq!(
            TimeUnit::infer(SAMPLE_US / 1_000_000),
            Some(TimeUnit::Seconds)
        );
        assert_eq!(TimeUnit::infer(0), None);
        assert_eq!(TimeUnit::infer(-5), None);
    }

    #[test]
    fn wrong_unit_is_rejected_not_silently_converted() {
        // Microseconds declared as milliseconds lands in the far future.
        assert_eq!(TimeUnit::Millis.to_plausible_ns(SAMPLE_US), None);
        // Microseconds declared as nanoseconds lands in 1970.
        assert_eq!(TimeUnit::Nanos.to_plausible_ns(SAMPLE_US), None);
        assert_eq!(
            TimeUnit::Micros.to_plausible_ns(SAMPLE_US),
            Some(SAMPLE_US * 1000)
        );
    }

    #[test]
    fn overflow_is_an_error() {
        assert_eq!(TimeUnit::Seconds.to_ns(i64::MAX / 2), None);
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Abs {
        #[serde(with = "int_string")]
        ns: i64,
    }

    #[test]
    fn absolute_ns_are_written_as_strings_and_read_in_either_form() {
        let v = Abs {
            ns: 1_700_000_000_000_000_001,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#"{"ns":"1700000000000000001"}"#);
        assert_eq!(serde_json::from_str::<Abs>(&json).unwrap(), v);
        // Files written before the adapter existed hold a JSON number.
        assert_eq!(
            serde_json::from_str::<Abs>(r#"{"ns":1700000000000000001}"#).unwrap(),
            v
        );
        assert_eq!(
            serde_json::from_str::<Abs>(r#"{"ns":-5}"#).unwrap(),
            Abs { ns: -5 }
        );
        assert!(serde_json::from_str::<Abs>(r#"{"ns":"x"}"#).is_err());
        assert!(serde_json::from_str::<Abs>(r#"{"ns":18446744073709551615}"#).is_err());
        assert_eq!(MAX_SAFE_INTEGER, 9_007_199_254_740_991);
    }
}
