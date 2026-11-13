//! BlackBox's own account ledger.
//!
//! The engine keeps a local-side account (`StateValues`), but in 0.9.4 that account only applies
//! responses whose status is `Filled` and uses the response's last `exec_qty`. Partial fills and
//! multi-level taker sweeps are therefore under-counted (see docs/ENGINE_NOTES.md). BlackBox
//! records every execution from the exchange side and does its own bookkeeping; the engine's
//! account is used only as a cross-check where the two are expected to agree.

/// Position, cash and average-cost P&L for one instrument.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Book {
    /// Signed position in base units.
    pub position: f64,
    /// Cumulative signed cash flow from fills, before fees. Same arithmetic as the engine's
    /// `StateValues::balance`.
    pub cash: f64,
    pub fees: f64,
    pub fills: u64,
    /// Average entry price of the open position (price units). Meaningless while flat.
    pub avg_entry: f64,
    /// Realized P&L under average-cost accounting, before fees.
    pub realized: f64,
}

impl Book {
    /// Applies one execution and returns the realized P&L it booked.
    ///
    /// * `sign` is `+1.0` for a buy, `-1.0` for a sell.
    /// * `value` is the traded value in quote currency as the engine computed it.
    /// * `half_lot` is the tolerance below which a position counts as flat.
    pub fn apply(
        &mut self,
        sign: f64,
        price: f64,
        qty: f64,
        value: f64,
        fee: f64,
        contract_size: f64,
        half_lot: f64,
    ) -> f64 {
        let prev = self.position;

        // Identical operations, in identical order, to the engine's `State::apply_fill`, so the
        // two can be compared exactly when they see the same fills.
        self.position += qty * sign;
        self.cash -= value * sign;
        self.fees += fee;
        self.fills += 1;

        let mut realized = 0.0;
        let prev_abs = prev.abs();
        if prev_abs < half_lot {
            // Opening from flat.
            self.avg_entry = price;
        } else if prev.signum() == sign {
            // Adding to the position: quantity-weighted average entry.
            self.avg_entry = (prev_abs * self.avg_entry + qty * price) / (prev_abs + qty);
        } else {
            // Reducing, closing or flipping.
            let closed = qty.min(prev_abs);
            realized = closed * (price - self.avg_entry) * prev.signum() * contract_size;
            self.realized += realized;
            if qty - prev_abs > half_lot {
                // Flipped: the remainder opens a new position at this price.
                self.avg_entry = price;
            }
        }
        realized
    }

    /// Unrealized P&L at `mark` (price units); zero while flat.
    pub fn unrealized(&self, mark: f64, contract_size: f64, half_lot: f64) -> f64 {
        if self.position.abs() < half_lot {
            0.0
        } else {
            self.position * (mark - self.avg_entry) * contract_size
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HL: f64 = 0.0005;

    fn fill(b: &mut Book, sign: f64, price: f64, qty: f64) -> f64 {
        b.apply(sign, price, qty, price * qty, 0.0, 1.0, HL)
    }

    #[test]
    fn round_trip_realizes_the_price_difference() {
        let mut b = Book::default();
        assert_eq!(fill(&mut b, 1.0, 100.0, 2.0), 0.0);
        let r = fill(&mut b, -1.0, 101.5, 2.0);
        assert!((r - 3.0).abs() < 1e-12);
        assert!(b.position.abs() < HL);
        assert!((b.realized - 3.0).abs() < 1e-12);
        assert_eq!(b.unrealized(200.0, 1.0, HL), 0.0);
    }

    #[test]
    fn adding_averages_and_partial_close_keeps_entry() {
        let mut b = Book::default();
        fill(&mut b, 1.0, 100.0, 1.0);
        fill(&mut b, 1.0, 102.0, 3.0);
        assert!((b.avg_entry - 101.5).abs() < 1e-12);
        let r = fill(&mut b, -1.0, 103.5, 1.0);
        assert!((r - 2.0).abs() < 1e-12);
        assert!((b.avg_entry - 101.5).abs() < 1e-12);
        assert!((b.position - 3.0).abs() < 1e-12);
    }

    #[test]
    fn flip_realizes_only_the_closed_part() {
        let mut b = Book::default();
        fill(&mut b, 1.0, 100.0, 1.0);
        let r = fill(&mut b, -1.0, 98.0, 3.0);
        assert!((r + 2.0).abs() < 1e-12);
        assert!((b.position + 2.0).abs() < 1e-12);
        assert_eq!(b.avg_entry, 98.0);
        // Short 2 from 98, marked at 97: +2.
        assert!((b.unrealized(97.0, 1.0, HL) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn realized_plus_unrealized_equals_cash_plus_marked_position() {
        // The accounting identity that ties average-cost P&L to the engine-style cash balance.
        let mut b = Book::default();
        let trades = [
            (1.0, 100.0, 2.0),
            (1.0, 101.0, 1.0),
            (-1.0, 103.0, 4.0),
            (-1.0, 99.0, 1.0),
            (1.0, 98.5, 3.5),
        ];
        for (s, p, q) in trades {
            fill(&mut b, s, p, q);
            let mark = 100.25;
            let lhs = b.realized + b.unrealized(mark, 1.0, HL);
            let rhs = b.cash + b.position * mark;
            assert!((lhs - rhs).abs() < 1e-9, "{lhs} vs {rhs}");
        }
    }
}
