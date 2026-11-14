//! Platform classes: which builds may be expected to produce bit-identical results.
//!
//! A run's result depends on the math library wherever the engine or a strategy calls a
//! transcendental function: `powf` and `ln` in the engine's probability queue models, `powf` and
//! `ln_1p` in Avellaneda-Stoikov, `ln_1p`, `exp` and `powf` in GLFT
//! (`docs/EXPERIMENT_REPRODUCIBILITY.md`, "Platform policy"). Rust takes these from the C
//! runtime the binary links, so the operating system alone does not decide them: CI run
//! 43564035306 showed GitHub's `windows-latest` runner (x86_64-pc-windows-gnu with rustup's
//! bundled MinGW) reproducing the Linux (glibc) golden hashes exactly, while the development
//! machine's WinLibs MinGW-w64 14.0.0 (msvcrt) build produced different ones.
//!
//! The *platform class* recorded in every run manifest is therefore
//! `<os>-<arch>[-<env>]+libm-<fingerprint>`, where the fingerprint ([`libm_fingerprint`]) is
//! measured at run time by evaluating exactly those functions on fixed inputs. Two builds with
//! the same fingerprint computed every one of those inputs bit for bit identically.
//!
//! [`CANONICAL_LIBM`] is the fingerprint of the reference results: the one observed on
//! `linux-x86_64` (glibc) in CI, the hosted worker's platform.

use std::sync::LazyLock;

use sha2::{Digest, Sha256};

use crate::rng::Rng;

/// Fingerprint of the math library whose results are the published reference: glibc on
/// `linux-x86_64`, as computed by the CI `rust` job. `None` until it is pinned from a CI log
/// (`linux_glibc_has_the_canonical_fingerprint` prints it); until then no class is canonical.
pub const CANONICAL_LIBM: Option<&str> = None;

/// Fingerprint of WinLibs MinGW-w64 14.0.0 (msvcrt runtime) on `windows-x86_64-gnu`, the
/// development machine's toolchain. Its `pow` / `ln` differ from glibc's in the last bit on some
/// inputs, and two golden runs (`maker_slow`, `avellaneda_stoikov`) differ with them. A
/// documented non-canonical toolchain: its results are never canonical.
pub const WINLIBS_MSVCRT_LIBM: &str = "e9b26b47";

/// The operating system, architecture and (unless it is the OS default or absent) C runtime
/// environment of this build: `linux-x86_64` (glibc), `linux-x86_64-musl`,
/// `windows-x86_64-gnu` (MinGW), `windows-x86_64-msvc`, `macos-aarch64`.
pub fn target_class() -> String {
    let base = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    match TARGET_ENV {
        "" => base,
        "gnu" if cfg!(target_os = "linux") => base,
        env => format!("{base}-{env}"),
    }
}

/// The platform class of this process: `<target class>+libm-<fingerprint>`, for example
/// `linux-x86_64+libm-0123abcd`.
pub fn platform_class() -> String {
    format!("{}+libm-{}", target_class(), libm_fingerprint())
}

/// The libm fingerprint part of a recorded platform class, if it has one. Classes recorded
/// before fingerprints existed (`windows-x86_64-gnu`) have none.
pub fn libm_of(platform: &str) -> Option<&str> {
    platform.rsplit_once("+libm-").map(|(_, fp)| fp)
}

/// Whether results recorded on `platform` are canonical: computed with the canonical math
/// library, whatever the operating system.
pub fn is_canonical(platform: &str) -> bool {
    CANONICAL_LIBM.is_some() && libm_of(platform) == CANONICAL_LIBM
}

/// First 8 hex digits of the SHA-256 of the bit patterns of [`libm_probe`]'s results.
/// Computed once per process.
pub fn libm_fingerprint() -> &'static str {
    static FINGERPRINT: LazyLock<String> = LazyLock::new(|| {
        let mut h = Sha256::new();
        for v in libm_probe() {
            h.update(v.to_bits().to_le_bytes());
        }
        hex::encode(h.finalize())[..8].to_string()
    });
    &FINGERPRINT
}

/// Number of inputs per probed function.
const PROBES_PER_FUNCTION: usize = 64;

/// Evaluates every platform-provided `f64` function on BlackBox's result path on fixed inputs
/// drawn from the domains where they are used:
///
/// * `x.powf(n)`: engine `PowerProbQueueFunc*` with queue quantities `x` in `[0, 100)` and
///   exponents `n` in `[0.25, 5)`; Avellaneda-Stoikov and GLFT `0.5.powf(1 / halflife)`.
/// * `(1 + x).ln()`: engine `LogProbQueueFunc*` with `x` in `[0, 1000)`.
/// * `x.ln_1p()`: Avellaneda-Stoikov `ln_1p(gamma / kappa)` and GLFT `ln_1p(gamma Delta / k)`,
///   `x` in `[1e-6, 10)` on a log scale.
/// * `x.exp()`: GLFT's power term, `x` in `[-20, 20)`.
/// * `x.sqrt()` and `10.powi(d)`: correctly rounded / compiler-provided, included so that a
///   platform where they differ would be noticed too.
///
/// Inputs come from BlackBox's own PRNG, which uses integer arithmetic only, so they are the
/// same everywhere; only the function results can differ.
pub fn libm_probe() -> Vec<f64> {
    let mut rng = Rng::for_stream(0x6c69_626d, "platform.libm_probe");
    let mut out = Vec::with_capacity(PROBES_PER_FUNCTION * 7);
    for _ in 0..PROBES_PER_FUNCTION {
        let x = rng.next_f64() * 100.0;
        let n = 0.25 + rng.next_f64() * 4.75;
        out.push(x.powf(n));
    }
    for halflife in 1..=PROBES_PER_FUNCTION {
        out.push(0.5f64.powf(1.0 / halflife as f64));
    }
    for _ in 0..PROBES_PER_FUNCTION {
        out.push((1.0 + rng.next_f64() * 1000.0).ln());
    }
    for _ in 0..PROBES_PER_FUNCTION {
        // 1e-6 * 10^(7u): log-uniform on [1e-6, 10).
        let x = 1e-6 * 10f64.powi((rng.next_f64() * 7.0) as i32) * (1.0 + rng.next_f64() * 9.0);
        out.push(x.ln_1p());
    }
    for _ in 0..PROBES_PER_FUNCTION {
        out.push((rng.next_f64() * 40.0 - 20.0).exp());
    }
    for _ in 0..PROBES_PER_FUNCTION {
        out.push((rng.next_f64() * 1e6).sqrt());
    }
    for d in 0..=12 {
        out.push(10f64.powi(d));
    }
    out
}

const TARGET_ENV: &str = if cfg!(target_env = "gnu") {
    "gnu"
} else if cfg!(target_env = "msvc") {
    "msvc"
} else if cfg!(target_env = "musl") {
    "musl"
} else {
    ""
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_build_has_the_documented_target_class() {
        let class = target_class();
        let expected = if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            "linux-x86_64"
        } else if cfg!(all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            "windows-x86_64-gnu"
        } else if cfg!(all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc"
        )) {
            "windows-x86_64-msvc"
        } else {
            return;
        };
        assert_eq!(class, expected);
    }

    #[test]
    fn platform_class_carries_a_stable_libm_fingerprint() {
        let fp = libm_fingerprint();
        // Visible with `--nocapture` (CI): the value to pin for a new toolchain.
        println!("platform class {}", platform_class());
        assert_eq!(fp.len(), 8);
        assert!(fp.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(libm_fingerprint(), fp, "computed once per process");
        let class = platform_class();
        assert_eq!(class, format!("{}+libm-{fp}", target_class()));
        assert_eq!(libm_of(&class), Some(fp));
        assert_eq!(libm_of("windows-x86_64-gnu"), None);
        if let Some(canonical) = CANONICAL_LIBM {
            // Canonical is a property of the math library, not of the operating system.
            assert!(is_canonical(&format!(
                "windows-x86_64-gnu+libm-{canonical}"
            )));
        }
        assert!(!is_canonical(&format!(
            "linux-x86_64+libm-{WINLIBS_MSVCRT_LIBM}"
        )));
        assert!(!is_canonical("linux-x86_64"));
        // The probe covers every function family listed in its documentation.
        assert_eq!(libm_probe().len(), 6 * PROBES_PER_FUNCTION + 13);
    }

    /// The canonical class is defined by CI's Linux (glibc) build: on that target the computed
    /// fingerprint must be the pinned one. A new glibc that changes any probed result fails
    /// here with the new value printed.
    #[test]
    fn linux_glibc_has_the_canonical_fingerprint() {
        if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            assert_eq!(
                Some(libm_fingerprint()),
                CANONICAL_LIBM,
                "\nlinux-x86_64 libm fingerprint changed or is not pinned yet: set \
                 bb_core::platform::CANONICAL_LIBM = Some(\"{}\")\n",
                libm_fingerprint()
            );
        }
    }
}
