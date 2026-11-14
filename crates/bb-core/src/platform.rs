//! Platform classes: which builds may be expected to produce bit-identical results.
//!
//! A run's result depends on the platform's math library wherever the engine or a strategy calls
//! a transcendental function (`powf` in the probability queue models, `ln` in Avellaneda-Stoikov,
//! see `docs/EXPERIMENT_REPRODUCIBILITY.md`). Two builds of the same commit agree bit for bit only
//! when they share the operating system, architecture and C runtime / math library. That triple
//! is the *platform class* recorded in every run manifest.
//!
//! [`CANONICAL_PLATFORM`] is the class whose results are published and pinned as the reference:
//! Linux x86-64 with glibc, the hosted worker and CI.

/// The platform class whose results are the reference ones.
pub const CANONICAL_PLATFORM: &str = "linux-x86_64";

/// The platform class of this build: `<os>-<arch>`, followed by `-<target_env>` unless the
/// environment is the default one for the OS (glibc on Linux) or there is none (macOS).
///
/// Examples: `linux-x86_64` (glibc), `linux-x86_64-musl`, `windows-x86_64-gnu` (MinGW),
/// `windows-x86_64-msvc`, `macos-aarch64`.
pub fn platform_class() -> String {
    let base = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    match TARGET_ENV {
        "" => base,
        "gnu" if cfg!(target_os = "linux") => base,
        env => format!("{base}-{env}"),
    }
}

/// Whether `platform` (a recorded platform class) is the canonical one.
pub fn is_canonical(platform: &str) -> bool {
    platform == CANONICAL_PLATFORM
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
    fn this_build_has_the_documented_class() {
        let class = platform_class();
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
        assert_eq!(is_canonical(&class), expected == CANONICAL_PLATFORM);
    }
}
