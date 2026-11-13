//! On-disk dataset store.
//!
//! Layout: `<root>/<id>/dataset.json` (the manifest, pretty JSON) and `<root>/<id>/events.npz`, a
//! zip archive with one entry `data.npy` in the engine's own structured-array format, readable by
//! py-hftbacktest as well. The SHA-256 of `events.npz` is the dataset hash that run manifests
//! record; `load` refuses a file whose bytes no longer match it.

use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use bb_core::{dataset::DatasetManifest, hash::sha256_reader};
use hftbacktest::backtest::data::{read_npz_file, write_npy};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
    BuiltDataset, DataError, Event, LoadedDataset, is_valid_id, normalize::validate_event_order,
};

pub const MANIFEST_FILE: &str = "dataset.json";
pub const EVENTS_FILE: &str = "events.npz";
/// Name of the array inside the archive, as hftbacktest's `read_npz_file` expects it.
const NPZ_ENTRY: &str = "data";

fn dataset_dir(root: &Path, id: &str) -> Result<PathBuf, DataError> {
    if !is_valid_id(id) {
        return Err(DataError::Invalid(format!(
            "dataset id `{id}` must match [a-z0-9_-]+"
        )));
    }
    Ok(root.join(id))
}

/// Writes the event archive and manifest, filling `events_file`, `events_sha256` and
/// `events_bytes`. Both files are written to temporary names and renamed into place, manifest
/// last, so a dataset directory never holds a manifest for a half-written event file.
pub fn save(root: &Path, ds: &BuiltDataset) -> Result<DatasetManifest, DataError> {
    let dir = dataset_dir(root, &ds.manifest.id)?;
    fs::create_dir_all(&dir).map_err(|e| DataError::io(&dir, e))?;
    validate_event_order(&ds.events)?;

    let events_path = dir.join(EVENTS_FILE);
    let tmp_events = dir.join(format!("{EVENTS_FILE}.tmp"));
    {
        let file = File::create(&tmp_events).map_err(|e| DataError::io(&tmp_events, e))?;
        let mut zip = ZipWriter::new(BufWriter::new(file));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            // Fixed timestamp so identical events always produce identical archive bytes.
            .last_modified_time(zip::DateTime::default())
            .large_file(true);
        zip.start_file(format!("{NPZ_ENTRY}.npy"), options)
            .map_err(|e| DataError::Invalid(format!("writing {}: {e}", tmp_events.display())))?;
        write_npy(&mut zip, &ds.events).map_err(|e| DataError::io(&tmp_events, e))?;
        let mut inner = zip
            .finish()
            .map_err(|e| DataError::Invalid(format!("writing {}: {e}", tmp_events.display())))?;
        inner.flush().map_err(|e| DataError::io(&tmp_events, e))?;
    }
    fs::rename(&tmp_events, &events_path).map_err(|e| DataError::io(&events_path, e))?;

    let bytes = fs::metadata(&events_path)
        .map_err(|e| DataError::io(&events_path, e))?
        .len();
    let sha256 = hash_file(&events_path)?;

    let mut manifest = ds.manifest.clone();
    manifest.events_file = EVENTS_FILE.to_string();
    manifest.events_sha256 = sha256;
    manifest.events_bytes = bytes;

    let manifest_path = dir.join(MANIFEST_FILE);
    let tmp_manifest = dir.join(format!("{MANIFEST_FILE}.tmp"));
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| DataError::Json {
        path: manifest_path.display().to_string(),
        source: e,
    })?;
    fs::write(&tmp_manifest, json).map_err(|e| DataError::io(&tmp_manifest, e))?;
    fs::rename(&tmp_manifest, &manifest_path).map_err(|e| DataError::io(&manifest_path, e))?;
    Ok(manifest)
}

fn hash_file(path: &Path) -> Result<String, DataError> {
    let file = File::open(path).map_err(|e| DataError::io(path, e))?;
    sha256_reader(std::io::BufReader::new(file)).map_err(|e| DataError::io(path, e))
}

fn read_manifest(path: &Path) -> Result<DatasetManifest, DataError> {
    let bytes = fs::read(path).map_err(|e| DataError::io(path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| DataError::Json {
        path: path.display().to_string(),
        source: e,
    })
}

/// Reads a dataset's manifest without touching its event file.
pub fn manifest(root: &Path, id: &str) -> Result<DatasetManifest, DataError> {
    let dir = dataset_dir(root, id)?;
    let path = dir.join(MANIFEST_FILE);
    if !path.is_file() {
        return Err(DataError::NotFound(id.to_string()));
    }
    read_manifest(&path)
}

/// Loads a dataset, verifying the event file against the manifest's hash and re-validating the
/// event order.
pub fn load(root: &Path, id: &str) -> Result<LoadedDataset, DataError> {
    let manifest = manifest(root, id)?;
    let dir = dataset_dir(root, id)?;
    let events_path = dir.join(&manifest.events_file);
    if manifest.events_file.contains(['/', '\\']) || manifest.events_file.is_empty() {
        return Err(DataError::Integrity {
            path: dir.join(MANIFEST_FILE).display().to_string(),
            message: format!(
                "events_file `{}` is not a plain file name",
                manifest.events_file
            ),
        });
    }
    let actual = hash_file(&events_path)?;
    if actual != manifest.events_sha256 {
        return Err(DataError::Integrity {
            path: events_path.display().to_string(),
            message: format!(
                "SHA-256 is {actual}, the manifest records {}",
                manifest.events_sha256
            ),
        });
    }
    let path_str = events_path.to_str().ok_or_else(|| {
        DataError::Invalid(format!("path {} is not valid UTF-8", events_path.display()))
    })?;
    let data =
        read_npz_file::<Event>(path_str, NPZ_ENTRY).map_err(|e| DataError::io(&events_path, e))?;
    // `Data<Event>` is `Rc`-based; copy into a shareable vector.
    let mut events = Vec::with_capacity(data.len());
    for i in 0..data.len() {
        events.push(data[i].clone());
    }
    if events.len() as u64 != manifest.counts.rows {
        return Err(DataError::Integrity {
            path: events_path.display().to_string(),
            message: format!(
                "holds {} rows, the manifest records {}",
                events.len(),
                manifest.counts.rows
            ),
        });
    }
    validate_event_order(&events)?;
    let origin_ns = manifest.origin().ok_or_else(|| {
        DataError::Invalid(format!("dataset `{id}` has an unparseable origin_ns"))
    })?;
    Ok(LoadedDataset {
        manifest,
        events: Arc::new(events),
        origin_ns,
    })
}

/// Manifests of every dataset under `root`, sorted by id. A missing root is an empty store.
pub fn list(root: &Path) -> Result<Vec<DatasetManifest>, DataError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(DataError::io(root, e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| DataError::io(root, e))?;
        let path = entry.path().join(MANIFEST_FILE);
        if path.is_file() {
            out.push(read_manifest(&path)?);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{synthetic, testutil::TempDir};

    fn small() -> BuiltDataset {
        let cfg = synthetic::SyntheticConfig {
            duration_ns: 3_000_000_000,
            ..Default::default()
        };
        synthetic::generate(&cfg).expect("generate")
    }

    #[test]
    fn save_then_load_round_trips_exactly() {
        let tmp = TempDir::new("store-rt");
        let built = small();
        let saved = save(tmp.path(), &built).unwrap();
        assert_eq!(saved.events_file, EVENTS_FILE);
        assert_eq!(saved.events_sha256.len(), 64);
        let loaded = load(tmp.path(), &saved.id).unwrap();
        assert_eq!(loaded.manifest, saved);
        assert_eq!(*loaded.events, built.events);
        assert_eq!(loaded.origin_ns, saved.origin().unwrap());

        // Saving the same events again gives the same bytes, hence the same dataset hash.
        let again = save(tmp.path(), &built).unwrap();
        assert_eq!(again.events_sha256, saved.events_sha256);

        let listed = list(tmp.path()).unwrap();
        assert_eq!(listed, vec![saved]);
    }

    #[test]
    fn corrupted_event_file_fails_integrity() {
        let tmp = TempDir::new("store-corrupt");
        let saved = save(tmp.path(), &small()).unwrap();
        let path = tmp.path().join(&saved.id).join(EVENTS_FILE);
        let mut bytes = fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x01;
        fs::write(&path, bytes).unwrap();
        match load(tmp.path(), &saved.id) {
            Err(DataError::Integrity { .. }) => {}
            other => panic!("expected an integrity error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_and_malformed_ids() {
        let tmp = TempDir::new("store-ids");
        assert!(matches!(
            load(tmp.path(), "nope"),
            Err(DataError::NotFound(_))
        ));
        assert!(matches!(
            load(tmp.path(), "../x"),
            Err(DataError::Invalid(_))
        ));
        assert!(list(&tmp.path().join("missing")).unwrap().is_empty());
    }
}
