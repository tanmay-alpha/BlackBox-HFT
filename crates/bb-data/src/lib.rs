//! BlackBox HFT data pipelines.
//!
//! Two sources produce the same artifact, an engine event array plus a [`DatasetManifest`]:
//!
//! * [`synthetic`] generates a seeded Level-2 market. It is labelled synthetic everywhere and is
//!   never presented as market data.
//! * [`tardis`] converts recorded Tardis.dev CSV files (`trades` + `incremental_book_L2`).
//!
//! Both run the shared [`normalize`] steps (a port of the py-hftbacktest reference converter) and
//! the shared [`quality`] checks, and are persisted and read back through [`store`]. [`book`]
//! reconstructs the book from an event array for replay views.
//!
//! [`itch`] is a local-only validation workflow for NASDAQ TotalView-ITCH 5.0 files: it decodes
//! them, rebuilds the order-by-order book and aggregates selected stocks into the same engine rows,
//! with its own manifest. It does not produce a stored dataset.
//!
//! Engine events carry absolute epoch nanoseconds; everything serialized is relative to the
//! dataset's `origin_ns`.

pub mod book;
mod decimal;
pub mod itch;
pub mod l3synth;
pub mod l3workbench;
pub mod normalize;
pub mod quality;
pub mod slice;
pub mod store;
pub mod synthetic;
pub mod tardis;

use std::{
    io::{self, Read},
    sync::Arc,
};

use bb_core::dataset::DatasetManifest;
pub use hftbacktest::types::Event;
use hftbacktest::types::{BUY_EVENT, SELL_EVENT};
use sha2::{Digest, Sha256};

/// Passes bytes through while hashing and counting them, so a manifest records the hash of the
/// file exactly as it is on disk (compressed bytes for `.gz`).
pub(crate) struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R: Read> HashingReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
        }
    }

    /// Reads whatever the consumer left unread, so that every stored byte has passed through
    /// the hasher, and returns the hex SHA-256 and size of the whole input.
    pub(crate) fn finish(mut self) -> io::Result<(String, u64)> {
        let mut sink = vec![0u8; 1 << 16];
        loop {
            match self.inner.read(&mut sink) {
                Ok(0) => break,
                Ok(n) => {
                    self.hasher.update(&sink[..n]);
                    self.bytes += n as u64;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        Ok((hex::encode(self.hasher.finalize()), self.bytes))
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}

/// A freshly built dataset that has not been persisted yet. `manifest.events_file`,
/// `events_sha256` and `events_bytes` are empty until [`store::save`] fills them.
#[derive(Clone, Debug)]
pub struct BuiltDataset {
    pub manifest: DatasetManifest,
    pub events: Vec<Event>,
}

/// A dataset read back from the store with its event file verified against the manifest hash.
#[derive(Clone, Debug)]
pub struct LoadedDataset {
    pub manifest: DatasetManifest,
    pub events: Arc<Vec<Event>>,
    /// Absolute epoch nanoseconds of the dataset origin (parsed `manifest.origin_ns`).
    pub origin_ns: i64,
}

#[derive(thiserror::Error, Debug)]
pub enum DataError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
    /// The file is not the expected format (wrong header).
    #[error("{file}: {message}")]
    Schema { file: String, message: String },
    /// A data row could not be accepted. `line` is 1-based and counts the header.
    #[error("{file}:{line}: {message}")]
    Parse {
        file: String,
        line: u64,
        message: String,
    },
    /// A timestamp is not a plausible epoch time under the declared unit.
    #[error("{file}:{line}: {message}")]
    Timestamp {
        file: String,
        line: u64,
        message: String,
    },
    /// Stored bytes do not match what the manifest recorded.
    #[error("integrity check failed for {path}: {message}")]
    Integrity { path: String, message: String },
    /// Exchange-clock or local-clock rows are not in time order.
    #[error("event order is invalid: {0}")]
    Order(String),
    #[error("{0}")]
    Invalid(String),
    /// No dataset with this id exists in the store.
    #[error("dataset `{0}` not found")]
    NotFound(String),
}

impl DataError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        DataError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// Low byte of the event flag: depth, trade, clear or snapshot.
#[inline]
pub(crate) fn kind(ev: &Event) -> u64 {
    ev.ev & 0xff
}

#[inline]
pub(crate) fn has(ev: &Event, flag: u64) -> bool {
    ev.ev & flag == flag
}

/// Side stated by the row's flag bits. Buy wins when both are set, as in the engine's dispatch.
#[inline]
pub(crate) fn side_of(ev: &Event) -> Option<bb_core::Side> {
    if has(ev, BUY_EVENT) {
        Some(bb_core::Side::Buy)
    } else if has(ev, SELL_EVENT) {
        Some(bb_core::Side::Sell)
    } else {
        None
    }
}

pub(crate) fn make_event(ev: u64, exch_ts: i64, local_ts: i64, px: f64, qty: f64) -> Event {
    Event {
        ev,
        exch_ts,
        local_ts,
        px,
        qty,
        order_id: 0,
        ival: 0,
        fval: 0.0,
    }
}

/// `[a-z0-9_-]+`, the only form a dataset id (a directory name) may take.
pub(crate) fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Refuses events whose timestamps relative to `origin_ns` would leave `±(2^53 − 1)` ns (about
/// 104 days): every serialized timestamp is relative to the origin, and beyond that span a JSON
/// client reading numbers as doubles (every browser) would round them.
pub(crate) fn check_span(events: &[Event], origin_ns: i64) -> Result<(), DataError> {
    let max_span = i128::from(bb_core::time::MAX_SPAN_NS);
    let origin = i128::from(origin_ns);
    let outside = events.iter().find(|e| {
        [e.exch_ts, e.local_ts]
            .iter()
            .any(|&ts| (i128::from(ts) - origin).abs() > max_span)
    });
    match outside {
        None => Ok(()),
        Some(e) => Err(DataError::Invalid(format!(
            "the dataset spans too long: a row at exchange time {} / local time {} is more than \
             2^53 - 1 ns (about 104 days) from the origin {origin_ns}, so its relative timestamps \
             would not be exact in JSON; split it into shorter datasets",
            e.exch_ts, e.local_ts
        ))),
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// A fresh directory under the system temp dir, removed when the guard drops.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "bb-data-test-{}-{}-{}",
                std::process::id(),
                tag,
                n
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
