//! Persistence of completed runs.
//!
//! [`RunStore`] says nothing about where runs live; [`FsRunStore`] keeps one directory per run.
//! A database-backed store can implement the same trait later without touching its callers.

use std::{
    collections::VecDeque,
    fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use bb_core::result::{RunResult, RunSummary};
use serde::Serialize;

use crate::error::AppError;

/// Length of a run id: the first 16 hex characters of the run's `config_sha256`.
pub const RUN_ID_LEN: usize = 16;

const SUMMARY_FILE: &str = "summary.json";
const RUN_FILE: &str = "run.json";

/// How many full runs [`FsRunStore`] keeps decoded in memory. A run with a long tape is large;
/// the forensics view pages through one or two runs at a time, so a small bound is enough.
const CACHE_CAPACITY: usize = 16;

/// Storage for completed runs, keyed by run id.
pub trait RunStore: Send + Sync {
    /// Stores `result` under its `manifest.run_id` with the given label, replacing any run with
    /// the same id, and returns its listing entry.
    fn put(&self, result: &RunResult, label: &str) -> Result<RunSummary, AppError>;
    /// The complete run, or `None` when no such run is stored.
    fn get(&self, run_id: &str) -> Result<Option<Arc<RunResult>>, AppError>;
    /// The listing entry of a run, without loading its orders and tape.
    fn summary(&self, run_id: &str) -> Result<Option<RunSummary>, AppError>;
    /// Every stored run, newest first.
    fn list(&self) -> Result<Vec<RunSummary>, AppError>;
    /// Removes a run. Returns whether it existed.
    fn delete(&self, run_id: &str) -> Result<bool, AppError>;
}

/// The listing entry for `result`.
pub fn summarize(result: &RunResult, label: &str) -> RunSummary {
    RunSummary {
        manifest: result.manifest.clone(),
        metrics: result.metrics.clone(),
        ticks: result.ticks,
        order_count: result.orders.len() as u64,
        fill_count: result.fills.len() as u64,
        tape_len: result.tape.len() as u64,
        label: label.to_string(),
    }
}

/// Whether `run_id` has the only shape a run id can have: 16 lowercase hex characters.
pub fn is_valid_run_id(run_id: &str) -> bool {
    run_id.len() == RUN_ID_LEN
        && run_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Rejects anything that is not a run id. Every store operation calls this before building a
/// path from the id, which is what keeps `../` and absolute paths out of the filesystem.
pub fn validate_run_id(run_id: &str) -> Result<(), AppError> {
    if is_valid_run_id(run_id) {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "malformed run id {run_id:?}: expected {RUN_ID_LEN} lowercase hexadecimal characters"
        )))
    }
}

/// Orders summaries newest first; ties (same millisecond) fall back to the id so the order is
/// total and stable.
pub fn sort_newest_first(summaries: &mut [RunSummary]) {
    summaries.sort_by(|a, b| {
        b.manifest
            .created_unix_ms
            .cmp(&a.manifest.created_unix_ms)
            .then_with(|| a.manifest.run_id.cmp(&b.manifest.run_id))
    });
}

/// Filesystem run store: `<root>/<run_id>/run.json` and `<root>/<run_id>/summary.json`.
///
/// A run is visible exactly when its `summary.json` exists. `put` writes `run.json` first and
/// `summary.json` last, each through a temporary file renamed into place, so a crash at any
/// point leaves either the previous state or a complete run, never a truncated file.
pub struct FsRunStore {
    root: PathBuf,
    /// Recently used runs, least recently used at the front.
    cache: Mutex<VecDeque<(String, Arc<RunResult>)>>,
    /// Distinguishes temporary files of concurrent writers within this process.
    tmp_seq: AtomicU64,
}

impl FsRunStore {
    /// Opens (and creates if necessary) the store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AppError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| io_error("creating", &root, e))?;
        Ok(Self {
            root,
            cache: Mutex::new(VecDeque::with_capacity(CACHE_CAPACITY)),
            tmp_seq: AtomicU64::new(0),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join(run_id)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, VecDeque<(String, Arc<RunResult>)>> {
        // Each critical section is a single queue operation, so a poisoned lock still guards a
        // consistent queue.
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cache_get(&self, run_id: &str) -> Option<Arc<RunResult>> {
        let mut cache = self.cache();
        let pos = cache.iter().position(|(id, _)| id == run_id)?;
        let entry = cache.remove(pos)?;
        let run = Arc::clone(&entry.1);
        cache.push_back(entry);
        Some(run)
    }

    fn cache_insert(&self, run_id: &str, run: Arc<RunResult>) {
        let mut cache = self.cache();
        cache.retain(|(id, _)| id != run_id);
        cache.push_back((run_id.to_string(), run));
        while cache.len() > CACHE_CAPACITY {
            cache.pop_front();
        }
    }

    fn cache_remove(&self, run_id: &str) {
        self.cache().retain(|(id, _)| id != run_id);
    }

    /// Serializes `value` to `dir/name` through a temporary file in the same directory (a rename
    /// within one directory is atomic; across directories it might not be).
    fn write_atomic<T: Serialize>(
        &self,
        dir: &Path,
        name: &str,
        value: &T,
    ) -> Result<(), AppError> {
        let target = dir.join(name);
        let tmp = dir.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| -> io::Result<()> {
            let mut writer = BufWriter::new(fs::File::create(&tmp)?);
            serde_json::to_writer(&mut writer, value).map_err(io::Error::from)?;
            writer.flush()?;
            let file = writer.into_inner().map_err(|e| e.into_error())?;
            // Data must be on disk before the rename makes it the visible version.
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, &target)
        })();
        written.map_err(|e| {
            // Best effort: the temporary file is never read, leaving it behind is harmless.
            let _ = fs::remove_file(&tmp);
            io_error("writing", &target, e)
        })
    }
}

fn io_error(action: &str, path: &Path, err: io::Error) -> AppError {
    AppError::Internal(format!("{action} {}: {err}", path.display()))
}

/// Reads and decodes a JSON file; `None` when the file does not exist.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, AppError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error("reading", path, e)),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| AppError::Internal(format!("decoding {}: {e}", path.display())))
}

impl RunStore for FsRunStore {
    fn put(&self, result: &RunResult, label: &str) -> Result<RunSummary, AppError> {
        let run_id = result.manifest.run_id.as_str();
        validate_run_id(run_id)?;
        let dir = self.run_dir(run_id);
        fs::create_dir_all(&dir).map_err(|e| io_error("creating", &dir, e))?;
        let summary = summarize(result, label);
        self.write_atomic(&dir, RUN_FILE, result)?;
        self.write_atomic(&dir, SUMMARY_FILE, &summary)?;
        self.cache_insert(run_id, Arc::new(result.clone()));
        Ok(summary)
    }

    fn get(&self, run_id: &str) -> Result<Option<Arc<RunResult>>, AppError> {
        validate_run_id(run_id)?;
        if let Some(run) = self.cache_get(run_id) {
            return Ok(Some(run));
        }
        let dir = self.run_dir(run_id);
        // Without its summary a run directory is an interrupted write, not a run.
        if !dir.join(SUMMARY_FILE).is_file() {
            return Ok(None);
        }
        let Some(run) = read_json::<RunResult>(&dir.join(RUN_FILE))? else {
            return Ok(None);
        };
        if run.manifest.run_id != run_id {
            return Err(AppError::Internal(format!(
                "stored run in {} claims run id {}",
                dir.display(),
                run.manifest.run_id
            )));
        }
        let run = Arc::new(run);
        self.cache_insert(run_id, Arc::clone(&run));
        Ok(Some(run))
    }

    fn summary(&self, run_id: &str) -> Result<Option<RunSummary>, AppError> {
        validate_run_id(run_id)?;
        read_json(&self.run_dir(run_id).join(SUMMARY_FILE))
    }

    fn list(&self) -> Result<Vec<RunSummary>, AppError> {
        let entries = fs::read_dir(&self.root).map_err(|e| io_error("listing", &self.root, e))?;
        let mut summaries = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error("listing", &self.root, e))?;
            let name = entry.file_name();
            // Anything that is not named like a run id was not put here by this store.
            let Some(run_id) = name.to_str().filter(|n| is_valid_run_id(n)) else {
                continue;
            };
            if let Some(summary) = self.summary(run_id)? {
                summaries.push(summary);
            }
        }
        sort_newest_first(&mut summaries);
        Ok(summaries)
    }

    fn delete(&self, run_id: &str) -> Result<bool, AppError> {
        validate_run_id(run_id)?;
        self.cache_remove(run_id);
        let dir = self.run_dir(run_id);
        let summary = dir.join(SUMMARY_FILE);
        // Removing the summary first makes the run disappear from listings in one step, even if
        // removing the (large) run file is interrupted afterwards.
        let existed = match fs::remove_file(&summary) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(io_error("deleting", &summary, e)),
        };
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error("deleting", &dir, e)),
        }
        Ok(existed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_shape_is_enforced() {
        assert!(is_valid_run_id("0123456789abcdef"));
        for bad in [
            "",
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789ABCDEF",
            "0123456789abcdeg",
            "../../../etc/pwd",
            "..\\..\\windows\\x",
            "0123456789abcd/f",
        ] {
            assert!(!is_valid_run_id(bad), "{bad:?} must be rejected");
            assert!(matches!(
                validate_run_id(bad),
                Err(AppError::InvalidRequest(_))
            ));
        }
    }

    #[test]
    fn malformed_ids_never_reach_the_filesystem() {
        let root = std::env::temp_dir().join(format!("bb-app-store-unit-{}", std::process::id()));
        let store = FsRunStore::open(&root).unwrap();
        assert!(matches!(
            store.get("../outside"),
            Err(AppError::InvalidRequest(_))
        ));
        assert!(matches!(
            store.summary("not-a-run-id"),
            Err(AppError::InvalidRequest(_))
        ));
        assert!(matches!(
            store.delete("../outside"),
            Err(AppError::InvalidRequest(_))
        ));
        // A well-formed id that was never stored is simply absent.
        assert!(store.get("0123456789abcdef").unwrap().is_none());
        assert!(store.summary("0123456789abcdef").unwrap().is_none());
        assert!(!store.delete("0123456789abcdef").unwrap());
        assert!(store.list().unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}
