//! Number formatting for reason strings.
//!
//! Reasons are shown verbatim in the forensics view and are part of a run's hashed output, so
//! every number is printed with a fixed number of decimals derived from the instrument's
//! increments: `100.10`, never `100.10000000000001`.

use std::fmt;

use bb_core::Side;
use bb_core::dataset::InstrumentMeta;

/// Most decimals ever printed for a price or quantity increment.
const MAX_DECIMALS: usize = 12;

/// Smallest number of decimals that prints `step` exactly (2 for `0.01`, 0 for `5`).
fn decimals_for(step: f64) -> usize {
    if !(step.is_finite() && step > 0.0) {
        return 0;
    }
    for decimals in 0..=MAX_DECIMALS {
        let scaled = step * 10f64.powi(decimals as i32);
        if scaled >= 0.5 && (scaled - scaled.round()).abs() <= 1e-9 * scaled {
            return decimals;
        }
    }
    MAX_DECIMALS
}

/// A number printed with a fixed number of decimals.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Fixed {
    value: f64,
    decimals: usize,
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A value that rounds to zero must not print as "-0.00": it would read as a sign.
        let scale = 10f64.powi(self.decimals as i32);
        let value = if (self.value * scale).round() == 0.0 {
            0.0
        } else {
            self.value
        };
        write!(f, "{value:.prec$}", prec = self.decimals)
    }
}

/// A best price that may be absent from the local book.
#[derive(Copy, Clone, Debug)]
pub(crate) struct OptFixed(Option<Fixed>);

impl fmt::Display for OptFixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => fmt::Display::fmt(value, f),
            None => f.write_str("none"),
        }
    }
}

/// A count of price ticks with the right plural.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Ticks(pub i64);

impl fmt::Display for Ticks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 1 || self.0 == -1 {
            write!(f, "{} tick", self.0)
        } else {
            write!(f, "{} ticks", self.0)
        }
    }
}

/// "bid" / "ask": what a resting quote on `side` is called.
pub(crate) fn quote_name(side: Side) -> &'static str {
    match side {
        Side::Buy => "bid",
        Side::Sell => "ask",
    }
}

/// "buy" / "sell".
pub(crate) fn verb(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

/// The instrument's increments plus the formatting they imply.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Text {
    instrument: InstrumentMeta,
    price_decimals: usize,
    qty_decimals: usize,
}

impl Text {
    pub(crate) fn new(instrument: &InstrumentMeta) -> Self {
        Self {
            instrument: *instrument,
            price_decimals: decimals_for(instrument.tick_size),
            qty_decimals: decimals_for(instrument.lot_size),
        }
    }

    pub(crate) fn tick_size(&self) -> f64 {
        self.instrument.tick_size
    }

    pub(crate) fn lot_size(&self) -> f64 {
        self.instrument.lot_size
    }

    /// Tolerance for comparing quantities, which are whole multiples of the lot size.
    pub(crate) fn half_lot(&self) -> f64 {
        0.5 * self.instrument.lot_size
    }

    /// Whole number of lots nearest to `qty`.
    pub(crate) fn lots(&self, qty: f64) -> i64 {
        self.instrument.lots(qty)
    }

    /// An on-grid price given in ticks, shown in price units.
    pub(crate) fn px(&self, tick: i64) -> Fixed {
        Fixed {
            value: self.instrument.price(tick),
            decimals: self.price_decimals,
        }
    }

    /// Like [`Text::px`], printing `none` for a missing best price.
    pub(crate) fn opt_px(&self, tick: Option<i64>) -> OptFixed {
        OptFixed(tick.map(|t| self.px(t)))
    }

    /// A value in price units that need not sit on the tick grid (mid, reservation price,
    /// half spread, volatility). Two extra decimals keep sub-tick differences visible.
    pub(crate) fn price(&self, value: f64) -> Fixed {
        Fixed {
            value,
            decimals: self.price_decimals + 2,
        }
    }

    pub(crate) fn qty(&self, qty: f64) -> Fixed {
        Fixed {
            value: qty,
            decimals: self.qty_decimals,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(tick_size: f64, lot_size: f64) -> Text {
        Text::new(&InstrumentMeta {
            tick_size,
            lot_size,
            contract_size: 1.0,
        })
    }

    #[test]
    fn decimals_follow_the_increment() {
        assert_eq!(decimals_for(0.01), 2);
        assert_eq!(decimals_for(0.001), 3);
        assert_eq!(decimals_for(0.5), 1);
        assert_eq!(decimals_for(0.25), 2);
        assert_eq!(decimals_for(1.0), 0);
        assert_eq!(decimals_for(5.0), 0);
        assert_eq!(decimals_for(0.000_000_01), 8);
        assert_eq!(decimals_for(0.0), 0);
        assert_eq!(decimals_for(f64::NAN), 0);
    }

    #[test]
    fn prices_and_quantities_print_without_float_noise() {
        let t = text(0.01, 0.001);
        // 10010 * 0.01 is 100.10000000000001 in binary floating point.
        assert_eq!(t.px(10_010).to_string(), "100.10");
        assert_eq!(t.opt_px(Some(-5)).to_string(), "-0.05");
        assert_eq!(t.opt_px(None).to_string(), "none");
        assert_eq!(t.price(100.123_456).to_string(), "100.1235");
        assert_eq!(t.qty(0.4).to_string(), "0.400");
        assert_eq!(t.qty(-1e-17).to_string(), "0.000");
        assert_eq!(t.qty(-0.0).to_string(), "0.000");
        assert_eq!(text(0.5, 1.0).px(3).to_string(), "1.5");
        assert_eq!(text(0.5, 1.0).qty(7.0).to_string(), "7");
    }

    #[test]
    fn ticks_are_pluralised() {
        assert_eq!(Ticks(1).to_string(), "1 tick");
        assert_eq!(Ticks(-1).to_string(), "-1 tick");
        assert_eq!(Ticks(0).to_string(), "0 ticks");
        assert_eq!(Ticks(-3).to_string(), "-3 ticks");
    }
}
