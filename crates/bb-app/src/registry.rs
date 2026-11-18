//! Dataset registry: dataset versions live in Postgres (`bb.dataset_versions`, the trust root);
//! their event files are local, ephemeral copies under `<data-dir>/registry/<sha prefix>/`.
//!
//! A synthetic version (`storage = 'regenerate'`) is rebuilt deterministically from
//! `manifest.generator` on first use and verified against `events_sha256` (no object storage, no
//! Blob operations). A sweep's time-window slice (`generator.slice`, `bb_data::slice`) is
//! `regenerate` too: its parent is rebuilt (or loaded) and verified first, then sliced. A local
//! version (`storage = 'fs'`) is loaded from `<data-dir>/datasets` and verified the same way. A
//! version whose bytes do not match is never used.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use bb_core::{
    dataset::{DatasetKind, DatasetManifest},
    sweep::TimeWindow,
};
use bb_data::{LoadedDataset, slice::SliceProvenance, synthetic::SyntheticConfig};
use bb_db::{Database, DatasetVersion, NewDataSource, NewDataset, NewDatasetVersion};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::{datasets::DatasetCtx, error::AppError};

/// Data source of every synthetic dataset (seeded by migration 0005).
pub const SYNTHETIC_SOURCE: &str = "blackbox-synthetic";
/// Data source of locally ingested recorded data (never hostable).
pub const LOCAL_SOURCE: &str = "local-recorded";
/// Version of the quality checker whose reports are stored at registration.
pub const QUALITY_CHECKER_VERSION: i32 = 1;

pub struct DatasetRegistry {
    root: PathBuf,
    local_datasets: PathBuf,
    loaded: Mutex<HashMap<Uuid, Arc<DatasetCtx>>>,
    loading: AsyncMutex<()>,
}

/// The bb-core manifest stored with a version.
pub fn manifest_of(version: &DatasetVersion) -> Result<DatasetManifest, AppError> {
    serde_json::from_value(version.manifest.clone()).map_err(|e| {
        AppError::Internal(format!(
            "dataset version {} has an unreadable manifest: {e}",
            version.id
        ))
    })
}

impl DatasetRegistry {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("registry"),
            local_datasets: data_dir.join("datasets"),
            loaded: Mutex::new(HashMap::new()),
            loading: AsyncMutex::new(()),
        }
    }

    /// The loaded, hash-verified dataset of `version`.
    pub async fn load(&self, version: &DatasetVersion) -> Result<Arc<DatasetCtx>, AppError> {
        if let Some(ctx) = self.cached(version.id) {
            return Ok(ctx);
        }
        // One load at a time: a hosted instance has room for one dataset build, not several.
        let _guard = self.loading.lock().await;
        if let Some(ctx) = self.cached(version.id) {
            return Ok(ctx);
        }
        let id = version.id;
        let version = version.clone();
        let root = self.root.clone();
        let local = self.local_datasets.clone();
        let ctx = tokio::task::spawn_blocking(move || {
            let manifest = manifest_of(&version)?;
            let loaded = build_or_load(
                &root,
                &local,
                &version.dataset_id,
                &hex::encode(&version.events_sha256),
                &version.storage,
                manifest.generator.as_ref(),
            )?;
            Ok::<_, AppError>(DatasetCtx::new(loaded))
        })
        .await
        .map_err(|e| AppError::Internal(format!("dataset loader panicked: {e}")))??;
        let ctx = Arc::new(ctx);
        self.remember(id, ctx.clone());
        Ok(ctx)
    }

    fn cached(&self, id: Uuid) -> Option<Arc<DatasetCtx>> {
        self.loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    /// Where the event file of a version hashing to `events_sha256` (hex) lives.
    fn version_root(root: &Path, events_sha256: &str) -> PathBuf {
        root.join(&events_sha256[..16.min(events_sha256.len())])
    }

    /// Stores an already loaded dataset under its version (registration path).
    fn remember(&self, id: Uuid, ctx: Arc<DatasetCtx>) {
        self.loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, ctx);
    }
}

/// The verified dataset `id` whose events hash to `expected` (hex): loaded from disk, or rebuilt
/// from `generator` when `storage` is `regenerate` and it is not on disk yet. A slice rebuilds
/// its parent the same way first (recursively, so a slice of a slice works).
fn build_or_load(
    root: &Path,
    local_datasets: &Path,
    id: &str,
    expected: &str,
    storage: &str,
    generator: Option<&serde_json::Value>,
) -> Result<LoadedDataset, AppError> {
    let (dir, regenerate) = match storage {
        "regenerate" => (DatasetRegistry::version_root(root, expected), true),
        "fs" => (local_datasets.to_path_buf(), false),
        other => {
            return Err(AppError::Internal(format!(
                "dataset storage `{other}` is not supported by this server"
            )));
        }
    };
    if !dir.join(id).join("dataset.json").is_file() {
        if !regenerate {
            return Err(AppError::NotFound(format!(
                "dataset '{id}' (events sha256 {expected}) is not available on this server"
            )));
        }
        let generator = generator.ok_or_else(|| {
            AppError::Internal(format!("dataset '{id}' has no generator parameters"))
        })?;
        let built = match generator.get("slice") {
            Some(slice) => {
                let p: SliceProvenance = serde_json::from_value(slice.clone()).map_err(|e| {
                    AppError::Internal(format!("dataset '{id}' slice provenance: {e}"))
                })?;
                // A parent without generator parameters is a local (`fs`) dataset.
                let parent_storage = if p.parent_generator.is_some() {
                    "regenerate"
                } else {
                    "fs"
                };
                let parent = build_or_load(
                    root,
                    local_datasets,
                    &p.parent_id,
                    &p.parent_events_sha256,
                    parent_storage,
                    p.parent_generator.as_ref(),
                )?;
                bb_data::slice::slice(&parent, p.window)?
            }
            None => {
                let config: SyntheticConfig =
                    serde_json::from_value(generator.clone()).map_err(|e| {
                        AppError::Internal(format!("dataset '{id}' generator parameters: {e}"))
                    })?;
                bb_data::synthetic::generate(&config)?
            }
        };
        std::fs::create_dir_all(&dir)?;
        bb_data::store::save(&dir, &built)?;
    }
    // `load` verifies the event file against the manifest's hash; the manifest must in turn
    // carry the hash the database recorded.
    let loaded = bb_data::store::load(&dir, id)?;
    if loaded.manifest.events_sha256 != expected {
        return Err(AppError::Internal(format!(
            "dataset '{id}' on disk has events sha256 {}, the registered version has {expected}",
            loaded.manifest.events_sha256
        )));
    }
    Ok(loaded)
}

/// Registers (or finds) the slice of `parent` covering `window`: a derived global dataset with
/// `storage = 'regenerate'` whose manifest carries its provenance (`generator.slice`), so any
/// worker can rebuild it from the parent. Its id is `TimeWindow::slice_dataset_id`, a pure
/// function of the parent and the window, and so is its content.
pub async fn register_slice(
    db: &Database,
    registry: &DatasetRegistry,
    parent: &DatasetVersion,
    window: TimeWindow,
    hosted: bool,
) -> Result<DatasetVersion, AppError> {
    let parent_sha = hex::encode(&parent.events_sha256);
    let id = window.slice_dataset_id(&parent.dataset_id, &parent_sha);
    match db.current_version(&id).await {
        Ok(existing) => {
            let same = SliceProvenance::of(&manifest_of(&existing)?).is_some_and(|p| {
                p.parent_id == parent.dataset_id
                    && p.parent_events_sha256 == parent_sha
                    && p.window == window
            });
            if same {
                return Ok(existing);
            }
            return Err(AppError::Internal(format!(
                "dataset '{id}' exists but is not the slice {window:?} of '{}'",
                parent.dataset_id
            )));
        }
        Err(bb_db::Error::NotFound) => {}
        Err(e) => return Err(db_error(e)),
    }
    let parent_ctx = registry.load(parent).await?;
    let root = registry.root.clone();
    let built = tokio::task::spawn_blocking(move || -> Result<DatasetCtx, AppError> {
        let built = bb_data::slice::slice(&parent_ctx.dataset, window)?;
        let staging = root
            .join("staging")
            .join(Uuid::new_v4().simple().to_string());
        let manifest = bb_data::store::save(&staging, &built)?;
        // Move the saved slice to where `build_or_load` looks for it; a concurrent
        // registration of the same slice may have put identical bytes there first.
        let target = DatasetRegistry::version_root(&root, &manifest.events_sha256);
        if !target.join(&manifest.id).join("dataset.json").is_file() {
            std::fs::create_dir_all(&target)?;
            let _ = std::fs::rename(staging.join(&manifest.id), target.join(&manifest.id));
        }
        let _ = std::fs::remove_dir_all(&staging);
        Ok(DatasetCtx::new(bb_data::store::load(
            &target,
            &manifest.id,
        )?))
    })
    .await
    .map_err(|e| AppError::Internal(format!("dataset slicer panicked: {e}")))??;
    let version = register(db, built.manifest(), "regenerate", hosted).await?;
    registry.remember(version.id, Arc::new(built));
    Ok(version)
}

/// Everything needed to register one dataset version.
pub fn new_version(
    manifest: &DatasetManifest,
    version_no: i32,
    storage: &str,
) -> Result<NewDatasetVersion, AppError> {
    let events_sha256 = hex::decode(&manifest.events_sha256)
        .map_err(|_| AppError::Internal("manifest events_sha256 is not hex".into()))?;
    let origin_ns: i64 = manifest
        .origin_ns
        .parse()
        .map_err(|_| AppError::Internal("manifest origin_ns is not an integer".into()))?;
    Ok(NewDatasetVersion {
        dataset_id: manifest.id.clone(),
        version_no,
        events_sha256,
        normalization_version: 1,
        contract_version: i32::try_from(manifest.contract_version).unwrap_or(1),
        tick_size: manifest.instrument.tick_size,
        lot_size: manifest.instrument.lot_size,
        contract_size: manifest.instrument.contract_size,
        origin_ns,
        start_rel_ns: manifest.start,
        end_rel_ns: manifest.end,
        timestamp_unit: "ns".into(),
        row_count: i64::try_from(manifest.counts.rows).unwrap_or(i64::MAX),
        events_bytes: i64::try_from(manifest.events_bytes).unwrap_or(i64::MAX),
        storage: storage.into(),
        storage_key: (storage != "regenerate").then(|| manifest.id.clone()),
        manifest: serde_json::to_value(manifest)?,
    })
}

/// Registers a dataset version (idempotent) with its quality report. `storage` is `regenerate`
/// for synthetic data and `fs` for a dataset under `<data-dir>/datasets`. The hosted profile
/// refuses sources that are not hostable (SEC-35). A refusal and a newly inserted version are
/// audited as `dataset.create` (SEC-25, SECURITY_REVIEW §15); the success row commits in the
/// version insert's transaction. Inside an API request (a sweep's derived window slice) the row
/// carries the caller's key and request id with `actor = "api"`, otherwise `actor = "cli"`.
pub async fn register(
    db: &Database,
    manifest: &DatasetManifest,
    storage: &str,
    hosted: bool,
) -> Result<DatasetVersion, AppError> {
    let synthetic = matches!(manifest.kind, DatasetKind::Synthetic);
    let refusal = if hosted && !synthetic {
        Some("source is not hostable")
    } else if hosted
        && manifest.counts.rows > bb_core::limits::ExecutionLimits::hosted().max_dataset_rows
    {
        Some("hosted dataset exceeds the 500000-row limit")
    } else {
        None
    };
    let request = crate::v1::auth::request_actor();
    let actor = if request.is_some() { "api" } else { "cli" };
    let audit = bb_db::Audit {
        // Datasets are global: the row names no workspace, the key identifies the actor.
        workspace: None,
        key: request.and_then(|r| r.key),
        request_id: request.map(|_| crate::v1::error::request_id()),
    };
    if let Some(reason) = refusal {
        db.audit(bb_db::NewAuditEvent {
            workspace: None,
            key: audit.key,
            action: "dataset.create",
            outcome: "denied",
            target_type: Some("dataset"),
            target_id: Some(&manifest.id),
            request_id: audit.request_id.as_deref(),
            client_ip: None,
            detail: &serde_json::json!({ "actor": actor, "reason": reason }),
        })
        .await
        .map_err(db_error)?;
        return Err(AppError::InvalidRequest(reason.into()));
    }
    let source = if synthetic {
        SYNTHETIC_SOURCE
    } else {
        db.register_source(NewDataSource {
            id: LOCAL_SOURCE,
            vendor: "local operator",
            licence: "see dataset licence note",
            licence_url: None,
            redistribution: false,
            hostable: false,
            notes: "Recorded market data ingested locally; never hosted.",
        })
        .await
        .map_err(db_error)?;
        LOCAL_SOURCE
    };
    let instrument = db
        .register_instrument(
            &manifest.source.exchange,
            &manifest.source.symbol,
            if synthetic { "synthetic" } else { "perpetual" },
        )
        .await
        .map_err(db_error)?;
    db.register_dataset(NewDataset {
        id: &manifest.id,
        owner: None,
        title: &manifest.title,
        kind: if synthetic { "synthetic" } else { "recorded" },
        instrument: instrument.id,
        source,
        hosted,
    })
    .await
    .map_err(|e| match e {
        bb_db::Error::Invalid(m) => AppError::InvalidRequest(m),
        other => db_error(other),
    })?;
    let version_no = match db.current_version(&manifest.id).await {
        Ok(current) if hex::encode(&current.events_sha256) == manifest.events_sha256 => {
            current.version_no
        }
        Ok(current) => current.version_no + 1,
        Err(bb_db::Error::NotFound) => 1,
        Err(e) => return Err(db_error(e)),
    };
    let version = db
        .register_version_audited(
            &new_version(manifest, version_no, storage)?,
            &audit,
            serde_json::json!({
                "actor": actor,
                "version_no": version_no,
                "events_sha256": manifest.events_sha256,
                "kind": if synthetic { "synthetic" } else { "recorded" },
                "storage": storage,
                "hosted": hosted,
            }),
        )
        .await
        .map_err(db_error)?;
    let report = serde_json::to_value(&manifest.quality)?;
    let errors = manifest
        .quality
        .checks
        .iter()
        .filter(|c| !c.passed && c.severity == bb_core::dataset::Severity::Error)
        .count();
    let warnings = manifest
        .quality
        .checks
        .iter()
        .filter(|c| !c.passed && c.severity == bb_core::dataset::Severity::Warn)
        .count();
    db.put_quality_report(&bb_db::QualityReport {
        dataset_version_id: version.id,
        checker_version: QUALITY_CHECKER_VERSION,
        passed: errors == 0,
        error_count: errors as i32,
        warning_count: warnings as i32,
        book_valid_fraction: manifest.quality.book_valid_fraction.clamp(0.0, 1.0),
        report_sha256: Sha256::digest(crate::canonical::to_vec(&report)).to_vec(),
        report,
        created_at: chrono::Utc::now(),
    })
    .await
    .map_err(db_error)?;
    Ok(version)
}

/// Generates (or reuses) the synthetic dataset `config` under the registry and registers it.
pub async fn register_synthetic(
    db: &Database,
    registry: &DatasetRegistry,
    config: &SyntheticConfig,
    hosted: bool,
) -> Result<(DatasetVersion, Arc<DatasetCtx>), AppError> {
    let config = config.clone();
    let scratch = registry.root.join("staging");
    let built = tokio::task::spawn_blocking(move || -> Result<DatasetCtx, AppError> {
        let built = bb_data::synthetic::generate(&config)?;
        let dir = scratch.join(&config.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&scratch)?;
        bb_data::store::save(&scratch, &built)?;
        Ok(DatasetCtx::new(bb_data::store::load(&scratch, &config.id)?))
    })
    .await
    .map_err(|e| AppError::Internal(format!("dataset generator panicked: {e}")))??;
    let version = register(db, built.manifest(), "regenerate", hosted).await?;
    let ctx = Arc::new(built);
    registry.remember(version.id, ctx.clone());
    Ok((version, ctx))
}

pub(crate) fn db_error(e: bb_db::Error) -> AppError {
    crate::v1::error::internal_db(e)
}
