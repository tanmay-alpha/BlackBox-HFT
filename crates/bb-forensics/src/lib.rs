//! Execution forensics over a recorded flight tape.
//!
//! Everything here is a pure function of bb-core types: no engine, no I/O, no clock. The same tape
//! always produces the same order records, metrics, explanations and comparison.
//!
//! The explanations and comparison notes follow one rule: a statement cites the tape events it
//! rests on and names its [`bb_core::Basis`]. Observed market data, model output, arithmetic and
//! things that cannot be known from a replay are never blended into one claim.

mod compare;
mod ensemble;
mod explain;
mod metrics;
mod orders;
mod sweep;
mod text;

#[cfg(test)]
mod testkit;

use bb_core::{
    flight::FlightEvent,
    result::{EquitySample, FillRecord},
};

pub use compare::{
    CompareInput, MetadataInput, compare_metadata, compare_runs, compare_runs_until, config_diffs,
};
pub use ensemble::{
    ENSEMBLE_ANALYSIS_VERSION, EnsembleMember, EnsembleReport, EnsembleRunOutcome, EnsembleSegment,
    MetricSpread, analyze_ensemble,
};
pub use explain::explain_order;
pub use metrics::{compute_metrics, enrich_fills};
pub use orders::build_orders;
pub use sweep::{
    Availability, Caveat, ConfigurationCounts, Dispersion, DispersionReport, Pbo, RankCorrelation,
    SWEEP_ANALYSIS_VERSION, SelectedCell, SelectionBias, SweepCellOutcome, SweepReport,
    analyze_sweep, cscv_pbo, expected_max_of_normals, inverse_normal_cdf, objective_blocks,
    spearman,
};

/// SHA-256 over the flight tape, fills and equity samples: the `result_sha256` of a run manifest.
pub fn result_sha256(
    tape: &[FlightEvent],
    fills: &[FillRecord],
    equity: &[EquitySample],
) -> String {
    bb_core::hash::sha256_json(&(tape, fills, equity))
}

#[cfg(test)]
mod tests {
    use bb_core::{OrderStatus, Side, Tif};

    use super::*;
    use crate::testkit::{MS, Tape};

    #[test]
    fn result_hash_is_stable_and_sensitive_to_content() {
        let mut tape = Tape::new();
        tape.rest(10 * MS, 256, Side::Buy, 20_000, 1.0, Tif::Gtc, Some(2.0));
        let base = result_sha256(&tape.0, &[], &[]);
        assert_eq!(base.len(), 64);
        assert_eq!(base, result_sha256(&tape.0, &[], &[]));

        tape.resp(50 * MS, 256, OrderStatus::Canceled, 1.0);
        assert_ne!(base, result_sha256(&tape.0, &[], &[]));
    }
}
