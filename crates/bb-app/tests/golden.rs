//! Golden results: the exact `result_sha256` of fixed experiments on the fixed synthetic dataset.
//!
//! Two jobs:
//! * **Guard the run identity.** Stored runs are content-addressed by configuration, dataset,
//!   engine version and `bb_core::SIMULATION_REVISION`. If a code change alters results, this
//!   test fails; the fix is to bump `SIMULATION_REVISION` and update the hashes below in the same
//!   commit, so a stale stored run can never be returned for the new code.
//! * **Cross-platform evidence.** It runs on Windows (development) and Linux (CI). Only
//!   strategies with exact arithmetic are pinned here; Avellaneda-Stoikov uses `ln`/`exp`, whose
//!   last bit may differ between math libraries (docs/BUILDING.md).

use bb_app::{datasets::DatasetCtx, pipeline, presets};
use bb_core::{config::ExchangeModel, time::NS_PER_S};
use bb_data::synthetic::SyntheticConfig;

/// Bumped together with the hashes below; the test asserts they agree.
const PINNED_REVISION: u32 = 2;

const DATASET_SHA256: &str = "6960c01ecce90c6e2943ec634c4d3dbef8bbe4d41781d652f1069e07cb6af386";

/// (preset id, partial-fill variant?, result_sha256)
const RESULTS: &[(&str, bool, &str)] = &[
    (
        "maker_baseline",
        false,
        "1f3f67f53941623ad4e07608bda53a65c24216dffc901b8d469ec587263807b4",
    ),
    (
        "maker_baseline",
        true,
        "1b9ee90b7914bb753de39d5f73a994b36fc57679c6de3fe333048009eb8554b4",
    ),
    (
        "maker_slow",
        false,
        "662ad3b639d58cc34c00ce67772a34d5f3607f562585b62594aff62da6d40fd7",
    ),
    (
        "maker_conservative_queue",
        false,
        "20bc0827bfeeba0ef3e61df8d6418ecee5629e5328fe93b0faeefcec8f477c5d",
    ),
    (
        "maker_rebate",
        false,
        "57b8ea6465338d152b19f27a698177c13c5eec1de98fd856f2efea9c0d23b860",
    ),
    (
        "imbalance_taker",
        false,
        "239d845a3467505fa4e270fd29785c54e8e6844347257cb16b0030ac04b404d3",
    ),
    (
        "forensics_probe",
        false,
        "9dfe0bb0967083b6667cf865a64ce38db3f2ab621a62b3312334eeff17e962e2",
    ),
];

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
    for (preset, partial, pinned) in RESULTS {
        let mut cfg = presets::find(&manifest, preset)
            .unwrap_or_else(|| panic!("preset {preset} missing"))
            .config;
        if *partial {
            cfg.execution.exchange_model = ExchangeModel::PartialFill;
        }
        let run = pipeline::execute(&ctx, &cfg).expect("simulation succeeds");
        if run.manifest.result_sha256 != *pinned {
            mismatches.push(format!(
                "(\"{preset}\", {partial}, \"{}\") was pinned as \"{pinned}\"",
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
