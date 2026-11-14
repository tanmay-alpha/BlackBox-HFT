//! Seeded stochastic order-latency models: `log_normal` and `heavy_tail` (ABIDES cubic).
//!
//! Both implement the engine's public `LatencyModel` trait. Like [`crate::taps::JitterLatency`],
//! every draw is a pure function of the run seed and of what is being delayed, never a position in
//! a sequential stream, because hftbacktest 0.9.4 emits simultaneous responses in `HashMap`
//! iteration order (`docs/EXECUTION_MODEL.md`, defect 1):
//!
//! * a new-order entry draw is keyed on `(order id, request time)` in the `latency.entry`
//!   sub-stream, a cancel entry draw on the same pair in `latency.cancel`;
//! * a response draw is keyed on the exchange timestamp only, in `latency.response`, so every
//!   response emitted at one instant shares one latency and arrival order cannot depend on
//!   emission order.
//!
//! The engine distinguishes the two entry legs for us: `LocalToExch::request` passes the order
//! with `req == Status::New` for a new order and `req == Status::Canceled` for a cancel
//! (`Local::submit_order` / `Local::cancel`). On the response path it passes the order as the
//! exchange left it, which does not say reliably which request it answers (a fill response
//! overwrites `req`), so there is one response leg.
//!
//! # Platform independence
//!
//! The samplers use only IEEE-754 `+ - * /`, `sqrt` (correctly rounded by IEEE-754), `round` and
//! integer bit manipulation. `exp` and `ln` are implemented here ([`exp`], [`ln`]) instead of
//! taken from the platform math library, whose last bit differs between C runtimes
//! (`docs/EXPERIMENT_REPRODUCIBILITY.md`, "Platform policy"). A latency model therefore never
//! makes a run's result platform-dependent.

use bb_core::{
    config::{CubicLeg, LogNormalLeg, MAX_LATENCY_NS},
    rng::{Rng, derive_seed},
};
use hftbacktest::{
    backtest::models::LatencyModel,
    types::{Order, Status},
};

/// A generator keyed on `(stream seed, a, b)`: the same key always yields the same numbers.
pub(crate) fn keyed_rng(stream_seed: u64, a: u64, b: u64) -> Rng {
    let key = stream_seed
        ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F).rotate_left(31);
    Rng::new(key)
}

// ---------------------------------------------------------------------------------------------
// Portable elementary functions
// ---------------------------------------------------------------------------------------------

/// `ln 2` split into a high part with trailing zero bits and a low correction (fdlibm's
/// `ln2_hi` / `ln2_lo`), so that `k * LN2_HI` is exact for every exponent `k` used here.
const LN2_HI: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
const LN2_LO: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);
const INV_LN2: f64 = std::f64::consts::LOG2_E;

/// `2^k` for `k` in the normal exponent range.
fn pow2(k: i64) -> f64 {
    debug_assert!((-1022..=1023).contains(&k));
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// `e^x` from `+ - * /` only. Range reduction `x = k ln 2 + r`, `|r| <= ln(2) / 2`, then the
/// Taylor series of `e^r` to degree 20 (the remainder is below `0.35^21 / 21! < 1e-28`), then
/// scaling by `2^k`. Results below the normal range are returned as 0; nothing here needs them.
pub(crate) fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -708.0 {
        return 0.0;
    }
    let k = (x * INV_LN2).round();
    let r = (x - k * LN2_HI) - k * LN2_LO;
    let mut sum = 1.0;
    for n in (1..=20).rev() {
        sum = 1.0 + r * sum / f64::from(n);
    }
    sum * pow2(k as i64)
}

/// Natural logarithm from `+ - * /` and bit manipulation, for finite `x > 0`. Writes
/// `x = m * 2^e` with `m` in `[sqrt(1/2), sqrt(2))`, then `ln m = 2 atanh(s)` with
/// `s = (m - 1) / (m + 1)`, `|s| <= 0.1716`, summed to `s^27` (the next term is below `1e-21`).
/// Returns NaN for `x <= 0` or NaN and infinity for infinity.
pub(crate) fn ln(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return f64::NAN;
    }
    if x.is_infinite() {
        return x;
    }
    if x < f64::MIN_POSITIVE {
        // Subnormal: scale into the normal range first (2^54 is exact).
        return ln(x * pow2(54)) - 54.0 * LN2_HI - 54.0 * LN2_LO;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut series = 0.0;
    for n in (0..=13).rev() {
        series = 1.0 / f64::from(2 * n + 1) + s2 * series;
    }
    let ln_m = 2.0 * s * series;
    let e = e as f64;
    e * LN2_HI + (e * LN2_LO + ln_m)
}

/// A standard normal draw by Marsaglia's polar method (Marsaglia & Bray 1964): exact in
/// distribution given exact `ln` and `sqrt`; consumes two uniforms per attempt and accepts with
/// probability `pi / 4`.
pub(crate) fn std_normal(rng: &mut Rng) -> f64 {
    loop {
        let u = 2.0 * rng.next_f64() - 1.0;
        let v = 2.0 * rng.next_f64() - 1.0;
        let s = u * u + v * v;
        if s > 0.0 && s < 1.0 {
            return u * (-2.0 * ln(s) / s).sqrt();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Legs
// ---------------------------------------------------------------------------------------------

/// The distribution of one leg (entry, cancel entry or response).
#[derive(Copy, Clone, Debug)]
pub(crate) enum Leg {
    LogNormal(LogNormalLeg),
    Cubic(CubicLeg),
}

impl Leg {
    /// One latency in ns, rounded to the nearest nanosecond and capped at
    /// [`MAX_LATENCY_NS`] (only a log-normal draw beyond 60 s can reach the cap).
    pub(crate) fn sample(&self, rng: &mut Rng) -> i64 {
        let v = match *self {
            Leg::LogNormal(l) => {
                if l.sigma == 0.0 {
                    return l.median_ns;
                }
                l.median_ns as f64 * exp(l.sigma * std_normal(rng))
            }
            Leg::Cubic(l) => {
                // ABIDES: `x = uniform(clip, 1)`, `latency = min + (a / x^3) * (min / unit)`.
                // `next_f64` is in [0, 1), so `x` is in (clip, 1].
                let x = 1.0 - rng.next_f64() * (1.0 - l.clip);
                let min = l.min_ns as f64;
                min + (l.jitter / (x * x * x)) * (min / l.unit)
            }
        };
        (v.round() as i64).clamp(0, MAX_LATENCY_NS)
    }
}

/// The `log_normal` and `heavy_tail` order-latency models.
#[derive(Clone, Debug)]
pub(crate) struct StochasticLatency {
    entry: Leg,
    cancel: Leg,
    response: Leg,
    entry_seed: u64,
    cancel_seed: u64,
    response_seed: u64,
}

impl StochasticLatency {
    /// `cancel: None` draws cancels from the entry distribution, still on the cancel sub-stream.
    pub(crate) fn new(entry: Leg, cancel: Option<Leg>, response: Leg, seed: u64) -> Self {
        Self {
            entry,
            cancel: cancel.unwrap_or(entry),
            response,
            entry_seed: derive_seed(seed, "latency.entry"),
            cancel_seed: derive_seed(seed, "latency.cancel"),
            response_seed: derive_seed(seed, "latency.response"),
        }
    }
}

impl LatencyModel for StochasticLatency {
    fn entry(&mut self, timestamp: i64, order: &Order) -> i64 {
        let (leg, seed) = if order.req == Status::Canceled {
            (&self.cancel, self.cancel_seed)
        } else {
            (&self.entry, self.entry_seed)
        };
        leg.sample(&mut keyed_rng(seed, order.order_id, timestamp as u64))
    }

    fn response(&mut self, timestamp: i64, _order: &Order) -> i64 {
        self.response
            .sample(&mut keyed_rng(self.response_seed, timestamp as u64, 0))
    }
}

#[cfg(test)]
mod tests {
    use hftbacktest::types::{OrdType, Side, TimeInForce};

    use super::*;

    fn order(id: u64, req: Status) -> Order {
        let mut o = Order::new(
            id,
            100,
            0.5,
            1.0,
            Side::Buy,
            OrdType::Limit,
            TimeInForce::GTC,
        );
        o.req = req;
        o
    }

    /// Nearest-rank quantile of a sorted sample.
    fn quantile(sorted: &[i64], p: f64) -> f64 {
        let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[rank - 1] as f64
    }

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs()
    }

    #[test]
    fn portable_exp_and_ln_agree_with_the_platform_to_the_last_bits() {
        // The platform functions are only the yardstick here; within a few ulps is the claim.
        let mut x = -40.0;
        while x <= 40.0 {
            let (ours, std) = (exp(x), x.exp());
            assert!(rel(ours, std) < 4e-15, "exp({x}): {ours} vs {std}");
            x += 0.013_7;
        }
        for x in [
            1e-300, 1e-10, 0.1, 0.5, 0.7, 1.0, 1.5, 2.0, 10.0, 12345.678, 1e10, 1e300,
        ] {
            let (ours, std) = (ln(x), x.ln());
            assert!(
                (ours - std).abs() <= 4e-16 * std.abs().max(1.0),
                "ln({x}): {ours} vs {std}"
            );
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert!(ln(0.0).is_nan() && ln(-1.0).is_nan());
        // Subnormal input.
        let tiny = f64::from_bits(1);
        assert!(rel(ln(tiny), -744.440_071_921_381_3) < 1e-15);
    }

    #[test]
    fn standard_normal_moments_and_quantiles() {
        let mut rng = Rng::new(20_261_114);
        let n = 200_000;
        let mut xs: Vec<f64> = (0..n).map(|_| std_normal(&mut rng)).collect();
        let mean = xs.iter().sum::<f64>() / n as f64;
        let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64;
        // Standard errors: mean 1/sqrt(n) = 0.0022, variance sqrt(2/n) = 0.0032.
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.015, "variance {var}");
        xs.sort_by(f64::total_cmp);
        // Phi^-1(0.975) = 1.959964.
        let q = xs[(0.975 * n as f64) as usize];
        assert!((q - 1.959_964).abs() < 0.03, "q97.5 {q}");
    }

    #[test]
    fn log_normal_leg_matches_its_analytic_quantiles_and_mean() {
        // median 1 ms, sigma 0.5: mean = median * e^(sigma^2 / 2) = 1.133148 ms,
        // q90 = median * e^(sigma * 1.2815516) = 1.897953 ms, q10 = 0.526884 ms.
        let leg = Leg::LogNormal(LogNormalLeg {
            median_ns: 1_000_000,
            sigma: 0.5,
        });
        let seed = derive_seed(7, "latency.entry");
        let mut xs: Vec<i64> = (0..100_000u64)
            .map(|i| leg.sample(&mut keyed_rng(seed, i, 42)))
            .collect();
        let mean = xs.iter().sum::<i64>() as f64 / xs.len() as f64;
        xs.sort_unstable();
        assert!(rel(mean, 1_133_148.5) < 0.01, "mean {mean}");
        assert!(rel(quantile(&xs, 0.5), 1_000_000.0) < 0.01);
        assert!(rel(quantile(&xs, 0.9), 1_897_953.0) < 0.015);
        assert!(rel(quantile(&xs, 0.1), 526_884.0) < 0.015);
        assert!(xs[0] > 0);
        // sigma = 0 is exactly the median.
        let flat = Leg::LogNormal(LogNormalLeg {
            median_ns: 1_234,
            sigma: 0.0,
        });
        assert_eq!(flat.sample(&mut Rng::new(1)), 1_234);
    }

    #[test]
    fn cubic_leg_matches_the_abides_formula_and_its_analytic_moments() {
        // ABIDES defaults (jitter 0.5, clip 0.1, unit 10) on a 1 ms minimum. With x uniform on
        // (0.1, 1]: extra = 0.05 ms / x^3, so
        //   bounds   [1.05 ms, 51 ms]
        //   mean     1 ms + 0.05 ms * E[x^-3] = 1 ms + 0.05 ms * (0.1^-2 - 1) / (2 * 0.9)
        //            = 3.75 ms
        //   quantile L_p = 1 ms + 0.05 ms / (0.1 + (1 - p) * 0.9)^3:
        //            median 1.300526 ms, q90 = 1 ms + 0.05 ms / 0.19^3 = 8.289692 ms.
        let leg = Leg::Cubic(CubicLeg {
            min_ns: 1_000_000,
            jitter: 0.5,
            clip: 0.1,
            unit: 10.0,
        });
        let seed = derive_seed(7, "latency.response");
        let mut xs: Vec<i64> = (0..200_000u64)
            .map(|t| leg.sample(&mut keyed_rng(seed, t, 0)))
            .collect();
        xs.sort_unstable();
        assert!(xs[0] >= 1_050_000 && *xs.last().unwrap() <= 51_000_000);
        let mean = xs.iter().sum::<i64>() as f64 / xs.len() as f64;
        // The mean of a heavy tail converges slowly: E[x^-6] makes its standard error about
        // 0.02 ms here.
        assert!(rel(mean, 3_750_000.0) < 0.03, "mean {mean}");
        assert!(rel(quantile(&xs, 0.5), 1_300_526.0) < 0.01);
        assert!(rel(quantile(&xs, 0.9), 8_289_692.0) < 0.03);
        // One hand-checked draw: the same arithmetic as ABIDES on the same uniform.
        let mut a = keyed_rng(seed, 5, 0);
        let mut b = keyed_rng(seed, 5, 0);
        let x = 1.0 - a.next_f64() * 0.9;
        let expected = (1e6 + (0.5 / (x * x * x)) * (1e6 / 10.0)).round() as i64;
        assert_eq!(leg.sample(&mut b), expected);
    }

    #[test]
    fn draws_are_pure_functions_and_cancels_have_their_own_leg_and_stream() {
        let entry = Leg::LogNormal(LogNormalLeg {
            median_ns: 1_000_000,
            sigma: 0.8,
        });
        let cancel = Leg::LogNormal(LogNormalLeg {
            median_ns: 200_000,
            sigma: 0.0,
        });
        let mut a = StochasticLatency::new(entry, Some(cancel), entry, 9);
        let mut b = StochasticLatency::new(entry, Some(cancel), entry, 9);
        let new1 = order(1, Status::New);
        let new2 = order(2, Status::New);
        // Order of asking does not matter.
        let x = (a.entry(5_000, &new1), a.entry(5_000, &new2));
        let y2 = b.entry(5_000, &new2);
        let y1 = b.entry(5_000, &new1);
        assert_eq!(x, (y1, y2));
        // Cancels use the cancel leg.
        assert_eq!(a.entry(5_000, &order(1, Status::Canceled)), 200_000);
        // Responses at one exchange instant share a latency across orders.
        for t in 0..500 {
            assert_eq!(a.response(t, &new1), a.response(t, &new2));
        }
        // `cancel: None` reuses the entry parameters on an independent stream.
        let mut c = StochasticLatency::new(entry, None, entry, 9);
        let differs = (0..64)
            .any(|t| c.entry(t, &order(3, Status::Canceled)) != c.entry(t, &order(3, Status::New)));
        assert!(differs);
        // A different seed gives different draws.
        let mut d = StochasticLatency::new(entry, None, entry, 10);
        let differs = (0..64).any(|t| d.entry(t, &new1) != c.entry(t, &new1));
        assert!(differs);
    }
}
