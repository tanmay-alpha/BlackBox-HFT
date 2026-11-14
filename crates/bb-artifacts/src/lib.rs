//! Immutable run bundles and private object storage, without engine or database dependencies.
//!
//! Format v1 is concatenated gzip NDJSON: header, bounded pages in orders/fills/equity/tape
//! order, then trailer. Page digests cover compressed bytes. Science hashes cover canonical
//! bb-core JSON: result = `(tape, fills, equity)` (identical to the current pipeline), summary
//! = `{metrics, engine_check, counts}` in declaration order. Counts include ticks and all four
//! sections. Store `BundleTrust` in the trusted database, never derive it from downloaded bytes.
//! Every page access authenticates the index, header and trailer against that trust root.
//!
//! Database integration: persist the **entire serialized `BundleTrust`** as the opaque
//! `run_artifacts.page_index` JSON object. The database column `page_index_sha256` authenticates
//! that entire stored JSON value and is a separate digest from `BundleTrust.page_index_sha256`,
//! which authenticates only the canonical typed `{"pages":[...]}` object. On load, verify the
//! database's outer JSON digest before deserializing `BundleTrust`, then pass it to the reader.
//!
//! `BundleWriter` uses temporary files and bounded page buffers; it returns only after a full
//! independent reread. `VerifiedBundle` owns and cleans up its temporary file. Store publication
//! never overwrites. `ArtifactStore` deliberately has no list operation. Credentials are supplied
//! explicitly by the application; this crate never reads environment variables.
mod bundle;
mod cache;
mod store;

pub use bundle::*;
pub use cache::DiskReadCache;
pub use store::*;

#[derive(Debug, thiserror::Error)]
#[error("{object} integrity failed: {reason}")]
pub struct IntegrityError {
    pub object: &'static str,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error(transparent)]
    Integrity(#[from] IntegrityError),
    #[error("artifact not found")]
    NotFound,
    #[error("artifact already exists")]
    AlreadyExists,
    #[error("invalid artifact input: {0}")]
    InvalidInput(String),
    #[error("artifact budget exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("storage transport failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("storage returned HTTP {0}")]
    Http(u16),
}

pub(crate) fn integrity(object: &'static str, reason: impl Into<String>) -> ArtifactError {
    IntegrityError {
        object,
        reason: reason.into(),
    }
    .into()
}

pub(crate) fn io_error(error: std::io::Error) -> ArtifactError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ArtifactError::NotFound,
        std::io::ErrorKind::AlreadyExists => ArtifactError::AlreadyExists,
        _ => ArtifactError::Io(error),
    }
}
