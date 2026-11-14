//! Dataset registry: dataset versions live in Postgres (`bb.dataset_versions`, the trust root);
//! their event files are local, ephemeral copies under `<data-dir>/registry/<sha prefix>/`.
//!
//! A synthetic version (`storage = 'regenerate'`) is rebuilt deterministically from
//! `manifest.generator` on first use and verified against `events_sha256` (no object storage, no
//! Blob operations). A local version (`storage = 'fs'`) is loaded from `<data-dir>/datasets` and
//! verified the same way. A version whose bytes do not match is never used.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use bb_core::dataset::{DatasetKind, DatasetManifest};
use bb_data::synthetic::SyntheticConfig;
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
        let root = self.version_root(&version);
        let local = self.local_datasets.clone();
        let ctx = tokio::task::spawn_blocking(move || materialise(&root, &local, &version))
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

    fn version_root(&self, version: &DatasetVersion) -> PathBuf {
        self.root.join(&hex::encode(&version.events_sha256)[..16])
    }

    /// Stores an already loaded dataset under its version (registration path).
    fn remember(&self, id: Uuid, ctx: Arc<DatasetCtx>) {
        self.loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, ctx);
    }
}

fn materialise(
    root: &Path,
    local_datasets: &Path,
    version: &DatasetVersion,
) -> Result<DatasetCtx, AppError> {
    let expected = hex::encode(&version.events_sha256);
    let id = &version.dataset_id;
    let (dir, regenerate) = match version.storage.as_str() {
        "regenerate" => (root.to_path_buf(), true),
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
                "dataset '{id}' version {} is not available on this server",
                version.version_no
            )));
        }
        let manifest = manifest_of(version)?;
        let generator = manifest.generator.clone().ok_or_else(|| {
            AppError::Internal(format!("dataset '{id}' has no generator parameters"))
        })?;
        let config: SyntheticConfig = serde_json::from_value(generator)
            .map_err(|e| AppError::Internal(format!("dataset '{id}' generator parameters: {e}")))?;
        let built = bb_data::synthetic::generate(&config)?;
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
    Ok(DatasetCtx::new(loaded))
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
/// refuses sources that are not hostable (SEC-35).
pub async fn register(
    db: &Database,
    manifest: &DatasetManifest,
    storage: &str,
    hosted: bool,
) -> Result<DatasetVersion, AppError> {
    let synthetic = matches!(manifest.kind, DatasetKind::Synthetic);
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
        .register_version(&new_version(manifest, version_no, storage)?)
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
