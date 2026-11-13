//! Golden results: the exact `result_sha256` of fixed experiments on the fixed synthetic dataset.
//!
//! Two jobs:
//! * **Guard the run identity.** Stored runs are content-addressed by configuration, dataset,
//!   engine version and `bb_core::SIMULATION_REVISION`. If a code change alters results, this
//!   test fails; the fix is to bump `SIMULATION_REVISION` and update the hashes below in the same
//!   commit, so a stale stored run can never be returned for the new code.
//! * **Cross-platform evidence.** It runs on Windows (development, MinGW) and Linux (CI, glibc).
//!   The probability queue models in hftbacktest call `f64::powf` / `f64::ln`, which come from
//!   the platform math library and may differ in the last bit. Almost always that changes
//!   nothing; occasionally a queue comparison sits exactly on the edge and a fill moves. That was
//!   observed for `maker_slow` (CI run 43503383714), so its hash is pinned per operating system.
//!   `maker_conservative_queue` uses the risk-adverse queue model, which is exact arithmetic,
//!   and must be identical everywhere.

use bb_app::{datasets::DatasetCtx, pipeline, presets};
use bb_core::{config::ExchangeModel, time::NS_PER_S};
use bb_data::synthetic::SyntheticConfig;

/// Bumped together with the hashes below; the test asserts they agree.
const PINNED_REVISION: u32 = 2;

const DATASET_SHA256: &str = "6960c01ecce90c6e2943ec634c4d3dbef8bbe4d41781d652f1069e07cb6af386";

struct Golden {
    preset: &'static str,
    partial_fill: bool,
    /// Uses no platform math-library function on the result path.
    portable: bool,
    windows: &'static str,
    linux: &'static str,
}

const fn same(
    preset: &'static str,
    partial_fill: bool,
    portable: bool,
    hash: &'static str,
) -> Golden {
    Golden {
        preset,
        partial_fill,
        portable,
        windows: hash,
        linux: hash,
    }
}

const RESULTS: &[Golden] = &[
    same(
        "maker_baseline",
        false,
        false,
        "1f3f67f53941623ad4e07608bda53a65c24216dffc901b8d469ec587263807b4",
    ),
    same(
        "maker_baseline",
        true,
        false,
        "1b9ee90b7914bb753de39d5f73a994b36fc57679c6de3fe333048009eb8554b4",
    ),
    Golden {
        preset: "maker_slow",
        partial_fill: false,
        portable: false,
        windows: "662ad3b639d58cc34c00ce67772a34d5f3607f562585b62594aff62da6d40fd7",
        linux: "9a809e78afee37bca943f878f173d5fa32d35fa427da4a47f96c1d26d6b778b8",
    },
    same(
        "maker_conservative_queue",
        false,
        true,
        "20bc0827bfeeba0ef3e61df8d6418ecee5629e5328fe93b0faeefcec8f477c5d",
    ),
    same(
        "maker_rebate",
        false,
        false,
        "57b8ea6465338d152b19f27a698177c13c5eec1de98fd856f2efea9c0d23b860",
    ),
    same(
        "imbalance_taker",
        false,
        false,
        "239d845a3467505fa4e270fd29785c54e8e6844347257cb16b0030ac04b404d3",
    ),
    same(
        "forensics_probe",
        false,
        false,
        "9dfe0bb0967083b6667cf865a64ce38db3f2ab621a62b3312334eeff17e962e2",
    ),
];

/// The hash this platform must produce, or `None` when no pin exists for it.
fn expected(g: &Golden) -> Option<&'static str> {
    if cfg!(windows) {
        Some(g.windows)
    } else if cfg!(target_os = "linux") {
        Some(g.linux)
    } else if g.portable {
        Some(g.windows)
    } else {
        None
    }
}

#[test]
fn portable_entries_agree_across_platforms() {
    for g in RESULTS.iter().filter(|g| g.portable) {
        assert_eq!(g.windows, g.linux, "{} is portable but pinned differently", g.preset);
    }
}

#[test]
fn results_match_the_pinned_hashes() {
    assert_eq!(
        bb_core::SIMULATION_REVISION,
        PINNED_REVISION,
        "SIMULATION_REVISION changed: recompute and update the pinned hashes in this file"
    );

    let dir = std::env::temp_dir().join(format!("bb-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = SyntheticConfig {
        id: "golden".into(),
        duration_ns: 30 * NS_PER_S,
        ..SyntheticConfig::default()
    };
    let built = bb_data::synthetic::generate(&config).expect("generate");
    let manifest = bb_data::store::save(&dir, &built).expect("save");
    let ctx = DatasetCtx::load(&dir, "golden").expect("load");

    let mut mismatches = Vec::new();
    if manifest.events_sha256 != DATASET_SHA256 {
        mismatches.push(format!(
            "dataset events_sha256: pinned \"{DATASET_SHA256}\", got \"{}\"",
            manifest.events_sha256
        ));
    }
    for g in RESULTS {
        let Some(pinned) = expected(g) else { continue };
        let mut cfg = presets::find(&manifest, g.preset)
            .unwrap_or_else(|| panic!("preset {} missing", g.preset))
            .config;
        if g.partial_fill {
            cfg.execution.exchange_model = ExchangeModel::PartialFill;
        }
        let run = pipeline::execute(&ctx, &cfg).expect("simulation succeeds");
        if run.manifest.result_sha256 != pinned {
            mismatches.push(format!(
                "{} (partial_fill={}) on {}: got \"{}\", pinned \"{pinned}\"",
                g.preset,
                g.partial_fill,
                std::env::consts::OS,
                run.manifest.result_sha256
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        mismatches.is_empty(),
        "results differ from the pinned golden hashes. If the change is intended, bump \
         bb_core::SIMULATION_REVISION and PINNED_REVISION and update:\n{}",
        mismatches.join("\n")
    );
}
