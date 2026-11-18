//! Artefact storage and integrity on read (TARGET_ARCHITECTURE §6–§7, SECURITY_REVIEW §6.3).
//!
//! * [`Storage`] owns the configured `ArtifactStore`; the store itself is private, so every
//!   request goes through a wrapper below. A metered store (Vercel Blob) can only be built with
//!   a [`ReadBudget`], and every GET and HEAD attempt (retries included) is charged against the
//!   persisted monthly/daily read budget *before* it is sent, every PUT attempt against the
//!   upload allowances; a refused charge fails closed (503 `storage`) without touching the
//!   network. Reads caused by anonymous requests may take at most half of the daily share
//!   ([`crate::v1::auth::read_class`]). Only the filesystem store is unmetered.
//! * [`BundleCache`] keeps whole verified bundles on local disk (`<data-dir>/cache/bundles`),
//!   byte-bounded. A bundle is fetched once (one GET), verified end to end against the trust
//!   root in Postgres, and only then cached. Every page read re-verifies the page and metadata
//!   digests, so a corrupted cache copy is detected; it is discarded and fetched again once,
//!   and only a store copy that also fails verification quarantines the run.

use std::{
    collections::HashMap,
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use bb_artifacts::{
    ArtifactError, ArtifactHead, ArtifactStore, BlobConfig, BundleLimits, BundleReader,
    BundleTrailer, BundleTrailerCore, BundleTrust, FsArtifactStore, RetryPolicy, Section,
    VercelBlobStore,
};
use bb_core::{
    flight::FlightEvent,
    result::{EquitySample, FillRecord, OrderRecord, RunResult},
};
use bb_db::{Database, RunArtifact, RunView};
use bytes::Bytes;
use serde::de::DeserializeOwned;

use crate::{config::StorageConfig, metrics::Metrics, v1::error::ApiError};

/// Largest single ranged GET when fetching a whole bundle.
const FETCH_CHUNK: u64 = 8 << 20;
const STORE_ATTEMPTS: usize = 3;

/// The persisted read budget of a metered store: the monthly Blob read-op and read-byte caps
/// (daily pacing, the anonymous half share and the upload allowances live in Postgres).
#[derive(Clone)]
pub struct ReadBudget {
    db: Database,
    reads_cap: i32,
    bytes_cap: i64,
}

impl ReadBudget {
    pub fn new(db: Database, reads_cap: i32, bytes_cap: i64) -> Self {
        Self {
            db,
            reads_cap,
            bytes_cap,
        }
    }
}

/// The configured artefact store plus its budget.
#[derive(Clone)]
pub struct Storage {
    store: Arc<dyn ArtifactStore>,
    pub backend: &'static str,
    /// `None` = unmetered (the filesystem store only).
    budget: Option<ReadBudget>,
}

/// What one store attempt reserves first.
#[derive(Clone, Copy)]
enum Charge {
    /// One read op (GET or HEAD) and its bytes.
    Read(i64),
    /// One upload attempt against the global and the workspace allowance.
    Upload(Option<uuid::Uuid>),
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage budget exhausted")]
    Budget,
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error("storage budget check failed: {0}")]
    Database(#[from] bb_db::Error),
}

impl StorageError {
    pub fn into_api(self) -> ApiError {
        match self {
            StorageError::Budget => ApiError::unavailable("storage", 3600),
            StorageError::Artifact(ArtifactError::Transport(e)) => {
                tracing::error!(error = %e, "storage transport failed");
                ApiError::unavailable("storage", 30)
            }
            StorageError::Artifact(ArtifactError::Http(status))
                if status == 429 || status >= 500 =>
            {
                tracing::error!(status, "storage unavailable");
                ApiError::unavailable("storage", 30)
            }
            StorageError::Artifact(e) => ApiError::internal(format!("storage: {e}")),
            StorageError::Database(e) => ApiError::from(e),
        }
    }
}

impl Storage {
    pub fn from_config(
        config: &StorageConfig,
        db: Database,
        reads_cap: i32,
        bytes_cap: i64,
    ) -> Result<Self, ArtifactError> {
        Ok(match config {
            StorageConfig::Fs(root) => Self::filesystem(Arc::new(FsArtifactStore::new(root)?)),
            StorageConfig::VercelBlob(token) => {
                let parts: Vec<&str> = token.expose().split('_').collect();
                let store_id = parts.get(3).copied().unwrap_or_default();
                let mut blob = BlobConfig::for_store(store_id)?;
                // Retries happen here, one budget charge per attempt.
                blob.retry = RetryPolicy {
                    attempts: 1,
                    ..RetryPolicy::default()
                };
                Self::metered(
                    Arc::new(VercelBlobStore::with_config(token.expose(), blob)?),
                    "vercel_blob",
                    ReadBudget::new(db, reads_cap, bytes_cap),
                )
            }
        })
    }

    /// A store charged like Vercel Blob: every GET/HEAD/PUT attempt reserves its budget first.
    pub fn metered(
        store: Arc<dyn ArtifactStore>,
        backend: &'static str,
        budget: ReadBudget,
    ) -> Self {
        Self {
            store,
            backend,
            budget: Some(budget),
        }
    }

    /// An unmetered filesystem store (`backend = "fs"`; local profile and tests).
    pub fn filesystem(store: Arc<dyn ArtifactStore>) -> Self {
        Self {
            store,
            backend: "fs",
            budget: None,
        }
    }

    /// Reserves `charge` on a metered store; `Budget` when it is refused.
    async fn reserve(&self, charge: Charge) -> Result<(), StorageError> {
        let Some(b) = &self.budget else {
            return Ok(());
        };
        let granted = match charge {
            Charge::Read(bytes) => {
                let class = crate::v1::auth::read_class();
                b.db.charge_storage_read(self.backend, bytes, b.reads_cap, b.bytes_cap, class)
                    .await?
            }
            Charge::Upload(workspace) => b.db.reserve_upload(self.backend, workspace).await?,
        };
        if granted {
            Ok(())
        } else {
            Err(StorageError::Budget)
        }
    }

    /// Runs `op` up to [`STORE_ATTEMPTS`] times on transient failures (transport, 429, 5xx),
    /// reserving `charge` before every attempt; a refused reservation ends it (fail closed).
    async fn attempts<T, F, Fut>(
        &self,
        charge: Charge,
        mut delay: Duration,
        mut op: F,
    ) -> Result<T, StorageError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, ArtifactError>>,
    {
        let mut last = None;
        for _ in 0..STORE_ATTEMPTS {
            self.reserve(charge).await?;
            match op().await {
                Ok(value) => return Ok(value),
                Err(e @ (ArtifactError::Transport(_) | ArtifactError::Http(429 | 500..=599))) => {
                    last = Some(e);
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(last.map(StorageError::from).unwrap_or(StorageError::Budget))
    }

    /// One metered ranged GET with bounded retries on transient failures.
    pub async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes, StorageError> {
        let len = i64::try_from(range.end.saturating_sub(range.start)).unwrap_or(i64::MAX);
        let store = &self.store;
        self.attempts(Charge::Read(len), Duration::from_millis(250), move || {
            store.get_range(key, range.clone())
        })
        .await
    }

    /// One metered HEAD (metadata retrieval is a simple op) with bounded retries.
    pub async fn head(&self, key: &str) -> Result<ArtifactHead, StorageError> {
        let store = &self.store;
        self.attempts(Charge::Read(0), Duration::from_millis(250), move || {
            store.head(key)
        })
        .await
    }

    /// Publishes a bundle with bounded retries. A metered store (Vercel Blob) reserves every
    /// attempt first, retries included, against the global monthly upload allowance and the
    /// monthly share of `workspace` (the job's submitter); a spent allowance or a failed
    /// reservation refuses the upload (fail closed). Other stores count uploads at completion.
    pub async fn put(
        &self,
        key: &str,
        path: &Path,
        workspace: Option<uuid::Uuid>,
    ) -> Result<(), StorageError> {
        let store = &self.store;
        self.attempts(
            Charge::Upload(workspace),
            Duration::from_millis(500),
            move || async move {
                match store.put_bundle_from_file(key, path).await {
                    // A previous attempt may have landed before its response was lost.
                    Ok(_) | Err(ArtifactError::AlreadyExists) => Ok(()),
                    Err(e) => Err(e),
                }
            },
        )
        .await
    }

    /// Deletes an object (garbage collection; Blob deletes are not billed operations).
    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        Ok(self.store.delete(key).await?)
    }
}

/// Parsed runs kept in memory for paged whole-run reads (timeline cursor pages).
const PARSED_RUNS: usize = 2;
/// Byte budget of the parsed runs (estimated by their decoded section bytes); a run above it is
/// never retained.
const PARSED_BYTES: u64 = 64 << 20;

/// One in-flight whole-run decode, shared by concurrent misses on the same bundle.
type ParsedLoad = Arc<tokio::sync::OnceCell<(Arc<RunResult>, u64)>>;

/// Count- and byte-bounded LRU (most recently used last).
struct ParsedLru<T> {
    entries: Vec<(String, T, u64)>,
    bytes: u64,
}

impl<T: Clone> ParsedLru<T> {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
        }
    }

    fn get(&mut self, key: &str) -> Option<T> {
        let i = self.entries.iter().position(|(k, _, _)| k == key)?;
        let hit = self.entries.remove(i);
        let value = hit.1.clone();
        self.entries.push(hit);
        Some(value)
    }

    fn insert(&mut self, key: &str, value: T, bytes: u64) {
        if bytes > PARSED_BYTES || self.entries.iter().any(|(k, _, _)| k == key) {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= PARSED_RUNS || self.bytes + bytes > PARSED_BYTES)
        {
            let (_, _, b) = self.entries.remove(0);
            self.bytes -= b;
        }
        self.bytes += bytes;
        self.entries.push((key.to_string(), value, bytes));
    }
}

/// Byte-bounded LRU of verified bundle files, plus the last few parsed runs.
pub struct BundleCache {
    dir: PathBuf,
    capacity: u64,
    entries: Mutex<HashMap<String, (u64, u64)>>,
    clock: std::sync::atomic::AtomicU64,
    /// Most recently used last, keyed by bundle sha256 (content-addressed, so never stale).
    parsed: Mutex<ParsedLru<Arc<RunResult>>>,
    /// Whole-run decodes in progress, keyed by bundle sha256 (single-flight).
    loading: Mutex<HashMap<String, ParsedLoad>>,
    /// Store fetches in progress, keyed by bundle sha256: concurrent cold reads of one bundle
    /// wait for the first fetch instead of downloading it again.
    fetching: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Every active or waiting reader owns one reference. The last reader removes the map's
/// reference on success, failure or cancellation, without letting a new reader get a second gate.
struct FetchFlight<'a> {
    cache: &'a BundleCache,
    sha: &'a str,
    gate: Option<Arc<tokio::sync::Mutex<()>>>,
}

impl Drop for FetchFlight<'_> {
    fn drop(&mut self) {
        let gate = self.gate.take().expect("flight owns its gate until drop");
        let mut fetching = self
            .cache
            .fetching
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if Arc::strong_count(&gate) == 2
            && fetching
                .get(self.sha)
                .is_some_and(|entry| Arc::ptr_eq(entry, &gate))
        {
            fetching.remove(self.sha);
        }
        // Decrement under the same lock as acquisition and other readers' drops: otherwise two
        // simultaneous cancellations can both observe another reader and leave the map behind.
        drop(gate);
    }
}

impl BundleCache {
    /// Starts empty: whatever an earlier process left on disk is discarded (not trusted).
    pub fn new(data_dir: &Path, capacity: u64) -> std::io::Result<Self> {
        let dir = data_dir.join("cache").join("bundles");
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
            clock: Default::default(),
            parsed: Mutex::new(ParsedLru::new()),
            loading: Mutex::new(HashMap::new()),
            fetching: Mutex::new(HashMap::new()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Active or waiting cold bundle fetches (diagnostic for cancellation regressions).
    #[doc(hidden)]
    pub fn fetching_len(&self) -> usize {
        self.fetching
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn path(&self, sha256: &str) -> PathBuf {
        self.dir.join(format!("{sha256}.bbz"))
    }

    fn touch(&self, sha256: &str) -> Option<PathBuf> {
        let tick = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = entries.get_mut(sha256)?;
        entry.1 = tick;
        let path = self.path(sha256);
        path.is_file().then_some(path)
    }

    fn forget(&self, sha256: &str) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(sha256);
        let _ = std::fs::remove_file(self.path(sha256));
    }

    /// Moves a verified file into the cache (evicting least recently used entries).
    pub fn insert(&self, sha256: &str, file: &Path, size: u64) -> std::io::Result<PathBuf> {
        let target = self.path(sha256);
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if size > self.capacity {
            return Err(std::io::Error::other("bundle larger than the cache"));
        }
        let mut used: u64 = entries.values().map(|e| e.0).sum();
        while used + size > self.capacity {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, e)| e.1)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((s, _)) = entries.remove(&oldest) {
                used -= s;
            }
            let _ = std::fs::remove_file(self.path(&oldest));
        }
        if std::fs::rename(file, &target).is_err() {
            std::fs::copy(file, &target)?;
            let _ = std::fs::remove_file(file);
        }
        let tick = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        entries.insert(sha256.to_string(), (size, tick));
        Ok(target)
    }
}

/// Limits for reading bundles written under the server's profile.
pub fn bundle_limits(max_output_bytes: u64) -> BundleLimits {
    let mut limits = BundleLimits::default();
    if max_output_bytes < limits.max_output_bytes {
        // The stored, compressed bundle is never larger than the serialized result bound.
        limits.max_output_bytes = max_output_bytes;
    }
    limits
}

/// The trust root of a run's bundle, checked against the database rows (outer digest first).
pub fn trust_of(view: &RunView, artifact: &RunArtifact) -> Result<BundleTrust, &'static str> {
    if crate::canonical::sha256(&artifact.page_index).as_slice() != artifact.page_index_sha256 {
        return Err("bundle");
    }
    let trust: BundleTrust =
        serde_json::from_value(artifact.page_index.clone()).map_err(|_| "bundle")?;
    let run = &view.run;
    let consistent = trust.result_sha256 == hex::encode(&run.result_sha256)
        && trust.summary_sha256 == hex::encode(&run.summary_sha256)
        && trust.sha256 == hex::encode(&artifact.sha256)
        && i64::try_from(trust.size_bytes).ok() == Some(artifact.size_bytes)
        && trust.fingerprint == hex::encode(&view.fingerprint)
        && trust.platform == run.platform
        && i64::try_from(trust.counts.orders).ok() == Some(run.order_count)
        && i64::try_from(trust.counts.fills).ok() == Some(run.fill_count)
        && i64::try_from(trust.counts.tape).ok() == Some(run.tape_len)
        && i64::try_from(trust.counts.equity).ok() == Some(run.equity_len)
        && i64::try_from(trust.counts.ticks).ok() == Some(run.ticks);
    if consistent { Ok(trust) } else { Err("bundle") }
}

/// A run whose bundle is verified and cached locally.
pub struct OpenRun {
    pub view: RunView,
    pub artifact: RunArtifact,
    pub trust: BundleTrust,
    pub limits: BundleLimits,
    path: PathBuf,
    cache: Arc<BundleCache>,
    storage: Storage,
    db: Database,
    metrics: Arc<Metrics>,
}

/// Opens (fetching and verifying if needed) the bundle of a visible run.
pub async fn open_run(
    db: &Database,
    storage: &Storage,
    cache: &Arc<BundleCache>,
    metrics: &Arc<Metrics>,
    limits: BundleLimits,
    view: RunView,
) -> Result<OpenRun, ApiError> {
    let artifact = db.artifact(view.run.id).await?;
    let trust = match trust_of(&view, &artifact) {
        Ok(trust) => trust,
        Err(object) => {
            return Err(quarantine(
                db,
                metrics,
                &view,
                object,
                &hex::encode(&artifact.page_index_sha256),
            )
            .await);
        }
    };
    let sha = trust.sha256.clone();
    let path = match cache.touch(&sha) {
        Some(path) => {
            Metrics::inc(&metrics.artifact_cache_hits_total);
            path
        }
        None => {
            // Single-flight: the first cold reader fetches; the others wait for it and then find
            // the bundle cached (a failed fetch leaves it uncached, so the next waiter retries).
            let flight = FetchFlight {
                cache,
                sha: &sha,
                gate: Some(
                    cache
                        .fetching
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .entry(sha.clone())
                        .or_default()
                        .clone(),
                ),
            };
            let fetched = {
                let _turn = flight.gate.as_ref().expect("live flight").lock().await;
                match cache.touch(&sha) {
                    Some(path) => {
                        Metrics::inc(&metrics.artifact_cache_hits_total);
                        Ok(path)
                    }
                    None => {
                        Metrics::inc(&metrics.artifact_cache_misses_total);
                        fetch_verified(
                            db, storage, cache, metrics, &view, &artifact, &trust, limits,
                        )
                        .await
                    }
                }
            };
            fetched?
        }
    };
    Ok(OpenRun {
        view,
        artifact,
        trust,
        limits,
        path,
        cache: cache.clone(),
        storage: storage.clone(),
        db: db.clone(),
        metrics: metrics.clone(),
    })
}

/// The verified bundle trailer (manifest, metrics, counts) of a visible run without fetching or
/// decoding any page: from the cached bundle if present, else from two ranged reads of the
/// stored header and trailer, both checked against the trust root in Postgres.
pub async fn verified_trailer(
    db: &Database,
    storage: &Storage,
    cache: &Arc<BundleCache>,
    metrics: &Arc<Metrics>,
    limits: BundleLimits,
    view: &RunView,
) -> Result<BundleTrailerCore, ApiError> {
    let artifact = db.artifact(view.run.id).await?;
    let trust = match trust_of(view, &artifact) {
        Ok(trust) => trust,
        Err(object) => {
            let expected = hex::encode(&artifact.page_index_sha256);
            return Err(quarantine(db, metrics, view, object, &expected).await);
        }
    };
    if let Some(path) = cache.touch(&trust.sha256) {
        Metrics::inc(&metrics.artifact_cache_hits_total);
        let open = OpenRun {
            view: view.clone(),
            artifact,
            trust,
            limits,
            path,
            cache: cache.clone(),
            storage: storage.clone(),
            db: db.clone(),
            metrics: metrics.clone(),
        };
        return open.trailer_core().await;
    }
    let mut members = Vec::with_capacity(2);
    for member in [&trust.header, &trust.trailer] {
        let range = member
            .range()
            .map_err(|e| ApiError::internal(format!("bundle index: {e}")))?;
        match storage.get_range(&artifact.storage_key, range).await {
            Ok(bytes) => members.push(bytes),
            Err(StorageError::Artifact(ArtifactError::NotFound)) => {
                tracing::error!(run = %view.short_id, "bundle object missing from the store");
                return Err(ApiError::unavailable("storage", 60));
            }
            Err(e) => return Err(e.into_api()),
        }
    }
    let checked = BundleReader::new(&trust, limits)
        .and_then(|reader| reader.check_metadata(&members[0], &members[1]));
    match checked {
        Ok(trailer) => Ok(trailer),
        Err(ArtifactError::Integrity(e)) => {
            tracing::error!(run = %view.short_id, error = %e, "bundle metadata failed verification");
            Err(quarantine(db, metrics, view, "bundle", &trust.sha256).await)
        }
        Err(e) => Err(ApiError::internal(format!("bundle metadata: {e}"))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn fetch_verified(
    db: &Database,
    storage: &Storage,
    cache: &Arc<BundleCache>,
    metrics: &Arc<Metrics>,
    view: &RunView,
    artifact: &RunArtifact,
    trust: &BundleTrust,
    limits: BundleLimits,
) -> Result<PathBuf, ApiError> {
    let mut temp = tempfile::NamedTempFile::new_in(cache.dir())
        .map_err(|e| ApiError::internal(format!("cache temp file: {e}")))?;
    let mut offset = 0;
    while offset < trust.size_bytes {
        let end = (offset + FETCH_CHUNK).min(trust.size_bytes);
        let bytes = match storage.get_range(&artifact.storage_key, offset..end).await {
            Ok(bytes) => bytes,
            // A missing object is a storage fault (or another instance's local store), not
            // proof of tampering: answer 503 instead of quarantining a valid run.
            Err(StorageError::Artifact(ArtifactError::NotFound)) => {
                tracing::error!(run = %view.short_id, "bundle object missing from the store");
                return Err(ApiError::unavailable("storage", 60));
            }
            Err(e) => return Err(e.into_api()),
        };
        temp.write_all(&bytes)
            .map_err(|e| ApiError::internal(format!("cache write: {e}")))?;
        offset = end;
    }
    temp.flush()
        .map_err(|e| ApiError::internal(format!("cache flush: {e}")))?;
    let trust_copy = trust.clone();
    let temp_path = temp.into_temp_path();
    let checked = temp_path.to_path_buf();
    let verified = tokio::task::spawn_blocking(move || {
        BundleReader::new(&trust_copy, limits)?.verify_file(&checked)
    })
    .await
    .map_err(|e| ApiError::internal(format!("verifier panicked: {e}")))?;
    match verified {
        Ok(trailer) => {
            if trailer.manifest.result_sha256 != hex::encode(&view.run.result_sha256) {
                return Err(quarantine(db, metrics, view, "bundle", &trust.sha256).await);
            }
        }
        Err(ArtifactError::Integrity(e)) => {
            tracing::error!(run = %view.short_id, error = %e, "bundle failed verification");
            return Err(quarantine(db, metrics, view, "bundle", &trust.sha256).await);
        }
        Err(e) => return Err(ApiError::internal(format!("verify bundle: {e}"))),
    }
    let kept = temp_path
        .keep()
        .map_err(|e| ApiError::internal(format!("cache keep: {e}")))?;
    cache
        .insert(&trust.sha256, &kept, trust.size_bytes)
        .map_err(|e| ApiError::internal(format!("cache insert: {e}")))
}

/// Quarantines a run after a failed verification and returns the `integrity_failed` error.
pub async fn quarantine(
    db: &Database,
    metrics: &Metrics,
    view: &RunView,
    object: &str,
    expected: &str,
) -> ApiError {
    Metrics::inc(&metrics.integrity_failures_total);
    let reason = format!("{object} failed verification on read");
    match db.quarantine_run(view.run.id, &reason).await {
        Ok(()) => Metrics::inc(&metrics.runs_quarantined_total),
        Err(e) => tracing::error!(run = %view.short_id, error = %e, "quarantine failed"),
    }
    ApiError::integrity_failed(&view.short_id, object, expected)
}

impl OpenRun {
    /// Runs `read` against the cached file. A failure of the cached copy discards it and
    /// fetches the bundle again once; a second failure quarantines the run.
    pub async fn read<T, F>(&self, read: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: Fn(&BundleReader<'_>, &Path) -> Result<T, ArtifactError> + Send + Sync + Clone + 'static,
    {
        let first = self.attempt(&self.path, read.clone()).await?;
        match first {
            Ok(value) => Ok(value),
            Err(ArtifactError::Integrity(e)) => {
                tracing::warn!(run = %self.view.short_id, error = %e, "cached bundle failed verification; refetching");
                self.cache.forget(&self.trust.sha256);
                let path = fetch_verified(
                    &self.db,
                    &self.storage,
                    &self.cache,
                    &self.metrics,
                    &self.view,
                    &self.artifact,
                    &self.trust,
                    self.limits,
                )
                .await?;
                match self.attempt(&path, read).await? {
                    Ok(value) => Ok(value),
                    Err(ArtifactError::Integrity(_)) => Err(quarantine(
                        &self.db,
                        &self.metrics,
                        &self.view,
                        "page",
                        &self.trust.sha256,
                    )
                    .await),
                    Err(e) => Err(ApiError::internal(format!("bundle read: {e}"))),
                }
            }
            Err(e) => Err(ApiError::internal(format!("bundle read: {e}"))),
        }
    }

    async fn attempt<T, F>(
        &self,
        path: &Path,
        read: F,
    ) -> Result<Result<T, ArtifactError>, ApiError>
    where
        T: Send + 'static,
        F: Fn(&BundleReader<'_>, &Path) -> Result<T, ArtifactError> + Send + 'static,
    {
        let trust = self.trust.clone();
        let limits = self.limits;
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let reader = BundleReader::new(&trust, limits)?;
            read(&reader, &path)
        })
        .await
        .map_err(|e| ApiError::internal(format!("bundle reader panicked: {e}")))
    }

    /// The complete verified trailer, including the analytics report.
    pub async fn trailer(&self) -> Result<BundleTrailer, ApiError> {
        self.read(|reader, path| reader.trailer(path)).await
    }

    /// The verified trailer without the analytics report (manifest, metrics, engine check).
    pub async fn trailer_core(&self) -> Result<BundleTrailerCore, ApiError> {
        self.read(|reader, path| reader.trailer_core(path)).await
    }

    /// Page numbers (indices into the trust page index) of one section.
    pub fn pages(&self, section: Section) -> Vec<(usize, u64, u64)> {
        self.trust
            .page_index
            .iter()
            .enumerate()
            .filter(|(_, p)| p.section == section)
            .map(|(i, p)| (i, p.first_row, p.rows))
            .collect()
    }

    pub fn page_index(&self) -> &[bb_artifacts::PageIndex] {
        &self.trust.page_index
    }

    /// Verified rows of one page.
    pub async fn page<T: DeserializeOwned + Send + 'static>(
        &self,
        page_no: usize,
    ) -> Result<Vec<T>, ApiError> {
        self.read(move |reader, path| {
            let bytes = reader.page(path, page_no)?;
            parse_rows(&bytes)
        })
        .await
    }

    /// The complete run (for comparisons, timelines, reports and analytics). Bounded by the
    /// profile's output limit, and only used behind the heavy-read concurrency limit.
    pub async fn full(&self) -> Result<RunResult, ApiError> {
        Ok(self.full_sized().await?.0)
    }

    /// [`Self::full`] and its decoded section bytes (the in-memory size estimate).
    async fn full_sized(&self) -> Result<(RunResult, u64), ApiError> {
        let sections: Vec<Section> = self.trust.page_index.iter().map(|p| p.section).collect();
        self.read(move |reader, path| {
            let mut orders: Vec<OrderRecord> = Vec::new();
            let mut fills: Vec<FillRecord> = Vec::new();
            let mut equity: Vec<EquitySample> = Vec::new();
            let mut tape: Vec<FlightEvent> = Vec::new();
            let mut decoded: u64 = 0;
            let all: Vec<usize> = (0..sections.len()).collect();
            let trailer = reader.trailer_and_pages(path, &all, |page_no, bytes| {
                decoded += bytes.len() as u64;
                match sections[page_no] {
                    Section::Orders => orders.extend(parse_rows::<OrderRecord>(&bytes)?),
                    Section::Fills => fills.extend(parse_rows::<FillRecord>(&bytes)?),
                    Section::Equity => equity.extend(parse_rows::<EquitySample>(&bytes)?),
                    Section::Tape => tape.extend(parse_rows::<FlightEvent>(&bytes)?),
                }
                Ok(())
            })?;
            Ok((
                RunResult {
                    manifest: trailer.manifest,
                    metrics: trailer.metrics,
                    engine_check: trailer.engine_check,
                    ticks: trailer.counts.ticks,
                    orders,
                    fills,
                    equity,
                    tape,
                },
                decoded,
            ))
        })
        .await
    }

    /// [`Self::full`], shared through a small in-memory cache keyed by the bundle digest, so
    /// paging over one run (timeline cursor pages) decodes it once rather than per page.
    pub async fn full_shared(&self) -> Result<Arc<RunResult>, ApiError> {
        let sha = &self.trust.sha256;
        let cache = &self.cache;
        if let Some(run) = cache
            .parsed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sha)
        {
            return Ok(run);
        }
        let load = cache
            .loading
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(sha.clone())
            .or_default()
            .clone();
        let result = load
            .get_or_try_init(|| async {
                let (run, bytes) = self.full_sized().await?;
                let run = Arc::new(run);
                cache
                    .parsed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(sha, run.clone(), bytes);
                Ok::<_, ApiError>((run, bytes))
            })
            .await
            .map(|(run, _)| run.clone());
        let mut loading = cache.loading.lock().unwrap_or_else(PoisonError::into_inner);
        if loading.get(sha).is_some_and(|l| Arc::ptr_eq(l, &load)) {
            loading.remove(sha);
        }
        result
    }

    /// Path of the verified cached bundle (export).
    pub fn cached_path(&self) -> &Path {
        &self.path
    }
}

fn parse_rows<T: DeserializeOwned>(bytes: &[u8]) -> Result<Vec<T>, ArtifactError> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(ArtifactError::from))
        .collect()
}

#[cfg(test)]
mod parsed_tests {
    use super::{PARSED_BYTES, PARSED_RUNS, ParsedLru};

    #[test]
    fn parsed_runs_are_byte_bounded() {
        let mut lru = ParsedLru::new();
        lru.insert("big", (), PARSED_BYTES + 1);
        assert_eq!(
            lru.get("big"),
            None,
            "a run above the budget is never retained"
        );
        lru.insert("a", (), PARSED_BYTES / 2);
        lru.insert("b", (), PARSED_BYTES / 2);
        assert!(lru.get("a").is_some() && lru.get("b").is_some());
        // Over budget: the least recently used (`a`) is evicted.
        lru.insert("c", (), 1);
        assert_eq!(lru.get("a"), None);
        assert!(lru.entries.len() <= PARSED_RUNS && lru.bytes <= PARSED_BYTES);
    }

    #[test]
    fn parsed_runs_evict_by_count_and_refresh_recency() {
        let mut lru = ParsedLru::new();
        lru.insert("a", 1, 1);
        lru.insert("b", 2, 1);
        assert_eq!(lru.get("a"), Some(1));
        lru.insert("c", 3, 1);
        assert_eq!(lru.get("b"), None);
        assert_eq!(lru.get("a"), Some(1));
        assert_eq!(lru.get("c"), Some(3));
        assert_eq!(lru.entries.len(), PARSED_RUNS);
        assert_eq!(lru.bytes, 2);
    }

    #[test]
    fn parsed_run_byte_eviction_drops_owned_values() {
        use std::sync::Arc;

        let mut lru = ParsedLru::new();
        let a = Arc::new(1);
        let b = Arc::new(2);
        lru.insert("a", a.clone(), PARSED_BYTES / 2 + 1);
        lru.insert("b", b.clone(), PARSED_BYTES / 2);
        assert_eq!(Arc::strong_count(&a), 1, "eviction releases the cached run");
        assert_eq!(lru.bytes, PARSED_BYTES / 2);
        lru.insert("too-big", a.clone(), PARSED_BYTES + 1);
        assert_eq!(Arc::strong_count(&a), 1);
        assert_eq!(Arc::strong_count(&b), 2);
        assert!(lru.get("b").is_some());
    }
}
