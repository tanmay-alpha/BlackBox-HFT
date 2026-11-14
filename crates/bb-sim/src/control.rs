//! Run control: progress reporting, cooperative cancellation and the runtime work budget.
//!
//! The simulation loop calls [`RunControl`] and checks the [`RunBudget`] every
//! `progress_every_ticks` strategy ticks and once at the end. Neither touches simulation state,
//! so a controlled run that completes is bit-identical to an uncontrolled one. Because the tape
//! and the equity series only grow, and the final check always runs, whether a run exceeds its
//! budget does not depend on the check cadence.

use bb_core::limits::ExecutionLimits;

/// How far a run has got.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Strategy ticks evaluated so far.
    pub ticks_done: u64,
    /// Predicted total ticks, never below `ticks_done`. Exact in the final report.
    pub ticks_total: u64,
    /// Flight-tape events recorded so far.
    pub tape_events: u64,
}

/// Observer and cancellation source for one run. Implementations must be cheap: both methods
/// are called on the simulation thread.
pub trait RunControl: Sync {
    /// Called at least every [`RunBudget::progress_every_ticks`] strategy ticks and once at the
    /// end.
    fn on_progress(&self, p: &Progress);
    /// Polled at the same cadence; returning true stops the run with
    /// [`crate::SimError::Cancelled`].
    fn should_cancel(&self) -> bool;
}

/// Reports nothing and never cancels.
#[derive(Copy, Clone, Debug, Default)]
pub struct NoControl;

impl RunControl for NoControl {
    fn on_progress(&self, _p: &Progress) {}

    fn should_cancel(&self) -> bool {
        false
    }
}

/// Hard limits enforced while a run executes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RunBudget {
    /// The run aborts with [`crate::SimError::BudgetExceeded`] once its tape holds more events.
    pub max_tape_events: u64,
    /// The run aborts once it holds more equity samples.
    pub max_equity_samples: u64,
    /// Strategy ticks between two progress reports and checks (at least 1).
    pub progress_every_ticks: u64,
}

impl RunBudget {
    /// The budget part of an execution-limits profile.
    pub const fn from_limits(limits: &ExecutionLimits) -> Self {
        Self {
            max_tape_events: limits.max_tape_events,
            max_equity_samples: limits.max_equity_samples,
            progress_every_ticks: limits.progress_every_ticks,
        }
    }

    /// [`ExecutionLimits::local`]: no tape or sample bound.
    pub const fn local() -> Self {
        Self::from_limits(&ExecutionLimits::local())
    }
}
