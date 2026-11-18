//! Golden results: the exact `result_sha256` of fixed experiments on the fixed synthetic dataset.
//!
//! Two jobs:
//! * **Guard the run identity.** Stored runs are content-addressed by configuration, dataset,
//!   engine version and `bb_core::SIMULATION_REVISION`. If a code change alters results, this
//!   test fails; the fix is to bump `SIMULATION_REVISION` and update the hashes below in the same
//!   commit, so a stale stored run can never be returned for the new code.
//! * **Cross-platform evidence.** Results that call a platform math function (`powf` / `ln` in
//!   the engine's probability queue models, `powf` / `ln_1p` in Avellaneda-Stoikov, `ln_1p` /
//!   `exp` / `powf` in GLFT) depend on the C runtime's math library, so pins are keyed by its
//!   fingerprint (`bb_core::platform::libm_fingerprint`), not by operating system. CI run
//!   43564035306 showed GitHub's `windows-latest` runner (rustup's bundled MinGW) reproducing
//!   the Linux (glibc) hashes, while the development machine's WinLibs MinGW-w64 14.0.0
//!   (msvcrt) build differs for `maker_slow` and `avellaneda_stoikov`.
//!   - `canonical`: the glibc fingerprint `bb_core::platform::CANONICAL_LIBM`, from CI.
//!   - `winlibs`: `bb_core::platform::WINLIBS_MSVCRT_LIBM`, a documented non-canonical
//!     toolchain kept so that `cargo test` passes on the development machine.
//!
//!   Entries marked `portable` use no platform math function on the result path (the
//!   risk-adverse queue model, the touch maker, any latency model: BlackBox's samplers use their
//!   own `exp` / `ln`) and must be identical under every fingerprint.
//!
//! A missing pin, or a fingerprint with no pins at all, fails with every computed hash printed in
//! one labelled block, so new pins are copied from the failing CI log rather than guessed.

use bb_app::{datasets::DatasetCtx, pipeline, presets};
use bb_core::{
    config::{CubicLeg, ExchangeModel, LogNormalLeg, OrderLatencyConfig, RunConfig},
    platform::{CANONICAL_LIBM, WINLIBS_MSVCRT_LIBM, libm_fingerprint, platform_class},
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
    /// Pin under the canonical (glibc) libm fingerprint; `None` means not pinned yet.
    canonical: Option<&'static str>,
    /// Pin under the WinLibs msvcrt libm fingerprint; `None` means not pinned yet.
    winlibs: Option<&'static str>,
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

/// Asymmetric log-normal legs with a distinct, faster cancel leg.
fn log_normal_asymmetric(cfg: &mut RunConfig) {
    cfg.execution.order_latency = OrderLatencyConfig::LogNormal {
        entry: LogNormalLeg {
            median_ns: 2_000_000,
            sigma: 0.7,
        },
        response: LogNormalLeg {
            median_ns: 1_000_000,
            sigma: 0.3,
        },
        cancel: Some(LogNormalLeg {
            median_ns: 500_000,
            sigma: 0.5,
        }),
    };
    cfg.seed = 7;
}

/// ABIDES cubic latency with 5 ms of strategy think time.
fn heavy_tail_think_time(cfg: &mut RunConfig) {
    let leg = CubicLeg {
        min_ns: 1_000_000,
        jitter: 0.5,
        clip: 0.1,
        unit: 10.0,
    };
    cfg.execution.order_latency = OrderLatencyConfig::HeavyTail {
        entry: leg,
        response: leg,
        cancel: None,
    };
    cfg.execution.think_time_ns = 5_000_000;
    cfg.seed = 11;
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
        canonical: Some(hash),
        winlibs: Some(hash),
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
        canonical: Some("9a809e78afee37bca943f878f173d5fa32d35fa427da4a47f96c1d26d6b778b8"),
        winlibs: Some("662ad3b639d58cc34c00ce67772a34d5f3607f562585b62594aff62da6d40fd7"),
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
    // Avellaneda-Stoikov calls `powf` / `ln_1p` and runs on the PowerProb3 queue model.
    Golden {
        name: "avellaneda_stoikov",
        preset: "avellaneda_stoikov",
        variant: unchanged,
        portable: false,
        canonical: Some("6cafe3ca74bfa4dc94216710916a0f644f015effb68ed9dbe9b425e9a47b074a"),
        winlibs: Some("0731525d84840b4a8622d53e0563ad5faf5c92b48d4683cc49dcadb701462c12"),
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
    Golden {
        must_record: Some("exch_fill"),
        ..same(
            "log_normal_asymmetric",
            "maker_conservative_queue",
            log_normal_asymmetric,
            true,
            "8df348ad5f16eef3fd1c08234dc1724e93ff521b97b77cfb43d8ca73af0abafd",
        )
    },
    Golden {
        must_record: Some("exch_fill"),
        ..same(
            "heavy_tail_think_time",
            "maker_conservative_queue",
            heavy_tail_think_time,
            true,
            "cc75176ad3a345560ea0ed6217318b77a332a8a41e14ef1ae277c7c078b82210",
        )
    },
    // The new presets as shipped: PowerProb3 (and GLFT's `ln_1p` / `exp`), so per fingerprint.
    Golden {
        name: "latency_lognormal",
        preset: "latency_lognormal",
        variant: unchanged,
        portable: false,
        canonical: Some("7b42759bae0616c6742689e6e1615fb7a5cbc7f1fc70e4ac7f0921be1da868de"),
        winlibs: Some("7b42759bae0616c6742689e6e1615fb7a5cbc7f1fc70e4ac7f0921be1da868de"),
        must_record: Some("exch_fill"),
    },
    Golden {
        name: "latency_heavy_tail",
        preset: "latency_heavy_tail",
        variant: unchanged,
        portable: false,
        canonical: Some("09e2b35bef0abf28ffcb545c06f4e79c6a19f055cce3fd01dd2ce890c5d0f15e"),
        winlibs: Some("09e2b35bef0abf28ffcb545c06f4e79c6a19f055cce3fd01dd2ce890c5d0f15e"),
        must_record: Some("exch_fill"),
    },
    Golden {
        name: "glft_baseline",
        preset: "glft_baseline",
        variant: unchanged,
        portable: false,
        canonical: Some("4e7c4dec3f4306b877a17fc59ed3f854c547d33d63d117009489673dc0b0bebd"),
        winlibs: Some("f501c04a4f9281f794d29773297b5fa862451f3b71455e2731a0d7e56424c964"),
        must_record: Some("exch_fill"),
    },
];

/// SHA-256 of each entry's run analytics as stored in the bundle (`bb_app::analytics::encode` of
/// `run_analytics`), keyed by entry name. The analytics pass calls no platform math function: the
/// glibc and WinLibs builds compute identical bytes, also for the entries whose result hashes
/// differ. Pinned from the code before the bounded-memory execution pass (one order's
/// accumulators live at a time), which must reproduce the report byte for byte. A deliberate
/// change to the report (`bb_analytics::ANALYTICS_VERSION`) updates these pins.
const ANALYTICS_SHA256: &[(&str, &str)] = &[
    (
        "maker_baseline",
        "8f8cb1e232194d54323d4cd120895d77ae92ea6403639a2318463a2027bfae39",
    ),
    (
        "maker_baseline partial_fill",
        "a8f6b73e713fdee0620f084ef9d60ee4a0d0472a7761895f89aac300c976c6c8",
    ),
    (
        "maker_slow",
        "76e5ffbdd4b44a411e1ec4eb010adbbe6118c51620880f3042b465a2ace1bb28",
    ),
    (
        "maker_conservative_queue",
        "d705ab425ce2041faa7be806b2b4de3db3b74b0dcd3ff79f1f37ca1365ff9612",
    ),
    (
        "maker_rebate",
        "c55e1a4a70a23edd6ef605ac04bc2cbafe8d10341e7208a0619d334352a00ead",
    ),
    (
        "imbalance_taker",
        "2bd04dca66ffb2aed804d4ad4307607d28094bea89372564a56d90607a85b995",
    ),
    (
        "forensics_probe",
        "796318894d51f9b91384a8e3fa48cd7cbcf5d9567e8bf296e008333e9aec8729",
    ),
    (
        "avellaneda_stoikov",
        "ccd2c0972451ec0506ed4fcbc2738658a73bcf82cf8fc6755418b1a6dae3751c",
    ),
    (
        "jittered_latency",
        "8e624a5358c068de12075654150f7d6e6e72ed65c08fde96299c7c64a6f2f8d1",
    ),
    (
        "feed_offset",
        "22ad0c6d704a900728620d4fce4703bf02076effa7c9e059f9605766453f7c22",
    ),
    (
        "drawdown_halt",
        "8d7ee9f503512d653695d278dd4ffc2c0ea4e5ee152954c8123dcea7bc1482d7",
    ),
    (
        "log_normal_asymmetric",
        "894cc593f41a3af6af0b0164d5afad7c6e4fbdbc2af505d0df47099a356358ec",
    ),
    (
        "heavy_tail_think_time",
        "08d3fe37edd144f45b631e79157869af2b29f2228c4eb9759f269129e6043b00",
    ),
    (
        "latency_lognormal",
        "f8c056bc84ec26a06763d399e1a6358594d479fec3d0c783dd00828974136b62",
    ),
    (
        "latency_heavy_tail",
        "cadb94a1709ae6f6b2284dc2dec20a0ee4bc5cfb906c8e368252e2c05a1ecc9b",
    ),
    (
        "glft_baseline",
        "a66e038297c96526c7881d978fd5dafdb4f2b72d7eb936f75255297410ce7e3b",
    ),
];

/// GitHub's `windows-latest` runner (rustup's bundled MinGW). Its fingerprint differs from glibc's
/// on some probe inputs, but CI run 43567594954 computed every golden entry identical to the
/// canonical pins, so the canonical pins apply to it.
const GITHUB_WINDOWS_RUNNER_LIBM: &str = "9a463940";

/// The pins for this process's libm fingerprint: `None` when the fingerprint is unknown.
fn expected(g: &Golden, fingerprint: &str) -> Option<Option<&'static str>> {
    if Some(fingerprint) == CANONICAL_LIBM || fingerprint == GITHUB_WINDOWS_RUNNER_LIBM {
        Some(g.canonical)
    } else if fingerprint == WINLIBS_MSVCRT_LIBM {
        Some(g.winlibs)
    } else if g.portable {
        // Portable results must equal the canonical pins under any math library.
        Some(g.canonical)
    } else {
        None
    }
}

#[test]
fn portable_entries_agree_across_platforms() {
    for g in RESULTS.iter().filter(|g| g.portable) {
        assert_eq!(
            g.canonical, g.winlibs,
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
    let fingerprint = libm_fingerprint();
    let mut mismatches = Vec::new();
    if manifest.events_sha256 != DATASET_SHA256 {
        mismatches.push(format!(
            "dataset events_sha256: pinned \"{DATASET_SHA256}\", got \"{}\"",
            manifest.events_sha256
        ));
    }
    if Some(fingerprint) != CANONICAL_LIBM
        && fingerprint != WINLIBS_MSVCRT_LIBM
        && fingerprint != GITHUB_WINDOWS_RUNNER_LIBM
    {
        mismatches.push(format!(
            "libm fingerprint {fingerprint} has no pins (canonical: {CANONICAL_LIBM:?}, winlibs: \
             {WINLIBS_MSVCRT_LIBM}); if this is the CI glibc build, set \
             bb_core::platform::CANONICAL_LIBM = Some(\"{fingerprint}\")"
        ));
    }
    // Every entry is run and reported, pinned or not, so one failing log holds all new pins.
    let mut computed = Vec::new();
    for g in RESULTS {
        let result = run(&ctx, &manifest, g);
        let analytics = bb_app::analytics::encode(
            &bb_app::analytics::run_analytics(&result, &ctx)
                .unwrap_or_else(|e| panic!("{}: run analytics: {e}", g.name)),
        )
        .unwrap_or_else(|e| panic!("{}: encode analytics: {e}", g.name));
        let stored: serde_json::Value = serde_json::from_str(&analytics.json).unwrap();
        for points in [100, 50, 25, 12, 6, 3, 2] {
            let mut old = bb_app::analytics::downsampled(&stored, points);
            bb_app::wire::to_wire(&mut old).unwrap();
            let mut projected =
                bb_app::analytics::downsampled_json(&analytics.json, points).unwrap();
            bb_app::wire::to_wire(&mut projected).unwrap();
            assert_eq!(
                serde_json::to_vec(&projected).unwrap(),
                serde_json::to_vec(&old).unwrap(),
                "{} analytics response at {points} points",
                g.name,
            );
        }
        let analytics = hex::encode(analytics.sha256);
        match ANALYTICS_SHA256.iter().find(|(name, _)| *name == g.name) {
            Some((_, pinned)) if analytics == *pinned => {}
            Some((_, pinned)) => mismatches.push(format!(
                "{} analytics: got \"{analytics}\", pinned \"{pinned}\"",
                g.name
            )),
            None => mismatches.push(format!("{} analytics: no pin yet", g.name)),
        }
        if let Some(kind) = g.must_record {
            assert!(
                result.tape.iter().any(|e| e.body.type_name() == kind),
                "{}: the run records no `{kind}` event, so the pin would not cover the feature",
                g.name
            );
        }
        let got = result.manifest.result_sha256.clone();
        match expected(g, fingerprint) {
            Some(Some(pinned)) if got == pinned => {}
            Some(Some(pinned)) => mismatches.push(format!(
                "{} on {class}: got \"{got}\", pinned \"{pinned}\"",
                g.name
            )),
            Some(None) => mismatches.push(format!("{} on {class}: no pin yet", g.name)),
            None => mismatches.push(format!(
                "{} on {class}: unknown libm fingerprint, no pin",
                g.name
            )),
        }
        computed.push(format!(
            "  {:<28} portable={:<5} \"{got}\" analytics \"{analytics}\"",
            g.name, g.portable
        ));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        mismatches.is_empty(),
        "results differ from the pinned golden hashes. If the change is intended, bump \
         bb_core::SIMULATION_REVISION and PINNED_REVISION and update (analytics pins change only \
         with the analytics report, ANALYTICS_SHA256):\n{}\n\n\
         ===== computed golden results: platform class {class}, libm fingerprint {fingerprint} \
         =====\n{}\n===== end of computed golden results =====",
        mismatches.join("\n"),
        computed.join("\n")
    );
}
