//! Golden results: the exact `result_sha256` of fixed experiments on the fixed synthetic dataset.
//!
//! Two jobs:
//! * **Guard the run identity.** Stored runs are content-addressed by configuration, dataset,
//!   engine version and `bb_core::SIMULATION_REVISION`. If a code change alters results, this
//!   test fails; the fix is to bump `SIMULATION_REVISION` and update the hashes below in the same
//!   commit, so a stale stored run can never be returned for the new code.
//! * **Cross-platform evidence.** CI runs it on `linux-x86_64` (the canonical platform class,
//!   glibc) and on `windows-x86_64-gnu` (MinGW). The probability queue models in hftbacktest
//!   call `f64::powf` and Avellaneda-Stoikov calls `f64::ln`; these come from the platform math
//!   library and may differ in the last bit. Almost always that changes nothing; occasionally a
//!   comparison sits exactly on the edge and a fill moves. That was observed for `maker_slow`
//!   (CI run 43503383714), so hashes are pinned per platform class. Entries marked `portable`
//!   use no platform math function on the result path (the risk-adverse queue model, constant or
//!   seeded-jitter latency, the touch maker) and must be identical everywhere.
//!
//! A platform class with no pin for an entry fails with the computed hash printed, so a new pin
//! is copied from the failing CI log rather than guessed.

use bb_app::{datasets::DatasetCtx, pipeline, presets};
use bb_core::{
    config::{ExchangeModel, OrderLatencyConfig, RunConfig},
    platform::platform_class,
    result::RunResult,
    time::NS_PER_S,
};
use bb_data::synthetic::SyntheticConfig;

/// Bumped together with the hashes below; the test asserts they agree.
const PINNED_REVISION: u32 = 2;

const DATASET_SHA256: &str = "6960c01ecce90c6e2943ec634c4d3dbef8bbe4d41781d652f1069e07cb6af386";

struct Golden {
    /// Unique label in failure messages.
    name: &'static str,
    preset: &'static str,
    /// Applied to the preset's configuration.
    variant: fn(&mut RunConfig),
    /// Uses no platform math-library function on the result path.
    portable: bool,
    /// Pins per platform class; `None` means not pinned yet on that class.
    windows_gnu: Option<&'static str>,
    linux: Option<&'static str>,
    /// A tape event type the run must contain, proving the variant exercised its feature.
    must_record: Option<&'static str>,
}

fn unchanged(_: &mut RunConfig) {}

fn partial_fill(cfg: &mut RunConfig) {
    cfg.execution.exchange_model = ExchangeModel::PartialFill;
}

fn jittered_latency(cfg: &mut RunConfig) {
    cfg.execution.order_latency = OrderLatencyConfig::Jitter {
        entry_ns: 1_000_000,
        response_ns: 1_000_000,
        jitter_ns: 4_000_000,
    };
    cfg.seed = 42;
}

fn feed_offset(cfg: &mut RunConfig) {
    cfg.execution.feed_latency_offset_ns = 5_000_000;
}

fn drawdown_halt(cfg: &mut RunConfig) {
    cfg.risk.max_drawdown = Some(0.05);
}

const fn same(
    name: &'static str,
    preset: &'static str,
    variant: fn(&mut RunConfig),
    portable: bool,
    hash: &'static str,
) -> Golden {
    Golden {
        name,
        preset,
        variant,
        portable,
        windows_gnu: Some(hash),
        linux: Some(hash),
        must_record: None,
    }
}

const RESULTS: &[Golden] = &[
    same(
        "maker_baseline",
        "maker_baseline",
        unchanged,
        false,
        "1f3f67f53941623ad4e07608bda53a65c24216dffc901b8d469ec587263807b4",
    ),
    same(
        "maker_baseline partial_fill",
        "maker_baseline",
        partial_fill,
        false,
        "1b9ee90b7914bb753de39d5f73a994b36fc57679c6de3fe333048009eb8554b4",
    ),
    Golden {
        name: "maker_slow",
        preset: "maker_slow",
        variant: unchanged,
        portable: false,
        windows_gnu: Some("662ad3b639d58cc34c00ce67772a34d5f3607f562585b62594aff62da6d40fd7"),
        linux: Some("9a809e78afee37bca943f878f173d5fa32d35fa427da4a47f96c1d26d6b778b8"),
        must_record: None,
    },
    same(
        "maker_conservative_queue",
        "maker_conservative_queue",
        unchanged,
        true,
        "20bc0827bfeeba0ef3e61df8d6418ecee5629e5328fe93b0faeefcec8f477c5d",
    ),
    same(
        "maker_rebate",
        "maker_rebate",
        unchanged,
        false,
        "57b8ea6465338d152b19f27a698177c13c5eec1de98fd856f2efea9c0d23b860",
    ),
    same(
        "imbalance_taker",
        "imbalance_taker",
        unchanged,
        false,
        "239d845a3467505fa4e270fd29785c54e8e6844347257cb16b0030ac04b404d3",
    ),
    same(
        "forensics_probe",
        "forensics_probe",
        unchanged,
        false,
        "9dfe0bb0967083b6667cf865a64ce38db3f2ab621a62b3312334eeff17e962e2",
    ),
    // Avellaneda-Stoikov calls `ln` and runs on the PowerProb3 queue model: pinned per class.
    Golden {
        name: "avellaneda_stoikov",
        preset: "avellaneda_stoikov",
        variant: unchanged,
        portable: false,
        windows_gnu: Some("0731525d84840b4a8622d53e0563ad5faf5c92b48d4683cc49dcadb701462c12"),
        linux: None,
        must_record: Some("exch_fill"),
    },
    // The following variants sit on the risk-adverse queue model so that their pins are
    // platform-independent; each isolates one feature.
    Golden {
        must_record: Some("exch_fill"),
        ..same(
            "jittered_latency",
            "maker_conservative_queue",
            jittered_latency,
            true,
            "e2c38fd187eedcb47ef3319ea54b7f92ce442c8372a4d516d8c9ab3ce4b56f58",
        )
    },
    Golden {
        must_record: Some("exch_fill"),
        ..same(
            "feed_offset",
            "maker_conservative_queue",
            feed_offset,
            true,
            "5cbb0f4526ba35d4af34e1e309c86ddc05c8b39f1b8f465d165e20d061389565",
        )
    },
    Golden {
        must_record: Some("risk_halt"),
        ..same(
            "drawdown_halt",
            "maker_conservative_queue",
            drawdown_halt,
            true,
            "a77054e9a52d3dd74522e3cdb90f198dbd2b28397f402ae4f1309b7e67ab27d1",
        )
    },
];

/// The pin for this build's platform class: `Err` names the class when it has no pins at all.
fn expected(g: &Golden) -> Result<Option<&'static str>, String> {
    match platform_class().as_str() {
        "windows-x86_64-gnu" => Ok(g.windows_gnu),
        "linux-x86_64" => Ok(g.linux),
        _ if g.portable => Ok(g.linux),
        other => Err(other.to_string()),
    }
}

#[test]
fn portable_entries_agree_across_platforms() {
    for g in RESULTS.iter().filter(|g| g.portable) {
        assert_eq!(
            g.windows_gnu, g.linux,
            "{} is portable but pinned differently",
            g.name
        );
    }
    let mut names: Vec<_> = RESULTS.iter().map(|g| g.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), RESULTS.len(), "golden names must be unique");
}

fn run(ctx: &DatasetCtx, manifest: &bb_core::dataset::DatasetManifest, g: &Golden) -> RunResult {
    let mut cfg = presets::find(manifest, g.preset)
        .unwrap_or_else(|| panic!("preset {} missing", g.preset))
        .config;
    (g.variant)(&mut cfg);
    pipeline::execute(ctx, &cfg).unwrap_or_else(|e| panic!("{}: {e}", g.name))
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

    let class = platform_class();
    let mut mismatches = Vec::new();
    if manifest.events_sha256 != DATASET_SHA256 {
        mismatches.push(format!(
            "dataset events_sha256: pinned \"{DATASET_SHA256}\", got \"{}\"",
            manifest.events_sha256
        ));
    }
    for g in RESULTS {
        let pinned = match expected(g) {
            Ok(pinned) => pinned,
            // A platform class with no pins (e.g. macOS) checks only the portable entries.
            Err(_) => continue,
        };
        let result = run(&ctx, &manifest, g);
        if let Some(kind) = g.must_record {
            assert!(
                result.tape.iter().any(|e| e.body.type_name() == kind),
                "{}: the run records no `{kind}` event, so the pin would not cover the feature",
                g.name
            );
        }
        let got = &result.manifest.result_sha256;
        match pinned {
            Some(pinned) if got == pinned => {}
            Some(pinned) => mismatches.push(format!(
                "{} on {class}: got \"{got}\", pinned \"{pinned}\"",
                g.name
            )),
            None => mismatches.push(format!(
                "{} on {class}: no pin yet; computed \"{got}\"",
                g.name
            )),
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
