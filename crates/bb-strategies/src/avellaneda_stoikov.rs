//! Avellaneda-Stoikov market maker.
//!
//! Reference: M. Avellaneda and S. Stoikov (2008), "High-frequency trading in a limit order
//! book", Quantitative Finance 8(3), 217-224.
//!
//! This is an original implementation of the formulas published in that paper, written against
//! the BlackBox strategy contract. No code was copied from any other implementation.
//!
//! With inventory `q`, risk aversion `gamma`, mid-price variance per second `sigma^2`, remaining
//! horizon `T - t` and order-arrival decay `kappa` (arrival intensity
//! `lambda(delta) = A * exp(-kappa * delta)`), the paper's approximate optimal quotes are centred
//! on the reservation price
//!
//! ```text
//! r = s - q * gamma * sigma^2 * (T - t)
//! ```
//!
//! with a total spread of `gamma * sigma^2 * (T - t) + (2 / gamma) * ln(1 + gamma / kappa)`, so
//! each quote sits a half spread
//!
//! ```text
//! delta = 0.5 * gamma * sigma^2 * (T - t) + (1 / gamma) * ln(1 + gamma / kappa)
//! ```
//!
//! away from `r`. `T - t` is held constant at `horizon_s` (the stationary variant commonly used
//! when there is no terminal time). `sigma^2` is not an input: it is estimated online from the
//! mid prices the strategy host has seen, as described on [`AvellanedaStoikov`].

use bb_core::Side;
use bb_core::config::AvellanedaStoikovParams;
use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::{Intent, LocalView, Strategy};
use bb_core::time::NS_PER_S;

use crate::quoting::{QuoteRules, Quotes, cancel_all_one_sided, sync_quotes};
use crate::text::Text;

const NAME: &str = "avellaneda_stoikov";

/// Relative tolerance, in ticks, within which a price is treated as lying exactly on the grid.
/// Without it `8.2 / 0.01` evaluating to `819.9999999999999` would floor a whole tick low.
const GRID_TOLERANCE: f64 = 1e-11;

fn on_grid(ticks: f64) -> Option<f64> {
    let nearest = ticks.round();
    ((ticks - nearest).abs() <= GRID_TOLERANCE * ticks.abs().max(1.0)).then_some(nearest)
}

fn floor_tick(ticks: f64) -> i64 {
    on_grid(ticks).unwrap_or_else(|| ticks.floor()) as i64
}

fn ceil_tick(ticks: f64) -> i64 {
    on_grid(ticks).unwrap_or_else(|| ticks.ceil()) as i64
}

/// Slot 0 is the bid quote, slot 1 the ask quote; both are `Gtx` limit orders at
/// `floor((r - delta) / tick_size)` and `ceil((r + delta) / tick_size)`, kept off the opposite
/// side of the locally known book. Order maintenance and the `max_position` gate are the same as
/// [`crate::TouchMaker`]'s.
///
/// # Volatility estimate
///
/// On every tick with a two-sided local book the mid (in price units) is one observation. The
/// squared change since the previous observation feeds an exponentially weighted variance with
/// decay `alpha = 1 - 0.5^(1 / vol_halflife_ticks)`; the mean change is taken as zero, the usual
/// choice at these horizons. If the book was one-sided in between, the squared change is divided
/// by the number of ticks it spans so that it still estimates a one-tick variance. The first
/// change seeds the average instead of being blended with an arbitrary zero. Then
/// `sigma^2 = ewma_var / interval_seconds`.
///
/// Nothing is quoted (and nothing cancelled) until `vol_halflife_ticks` observations exist.
/// After warm-up a one-sided book cancels all quotes: there is no mid to centre them on.
#[derive(Clone, Debug)]
pub struct AvellanedaStoikov {
    params: AvellanedaStoikovParams,
    text: Text,
    /// Strategy grid step in seconds; converts per-tick variance to per-second variance.
    interval_s: f64,
    alpha: f64,
    /// Tick number and mid (price units) of the latest observation.
    last_mid: Option<(u64, f64)>,
    /// Variance of the mid change over one tick, in price units squared.
    ewma_var: f64,
    observations: u64,
}

impl AvellanedaStoikov {
    /// `interval_ns` is the strategy grid step, [`bb_core::config::StrategyConfig::interval_ns`].
    pub fn new(
        params: AvellanedaStoikovParams,
        interval_ns: i64,
        instrument: &InstrumentMeta,
    ) -> Self {
        Self {
            params,
            text: Text::new(instrument),
            interval_s: interval_ns as f64 / NS_PER_S as f64,
            alpha: 1.0 - 0.5f64.powf(1.0 / f64::from(params.vol_halflife_ticks)),
            last_mid: None,
            ewma_var: 0.0,
            observations: 0,
        }
    }

    fn warmed_up(&self) -> bool {
        self.observations >= u64::from(self.params.vol_halflife_ticks)
    }

    fn observe(&mut self, tick_no: u64, mid: f64) {
        if let Some((prev_tick, prev_mid)) = self.last_mid {
            let elapsed_ticks = tick_no.saturating_sub(prev_tick).max(1) as f64;
            let change = mid - prev_mid;
            let sample = change * change / elapsed_ticks;
            self.ewma_var = if self.observations == 1 {
                sample
            } else {
                self.ewma_var + self.alpha * (sample - self.ewma_var)
            };
        }
        self.last_mid = Some((tick_no, mid));
        self.observations += 1;
    }
}

impl Strategy for AvellanedaStoikov {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        let (Some(best_bid), Some(best_ask)) =
            (view.market.best_bid_tick, view.market.best_ask_tick)
        else {
            if self.warmed_up() {
                cancel_all_one_sided(NAME, &self.text, view, out);
            }
            return;
        };

        let tick_size = self.text.tick_size();
        let mid = (best_bid as f64 + best_ask as f64) * 0.5 * tick_size;
        self.observe(view.tick_no, mid);
        if !self.warmed_up() {
            return;
        }

        let p = &self.params;
        let text = &self.text;
        let sigma2 = self.ewma_var / self.interval_s;
        let inventory_risk = p.gamma * sigma2 * p.horizon_s;
        let reservation = mid - view.position * inventory_risk;
        let half_spread = 0.5 * inventory_risk + (p.gamma / p.kappa).ln_1p() / p.gamma;
        if !(reservation.is_finite() && half_spread.is_finite()) {
            // Only reachable with parameters `RunConfig::validate` rejects (a non-positive
            // interval, gamma or kappa). A price derived from NaN or infinity would be noise,
            // so nothing is quoted.
            return;
        }

        let quotes = Quotes::clamped(
            floor_tick((reservation - half_spread) / tick_size),
            ceil_tick((reservation + half_spread) / tick_size),
            best_bid,
            best_ask,
        );
        let rules = QuoteRules {
            name: NAME,
            text,
            order_qty: p.order_qty,
            max_position: p.max_position,
        };
        let rationale = |side: Side| {
            let rounding = match side {
                Side::Buy => "floor((r - delta) / tick size)",
                Side::Sell => "ceil((r + delta) / tick size)",
            };
            format!(
                "{rounding} with reservation price r {} = mid {} - position {} x gamma {} x \
                 sigma^2 x horizon {} s, half spread delta {}, sigma {} per sqrt(s), kappa {}, \
                 tick size {}",
                text.price(reservation),
                text.price(mid),
                text.qty(view.position),
                p.gamma,
                p.horizon_s,
                text.price(half_spread),
                text.price(sigma2.sqrt()),
                p.kappa,
                tick_size,
            )
        };
        sync_quotes(&rules, view, &quotes, rationale, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        INSTRUMENT, INTERVAL_NS, cancelled, market, placed, reasons, resting, run, view,
    };
    use bb_core::strategy::{MarketView, order_id};

    const HALFLIFE: u32 = 4;

    fn params(gamma: f64) -> AvellanedaStoikovParams {
        AvellanedaStoikovParams {
            order_qty: 1.0,
            max_position: 10.0,
            gamma,
            kappa: 1.5,
            horizon_s: 10.0,
            vol_halflife_ticks: HALFLIFE,
        }
    }

    fn strategy(gamma: f64) -> AvellanedaStoikov {
        AvellanedaStoikov::new(params(gamma), INTERVAL_NS, &INSTRUMENT)
    }

    /// Mid alternates between 100.50 and 100.60, so every one-tick change is 0.10 price units:
    /// the per-tick variance is 0.01 and, on a 0.1 s grid, `sigma^2 = 0.1` per second. The book
    /// is 100 ticks wide so that no quote in these tests is clamped.
    fn book(tick_no: u64) -> MarketView {
        let shift = if tick_no % 2 == 0 { 0 } else { 10 };
        market(Some(10_000 + shift), Some(10_100 + shift))
    }

    /// Feeds the first `HALFLIFE` ticks (enough to finish warm-up), then returns the intents of
    /// tick `HALFLIFE` at the given position with no orders open.
    fn quotes_after_warm_up(s: &mut AvellanedaStoikov, position: f64) -> Vec<Intent> {
        for tick_no in 0..u64::from(HALFLIFE) {
            run(s, &view(tick_no, book(tick_no), 0.0, &[]));
        }
        let tick_no = u64::from(HALFLIFE);
        run(s, &view(tick_no, book(tick_no), position, &[]))
    }

    fn bid_ask(intents: &[Intent]) -> (i64, i64) {
        let quotes = placed(intents);
        assert_eq!(quotes.len(), 2, "expected one bid and one ask: {intents:?}");
        assert_eq!((quotes[0].0, quotes[0].1), (0, Side::Buy));
        assert_eq!((quotes[1].0, quotes[1].1), (1, Side::Sell));
        (quotes[0].2, quotes[1].2)
    }

    #[test]
    fn silent_during_warm_up() {
        let mut s = strategy(0.1);
        // A stray order must not be cancelled either while the estimate is warming up.
        let open = [resting(order_id(0, 0), Side::Buy, 9_000)];
        for tick_no in 0..u64::from(HALFLIFE) - 1 {
            assert!(run(&mut s, &view(tick_no, book(tick_no), 0.0, &open)).is_empty());
        }
        // A one-sided tick is not an observation and does not end the warm-up.
        assert!(run(&mut s, &view(3, market(Some(10_000), None), 0.0, &open)).is_empty());
        // The fourth mid observation completes it.
        let out = run(&mut s, &view(4, book(4), 0.0, &[]));
        assert_eq!(placed(&out).len(), 2);
    }

    #[test]
    fn zero_inventory_quotes_are_symmetric_around_mid() {
        let mut s = strategy(0.1);
        let out = quotes_after_warm_up(&mut s, 0.0);
        let (bid, ask) = bid_ask(&out);
        let mid = book(u64::from(HALFLIFE)).mid_tick().unwrap();
        assert_eq!(mid, 10_050.0);
        assert_eq!(mid - bid as f64, ask as f64 - mid);
        // delta = 0.5 * 0.1 * 0.1 * 10 + ln(1 + 0.1 / 1.5) / 0.1 = 0.6954 price units, i.e.
        // 69.54 ticks, rounded outward to 70.
        assert_eq!((bid, ask), (9_980, 10_120));

        let reason = reasons(&out)[0];
        assert!(reason.starts_with("avellaneda_stoikov: quote bid 99.80 x 1.000 post-only:"));
        assert!(reason.contains("r 100.5000"), "{reason}");
        assert!(reason.contains("delta 0.6954"), "{reason}");
        assert!(reason.contains("sigma 0.3162"), "{reason}");
    }

    #[test]
    fn long_position_lowers_both_quotes() {
        let (flat_bid, flat_ask) = bid_ask(&quotes_after_warm_up(&mut strategy(0.1), 0.0));
        let (long_bid, long_ask) = bid_ask(&quotes_after_warm_up(&mut strategy(0.1), 5.0));
        assert!(long_bid < flat_bid, "{long_bid} vs {flat_bid}");
        assert!(long_ask < flat_ask, "{long_ask} vs {flat_ask}");
        // r moves by q * gamma * sigma^2 * horizon = 5 * 0.1 * 0.1 * 10 = 0.50 price units.
        assert_eq!((flat_bid - long_bid, flat_ask - long_ask), (50, 50));

        let (short_bid, short_ask) = bid_ask(&quotes_after_warm_up(&mut strategy(0.1), -5.0));
        assert!(short_bid > flat_bid && short_ask > flat_ask);
    }

    #[test]
    fn larger_gamma_widens_the_spread() {
        // The inventory-risk term grows with gamma while the order-arrival term shrinks; the
        // spread widens when sigma^2 * horizon exceeds roughly 1 / kappa^2 (here 1.0 vs 0.44).
        let (low_bid, low_ask) = bid_ask(&quotes_after_warm_up(&mut strategy(0.1), 0.0));
        let (high_bid, high_ask) = bid_ask(&quotes_after_warm_up(&mut strategy(1.0), 0.0));
        assert!(high_ask - high_bid > low_ask - low_bid);
        // delta(gamma = 1) = 0.5 + ln(1 + 1 / 1.5) = 1.0108 price units -> 102 ticks each side.
        assert_eq!((high_bid, high_ask), (9_948, 10_152));
    }

    #[test]
    fn volatility_is_scaled_by_the_tick_interval() {
        // The same mid path on a 1 s grid is ten times less variance per second than on a
        // 0.1 s grid, so the inventory-risk part of the spread shrinks.
        let mut fast = AvellanedaStoikov::new(params(1.0), INTERVAL_NS, &INSTRUMENT);
        let mut slow = AvellanedaStoikov::new(params(1.0), 10 * INTERVAL_NS, &INSTRUMENT);
        let (fast_bid, fast_ask) = bid_ask(&quotes_after_warm_up(&mut fast, 0.0));
        let (slow_bid, slow_ask) = bid_ask(&quotes_after_warm_up(&mut slow, 0.0));
        assert!(slow_ask - slow_bid < fast_ask - fast_bid);
    }

    #[test]
    fn constant_mid_quotes_the_order_arrival_spread_only() {
        let mut s = strategy(0.1);
        let flat = market(Some(10_000), Some(10_100));
        let mut out = Vec::new();
        for tick_no in 0..=u64::from(HALFLIFE) {
            out = run(&mut s, &view(tick_no, flat, 3.0, &[]));
        }
        // sigma = 0: r = mid regardless of inventory and delta = ln(1 + 0.1 / 1.5) / 0.1 =
        // 0.6454 price units -> 65 ticks each side.
        assert_eq!(bid_ask(&out), (9_985, 10_115));
    }

    #[test]
    fn quotes_never_cross_the_local_book() {
        let mut s = strategy(0.1);
        for tick_no in 0..u64::from(HALFLIFE) {
            run(&mut s, &view(tick_no, book(tick_no), 0.0, &[]));
        }
        // Short 9 units: r = 100.50 + 0.90, so the raw bid (10_070) lies above the best ask of
        // this 2-tick-wide book and is held one tick below it.
        let tight = market(Some(10_049), Some(10_051));
        let out = run(&mut s, &view(u64::from(HALFLIFE), tight, -9.0, &[]));
        let (bid, ask) = bid_ask(&out);
        assert_eq!(bid, 10_050);
        assert!(ask > 10_051);
        assert!(reasons(&out)[0].contains("clamped"));
    }

    #[test]
    fn replaces_stale_quotes_and_respects_max_position() {
        let mut s = strategy(0.1);
        for tick_no in 0..u64::from(HALFLIFE) {
            run(&mut s, &view(tick_no, book(tick_no), 0.0, &[]));
        }
        let tick_no = u64::from(HALFLIFE);
        // Flat quotes are 9_980 / 10_120: the bid is stale, the ask is already right.
        let open = [
            resting(order_id(3, 0), Side::Buy, 9_990),
            resting(order_id(3, 1), Side::Sell, 10_120),
        ];
        let out = run(&mut s, &view(tick_no, book(tick_no), 0.0, &open));
        assert_eq!(cancelled(&out), vec![order_id(3, 0)]);
        assert_eq!(placed(&out).len(), 1);
        assert_eq!(placed(&out)[0].2, 9_980);

        // At max long the buy side is disabled: its order is cancelled and not replaced.
        let mut maxed = strategy(0.1);
        for tick_no in 0..u64::from(HALFLIFE) {
            run(&mut maxed, &view(tick_no, book(tick_no), 0.0, &[]));
        }
        let out = run(&mut maxed, &view(tick_no, book(tick_no), 10.0, &open[..1]));
        assert_eq!(cancelled(&out), vec![order_id(3, 0)]);
        assert_eq!(placed(&out).len(), 1);
        assert_eq!(placed(&out)[0].1, Side::Sell);
    }

    #[test]
    fn one_sided_book_after_warm_up_cancels_quotes() {
        let mut s = strategy(0.1);
        for tick_no in 0..u64::from(HALFLIFE) {
            run(&mut s, &view(tick_no, book(tick_no), 0.0, &[]));
        }
        let open = [resting(order_id(3, 0), Side::Buy, 9_980)];
        let out = run(&mut s, &view(4, market(None, Some(10_100)), 0.0, &open));
        assert_eq!(cancelled(&out), vec![order_id(3, 0)]);
        assert!(placed(&out).is_empty());
        assert!(reasons(&out)[0].contains("one-sided"));
    }

    #[test]
    fn prices_on_the_grid_are_not_rounded_a_tick_away() {
        // 8.2 / 0.01 is 819.9999999999999 and 0.07 / 0.01 is 7.000000000000001 in binary
        // floating point.
        assert_eq!(floor_tick(8.2 / 0.01), 820);
        assert_eq!(ceil_tick(8.2 / 0.01), 820);
        assert_eq!(floor_tick(0.07 / 0.01), 7);
        assert_eq!(ceil_tick(0.07 / 0.01), 7);
        assert_eq!(floor_tick(10_006.4), 10_006);
        assert_eq!(ceil_tick(10_006.4), 10_007);
        assert_eq!(floor_tick(-0.5), -1);
        assert_eq!(ceil_tick(-0.5), 0);
    }
}
