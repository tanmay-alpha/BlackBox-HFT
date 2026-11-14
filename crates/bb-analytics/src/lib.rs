//! Exchange-clock research analytics; never used by a strategy decision path.
use serde::{Deserialize, Serialize};

pub mod execution;
pub mod market;
pub mod risk;

pub const ANALYTICS_VERSION: &str = "1.0.0";

/// A value always carries its units and contributing/excluded observation counts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Value {
    Available {
        value: f64,
        unit: String,
        n: u64,
        excluded: u64,
    },
    Unavailable {
        reason: String,
        unit: String,
        n: u64,
        excluded: u64,
    },
}
/// `Value` under a name utoipa does not mistake for `serde_json::Value` (it treats any type whose
/// last path segment is `Value` as an arbitrary JSON value). Fields typed [`Value`] use it as their
/// schema `value_type`.
#[cfg(feature = "openapi")]
pub type AnalyticsValue = Value;
impl Value {
    pub fn measured(value: f64, unit: &str, n: u64, excluded: u64) -> Self {
        if value.is_finite() {
            Self::Available {
                value,
                unit: unit.into(),
                n,
                excluded,
            }
        } else {
            Self::unavailable("non-finite result", unit, excluded)
        }
    }
    pub fn unavailable(reason: &str, unit: &str, excluded: u64) -> Self {
        Self::Unavailable {
            reason: reason.into(),
            unit: unit.into(),
            n: 0,
            excluded,
        }
    }
    pub fn get(&self) -> Option<f64> {
        match self {
            Self::Available { value, .. } => Some(*value),
            Self::Unavailable { .. } => None,
        }
    }
}

/// Compensated long sum shared by aggregators.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Sum {
    value: f64,
    correction: f64,
}
impl Sum {
    pub(crate) fn add(&mut self, x: f64) {
        let y = x - self.correction;
        let t = self.value + y;
        self.correction = (t - self.value) - y;
        self.value = t;
    }
    pub(crate) fn get(self) -> f64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_outputs_never_serialize_nonfinite_as_missing_numbers() {
        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let value = Value::measured(number, "ticks", 1, 2);
            assert!(
                matches!(&value, Value::Unavailable { reason, unit, excluded: 2, .. }
                if reason == "non-finite result" && unit == "ticks")
            );
            let json = serde_json::to_value(&value).unwrap();
            assert_eq!(json["status"], "unavailable");
            assert!(json.get("value").is_none());
        }
        let available = Value::measured(2.5, "ticks", 3, 4);
        let json = serde_json::to_value(&available).unwrap();
        assert_eq!(json["value"], 2.5);
        assert_eq!(json["unit"], "ticks");
        assert_eq!(json["n"], 3);
        assert_eq!(json["excluded"], 4);
    }

    #[test]
    fn compensated_sum_retains_small_contributions() {
        // Adding 10,000 units to 1e16 naively loses every unit; compensation retains the sum.
        let mut sum = Sum::default();
        sum.add(1e16);
        for _ in 0..10_000 {
            sum.add(1.0);
        }
        sum.add(-1e16);
        assert_eq!(sum.get(), 10_000.0);
    }
}
