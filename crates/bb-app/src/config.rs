//! Runtime configuration from the environment (DEPLOYMENT.md §5). `bb-app` is the only crate that
//! reads environment variables (TARGET_ARCHITECTURE §3 rule 5); everything below receives an
//! [`AppConfig`].
//!
//! Secrets (database URLs, the Blob token) are wrapped in [`Secret`], whose `Debug`/`Display`
//! never print the value, so the startup log ([`AppConfig::summary`]) cannot leak them.

use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};

use bb_core::limits::ExecutionLimits;

/// Execution and access profile (`BB_PROFILE`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Developer machine: `ExecutionLimits::local()`, loopback hosts, filesystem artefacts.
    Local,
    /// Render free instance: `ExecutionLimits::hosted()`, every required variable enforced.
    Hosted,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Local => "local",
            Profile::Hosted => "hosted",
        }
    }
}

/// A credential-bearing value. Never printed.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where run bundles are stored.
#[derive(Clone, Debug)]
pub enum StorageConfig {
    /// `FsArtifactStore` under this directory (local profile default: `<data-dir>/artifacts`).
    Fs(PathBuf),
    /// Vercel Blob private store (`BLOB_READ_WRITE_TOKEN`).
    VercelBlob(Secret),
}

impl StorageConfig {
    /// Backend name as stored in `bb.run_artifacts.storage_backend`.
    pub fn backend(&self) -> &'static str {
        match self {
            StorageConfig::Fs(_) => "fs",
            StorageConfig::VercelBlob(_) => "vercel_blob",
        }
    }
}

/// Request-rate limits (API_CONTRACT §5).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RateLimits {
    pub anonymous_per_min: u32,
    pub anonymous_burst: u32,
    pub keyed_per_min: u32,
    pub keyed_burst: u32,
}

/// Validated configuration of one `blackbox` process.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub profile: Profile,
    pub data_dir: PathBuf,
    /// Listen address from `BB_ADDR` / `PORT` (the CLI `--addr` overrides it).
    pub addr: SocketAddr,
    pub database_url: Option<Secret>,
    pub database_url_unpooled: Option<Secret>,
    pub storage: StorageConfig,
    /// Exact `Origin` values allowed cross-origin (`BB_CORS_ALLOWED_ORIGINS`).
    pub cors_allowed_origins: Vec<String>,
    /// Host names (without port) accepted in `Host` (`BB_ALLOWED_HOSTS` ∪
    /// `RENDER_EXTERNAL_HOSTNAME`, plus loopback names in the local profile).
    pub allowed_hosts: Vec<String>,
    /// In-process worker loops (`BB_WORKER_SLOTS`; 0 = API only).
    pub worker_slots: usize,
    pub migrate_on_start: bool,
    pub shutdown_grace: Duration,
    pub git_sha: Option<String>,
    /// sha256 digest of a bootstrap admin key (`BB_ADMIN_BOOTSTRAP_KEY_SHA256`).
    pub admin_bootstrap_key_sha256: Option<[u8; 32]>,
    /// Platform class recorded on jobs submitted here (`BB_RESULT_PLATFORM`).
    pub result_platform: String,
    /// Platform class of this binary: the only class its worker claims.
    pub worker_platform: String,
    /// `RENDER_INSTANCE_ID`, used in the worker id.
    pub instance_id: Option<String>,
    pub limits: ExecutionLimits,
    pub max_sweep_cells: usize,
    pub deadline_ms: i32,
    pub lease_seconds: i32,
    pub heartbeat_interval: Duration,
    /// Longest idle sleep of a standalone worker that cannot be woken in process.
    pub idle_poll_max: Duration,
    pub artifact_disk_cache_bytes: u64,
    pub max_response_bytes: usize,
    pub rate_limits: RateLimits,
    pub heavy_read_concurrency: usize,
    /// Requests in flight across the whole server before new ones get 503 `busy`.
    pub max_in_flight: usize,
    /// HMAC pepper of the API key check tag (`BB_API_KEY_PEPPER`, SECURITY_REVIEW §9.1).
    pub api_key_pepper: Secret,
    /// Anonymous requests per minute across all clients (SEC-9 global cap).
    pub anonymous_global_per_min: u32,
    /// Daily response-byte budgets (SECURITY_REVIEW §6.2).
    pub egress_anonymous_daily_bytes: i64,
    pub egress_keyed_daily_bytes: i64,
    /// Largest anonymous response body; longer pages end with `truncated_by: "bytes"`.
    pub anonymous_max_response_bytes: usize,
    /// Blob read budget per month (SECURITY_REVIEW §6.3).
    pub blob_monthly_reads_cap: i32,
    pub blob_monthly_read_bytes_cap: i64,
}

/// One environment variable that is missing or invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    pub variable: &'static str,
    pub problem: String,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid configuration: {}", .0.iter().map(|p| format!("{} {}", p.variable, p.problem)).collect::<Vec<_>>().join("; "))]
pub struct ConfigError(pub Vec<ConfigProblem>);

const DEFAULT_LOCAL_ADDR: &str = "127.0.0.1:8787";
/// Pepper used when `BB_API_KEY_PEPPER` is unset in the local profile. Public on purpose: it only
/// protects keys of a developer's own loopback server; the hosted profile requires a real one.
pub const LOCAL_DEV_PEPPER: &str = "blackbox-local-development-pepper-not-secret";

/// Listen address precedence for commands that do not need the full configuration (`demo`):
/// `--addr` > `BB_ADDR` > `0.0.0.0:$PORT` > `127.0.0.1:8787`.
pub fn listen_addr(cli: Option<SocketAddr>) -> Result<SocketAddr, ConfigError> {
    if let Some(addr) = cli {
        return Ok(addr);
    }
    let problem = |variable: &'static str, problem: String| {
        ConfigError(vec![ConfigProblem { variable, problem }])
    };
    if let Some(addr) = std::env::var("BB_ADDR")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        return addr
            .trim()
            .parse()
            .map_err(|_| problem("BB_ADDR", format!("`{addr}` is not a socket address")));
    }
    if let Some(port) = std::env::var("PORT").ok().filter(|v| !v.trim().is_empty()) {
        return port
            .trim()
            .parse::<u16>()
            .map(|p| SocketAddr::from(([0, 0, 0, 0], p)))
            .map_err(|_| problem("PORT", format!("`{port}` is not a TCP port")));
    }
    Ok(DEFAULT_LOCAL_ADDR.parse().expect("constant address"))
}

impl AppConfig {
    /// Reads the process environment.
    pub fn from_env(data_dir: PathBuf) -> Result<Self, ConfigError> {
        Self::from_lookup(data_dir, |name| std::env::var(name).ok())
    }

    /// Builds and validates the configuration from a variable lookup (tests pass a map).
    pub fn from_lookup(
        data_dir: PathBuf,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let get = |name: &str| {
            lookup(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut problems = Vec::new();
        let mut problem = |variable: &'static str, text: String| {
            problems.push(ConfigProblem {
                variable,
                problem: text,
            })
        };

        let profile = match get("BB_PROFILE").as_deref() {
            None | Some("local") => Profile::Local,
            Some("hosted") => Profile::Hosted,
            Some(other) => {
                problem(
                    "BB_PROFILE",
                    format!("must be `local` or `hosted`, not `{other}`"),
                );
                Profile::Local
            }
        };
        let hosted = profile == Profile::Hosted;

        let addr = match (get("BB_ADDR"), get("PORT")) {
            (Some(addr), _) => addr.parse().unwrap_or_else(|_| {
                problem("BB_ADDR", format!("`{addr}` is not a socket address"));
                DEFAULT_LOCAL_ADDR.parse().expect("constant address")
            }),
            (None, Some(port)) => match port.parse::<u16>() {
                Ok(port) => SocketAddr::from(([0, 0, 0, 0], port)),
                Err(_) => {
                    problem("PORT", format!("`{port}` is not a TCP port"));
                    DEFAULT_LOCAL_ADDR.parse().expect("constant address")
                }
            },
            (None, None) => DEFAULT_LOCAL_ADDR.parse().expect("constant address"),
        };

        let database_url = get("DATABASE_URL").map(Secret::new);
        let database_url_unpooled = get("DATABASE_URL_UNPOOLED").map(Secret::new);
        if hosted && database_url.is_none() {
            problem("DATABASE_URL", "is required when BB_PROFILE=hosted".into());
        }
        if hosted && database_url_unpooled.is_none() {
            problem(
                "DATABASE_URL_UNPOOLED",
                "is required when BB_PROFILE=hosted".into(),
            );
        }
        // SEC-27: the hosted profile only talks to Postgres over TLS with a verified certificate
        // and host name.
        if hosted {
            for (name, url) in [
                ("DATABASE_URL", &database_url),
                ("DATABASE_URL_UNPOOLED", &database_url_unpooled),
            ] {
                if let Some(url) = url
                    && !url.expose().contains("sslmode=verify-full")
                {
                    problem(
                        name,
                        "must use sslmode=verify-full when BB_PROFILE=hosted".into(),
                    );
                }
            }
        }

        let storage = match get("BLOB_READ_WRITE_TOKEN") {
            Some(token) => {
                if !token.starts_with("vercel_blob_rw_") {
                    problem(
                        "BLOB_READ_WRITE_TOKEN",
                        "does not look like a Vercel Blob read-write token".into(),
                    );
                }
                StorageConfig::VercelBlob(Secret::new(token))
            }
            None => {
                if hosted {
                    problem(
                        "BLOB_READ_WRITE_TOKEN",
                        "is required when BB_PROFILE=hosted (local disk does not survive a Render restart)"
                            .into(),
                    );
                }
                StorageConfig::Fs(match get("BB_ARTIFACT_DIR") {
                    Some(dir) => PathBuf::from(dir),
                    None => data_dir.join("artifacts"),
                })
            }
        };

        let cors_allowed_origins: Vec<String> = list(get("BB_CORS_ALLOWED_ORIGINS"));
        for origin in &cors_allowed_origins {
            if !is_origin(origin) {
                problem(
                    "BB_CORS_ALLOWED_ORIGINS",
                    format!("`{origin}` is not an exact origin like https://example.org"),
                );
            }
        }
        if hosted && cors_allowed_origins.is_empty() {
            problem(
                "BB_CORS_ALLOWED_ORIGINS",
                "is required when BB_PROFILE=hosted (the web app's origin)".into(),
            );
        }

        let mut allowed_hosts: Vec<String> = list(get("BB_ALLOWED_HOSTS"))
            .into_iter()
            .map(|h| h.to_ascii_lowercase())
            .collect();
        if let Some(render) = get("RENDER_EXTERNAL_HOSTNAME") {
            allowed_hosts.push(render.to_ascii_lowercase());
        }
        if hosted && allowed_hosts.is_empty() {
            problem(
                "RENDER_EXTERNAL_HOSTNAME",
                "or BB_ALLOWED_HOSTS is required when BB_PROFILE=hosted (Host allowlist)".into(),
            );
        }
        if !hosted {
            for host in ["localhost", "127.0.0.1", "[::1]"] {
                if !allowed_hosts.iter().any(|h| h == host) {
                    allowed_hosts.push(host.to_string());
                }
            }
        }

        let worker_slots = parse(
            &get("BB_WORKER_SLOTS"),
            "BB_WORKER_SLOTS",
            1usize,
            &mut problem,
        );
        if hosted && worker_slots > 1 {
            problem(
                "BB_WORKER_SLOTS",
                "must be 0 or 1 when BB_PROFILE=hosted (one 512 MB instance)".into(),
            );
        }
        if worker_slots > 16 {
            problem("BB_WORKER_SLOTS", "must be at most 16".into());
        }
        let migrate_on_start = match get("BB_MIGRATE_ON_START").as_deref() {
            None | Some("false") | Some("0") => false,
            Some("true") | Some("1") => true,
            Some(other) => {
                problem(
                    "BB_MIGRATE_ON_START",
                    format!("must be `true` or `false`, not `{other}`"),
                );
                false
            }
        };
        let grace_ms = parse(
            &get("BB_SHUTDOWN_GRACE_MS"),
            "BB_SHUTDOWN_GRACE_MS",
            25_000u64,
            &mut problem,
        );
        if !(1_000..=300_000).contains(&grace_ms) {
            problem(
                "BB_SHUTDOWN_GRACE_MS",
                "must be between 1000 and 300000".into(),
            );
        }
        let git_sha = get("BB_GIT_SHA");
        if let Some(sha) = &git_sha
            && (sha.len() < 7 || sha.len() > 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            problem("BB_GIT_SHA", "must be 7 to 40 hex characters".into());
        }
        let admin_bootstrap_key_sha256 = match get("BB_ADMIN_BOOTSTRAP_KEY_SHA256") {
            None => None,
            Some(hex_digest) => {
                let parsed = hex::decode(&hex_digest)
                    .ok()
                    .filter(|_| hex_digest.bytes().all(|b| !b.is_ascii_uppercase()))
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
                if parsed.is_none() {
                    problem(
                        "BB_ADMIN_BOOTSTRAP_KEY_SHA256",
                        "must be 64 lowercase hex characters (sha256 of a bbk_ key, never the key)"
                            .into(),
                    );
                }
                parsed
            }
        };

        let api_key_pepper = match get("BB_API_KEY_PEPPER") {
            Some(pepper) if pepper.len() >= 32 => Secret::new(pepper),
            Some(_) => {
                problem(
                    "BB_API_KEY_PEPPER",
                    "must be at least 32 characters of random data".into(),
                );
                Secret::new(String::new())
            }
            None if hosted => {
                problem(
                    "BB_API_KEY_PEPPER",
                    "is required when BB_PROFILE=hosted (API key check tag)".into(),
                );
                Secret::new(String::new())
            }
            // Local development only: a fixed pepper, so locally minted keys keep working.
            None => Secret::new(LOCAL_DEV_PEPPER),
        };

        let worker_platform = bb_core::platform::platform_class();
        let result_platform = get("BB_RESULT_PLATFORM").unwrap_or_else(|| worker_platform.clone());
        if !is_platform(&result_platform) {
            problem(
                "BB_RESULT_PLATFORM",
                format!("`{result_platform}` is not a platform class like {worker_platform}"),
            );
        }

        let artifact_disk_cache_bytes = parse(
            &get("BB_ARTIFACT_DISK_CACHE_BYTES"),
            "BB_ARTIFACT_DISK_CACHE_BYTES",
            256u64 << 20,
            &mut problem,
        );
        let max_response_bytes = parse(
            &get("BB_MAX_RESPONSE_BYTES"),
            "BB_MAX_RESPONSE_BYTES",
            4usize << 20,
            &mut problem,
        );
        if max_response_bytes < 64 * 1024 {
            problem("BB_MAX_RESPONSE_BYTES", "must be at least 65536".into());
        }
        let heartbeat_ms = parse(
            &get("BB_HEARTBEAT_INTERVAL_MS"),
            "BB_HEARTBEAT_INTERVAL_MS",
            15_000u64,
            &mut problem,
        );
        let lease_seconds = parse(
            &get("BB_LEASE_SECONDS"),
            "BB_LEASE_SECONDS",
            60i32,
            &mut problem,
        );
        if lease_seconds <= 0 || u64::try_from(lease_seconds).unwrap_or(0) * 1000 <= heartbeat_ms {
            problem(
                "BB_LEASE_SECONDS",
                "must be positive and longer than BB_HEARTBEAT_INTERVAL_MS".into(),
            );
        }

        let (limits, max_sweep_cells, rate_limits) = if hosted {
            (
                ExecutionLimits::hosted(),
                25,
                RateLimits {
                    anonymous_per_min: 120,
                    anonymous_burst: 30,
                    keyed_per_min: 600,
                    keyed_burst: 100,
                },
            )
        } else {
            (
                ExecutionLimits::local(),
                bb_core::compare::MAX_GRID_CELLS,
                RateLimits {
                    anonymous_per_min: 60_000,
                    anonymous_burst: 10_000,
                    keyed_per_min: 60_000,
                    keyed_burst: 10_000,
                },
            )
        };

        if !problems.is_empty() {
            return Err(ConfigError(problems));
        }
        Ok(Self {
            profile,
            data_dir,
            addr,
            database_url,
            database_url_unpooled,
            storage,
            cors_allowed_origins,
            allowed_hosts,
            worker_slots,
            migrate_on_start,
            shutdown_grace: Duration::from_millis(grace_ms),
            git_sha,
            admin_bootstrap_key_sha256,
            result_platform,
            worker_platform,
            instance_id: get("RENDER_INSTANCE_ID"),
            limits,
            max_sweep_cells,
            deadline_ms: 600_000,
            lease_seconds,
            heartbeat_interval: Duration::from_millis(heartbeat_ms),
            idle_poll_max: Duration::from_secs(30),
            artifact_disk_cache_bytes,
            max_response_bytes,
            rate_limits,
            heavy_read_concurrency: 4,
            max_in_flight: if hosted { 32 } else { 256 },
            api_key_pepper,
            anonymous_global_per_min: if hosted { 1_200 } else { 600_000 },
            egress_anonymous_daily_bytes: 60_000_000,
            egress_keyed_daily_bytes: 90_000_000,
            anonymous_max_response_bytes: 1 << 20,
            blob_monthly_reads_cap: 9_000,
            blob_monthly_read_bytes_cap: 9_000_000_000,
        })
    }

    /// The pooled and direct database URLs, or an error naming the missing variable.
    pub fn database_urls(&self) -> Result<(&str, &str), ConfigError> {
        let pooled = self.database_url.as_ref().ok_or_else(|| {
            ConfigError(vec![ConfigProblem {
                variable: "DATABASE_URL",
                problem: "is required for this command".into(),
            }])
        })?;
        // A local cluster has no pooler; the direct URL defaults to the pooled one.
        let direct = self.database_url_unpooled.as_ref().unwrap_or(pooled);
        Ok((pooled.expose(), direct.expose()))
    }

    /// One log line describing the configuration without any secret.
    pub fn summary(&self) -> String {
        format!(
            "profile={} addr={} database={} storage={} cors_allowed_origins=[{}] allowed_hosts=[{}] \
             worker_slots={} migrate_on_start={} shutdown_grace_ms={} git_sha={} \
             bootstrap_admin_key={} result_platform={} worker_platform={} limits={{max_ticks={}, \
             max_tape_events={}, max_output_bytes={}}} max_sweep_cells={} data_dir={}",
            self.profile.as_str(),
            self.addr,
            if self.database_url.is_some() {
                "configured"
            } else {
                "none"
            },
            self.storage.backend(),
            self.cors_allowed_origins.join(","),
            self.allowed_hosts.join(","),
            self.worker_slots,
            self.migrate_on_start,
            self.shutdown_grace.as_millis(),
            self.git_sha.as_deref().unwrap_or("unknown"),
            if self.admin_bootstrap_key_sha256.is_some() {
                "configured"
            } else {
                "none"
            },
            self.result_platform,
            self.worker_platform,
            self.limits.max_ticks,
            self.limits.max_tape_events,
            self.limits.max_output_bytes,
            self.max_sweep_cells,
            self.data_dir.display(),
        )
    }
}

fn list(value: Option<String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse<T: std::str::FromStr>(
    value: &Option<String>,
    name: &'static str,
    default: T,
    problem: &mut impl FnMut(&'static str, String),
) -> T {
    match value {
        None => default,
        Some(text) => text.parse().unwrap_or_else(|_| {
            problem(name, format!("`{text}` is not a valid number"));
            default
        }),
    }
}

/// `scheme://host[:port]` with no path, query, wildcard or trailing slash.
pub fn is_origin(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https")
        && !rest.is_empty()
        && !rest.contains(['/', '?', '#', '*', '@', ' '])
}

/// Same shape as the database constraint (`0007_platform_libm.sql`):
/// `<os>-<arch>[-<env>][+libm-<8 lowercase hex>]`. Classes without the libm suffix were recorded
/// before fingerprints existed and remain valid for reading old rows.
pub fn is_platform(value: &str) -> bool {
    let (target, libm) = match value.split_once("+libm-") {
        Some((target, fp)) => (target, Some(fp)),
        None => (value, None),
    };
    let mut parts = target.split('-');
    let ok_part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    value.len() <= 64
        && libm.is_none_or(|fp| {
            fp.len() == 8
                && fp
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        && parts.next().is_some_and(|p| ok_part(p) && !p.contains('_'))
        && parts.clone().count() >= 1
        && parts.all(ok_part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(vars: &[(&str, &str)]) -> Result<AppConfig, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        AppConfig::from_lookup(PathBuf::from("data"), move |k| map.get(k).cloned())
    }

    #[test]
    fn local_defaults_need_nothing() {
        let c = config(&[]).unwrap();
        assert_eq!(c.profile, Profile::Local);
        assert_eq!(c.addr.to_string(), "127.0.0.1:8787");
        assert_eq!(c.worker_slots, 1);
        assert!(c.allowed_hosts.contains(&"localhost".to_string()));
        assert_eq!(c.storage.backend(), "fs");
    }

    #[test]
    fn listen_address_precedence_is_bb_addr_then_port() {
        assert_eq!(
            config(&[("PORT", "10000")]).unwrap().addr.to_string(),
            "0.0.0.0:10000"
        );
        assert_eq!(
            config(&[("PORT", "10000"), ("BB_ADDR", "127.0.0.1:9000")])
                .unwrap()
                .addr
                .to_string(),
            "127.0.0.1:9000"
        );
    }

    #[test]
    fn hosted_profile_names_every_missing_variable() {
        let err = config(&[("BB_PROFILE", "hosted")]).unwrap_err();
        let names: Vec<_> = err.0.iter().map(|p| p.variable).collect();
        for name in [
            "DATABASE_URL",
            "DATABASE_URL_UNPOOLED",
            "BLOB_READ_WRITE_TOKEN",
            "BB_CORS_ALLOWED_ORIGINS",
            "RENDER_EXTERNAL_HOSTNAME",
        ] {
            assert!(names.contains(&name), "{name} missing from {err}");
        }
    }

    #[test]
    fn summary_never_contains_secrets() {
        let c = config(&[
            ("BB_PROFILE", "hosted"),
            (
                "DATABASE_URL",
                "postgres://u:supersecret@db/x?sslmode=verify-full",
            ),
            (
                "DATABASE_URL_UNPOOLED",
                "postgres://u:supersecret@db/x?sslmode=verify-full",
            ),
            ("BLOB_READ_WRITE_TOKEN", "vercel_blob_rw_store_supersecret"),
            ("BB_CORS_ALLOWED_ORIGINS", "https://app.example.org"),
            ("RENDER_EXTERNAL_HOSTNAME", "api.example.org"),
            ("BB_WORKER_SLOTS", "1"),
            ("BB_API_KEY_PEPPER", "supersecret-pepper-0123456789abcdef"),
        ])
        .unwrap();
        let text = format!("{} {:?}", c.summary(), c);
        assert!(!text.contains("supersecret"), "{text}");
        assert_eq!(c.limits, ExecutionLimits::hosted());
    }

    #[test]
    fn origins_and_digests_are_validated() {
        assert!(config(&[("BB_CORS_ALLOWED_ORIGINS", "https://*.vercel.app")]).is_err());
        assert!(config(&[("BB_CORS_ALLOWED_ORIGINS", "https://a.example/")]).is_err());
        assert!(config(&[("BB_ADMIN_BOOTSTRAP_KEY_SHA256", "abc")]).is_err());
        let digest = "a".repeat(64);
        assert!(
            config(&[("BB_ADMIN_BOOTSTRAP_KEY_SHA256", digest.as_str())])
                .unwrap()
                .admin_bootstrap_key_sha256
                .is_some()
        );
        assert!(is_platform("windows-x86_64-gnu"));
        assert!(is_platform("linux-x86_64"));
        assert!(is_platform("linux-x86_64+libm-53154a4e"));
        assert!(is_platform("windows-x86_64-gnu+libm-e9b26b47"));
        assert!(is_platform(&bb_core::platform::platform_class()));
        assert!(!is_platform("linux-x86_64+libm-53154A4E"));
        assert!(!is_platform("linux-x86_64+libm-123"));
        assert!(!is_platform("Linux"));
    }
}
