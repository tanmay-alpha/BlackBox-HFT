//! Execution limits: how much work one run may do, predicted before it starts ([`WorkEstimate`],
//! checked by [`ExecutionLimits::admit`]) and enforced while it runs (the simulator's run budget
//! aborts a run that crosses `max_tape_events` or `max_equity_samples`; the application refuses to
//! store a result larger than `max_output_bytes`).
//!
//! Two profiles exist. [`ExecutionLimits::local`] is the behaviour of the local tool: only the
//! strategy-evaluation bound. [`ExecutionLimits::hosted`] is sized from measurements so that one
//! run's peak memory stays well under 300 MB (`docs/PERFORMANCE.md`, "Execution budgets").

use serde::{Deserialize, Serialize};

use crate::{
    config::{MAX_TICKS_PER_RUN, RunConfig, StrategyKind},
    dataset::DatasetManifest,
};

/// Predicted size of one run, computed from the configuration and the dataset manifest alone.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkEstimate {
    /// Engine rows the run replays (the engine holds its own copy of them).
    pub dataset_rows: u64,
    /// Strategy evaluations. An upper bound: the dataset's span plus the feed offset and the
    /// largest feed latency, divided by the interval, plus one.
    pub ticks: u64,
    /// Equity samples, an upper bound derived from `ticks` and `record.sample_every_ticks`.
    pub equity_samples: u64,
    /// Flight-tape events. A conservative per-strategy prediction calibrated on the bundled
    /// strategies (every measured run stayed below it), not a proof: the run budget enforces
    /// the hard limit while the run executes.
    pub tape_events_upper: u64,
    /// Bytes of the stored result, from `tape_events_upper` and `equity_samples` at the largest
    /// measured bytes per element.
    pub output_bytes_upper: u64,
}

/// Work and output bounds for one run.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExecutionLimits {
    /// Engine rows of the dataset. Checked at admission.
    pub max_dataset_rows: u64,
    /// Strategy evaluations. Checked at admission against [`WorkEstimate::ticks`].
    pub max_ticks: u64,
    /// Equity samples. Checked at admission and enforced while running.
    pub max_equity_samples: u64,
    /// Flight-tape events. Checked at admission against the prediction and enforced while
    /// running.
    pub max_tape_events: u64,
    /// Bytes of the serialized result. Checked at admission against the prediction and enforced
    /// on the finished result before it is stored.
    pub max_output_bytes: u64,
    /// Strategy ticks between two progress reports / cancellation and budget checks.
    pub progress_every_ticks: u64,
}

/// Default cadence of progress reports and cooperative checks. At the minimum 1 ms interval this
/// is one second of simulated time; the cost per check is not measurable
/// (`docs/PERFORMANCE.md`, "Run control overhead").
pub const PROGRESS_EVERY_TICKS: u64 = 1_000;

/// Stored bytes per flight-tape event (the event plus its share of the derived order records and
/// fills). Largest measured: 574 (imbalance taker, whose fills carry markouts).
pub const STORED_BYTES_PER_TAPE_EVENT: u64 = 600;
/// Stored bytes per equity sample. Measured: 237 to 240.
pub const STORED_BYTES_PER_EQUITY_SAMPLE: u64 = 300;
/// Fixed part of a stored result (manifest, metrics, engine check), with margin.
pub const STORED_BYTES_BASE: u64 = 64 * 1024;
/// Tape events of one scripted order apart from queue updates (intent, request, responses,
/// fill, cancel), with margin.
const EVENTS_PER_SCRIPTED_ORDER: u64 = 16;
/// Tape events of one immediate-or-cancel order of the imbalance taker, with margin.
const EVENTS_PER_TAKER_ORDER: u64 = 10;
/// Run-level events (risk halt and the like), with margin.
const RUN_EVENTS: u64 = 64;

impl ExecutionLimits {
    /// The local tool: today's single bound on strategy evaluations, nothing else.
    pub const fn local() -> Self {
        Self {
            max_dataset_rows: u64::MAX,
            max_ticks: MAX_TICKS_PER_RUN as u64,
            max_equity_samples: u64::MAX,
            max_tape_events: u64::MAX,
            max_output_bytes: u64::MAX,
            progress_every_ticks: PROGRESS_EVERY_TICKS,
        }
    }

    /// The hosted worker (one 512 MB instance shared with the API). Every preset of the 120 s
    /// demo dataset is admitted down to a 1 ms interval except the imbalance taker (10 ms); the
    /// largest admitted measured run peaked at 110 MiB working set (147 MiB private bytes) on
    /// Windows, and the hard budget bounds a mispredicted run at about 225 MiB working set by the
    /// measured per-element costs.
    /// Derivation in `docs/PERFORMANCE.md`, "Execution budgets".
    pub const fn hosted() -> Self {
        Self {
            max_dataset_rows: 500_000,
            max_ticks: 150_000,
            max_equity_samples: 150_000,
            max_tape_events: 150_000,
            max_output_bytes: 128 * 1024 * 1024,
            progress_every_ticks: PROGRESS_EVERY_TICKS,
        }
    }

    /// Rejects an estimate that exceeds any limit, naming the first one in a fixed order
    /// (dataset rows, ticks, equity samples, tape events, output bytes).
    pub fn admit(&self, estimate: &WorkEstimate) -> Result<(), BudgetViolation> {
        let checks = [
            ("dataset_rows", estimate.dataset_rows, self.max_dataset_rows),
            ("ticks", estimate.ticks, self.max_ticks),
            (
                "equity_samples",
                estimate.equity_samples,
                self.max_equity_samples,
            ),
            (
                "tape_events",
                estimate.tape_events_upper,
                self.max_tape_events,
            ),
            (
                "output_bytes",
                estimate.output_bytes_upper,
                self.max_output_bytes,
            ),
        ];
        for (what, predicted, limit) in checks {
            if predicted > limit {
                return Err(BudgetViolation {
                    what,
                    limit,
                    predicted,
                });
            }
        }
        Ok(())
    }
}

/// A predicted quantity that exceeds its limit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BudgetViolation {
    /// `dataset_rows`, `ticks`, `equity_samples`, `tape_events` or `output_bytes`.
    pub what: &'static str,
    pub limit: u64,
    pub predicted: u64,
}

impl std::fmt::Display for BudgetViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hint = match self.what {
            "dataset_rows" => "use a shorter dataset",
            "ticks" => "use a longer strategy interval or a shorter dataset",
            "equity_samples" => "raise record.sample_every_ticks or use a longer interval",
            _ => "use a longer strategy interval, a shorter dataset or a less active strategy",
        };
        write!(
            f,
            "predicted {} {} exceed the limit of {} for this server; {hint}",
            self.predicted, self.what, self.limit
        )
    }
}

impl RunConfig {
    /// Predicts the work and output of this configuration on the dataset described by
    /// `manifest`, without running it.
    pub fn estimate_work(&self, manifest: &DatasetManifest) -> WorkEstimate {
        let interval = self.strategy.interval_ns.max(1);
        let feed_latency_max = manifest
            .quality
            .feed_latency_ns
            .map(|q| q.max)
            .filter(|v| v.is_finite() && *v > 0.0)
            .map_or(0, |v| v.ceil() as i64);
        // The last tick falls at most one interval after the last local receipt time, which is
        // at most the last exchange time plus the feed latency and the configured offset.
        let span = manifest
            .end
            .saturating_sub(manifest.start)
            .max(0)
            .saturating_add(self.execution.feed_latency_offset_ns.max(0))
            .saturating_add(feed_latency_max);
        let ticks = (span / interval) as u64 + 1;
        let every = u64::from(self.record.sample_every_ticks.max(1));
        // One sample every `every` ticks starting at tick 0, plus the closing sample.
        let equity_samples = ticks.div_ceil(every) + 1;
        let rows = manifest.counts.rows;
        let strategy_events = match &self.strategy.kind {
            // Resting quotes: queue updates follow the depth rows at the quoted levels, and
            // re-quoting follows the ticks. Measured on the bundled presets: at most 1.23 events
            // per row at a 1 ms interval, about 1.0 at 10 ms and 0.4 at 100 ms. GLFT maintains
            // the same two quotes through the same code (`bb-strategies` `quoting.rs`).
            StrategyKind::TouchMaker(_)
            | StrategyKind::AvellanedaStoikov(_)
            | StrategyKind::Glft(_) => rows.saturating_mul(5) / 4 + ticks / 2,
            // Immediate-or-cancel orders never rest: at most one entry per cooldown and one exit
            // per holding period.
            StrategyKind::ImbalanceTaker(p) => {
                let entries = ticks / u64::from(p.cooldown_ticks.max(1));
                let exits = ticks / u64::from(p.hold_ticks.max(1));
                (entries + exits).saturating_mul(EVENTS_PER_TAKER_ORDER)
            }
            // Each order's lifecycle plus its capped depth-driven queue updates. Trade-driven
            // updates are uncapped, so a long-resting order can exceed this; the run budget
            // catches it.
            StrategyKind::Scripted(p) => (p.orders.len() as u64).saturating_mul(
                EVENTS_PER_SCRIPTED_ORDER + u64::from(self.record.max_queue_updates_per_order),
            ),
        };
        let tape_events_upper = strategy_events.saturating_add(RUN_EVENTS);
        let output_bytes_upper = STORED_BYTES_BASE
            .saturating_add(STORED_BYTES_PER_TAPE_EVENT.saturating_mul(tape_events_upper))
            .saturating_add(STORED_BYTES_PER_EQUITY_SAMPLE.saturating_mul(equity_samples));
        WorkEstimate {
            dataset_rows: rows,
            ticks,
            equity_samples,
            tape_events_upper,
            output_bytes_upper,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(ticks: u64, tape: u64) -> WorkEstimate {
        WorkEstimate {
            dataset_rows: 63_000,
            ticks,
            equity_samples: ticks + 1,
            tape_events_upper: tape,
            output_bytes_upper: STORED_BYTES_BASE
                + tape * STORED_BYTES_PER_TAPE_EVENT
                + (ticks + 1) * STORED_BYTES_PER_EQUITY_SAMPLE,
        }
    }

    #[test]
    fn local_bounds_only_strategy_evaluations() {
        let local = ExecutionLimits::local();
        assert!(local.admit(&estimate(2_000_000, 100_000_000)).is_ok());
        let err = local.admit(&estimate(2_000_001, 0)).unwrap_err();
        assert_eq!((err.what, err.limit), ("ticks", 2_000_000));
    }

    #[test]
    fn hosted_names_the_first_limit_exceeded() {
        let hosted = ExecutionLimits::hosted();
        assert!(hosted.admit(&estimate(120_005, 138_790)).is_ok());
        let err = hosted.admit(&estimate(1_800_005, 2_000_000)).unwrap_err();
        assert_eq!(err.what, "ticks");
        assert_eq!(err.predicted, 1_800_005);
        let err = hosted.admit(&estimate(1_000, 150_001)).unwrap_err();
        assert_eq!(err.what, "tape_events");
        let message = err.to_string();
        assert!(message.contains("150001 tape_events"), "{message}");
        assert!(message.contains("limit of 150000"), "{message}");
        let rows = WorkEstimate {
            dataset_rows: 500_001,
            ..estimate(1_000, 1_000)
        };
        assert_eq!(hosted.admit(&rows).unwrap_err().what, "dataset_rows");
    }
}
