//! Number, price and time formatting shared by explanations and comparison notes.
//!
//! Statements quote prices in price units and quantities in base units, at the precision of the
//! instrument's tick and lot, so a reader can match them against the book without converting.

use bb_core::{
    OrderStatus, RelNs, Side, Tif, dataset::InstrumentMeta, flight::RiskRule, result::OrderOutcome,
};

/// Decimal places needed to print multiples of `step` exactly (at most 9).
pub(crate) fn decimals_for(step: f64) -> usize {
    if !(step.is_finite() && step > 0.0) {
        return 6;
    }
    for dp in 0..=9usize {
        let scaled = step * 10f64.powi(dp as i32);
        let nearest = scaled.round();
        if nearest >= 1.0 && (scaled - nearest).abs() <= 1e-9 * nearest {
            return dp;
        }
    }
    9
}

/// Fixed-point with `dp` decimals. A value that rounds to zero never prints as `-0.00`.
pub(crate) fn fixed(v: f64, dp: usize) -> String {
    let s = format!("{v:.dp$}");
    match s.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|c| c == b'0' || c == b'.') => rest.to_string(),
        _ => s,
    }
}

fn trim_zeros(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Up to nine decimals with trailing zeros removed. For values without an instrument context.
pub(crate) fn num(v: f64) -> String {
    trim_zeros(fixed(v, 9))
}

/// `dp` decimals when `v` is exact at that precision, two more otherwise. Model estimates are not
/// multiples of the lot size, and rounding them to it would hide part of the number.
fn fixed_adaptive(v: f64, dp: usize) -> String {
    let scaled = v * 10f64.powi(dp as i32);
    if (scaled - scaled.round()).abs() <= 1e-6 {
        fixed(v, dp)
    } else {
        fixed(v, dp + 2)
    }
}

/// Instrument-aware formatting.
pub(crate) struct Fmt {
    tick_size: f64,
    lot_size: f64,
    price_dp: usize,
    qty_dp: usize,
}

impl Fmt {
    pub(crate) fn new(instrument: &InstrumentMeta) -> Self {
        Self {
            tick_size: instrument.tick_size,
            lot_size: instrument.lot_size,
            price_dp: decimals_for(instrument.tick_size),
            qty_dp: decimals_for(instrument.lot_size),
        }
    }

    /// A price on the tick grid.
    pub(crate) fn price(&self, tick: i64) -> String {
        fixed(tick as f64 * self.tick_size, self.price_dp)
    }

    /// A price that may lie between ticks (an average fill price).
    pub(crate) fn price_frac(&self, tick: f64) -> String {
        fixed_adaptive(tick * self.tick_size, self.price_dp)
    }

    /// An order, fill, trade or displayed quantity.
    pub(crate) fn qty(&self, qty: f64) -> String {
        fixed(qty, self.qty_dp)
    }

    /// A modeled quantity, which need not be a multiple of the lot size.
    pub(crate) fn est(&self, qty: f64) -> String {
        fixed_adaptive(qty, self.qty_dp)
    }

    /// Whether `qty` is at least one lot after rounding, the engine's own test for "non-zero".
    pub(crate) fn has_qty(&self, qty: f64) -> bool {
        if self.lot_size.is_finite() && self.lot_size > 0.0 {
            (qty / self.lot_size).round() >= 1.0
        } else {
            qty > 0.0
        }
    }
}

/// A duration, in the largest unit that keeps it at or above one.
pub(crate) fn dur(ns: i64) -> String {
    let sign = if ns < 0 { "-" } else { "" };
    let a = ns.unsigned_abs();
    let (value, unit) = if a < 1_000 {
        return format!("{sign}{a} ns");
    } else if a < 1_000_000 {
        (a as f64 / 1e3, "µs")
    } else if a < 1_000_000_000 {
        (a as f64 / 1e6, "ms")
    } else {
        (a as f64 / 1e9, "s")
    };
    format!("{sign}{} {unit}", trim_zeros(fixed(value, 3)))
}

/// A point in time relative to the dataset origin, exact to the nanosecond.
pub(crate) fn at(ts: RelNs) -> String {
    let sign = if ts < 0 { "-" } else { "" };
    let a = ts.unsigned_abs();
    let frac = format!("{:09}", a % 1_000_000_000);
    let keep = frac.trim_end_matches('0').len().max(3);
    format!("t={sign}{}.{}s", a / 1_000_000_000, &frac[..keep])
}

/// `"1 fill"`, `"3 fills"`.
pub(crate) fn count(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

pub(crate) fn side_verb(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

pub(crate) fn tif_label(tif: Tif) -> &'static str {
    match tif {
        Tif::Gtc => "GTC",
        Tif::Gtx => "post-only (GTX)",
        Tif::Ioc => "IOC",
        Tif::Fok => "FOK",
    }
}

pub(crate) fn status_label(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::None => "unacknowledged",
        OrderStatus::New => "resting",
        OrderStatus::Expired => "expired",
        OrderStatus::Filled => "filled",
        OrderStatus::Canceled => "canceled",
        OrderStatus::PartiallyFilled => "partially filled",
        OrderStatus::Unsupported => "in an unsupported state",
    }
}

/// Completes "the order was ...".
pub(crate) fn outcome_label(outcome: OrderOutcome) -> &'static str {
    match outcome {
        OrderOutcome::RiskBlocked => "blocked by a risk rule",
        OrderOutcome::Refused => "refused by the engine",
        OrderOutcome::InFlightAtEnd => "still in flight when the data ended",
        OrderOutcome::Filled => "filled",
        OrderOutcome::PartiallyFilled => "partially filled",
        OrderOutcome::Canceled => "canceled unfilled",
        OrderOutcome::Expired => "expired unfilled",
        OrderOutcome::WorkingAtEnd => "still working when the data ended",
    }
}

/// The rule's serialized name, so it can be matched against the risk configuration.
pub(crate) fn rule_label(rule: RiskRule) -> &'static str {
    match rule {
        RiskRule::MaxPosition => "max_position",
        RiskRule::MaxOrderQty => "max_order_qty",
        RiskRule::MaxOpenOrders => "max_open_orders",
        RiskRule::MaxDrawdown => "max_drawdown",
        RiskRule::InvalidOrder => "invalid_order",
        RiskRule::Halted => "halted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_follow_the_step() {
        assert_eq!(decimals_for(1.0), 0);
        assert_eq!(decimals_for(10.0), 0);
        assert_eq!(decimals_for(0.5), 1);
        assert_eq!(decimals_for(0.25), 2);
        assert_eq!(decimals_for(0.01), 2);
        assert_eq!(decimals_for(0.001), 3);
        assert_eq!(decimals_for(0.00001), 5);
    }

    #[test]
    fn prices_and_quantities_use_instrument_precision() {
        let f = Fmt::new(&InstrumentMeta {
            tick_size: 0.5,
            lot_size: 0.001,
            contract_size: 1.0,
        });
        assert_eq!(f.price(20_001), "10000.5");
        assert_eq!(f.price_frac(20_000.5), "10000.250");
        assert_eq!(f.price_frac(20_002.0), "10001.0");
        assert_eq!(f.qty(1.25), "1.250");
        assert_eq!(f.est(0.8), "0.800");
        assert_eq!(f.est(0.73456), "0.73456");
        assert_eq!(f.est(-0.000_000_000_1), "0.000");
        assert!(f.has_qty(0.001));
        assert!(!f.has_qty(0.0004));
    }

    #[test]
    fn durations_and_timestamps() {
        assert_eq!(dur(750), "750 ns");
        assert_eq!(dur(150_000), "150 µs");
        assert_eq!(dur(1_500_000), "1.5 ms");
        assert_eq!(dur(2_000_000_000), "2 s");
        assert_eq!(dur(-1_000_000), "-1 ms");
        assert_eq!(at(12_345_678_901), "t=12.345678901s");
        assert_eq!(at(11_000_000), "t=0.011s");
        assert_eq!(at(0), "t=0.000s");
        assert_eq!(num(0.1 + 0.2), "0.3");
        assert_eq!(num(-0.0), "0");
        assert_eq!(count(1, "fill"), "1 fill");
        assert_eq!(count(2, "fill"), "2 fills");
    }
}
