//! Guéant-Lehalle-Fernandez-Tapia market maker with Guéant's trade-size scaling.
//!
//! References:
//! * O. Guéant, C.-A. Lehalle and J. Fernandez-Tapia (2013), "Dealing with the inventory risk: a
//!   solution to the market making problem", Mathematics and Financial Economics 7(4), 477-507
//!   (arXiv:1105.3115): closed-form asymptotic approximations of the optimal quotes far from the
//!   terminal time.
//! * O. Guéant (2017), "Optimal market making", Applied Mathematical Finance 24(2), 112-154
//!   (arXiv:1605.01862), eqs. (4.6)-(4.7): the same approximations for transactions of size
//!   `Delta` and exponential intensities `Lambda(delta) = A exp(-k delta)`.
//!
//! An original implementation of the published formulas; no code was copied.
//!
//! With reference price `S`, inventory `q`, trade size `Delta`, risk aversion `gamma` (Model A,
//! `xi = gamma`), price volatility `sigma` (per square-root second), and intensity parameters
//! `A` (per second) and `k` (per price unit), Guéant (2017) eqs. (4.6)-(4.7) give
//!
//! ```text
//! c1    = 1 / (gamma Delta) * ln(1 + gamma Delta / k)
//! omega = sqrt( gamma sigma^2 / (2 A Delta k) * (1 + gamma Delta / k)^(k / (gamma Delta) + 1) )
//! delta_b(q) = c1 + (2q + Delta) / 2 * omega        bid = S - delta_b(q)
//! delta_a(q) = c1 - (2q - Delta) / 2 * omega        ask = S + delta_a(q)
//! ```
//!
//! With `Delta = 1` this is GLFT (2013) section 4 exactly. The spread `delta_b + delta_a =
//! 2 c1 + Delta omega` does not depend on `q`; the skew is linear in `q` (Guéant 2017, eqs.
//! (4.8)-(4.9)).
//!
//! # Continuous-time model versus this implementation
//!
//! The formulas are the `T -> infinity` limit of a continuous-time control problem: `S` is an
//! arithmetic Brownian motion, quotes are revised continuously at no cost, fills arrive as
//! Poisson events with intensity `A exp(-k delta)` independent of the price path, each fill is of
//! size exactly `Delta`, and the inventory stays in `[-Q, Q]`. Here:
//!
//! * quotes are revised only on the strategy's fixed tick grid, and each revision is a cancel and
//!   a new order subject to the configured order latency (and think time);
//! * fills come from replayed market data and the queue model, not from the assumed intensity, so
//!   `A` and `k` are parameters of the quote formula, not a description of the simulated fills;
//! * prices are rounded outward to the tick grid and kept off the opposite side of the local
//!   book, which widens each quote by less than one tick;
//! * `S` is the local mid and `sigma` an online estimate (below), both as seen through the feed;
//! * a partial fill can leave the inventory off the `Delta` grid the model assumes.

use bb_core::Side;
use bb_core::config::{GlftParams, SigmaSource};
use bb_core::dataset::InstrumentMeta;
use bb_core::strategy::{Intent, LocalView, Strategy};
use bb_core::time::NS_PER_S;

use crate::quoting::{
    QuoteRules, Quotes, cancel_all_one_sided, ceil_tick, floor_tick, sync_quotes,
};
use crate::text::Text;

const NAME: &str = "glft";

/// The inventory-independent terms of the approximation.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct GlftTerms {
    /// `c1 = ln(1 + gamma Delta / k) / (gamma Delta)`, price units.
    pub c1: f64,
    /// `omega`, price units per unit of inventory.
    pub omega: f64,
}

impl GlftTerms {
    /// Guéant (2017) eqs. (4.6)-(4.7) with `xi = gamma`. `sigma2` is the price variance per
    /// second. The power is evaluated as `exp((k / (gamma Delta) + 1) ln(1 + gamma Delta / k))`
    /// with `ln_1p`, which stays accurate when `gamma Delta / k` is small.
    pub(crate) fn new(gamma: f64, kappa: f64, intensity_a: f64, delta: f64, sigma2: f64) -> Self {
        let x = gamma * delta / kappa;
        let ln_1p_x = x.ln_1p();
        let c1 = ln_1p_x / (gamma * delta);
        let power = ((kappa / (gamma * delta) + 1.0) * ln_1p_x).exp();
        let omega = (gamma * sigma2 / (2.0 * intensity_a * delta * kappa) * power).sqrt();
        Self { c1, omega }
    }

    /// `(delta_b, delta_a)` for inventory `q` and trade size `delta`.
    pub(crate) fn depths(&self, q: f64, delta: f64) -> (f64, f64) {
        (
            self.c1 + (2.0 * q + delta) / 2.0 * self.omega,
            self.c1 - (2.0 * q - delta) / 2.0 * self.omega,
        )
    }
}

fn source_name(source: SigmaSource) -> &'static str {
    match source {
        SigmaSource::Mid => "mid",
        SigmaSource::WeightedMid => "size-weighted mid",
        SigmaSource::Trade => "last trade price",
    }
}

/// Slot 0 is the bid, slot 1 the ask; both `Gtx` limit orders at `floor((S - delta_b) / tick)`
/// and `ceil((S + delta_a) / tick)`, kept off the opposite side of the locally known book.
/// Order maintenance and the inventory bound (`max_position`, GLFT's `Q`) are those of
/// [`crate::TouchMaker`].
///
/// # Volatility estimate
///
/// Each tick the chosen series ([`SigmaSource`]) gives one observation when it exists: the mid
/// and the size-weighted mid need a two-sided local book (and, for the weighted mid, displayed
/// size); the last trade price exists once any trade has been received and is carried forward.
/// The squared change since the previous observation, divided by the number of ticks between
/// them, feeds an exponentially weighted variance with `alpha = 1 - 0.5^(1 / vol_halflife_ticks)`
/// and zero mean, seeded by the first change; `sigma^2 = ewma_var / interval_seconds`. This is
/// the estimator of [`crate::AvellanedaStoikov`] on a selectable series.
///
/// The model's `sigma` is the volatility of an efficient price. A wide, flickering book makes
/// the mid jump by half the spread when one side moves, which inflates a mid-based estimate
/// (`docs/research/MARKET_MICROSTRUCTURE.md` section 4.2, caveat 3). The size-weighted mid
/// passes on only `bid_qty / (bid_qty + ask_qty)` of an ask-price change (and the complement of
/// a bid-price change), so a price change on the side with the deeper queue moves it little;
/// trade prices avoid quote flicker but bounce between bid and ask (Roll 1984). Neither is the
/// efficient price.
///
/// Nothing is quoted (and nothing cancelled) until `vol_halflife_ticks` observations exist.
/// After warm-up a one-sided book cancels all quotes: there is no mid to quote around.
#[derive(Clone, Debug)]
pub struct Glft {
    params: GlftParams,
    text: Text,
    interval_s: f64,
    alpha: f64,
    /// Tick number and value (price units) of the latest observation.
    last_obs: Option<(u64, f64)>,
    /// Variance of the series' change over one tick, price units squared.
    ewma_var: f64,
    observations: u64,
    /// Last trade price received locally (price units), for [`SigmaSource::Trade`].
    last_trade: Option<f64>,
}

impl Glft {
    /// `interval_ns` is the strategy grid step, [`bb_core::config::StrategyConfig::interval_ns`].
    pub fn new(params: GlftParams, interval_ns: i64, instrument: &InstrumentMeta) -> Self {
        Self {
            params,
            text: Text::new(instrument),
            interval_s: interval_ns as f64 / NS_PER_S as f64,
            alpha: 1.0 - 0.5f64.powf(1.0 / f64::from(params.vol_halflife_ticks)),
            last_obs: None,
            ewma_var: 0.0,
            observations: 0,
            last_trade: None,
        }
    }

    fn warmed_up(&self) -> bool {
        self.observations >= u64::from(self.params.vol_halflife_ticks)
    }

    /// Price variance per second.
    fn sigma2(&self) -> f64 {
        self.ewma_var / self.interval_s
    }

    fn observe(&mut self, tick_no: u64, value: f64) {
        if let Some((prev_tick, prev)) = self.last_obs {
            let elapsed_ticks = tick_no.saturating_sub(prev_tick).max(1) as f64;
            let change = value - prev;
            let sample = change * change / elapsed_ticks;
            self.ewma_var = if self.observations == 1 {
                sample
            } else {
                self.ewma_var + self.alpha * (sample - self.ewma_var)
            };
        }
        self.last_obs = Some((tick_no, value));
        self.observations += 1;
    }

    /// This tick's value of the volatility series, in price units.
    fn series_value(&self, view: &LocalView<'_>) -> Option<f64> {
        let tick_size = self.text.tick_size();
        let m = &view.market;
        match self.params.sigma_source {
            SigmaSource::Mid => m.mid_tick().map(|mid| mid * tick_size),
            SigmaSource::WeightedMid => {
                let (bid, ask) = (m.best_bid_tick?, m.best_ask_tick?);
                let total = m.best_bid_qty + m.best_ask_qty;
                (total > 0.0).then(|| {
                    (bid as f64 * m.best_ask_qty + ask as f64 * m.best_bid_qty) / total * tick_size
                })
            }
            SigmaSource::Trade => self.last_trade,
        }
    }
}

impl Strategy for Glft {
    fn on_tick(&mut self, view: &LocalView<'_>, out: &mut Vec<Intent>) {
        if let Some(trade) = view.recent_trades.last() {
            self.last_trade = Some(trade.price_tick as f64 * self.text.tick_size());
        }
        if let Some(value) = self.series_value(view) {
            self.observe(view.tick_no, value);
        }
        let (Some(best_bid), Some(best_ask)) =
            (view.market.best_bid_tick, view.market.best_ask_tick)
        else {
            if self.warmed_up() {
                cancel_all_one_sided(NAME, &self.text, view, out);
            }
            return;
        };
        if !self.warmed_up() {
            return;
        }

        let p = &self.params;
        let text = &self.text;
        let tick_size = text.tick_size();
        let mid = (best_bid as f64 + best_ask as f64) * 0.5 * tick_size;
        let sigma2 = self.sigma2();
        let terms = GlftTerms::new(p.gamma, p.kappa, p.intensity_a, p.order_qty, sigma2);
        let q = view.position;
        let (delta_b, delta_a) = terms.depths(q, p.order_qty);
        if !(delta_b.is_finite() && delta_a.is_finite()) {
            // Only reachable with parameters `RunConfig::validate` rejects.
            return;
        }

        let quotes = Quotes::clamped(
            floor_tick((mid - delta_b) / tick_size),
            ceil_tick((mid + delta_a) / tick_size),
            best_bid,
            best_ask,
        );
        let rules = QuoteRules {
            name: NAME,
            text,
            order_qty: p.order_qty,
            max_position: p.max_position,
        };
        let source = source_name(p.sigma_source);
        let rationale = |side: Side| {
            let (rounding, depth, factor) = match side {
                Side::Buy => (
                    "floor((S - delta_b) / tick size)",
                    delta_b,
                    "delta_b = c1 + (2q + Delta) / 2 x omega",
                ),
                Side::Sell => (
                    "ceil((S + delta_a) / tick size)",
                    delta_a,
                    "delta_a = c1 - (2q - Delta) / 2 x omega",
                ),
            };
            format!(
                "{rounding} with reference mid S {}, {factor} = {}, c1 {}, omega {}, q {}, \
                 Delta {}, sigma {} per sqrt(s) from the {source}, gamma {}, k {}, A {} per s, \
                 tick size {}",
                text.price(mid),
                text.price(depth),
                text.price(terms.c1),
                text.price(terms.omega),
                text.qty(q),
                text.qty(p.order_qty),
                text.price(sigma2.sqrt()),
                p.gamma,
                p.kappa,
                p.intensity_a,
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
        INSTRUMENT, INTERVAL_NS, cancelled, market, market_with_qty, placed, reasons, run, view,
    };
    use bb_core::strategy::{MarketView, TradePrint};

    const HALFLIFE: u32 = 4;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }

    /// The worked example of `docs/research/MARKET_MICROSTRUCTURE.md` section 4.2: GLFT (2013)
    /// with `gamma = 0.1`, `k = 1.5`, `A = 140`, `sigma^2 = 4`, `q = 1`, `Delta = 1`:
    /// `c1 = ln(1 + 0.1 / 1.5) / 0.1 = 0.645385`,
    /// `omega = sqrt(0.4 / 420 * (1 + 0.1 / 1.5)^16) = 0.051717`,
    /// `delta_b = c1 + 1.5 omega = 0.722961`, `delta_a = c1 - 0.5 omega = 0.619527`.
    #[test]
    fn reproduces_the_glft_closed_form() {
        let t = GlftTerms::new(0.1, 1.5, 140.0, 1.0, 4.0);
        close(t.c1, 0.645_385);
        close(t.omega, 0.051_717);
        let (b, a) = t.depths(1.0, 1.0);
        close(b, 0.722_961);
        close(a, 0.619_527);
        // Flat: symmetric, half spread c1 + omega / 2.
        let (b0, a0) = t.depths(0.0, 1.0);
        close(b0, a0);
        close(b0, 0.645_385 + 0.051_717 / 2.0);
    }

    /// Guéant (2017) trade-size scaling on the same inputs with `Delta = 0.1`:
    /// `c1 = ln(1 + 0.01 / 1.5) / 0.01 = 0.664454` (section 4.2's "liquidity term"),
    /// `omega = sqrt(0.4 / 42 * (1 + 0.01 / 1.5)^151) = 0.161167`,
    /// `delta_b(1) = c1 + 2.1 / 2 omega = 0.833679`, `delta_a(1) = c1 - 1.9 / 2 omega = 0.511346`.
    #[test]
    fn reproduces_the_gueant_trade_size_scaling() {
        let t = GlftTerms::new(0.1, 1.5, 140.0, 0.1, 4.0);
        close(t.c1, 0.664_454);
        close(t.omega, 0.161_167);
        let (b, a) = t.depths(1.0, 0.1);
        close(b, 0.833_679);
        close(a, 0.511_346);
        // The spread does not depend on the inventory (Guéant 2017, eq. 4.8): 2 c1 + Delta omega.
        for q in [-3.0, 0.0, 0.7, 5.0] {
            let (b, a) = t.depths(q, 0.1);
            close(b + a, 2.0 * 0.664_454 + 0.1 * 0.161_167);
        }
    }

    fn params(source: SigmaSource) -> GlftParams {
        GlftParams {
            order_qty: 1.0,
            max_position: 10.0,
            gamma: 0.1,
            kappa: 1.5,
            intensity_a: 1.0,
            sigma_source: source,
            vol_halflife_ticks: HALFLIFE,
        }
    }

    fn strategy(source: SigmaSource) -> Glft {
        Glft::new(params(source), INTERVAL_NS, &INSTRUMENT)
    }

    /// Mid alternating 100.50 / 100.60 on a 0.1 s grid: per-tick variance 0.01, so
    /// `sigma^2 = 0.1` per second (as in the Avellaneda-Stoikov tests).
    fn book(tick_no: u64) -> MarketView {
        let shift = if tick_no.is_multiple_of(2) { 0 } else { 10 };
        market(Some(10_000 + shift), Some(10_100 + shift))
    }

    fn quotes_after_warm_up(s: &mut Glft, position: f64) -> Vec<Intent> {
        for tick_no in 0..u64::from(HALFLIFE) {
            run(s, &view(tick_no, book(tick_no), 0.0, &[]));
        }
        let tick_no = u64::from(HALFLIFE);
        run(s, &view(tick_no, book(tick_no), position, &[]))
    }

    fn bid_ask(intents: &[Intent]) -> (i64, i64) {
        let quotes = placed(intents);
        assert_eq!(quotes.len(), 2, "expected one bid and one ask: {intents:?}");
        (quotes[0].2, quotes[1].2)
    }

    /// `S = 100.50`, `sigma^2 = 0.1`, `gamma 0.1`, `k 1.5`, `A 1`, `Delta 1`:
    /// `c1 = 0.645385`, `omega = sqrt(0.01 / 3 * (16/15)^16) = 0.096754`.
    /// * flat: `delta = c1 + omega / 2 = 0.693762`: bid `floor(9980.62) = 9980`, ask
    ///   `ceil(10119.38) = 10120`;
    /// * long 2: `delta_b = c1 + 2.5 omega = 0.887270`, `delta_a = c1 - 1.5 omega = 0.500254`:
    ///   `(9961, 10101)`; short 2 mirrors it: `(9999, 10139)`.
    #[test]
    fn quotes_follow_the_formula_on_a_hand_checked_book() {
        let out = quotes_after_warm_up(&mut strategy(SigmaSource::Mid), 0.0);
        assert_eq!(bid_ask(&out), (9_980, 10_120));
        let reason = reasons(&out)[0];
        assert!(
            reason.starts_with("glft: quote bid 99.80 x 1.000 post-only:"),
            "{reason}"
        );
        assert!(reason.contains("S 100.5000"), "{reason}");
        assert!(reason.contains("c1 0.6454"), "{reason}");
        assert!(reason.contains("omega 0.0968"), "{reason}");
        assert!(reason.contains("sigma 0.3162"), "{reason}");
        assert_eq!(
            bid_ask(&quotes_after_warm_up(&mut strategy(SigmaSource::Mid), 2.0)),
            (9_961, 10_101)
        );
        assert_eq!(
            bid_ask(&quotes_after_warm_up(&mut strategy(SigmaSource::Mid), -2.0)),
            (9_999, 10_139)
        );
    }

    #[test]
    fn silent_until_warm_and_respects_the_inventory_bound() {
        let mut s = strategy(SigmaSource::Mid);
        // The fourth observation (tick 3) completes the warm-up; ticks 0-2 are silent.
        for tick_no in 0..u64::from(HALFLIFE) - 1 {
            assert!(run(&mut s, &view(tick_no, book(tick_no), 0.0, &[])).is_empty());
        }
        assert_eq!(placed(&run(&mut s, &view(3, book(3), 0.0, &[]))).len(), 2);
        // At the bound Q = 10 the bid is not quoted.
        let out = run(&mut s, &view(4, book(4), 10.0, &[]));
        let quotes = placed(&out);
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].1, Side::Sell);
        // One-sided book after warm-up cancels.
        let open = [crate::testutil::resting(
            bb_core::strategy::order_id(4, 1),
            Side::Sell,
            10_200,
        )];
        let out = run(&mut s, &view(5, market(Some(10_000), None), 0.0, &open));
        assert_eq!(cancelled(&out), vec![bb_core::strategy::order_id(4, 1)]);
    }

    /// The bid stays at 100.00 and the ask alternates 100.10 / 101.00, so the mid moves by 0.45
    /// per tick: `sigma^2 = 0.45^2 / 0.1 = 2.025`. The size-weighted mid
    /// `W = (bid * ask_qty + ask * bid_qty) / (bid_qty + ask_qty)` weights the ask price by the
    /// bid size, so it moves by `0.9 * bid_qty / (bid_qty + ask_qty)`: with sizes (1, 9) by
    /// 0.09 (`sigma^2 = 0.0081 / 0.1 = 0.081`), with sizes (9, 1) by 0.81
    /// (`sigma^2 = 0.6561 / 0.1 = 6.561`).
    #[test]
    fn sigma_sources_measure_different_series() {
        let ask_moves = |tick_no: u64, bid_qty: f64, ask_qty: f64| {
            let ask = if tick_no.is_multiple_of(2) {
                10_010
            } else {
                10_100
            };
            market_with_qty(Some(10_000), Some(ask), bid_qty, ask_qty)
        };
        let warm = |s: &mut Glft, bid_qty: f64, ask_qty: f64| {
            for tick_no in 0..=u64::from(HALFLIFE) {
                run(
                    s,
                    &view(tick_no, ask_moves(tick_no, bid_qty, ask_qty), 0.0, &[]),
                );
            }
            s.sigma2()
        };
        close(warm(&mut strategy(SigmaSource::Mid), 1.0, 9.0), 2.025);
        close(
            warm(&mut strategy(SigmaSource::WeightedMid), 1.0, 9.0),
            0.081,
        );
        close(
            warm(&mut strategy(SigmaSource::WeightedMid), 9.0, 1.0),
            6.561,
        );

        // Trade prices: carried forward between trades; one 0.20 move over two ticks.
        let mut s = strategy(SigmaSource::Trade);
        let trade = |tick: i64| TradePrint {
            local_ts: 0,
            price_tick: tick,
            qty: 1.0,
            aggressor: None,
        };
        let prints = [
            vec![trade(10_000)],
            vec![],
            vec![trade(10_010), trade(10_020)],
            vec![],
            vec![],
        ];
        for (tick_no, trades) in prints.iter().enumerate() {
            let mut v = view(tick_no as u64, book(0), 0.0, &[]);
            v.recent_trades = trades;
            run(&mut s, &v);
        }
        // Observations 100.00, 100.00, 100.20, 100.20, 100.20: changes 0, 0.2, 0, 0 per tick.
        // EWMA seeded with 0, alpha = 1 - 0.5^(1/4): 0 -> 0.04 alpha -> ... (1 - alpha)^2.
        let alpha = 1.0 - 0.5f64.powf(0.25);
        let expected = (0.04 * alpha) * (1.0 - alpha) * (1.0 - alpha) / 0.1;
        close(s.sigma2(), expected);
    }
}
