use std::{
    collections::HashMap,
    io::Write,
    ops::Range,
    path::Path,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use tempfile::TempDir;

use crate::{ArtifactError, ArtifactStore, integrity};

struct Entry {
    size: u64,
    touched: u64,
}
struct State {
    directory: TempDir,
    entries: HashMap<String, Entry>,
    used: u64,
    clock: u64,
}

/// Optional compressed-range disk LRU. Capacity is measured in bytes, not entry count.
/// Each instance owns an isolated temporary directory, removed on drop. Every hit is rehashed
/// against the caller's database-sourced digest; cache filenames never establish trust.
#[derive(Clone)]
pub struct DiskReadCache {
    state: Arc<Mutex<State>>,
    capacity: u64,
}
impl DiskReadCache {
    pub fn new(parent: &Path, capacity_bytes: u64) -> Result<Self, ArtifactError> {
        if capacity_bytes == 0 {
            return Err(ArtifactError::InvalidInput(
                "cache capacity must be positive".into(),
            ));
        }
        let directory = tempfile::Builder::new()
            .prefix("bb-read-cache-")
            .tempdir_in(parent)?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                directory,
                entries: HashMap::new(),
                used: 0,
                clock: 0,
            })),
            capacity: capacity_bytes,
        })
    }
    pub fn used_bytes(&self) -> Result<u64, ArtifactError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| ArtifactError::Io(std::io::Error::other("cache lock poisoned")))?
            .used)
    }
    /// Cache only bytes authenticated by `expected_sha256`; a range is never cached by URL alone.
    pub async fn get_range(
        &self,
        store: &dyn ArtifactStore,
        key: &str,
        range: Range<u64>,
        expected_sha256: &str,
    ) -> Result<Bytes, ArtifactError> {
        if expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ArtifactError::InvalidInput(
                "expected SHA-256 must be lowercase hex".into(),
            ));
        }
        let len = range
            .end
            .checked_sub(range.start)
            .filter(|n| *n > 0)
            .ok_or_else(|| ArtifactError::InvalidInput("invalid cache range".into()))?;
        let cache = self.clone();
        let digest = expected_sha256.to_owned();
        let cached = tokio::task::spawn_blocking(move || cache.read(&digest, len))
            .await
            .map_err(|e| ArtifactError::Io(std::io::Error::other(e)))??;
        if let Some(bytes) = cached {
            return Ok(bytes);
        }
        let bytes = store.get_range(key, range).await?;
        if bytes.len() as u64 != len || bb_core::hash::sha256_hex(&bytes) != expected_sha256 {
            return Err(integrity("cache", "download hash or length mismatch"));
        }
        if len <= self.capacity {
            let (cache, digest, data) = (self.clone(), expected_sha256.to_owned(), bytes.clone());
            tokio::task::spawn_blocking(move || cache.insert(&digest, &data))
                .await
                .map_err(|e| ArtifactError::Io(std::io::Error::other(e)))??;
        }
        Ok(bytes)
    }
    fn read(&self, digest: &str, len: u64) -> Result<Option<Bytes>, ArtifactError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ArtifactError::Io(std::io::Error::other("cache lock poisoned")))?;
        if !state.entries.contains_key(digest) {
            return Ok(None);
        }
        let path = state.directory.path().join(digest);
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.len() != len
            || len > self.capacity
        {
            return Err(integrity("cache", "cached file length or type changed"));
        }
        let bytes = std::fs::read(&path)?;
        if bb_core::hash::sha256_hex(&bytes) != digest {
            return Err(integrity("cache", "cached bytes changed"));
        }
        state.clock = state.clock.wrapping_add(1);
        let clock = state.clock;
        if let Some(entry) = state.entries.get_mut(digest) {
            entry.touched = clock;
        }
        Ok(Some(bytes.into()))
    }
    fn insert(&self, digest: &str, bytes: &[u8]) -> Result<(), ArtifactError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ArtifactError::Io(std::io::Error::other("cache lock poisoned")))?;
        if state.entries.contains_key(digest) {
            return Ok(());
        }
        let size = bytes.len() as u64;
        while state.used > self.capacity - size {
            let oldest = state
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(k, _)| k.clone())
                .ok_or_else(|| integrity("cache", "byte accounting mismatch"))?;
            std::fs::remove_file(state.directory.path().join(&oldest))?;
            if let Some(entry) = state.entries.remove(&oldest) {
                state.used -= entry.size;
            }
        }
        let mut file = tempfile::NamedTempFile::new_in(state.directory.path())?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(state.directory.path().join(digest))
            .map_err(|e| ArtifactError::Io(e.error))?;
        state.clock = state.clock.wrapping_add(1);
        let touched = state.clock;
        state.entries.insert(digest.into(), Entry { size, touched });
        state.used += size;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactStore, FsArtifactStore};
    #[tokio::test]
    async fn evicts_by_bytes_and_rejects_corrupted_hit() {
        let root = tempfile::tempdir().unwrap();
        let store = FsArtifactStore::new(root.path().join("objects")).unwrap();
        let cache = DiskReadCache::new(root.path(), 6).unwrap();
        let source = root.path().join("input");
        std::fs::write(&source, b"abcdefghi").unwrap();
        store
            .put_bundle_from_file("run.bbz", &source)
            .await
            .unwrap();
        let a = bb_core::hash::sha256_hex(b"abc");
        let b = bb_core::hash::sha256_hex(b"def");
        let c = bb_core::hash::sha256_hex(b"ghi");
        cache.get_range(&store, "run.bbz", 0..3, &a).await.unwrap();
        cache.get_range(&store, "run.bbz", 3..6, &b).await.unwrap();
        cache.get_range(&store, "run.bbz", 6..9, &c).await.unwrap();
        assert_eq!(cache.used_bytes().unwrap(), 6);
        {
            let state = cache.state.lock().unwrap();
            assert!(!state.entries.contains_key(&a));
            std::fs::write(state.directory.path().join(&b), b"xyz").unwrap();
        }
        assert!(matches!(
            cache.get_range(&store, "run.bbz", 3..6, &b).await,
            Err(ArtifactError::Integrity(_))
        ));
        let wrong = bb_core::hash::sha256_hex(b"xxx");
        assert!(matches!(
            cache.get_range(&store, "run.bbz", 0..3, &wrong).await,
            Err(ArtifactError::Integrity(_))
        ));
    }
}
