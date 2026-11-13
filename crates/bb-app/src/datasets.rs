//! Loaded datasets and their cache.
//!
//! Loading a dataset verifies its hash and decodes the whole event array, and the top-of-book
//! series needs a full pass over it. Both are done once per dataset and shared by every run,
//! replay request and grid cell.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

use bb_core::{dataset::DatasetManifest, market::BboSeries, result::RunManifest};
use bb_data::LoadedDataset;

use crate::error::AppError;

/// File that marks a directory under the dataset root as a dataset.
const MANIFEST_FILE: &str = "dataset.json";

/// A dataset ready to simulate on, plus the derived market series forensics needs.
///
/// Read-only after construction and made of plain data behind an `Arc`, so it is shared across
/// the grid worker threads by reference.
pub struct DatasetCtx {
    pub dataset: LoadedDataset,
    /// Exchange-clock top of book of the whole dataset; used for fill mids and markouts.
    pub bbo: BboSeries,
}

impl DatasetCtx {
    pub fn new(dataset: LoadedDataset) -> Self {
        let bbo = bb_data::book::bbo_series(
            dataset.events.as_slice(),
            &dataset.manifest.instrument,
            dataset.origin_ns,
        );
        Self { dataset, bbo }
    }

    /// Loads dataset `id` from `root`. A missing dataset is `not_found`; a dataset that exists
    /// but fails its integrity checks is an error, never silently skipped.
    pub fn load(root: &Path, id: &str) -> Result<Self, AppError> {
        if !is_valid_dataset_id(id) || !root.join(id).join(MANIFEST_FILE).is_file() {
            return Err(dataset_not_found(id));
        }
        Ok(Self::new(bb_data::store::load(root, id)?))
    }

    pub fn manifest(&self) -> &DatasetManifest {
        &self.dataset.manifest
    }
}

/// Dataset ids are directory names restricted to `[a-z0-9_-]+`. Checked before an id is joined
/// onto a path, so a request can never reach outside the dataset root.
pub fn is_valid_dataset_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn dataset_not_found(id: &str) -> AppError {
    AppError::NotFound(format!("dataset '{id}' not found"))
}

/// Manifests of every dataset under `root`, sorted by id. A root that does not exist yet simply
/// holds no datasets.
pub fn list_manifests(root: &Path) -> Result<Vec<DatasetManifest>, AppError> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    Ok(bb_data::store::list(root)?)
}

/// The manifest of dataset `id`, without loading its events.
pub fn find_manifest(root: &Path, id: &str) -> Result<DatasetManifest, AppError> {
    if !is_valid_dataset_id(id) {
        return Err(dataset_not_found(id));
    }
    list_manifests(root)?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| dataset_not_found(id))
}

/// The manifest of the dataset `run` was simulated on. Refuses a dataset that has the same id
/// but different events: explaining a run against other market data would be wrong.
pub fn manifest_for_run(root: &Path, run: &RunManifest) -> Result<DatasetManifest, AppError> {
    let manifest = find_manifest(root, &run.dataset.id).map_err(|_| {
        AppError::NotFound(format!(
            "dataset '{}' used by run {} is no longer available",
            run.dataset.id, run.run_id
        ))
    })?;
    if manifest.events_sha256 != run.dataset.sha256 {
        return Err(AppError::NotFound(format!(
            "dataset '{}' has changed since run {} was simulated (run used events sha256 {}, \
             dataset now has {})",
            run.dataset.id, run.run_id, run.dataset.sha256, manifest.events_sha256
        )));
    }
    Ok(manifest)
}

/// Loaded datasets keyed by id.
#[derive(Default)]
pub struct DatasetCache {
    inner: Mutex<BTreeMap<String, Arc<DatasetCtx>>>,
}

impl DatasetCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached dataset, if it was loaded before.
    pub fn get(&self, id: &str) -> Option<Arc<DatasetCtx>> {
        self.lock().get(id).cloned()
    }

    /// Returns the cached dataset or loads it from `root`.
    ///
    /// The lock is not held while loading, so two first requests for the same dataset may both
    /// load it; the first one inserted wins and the other copy is dropped. That costs time once,
    /// never correctness, and keeps cached datasets available while a large one loads.
    pub fn get_or_load(&self, root: &Path, id: &str) -> Result<Arc<DatasetCtx>, AppError> {
        if let Some(ctx) = self.get(id) {
            return Ok(ctx);
        }
        let loaded = Arc::new(DatasetCtx::load(root, id)?);
        Ok(self.lock().entry(id.to_string()).or_insert(loaded).clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<DatasetCtx>>> {
        // The map is only ever touched by single insert/lookup calls, so a panic elsewhere while
        // the lock was held cannot leave it half-updated; a poisoned lock is still usable.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_ids_cannot_escape_the_root() {
        assert!(is_valid_dataset_id("synthetic-demo"));
        assert!(is_valid_dataset_id("btcusdt_2020-02-01"));
        for bad in ["", "..", "a/b", "a\\b", "A", "a b", "a.b", "../x"] {
            assert!(!is_valid_dataset_id(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn missing_root_lists_nothing_and_unknown_dataset_is_not_found() {
        let root =
            std::env::temp_dir().join(format!("bb-app-datasets-missing-{}", std::process::id()));
        assert!(list_manifests(&root).unwrap().is_empty());
        assert!(matches!(
            find_manifest(&root, "nope"),
            Err(AppError::NotFound(_))
        ));
        let cache = DatasetCache::new();
        assert!(matches!(
            cache.get_or_load(&root, "nope"),
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            cache.get_or_load(&root, "../nope"),
            Err(AppError::NotFound(_))
        ));
    }
}
