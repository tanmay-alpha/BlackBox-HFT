//! Sampled-path statistics, not risk forecasts. No annualisation is performed.
use crate::{Sum, Value};
use bb_core::{
    dataset::InstrumentMeta,
    result::{EquitySample, FillRecord},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiskConfig {
    pub bucket_ns: i64,
    pub hac_lag: usize,
    pub position_limit_base: Option<f64>,
    /// Number of grid/strategy trials from which this run was selected.
    pub trials: u64,
}
impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            bucket_ns: 10_000_000_000,
            hac_lag: 2,
            position_limit_base: None,
            trials: 1,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiskReport {
    pub analytics_version: String,
    pub mark: String,
    pub sample_interval_ns: Value,
    pub bucket_horizon_ns: Value,
    pub trials: u64,
    pub net_pnl: Value,
    pub turnover: Value,
    pub pnl_per_turnover_bps: Value,
    pub max_drawdown: Value,
    pub time_under_water_ns: Value,
    pub mean_absolute_position: Value,
    pub time_at_position_limit_ns: Value,
    pub max_long: Value,
    pub max_short: Value,
    pub bucket_sharpe: Value,
    pub sharpe_hac_se: Value,
    pub caveats: Vec<String>,
}

/// Previous-tick bucket P&L. Invalid marks anywhere between endpoints invalidate that bucket.
/// Iteration stores no history; partial final buckets are not observations.
fn buckets(samples: &[EquitySample], step: i64) -> impl Iterator<Item = Option<f64>> + '_ {
    let start = samples.first().map_or(0, |s| s.ts);
    let end = samples.last().map_or(start, |s| s.ts);
    let count = if step > 0 {
        (end - start).max(0) / step
    } else {
        0
    };
    let mut index = 0;
    let mut previous = samples.first();
    (1..=count).map(move |k| {
        let t = start + k * step;
        let mut valid = previous.is_some_and(|s| s.mid_tick.is_some() && s.equity.is_finite());
        while index + 1 < samples.len() && samples[index + 1].ts <= t {
            index += 1;
            valid &= samples[index].mid_tick.is_some() && samples[index].equity.is_finite();
        }
        let current = samples.get(index);
        let result = match (previous, current) {
            (Some(a), Some(b)) if valid => Some(b.equity - a.equity),
            _ => None,
        };
        previous = current;
        result
    })
}

/// Non-annualised mean / sample standard deviation, with Newey–West delta-method SE.
/// At least 30 complete, non-overlapping buckets and lag <= n/10 are required.
fn sharpe(samples: &[EquitySample], config: &RiskConfig) -> (Value, Value) {
    let unavailable = |reason, excluded| {
        (
            Value::unavailable(reason, "dimensionless", excluded),
            Value::unavailable(reason, "dimensionless", excluded),
        )
    };
    if config.bucket_ns <= 0 {
        return unavailable("bucket horizon must be positive", 0);
    }
    let (mut n, mut excluded, mut mean, mut m2) = (0u64, 0u64, 0.0, 0.0);
    for value in buckets(samples, config.bucket_ns) {
        if let Some(x) = value {
            n += 1;
            let d = x - mean;
            mean += d / n as f64;
            m2 += d * (x - mean);
        } else {
            excluded += 1;
        }
    }
    if n < 30 {
        return unavailable(
            "insufficient sample: require at least 30 non-overlapping buckets",
            excluded,
        );
    }
    if config.hac_lag > 1_000 || config.hac_lag as u64 > n / 10 {
        return unavailable("HAC truncation lag must be <= n/10 and <= 1000", excluded);
    }
    let variance = m2 / (n - 1) as f64;
    if variance <= 0.0 {
        return unavailable("zero bucket P&L variance", excluded);
    }
    let sd = variance.sqrt();
    let ratio = mean / sd;
    let mut covariance = vec![Sum::default(); config.hac_lag + 1];
    let mut ring = vec![None; config.hac_lag + 1];
    for (index, value) in buckets(samples, config.bucket_ns).enumerate() {
        let influence = value.map(|x| {
            let d = x - mean;
            d / sd - mean * (d * d - variance) / (2.0 * sd * variance)
        });
        if let Some(y) = influence {
            covariance[0].add(y * y);
            for lag in 1..=config.hac_lag.min(index) {
                if let Some(past) = ring[(index - lag) % ring.len()] {
                    covariance[lag].add(y * past);
                }
            }
        }
        let slot = index % ring.len();
        ring[slot] = influence;
    }
    let mut long_run = covariance[0].get() / n as f64;
    for (lag, sum) in covariance.iter().enumerate().skip(1) {
        let bartlett = 1.0 - lag as f64 / (config.hac_lag + 1) as f64;
        long_run += 2.0 * bartlett * sum.get() / n as f64;
    }
    (
        Value::measured(ratio, "dimensionless", n, excluded),
        Value::measured(
            (long_run.max(0.0) / n as f64).sqrt(),
            "dimensionless",
            n,
            excluded,
        ),
    )
}

/// `net_pnl_quote` must use the caller's explicitly chosen final mark (normally E9).
/// Path statistics retain the existing equity series' local-mid mark, stated in the output.
pub fn compute_risk(
    samples: &[EquitySample],
    fills: &[FillRecord],
    instrument: &InstrumentMeta,
    net_pnl_quote: f64,
    config: &RiskConfig,
) -> RiskReport {
    let mut turnover = Sum::default();
    for f in fills {
        turnover.add(
            f.price_tick as f64 * instrument.tick_size * f.qty.abs() * instrument.contract_size,
        );
    }
    let turnover = turnover.get();
    let mut peak = 0.0f64;
    let mut drawdown = 0.0f64;
    let mut underwater = 0i64;
    let mut inventory = Sum::default();
    let mut elapsed = 0i64;
    let mut at_limit = 0i64;
    let mut max_long = 0.0f64;
    let mut max_short = 0.0f64;
    let mut valid_n = 0;
    let mut excluded = 0;
    for (i, sample) in samples.iter().enumerate() {
        let dt = samples
            .get(i + 1)
            .map_or(0, |next| (next.ts - sample.ts).max(0));
        max_long = max_long.max(sample.position);
        max_short = max_short.max(-sample.position);
        inventory.add(sample.position.abs() * dt as f64);
        elapsed += dt;
        if config
            .position_limit_base
            .is_some_and(|limit| limit > 0.0 && sample.position.abs() >= limit)
        {
            at_limit += dt;
        }
        if sample.mid_tick.is_none() || !sample.equity.is_finite() {
            excluded += 1;
            continue;
        }
        valid_n += 1;
        peak = peak.max(sample.equity);
        drawdown = drawdown.max(peak - sample.equity);
        if sample.equity < peak {
            underwater += dt;
        }
    }
    let interval = samples.windows(2).next().map(|w| w[1].ts - w[0].ts);
    let regular =
        interval.is_some_and(|dt| dt > 0 && samples.windows(2).all(|w| w[1].ts - w[0].ts == dt));
    let (bucket_sharpe, sharpe_hac_se) = sharpe(samples, config);
    RiskReport {
        analytics_version: crate::ANALYTICS_VERSION.into(),
        mark: "Path: supplied equity, exchange-truth inventory marked at local mid; net P&L: caller-specified final mark".into(),
        sample_interval_ns: if regular { Value::measured(interval.unwrap_or_default() as f64, "ns", samples.len() as u64, 0) } else { Value::unavailable("irregular or insufficient equity samples; consult sample timestamps", "ns", 0) },
        bucket_horizon_ns: Value::measured(config.bucket_ns as f64, "ns", 1, 0),
        trials: config.trials,
        net_pnl: Value::measured(net_pnl_quote, "quote_currency", fills.len() as u64, 0),
        turnover: Value::measured(turnover, "quote_currency", fills.len() as u64, 0),
        pnl_per_turnover_bps: if turnover > 0.0 { Value::measured(net_pnl_quote / turnover * 10_000.0, "bps_of_turnover", fills.len() as u64, 0) } else { Value::unavailable("no traded turnover", "bps_of_turnover", 0) },
        max_drawdown: if valid_n > 0 { Value::measured(drawdown, "quote_currency", valid_n, excluded) } else { Value::unavailable("no valid equity marks", "quote_currency", excluded) },
        time_under_water_ns: if valid_n > 0 { Value::measured(underwater as f64, "ns", valid_n, excluded) } else { Value::unavailable("no valid equity marks", "ns", excluded) },
        mean_absolute_position: if elapsed > 0 { Value::measured(inventory.get() / elapsed as f64, "base_units", samples.len() as u64, 0) } else { Value::unavailable("zero sampled duration", "base_units", 0) },
        time_at_position_limit_ns: if config.position_limit_base.is_some_and(|x| x.is_finite() && x > 0.0) { Value::measured(at_limit as f64, "ns", samples.len() as u64, 0) } else { Value::unavailable("positive position limit not supplied", "ns", 0) },
        max_long: if samples.is_empty() { Value::unavailable("no equity samples", "base_units", 0) } else { Value::measured(max_long, "base_units", samples.len() as u64, 0) },
        max_short: if samples.is_empty() { Value::unavailable("no equity samples", "base_units", 0) } else { Value::measured(max_short, "base_units", samples.len() as u64, 0) },
        bucket_sharpe, sharpe_hac_se,
        caveats: vec!["Replay: no market impact; queue position modeled".into(), "Path-dependent sampled drawdown and time under water; one realisation, not a risk forecast. Invalid marks excluded, not forward-filled.".into(), "P&L per turnover may be fee-dominated at small edge.".into(), "Sharpe is non-annualised bucket P&L mean/sample SD with HAC delta-method SE, not an IID 100 ms statistic. Multiple trials require selection-bias assessment; prefer Deflated SR in grids.".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(t: i64, equity: f64, position: f64) -> EquitySample {
        EquitySample {
            ts: t,
            tick_no: t as u64,
            mid_tick: Some(200.0),
            position,
            known_position: position,
            cash: 0.0,
            fees: 0.0,
            realized_pnl: equity,
            unrealized_pnl: 0.0,
            equity,
            open_orders: 0,
        }
    }
    #[test]
    fn sampled_drawdown_inventory_and_underwater_are_time_weighted() {
        // [0,2): pos 1 at peak 0; [2,5): pos -2 with equity -3; last equity 1.
        // DD=3; underwater=3ns; mean|pos|=(1*2+2*3)/5=1.6; limit time=3ns.
        let samples = [
            sample(0, 0.0, 1.0),
            sample(2, -3.0, -2.0),
            sample(5, 1.0, 0.0),
        ];
        let r = compute_risk(
            &samples,
            &[],
            &InstrumentMeta {
                tick_size: 0.5,
                lot_size: 1.0,
                contract_size: 1.0,
            },
            1.0,
            &RiskConfig {
                position_limit_base: Some(2.0),
                ..RiskConfig::default()
            },
        );
        assert_eq!(r.max_drawdown.get(), Some(3.0));
        assert_eq!(r.time_under_water_ns.get(), Some(3.0));
        assert_eq!(r.mean_absolute_position.get(), Some(1.6));
        assert_eq!(r.time_at_position_limit_ns.get(), Some(3.0));
        assert!(r.bucket_sharpe.get().is_none());
        assert!(r.pnl_per_turnover_bps.get().is_none());
    }
    #[test]
    fn short_spec_sharpe_fixture_is_explicitly_insufficient() {
        // Spec §5.1 has ten increments: even though naive SR=0.419627, §5.3 forbids display.
        let increments = [1., -1., 2., 0., 1., 3., -2., 0., 1., 1.];
        let mut samples = vec![sample(0, 0.0, 0.0)];
        let mut equity = 0.0;
        for (i, x) in increments.iter().enumerate() {
            equity += x;
            samples.push(sample(i as i64 + 1, equity, 0.0));
        }
        assert!(
            matches!(sharpe(&samples, &RiskConfig { bucket_ns: 1, hac_lag: 1, ..RiskConfig::default() }).0, Value::Unavailable { reason, .. } if reason.contains("insufficient sample"))
        );
    }
    #[test]
    fn thirty_buckets_allow_nonannualised_sharpe_and_hac() {
        // Repeat the ten spec returns three times: sum=18, sum squares=66, n=30.
        // Sample variance=(66-30*.6²)/29=55.2/29; SR=.6/sqrt(55.2/29).
        let increments = [1., -1., 2., 0., 1., 3., -2., 0., 1., 1.];
        let mut samples = vec![sample(0, 0.0, 0.0)];
        let mut equity = 0.0;
        for i in 0..30 {
            equity += increments[i % 10];
            samples.push(sample(i as i64 + 1, equity, 0.0));
        }
        let (sr, se) = sharpe(
            &samples,
            &RiskConfig {
                bucket_ns: 1,
                hac_lag: 2,
                ..RiskConfig::default()
            },
        );
        assert!((sr.get().unwrap() - 0.6 / (55.2f64 / 29.0).sqrt()).abs() < 1e-12);
        assert!(se.get().unwrap() > 0.0);
    }
    #[test]
    fn invalid_mark_invalidates_whole_bucket() {
        let mut samples = vec![
            sample(0, 0.0, 0.0),
            sample(1, 1.0, 0.0),
            sample(2, 2.0, 0.0),
            sample(3, 3.0, 0.0),
        ];
        samples[1].mid_tick = None;
        assert_eq!(buckets(&samples, 2).collect::<Vec<_>>(), vec![None]);
    }
}
