use std::{
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url, header};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{ArtifactError, integrity, io_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactHead {
    pub size_bytes: u64,
    pub etag: Option<String>,
}

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Publishes one immutable object. Existing keys are errors, never overwritten.
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<ArtifactHead, ArtifactError>;
    /// Half-open byte range. Missing objects and malformed/short ranges are distinct errors.
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes, ArtifactError>;
    async fn head(&self, key: &str) -> Result<ArtifactHead, ArtifactError>;
    /// Idempotent deletion; no listing is needed for the database deletion outbox.
    async fn delete(&self, key: &str) -> Result<(), ArtifactError>;
}

pub(crate) fn validate_key(key: &str) -> Result<(), ArtifactError> {
    if key.is_empty()
        || key.len() > 1024
        || key.split('/').any(|s| {
            s.is_empty()
                || s == "."
                || s == ".."
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
    {
        return Err(ArtifactError::InvalidInput("invalid storage key".into()));
    }
    // Windows aliases and drive/UNC forms must not acquire filesystem meaning.
    if key
        .split('/')
        .any(|s| s.ends_with('.') || is_windows_device(s))
    {
        return Err(ArtifactError::InvalidInput(
            "reserved storage key component".into(),
        ));
    }
    Ok(())
}
fn is_windows_device(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
fn checked_range(range: &Range<u64>, max: u64) -> Result<u64, ArtifactError> {
    let len = range
        .end
        .checked_sub(range.start)
        .filter(|n| *n > 0)
        .ok_or_else(|| {
            ArtifactError::InvalidInput("range must be nonempty and increasing".into())
        })?;
    if len > max || usize::try_from(len).is_err() {
        return Err(ArtifactError::LimitExceeded("range bytes"));
    }
    Ok(len)
}

/// The root must be private to the service account. Symlink components are rejected, including
/// the final object; keys have identical safe semantics on Windows and Unix. Publishing uses
/// an atomic no-replace hard link from a synced sibling tempfile, not check-then-rename.
#[derive(Clone, Debug)]
pub struct FsArtifactStore {
    root: PathBuf,
    max_range_bytes: u64,
}
impl FsArtifactStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        std::fs::create_dir_all(root.as_ref())?;
        if std::fs::symlink_metadata(root.as_ref())?
            .file_type()
            .is_symlink()
        {
            return Err(ArtifactError::InvalidInput(
                "store root is a symlink".into(),
            ));
        }
        Ok(Self {
            root: std::fs::canonicalize(root)?,
            max_range_bytes: 64 << 20,
        })
    }
    pub fn with_max_range_bytes(mut self, bytes: u64) -> Result<Self, ArtifactError> {
        if bytes == 0 {
            return Err(ArtifactError::InvalidInput(
                "range budget must be positive".into(),
            ));
        }
        self.max_range_bytes = bytes;
        Ok(self)
    }
    fn object_path(&self, key: &str, create: bool) -> Result<PathBuf, ArtifactError> {
        validate_key(key)?;
        let mut path = self.root.clone();
        let parts: Vec<_> = key.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            path.push(part);
            let last = i + 1 == parts.len();
            if create && !last {
                match std::fs::create_dir(&path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(io_error(e)),
                }
            }
            match std::fs::symlink_metadata(&path) {
                Ok(meta)
                    if meta.file_type().is_symlink()
                        || (!last && !meta.is_dir())
                        || (last && !meta.is_file()) =>
                {
                    return Err(ArtifactError::InvalidInput(
                        "nonregular storage path".into(),
                    ));
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && last => (),
                Err(e) => return Err(io_error(e)),
            }
        }
        Ok(path)
    }
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ArtifactError> + Send + 'static,
) -> Result<T, ArtifactError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ArtifactError::Io(std::io::Error::other(e)))?
}
#[async_trait]
impl ArtifactStore for FsArtifactStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        source: &Path,
    ) -> Result<ArtifactHead, ArtifactError> {
        let (store, key, source) = (self.clone(), key.to_owned(), source.to_owned());
        blocking(move || {
            let path = store.object_path(&key, true)?;
            let parent = path
                .parent()
                .ok_or_else(|| ArtifactError::InvalidInput("missing parent".into()))?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            let mut input = std::fs::File::open(source).map_err(io_error)?;
            let size_bytes = std::io::copy(&mut input, temp.as_file_mut())?;
            temp.as_file().sync_all()?;
            std::fs::hard_link(temp.path(), &path).map_err(io_error)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(ArtifactHead {
                size_bytes,
                etag: None,
            })
        })
        .await
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes, ArtifactError> {
        let len = checked_range(&range, self.max_range_bytes)?;
        let path = self.object_path(key, false)?;
        let mut file = tokio::fs::File::open(path).await.map_err(io_error)?;
        if range.end > file.metadata().await?.len() {
            return Err(integrity("range", "range exceeds object"));
        }
        file.seek(std::io::SeekFrom::Start(range.start)).await?;
        let mut bytes = vec![0; len as usize];
        file.read_exact(&mut bytes).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                integrity("range", "truncated object")
            } else {
                e.into()
            }
        })?;
        Ok(bytes.into())
    }
    async fn head(&self, key: &str) -> Result<ArtifactHead, ArtifactError> {
        let path = self.object_path(key, false)?;
        let meta = tokio::fs::metadata(path).await.map_err(io_error)?;
        Ok(ArtifactHead {
            size_bytes: meta.len(),
            etag: None,
        })
    }
    async fn delete(&self, key: &str) -> Result<(), ArtifactError> {
        let path = match self.object_path(key, false) {
            Err(ArtifactError::NotFound) => return Ok(()),
            result => result?,
        };
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(e)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    pub attempts: usize,
    pub base_delay: Duration,
    pub max_backoff: Duration,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(250),
            max_backoff: Duration::from_secs(5),
        }
    }
}
impl RetryPolicy {
    fn delay(&self, attempt: usize, retry_after: Option<&str>) -> Duration {
        if let Some(value) = retry_after {
            if let Ok(seconds) = value.trim().parse::<u64>() {
                return Duration::from_secs(seconds);
            }
            if let Ok(date) = httpdate::parse_http_date(value) {
                return date.duration_since(SystemTime::now()).unwrap_or_default();
            }
        }
        self.base_delay
            .saturating_mul(1u32.checked_shl(attempt.min(31) as u32).unwrap_or(u32::MAX))
            .min(self.max_backoff)
    }
}

/// Explicit endpoints permit loopback-only mock servers. Production endpoints require HTTPS;
/// redirects are disabled to prevent bearer-token leakage to a different host.
#[derive(Clone, Debug)]
pub struct BlobConfig {
    pub api_url: Url,
    pub read_base_url: Url,
    pub retry: RetryPolicy,
    pub max_range_bytes: u64,
    pub request_timeout: Duration,
}
impl BlobConfig {
    pub fn for_store(store_id: &str) -> Result<Self, ArtifactError> {
        if store_id.is_empty() || !store_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(ArtifactError::InvalidInput("invalid Blob store id".into()));
        }
        Ok(Self {
            api_url: parse_url("https://vercel.com/api/blob")?,
            read_base_url: parse_url(&format!(
                "https://{}.private.blob.vercel-storage.com/",
                store_id.to_ascii_lowercase()
            ))?,
            retry: RetryPolicy::default(),
            max_range_bytes: 64 << 20,
            request_timeout: Duration::from_secs(120),
        })
    }
}
fn parse_url(value: &str) -> Result<Url, ArtifactError> {
    Url::parse(value).map_err(|e| ArtifactError::InvalidInput(e.to_string()))
}
fn validate_url(url: &Url) -> Result<(), ArtifactError> {
    let loopback = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ArtifactError::InvalidInput("Blob endpoint must be HTTPS (HTTP only on loopback), with no credentials/query/fragment".into()));
    }
    Ok(())
}

#[derive(Clone)]
pub struct VercelBlobStore {
    client: Client,
    token: header::HeaderValue,
    config: BlobConfig,
}
impl VercelBlobStore {
    /// Token is intentionally not exposed by Debug or errors. `store_id` is the token's fourth
    /// underscore-separated component: vercel_blob_rw_<storeId>_<secret>.
    pub fn new(token: &str) -> Result<Self, ArtifactError> {
        let parts: Vec<_> = token.split('_').collect();
        if parts.len() != 5 || parts[0..3] != ["vercel", "blob", "rw"] || parts[4].is_empty() {
            return Err(ArtifactError::InvalidInput(
                "invalid Blob token shape".into(),
            ));
        }
        Self::with_config(token, BlobConfig::for_store(parts[3])?)
    }
    pub fn with_config(token: &str, config: BlobConfig) -> Result<Self, ArtifactError> {
        validate_url(&config.api_url)?;
        validate_url(&config.read_base_url)?;
        if !config.read_base_url.path().ends_with('/')
            || config.retry.attempts == 0
            || config.max_range_bytes == 0
            || config.request_timeout.is_zero()
        {
            return Err(ArtifactError::InvalidInput(
                "invalid Blob configuration".into(),
            ));
        }
        let mut bearer = header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ArtifactError::InvalidInput("invalid bearer token".into()))?;
        bearer.set_sensitive(true);
        // reqwest's provider-free rustls feature needs explicit initialization. A concurrent
        // constructor may win installation; in that case its process-wide provider is retained.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.request_timeout)
            .build()?;
        Ok(Self {
            client,
            token: bearer,
            config,
        })
    }
    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        request
            .header(header::AUTHORIZATION, self.token.clone())
            .header("x-api-version", "12")
            .header(header::ACCEPT_ENCODING, "identity")
    }
    fn read_url(&self, key: &str) -> Result<Url, ArtifactError> {
        validate_key(key)?;
        self.config
            .read_base_url
            .join(key)
            .map_err(|e| ArtifactError::InvalidInput(e.to_string()))
    }
    async fn send<F, Fut>(&self, mut build: F) -> Result<Response, ArtifactError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<RequestBuilder, ArtifactError>>,
    {
        for attempt in 0..self.config.retry.attempts {
            match build().await?.send().await {
                Ok(response) => {
                    let status = response.status();
                    if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                        && attempt + 1 < self.config.retry.attempts
                    {
                        let delay = self.config.retry.delay(
                            attempt,
                            response
                                .headers()
                                .get(header::RETRY_AFTER)
                                .and_then(|v| v.to_str().ok()),
                        );
                        drop(response);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Ok(response);
                }
                Err(error)
                    if (error.is_connect() || error.is_timeout())
                        && attempt + 1 < self.config.retry.attempts =>
                {
                    tokio::time::sleep(self.config.retry.delay(attempt, None)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("positive retry attempts checked at construction")
    }
}
fn status(response: &Response) -> Result<(), ArtifactError> {
    match response.status() {
        StatusCode::NOT_FOUND => Err(ArtifactError::NotFound),
        StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => Err(ArtifactError::AlreadyExists),
        s if s.is_success() => Ok(()),
        s => Err(ArtifactError::Http(s.as_u16())),
    }
}
async fn bounded_body(mut response: Response, max: u64) -> Result<Bytes, ArtifactError> {
    if response.content_length().is_some_and(|n| n > max) {
        return Err(integrity("range", "response exceeds requested length"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() as u64 > max.saturating_sub(bytes.len() as u64) {
            return Err(integrity("range", "response exceeds byte budget"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.into())
}

#[async_trait]
impl ArtifactStore for VercelBlobStore {
    async fn put_bundle_from_file(
        &self,
        key: &str,
        path: &Path,
    ) -> Result<ArtifactHead, ArtifactError> {
        validate_key(key)?;
        let mut url = self.config.api_url.clone();
        url.query_pairs_mut().append_pair("pathname", key);
        let size = tokio::fs::metadata(path).await.map_err(io_error)?.len();
        let response = self
            .send(|| async {
                // Reopen for each attempt; no buffered whole-bundle copy and no exhausted-body retry.
                let file = tokio::fs::File::open(path).await.map_err(io_error)?;
                if file.metadata().await?.len() != size {
                    return Err(integrity("upload", "source size changed"));
                }
                Ok(self
                    .authorize(self.client.put(url.clone()))
                    .header("x-vercel-blob-access", "private")
                    .header("x-add-random-suffix", "0")
                    .header("x-allow-overwrite", "0")
                    .header("x-content-type", "application/gzip")
                    .header(header::CONTENT_TYPE, "application/gzip")
                    .header(header::CONTENT_LENGTH, size)
                    .body(reqwest::Body::wrap_stream(ReaderStream::new(file))))
            })
            .await?;
        status(&response)?;
        #[derive(Deserialize)]
        struct PutResponse {
            pathname: String,
            url: String,
            etag: Option<String>,
        }
        let data: PutResponse = serde_json::from_slice(&bounded_body(response, 64 << 10).await?)?;
        if data.pathname != key || parse_url(&data.url)? != self.read_url(key)? {
            return Err(integrity(
                "upload",
                "Blob returned a different object identity",
            ));
        }
        Ok(ArtifactHead {
            size_bytes: size,
            etag: data.etag,
        })
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes, ArtifactError> {
        let len = checked_range(&range, self.config.max_range_bytes)?;
        let url = self.read_url(key)?;
        let response = self
            .send(|| async {
                Ok(self.authorize(self.client.get(url.clone())).header(
                    header::RANGE,
                    format!("bytes={}-{}", range.start, range.end - 1),
                ))
            })
            .await?;
        status(&response)?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(integrity("range", "server ignored Range"));
        }
        let value = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| integrity("range", "missing Content-Range"))?;
        let prefix = format!("bytes {}-{}/", range.start, range.end - 1);
        let total = value
            .strip_prefix(&prefix)
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| integrity("range", "wrong Content-Range"))?;
        if total < range.end
            || response
                .headers()
                .get(header::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err(integrity(
                "range",
                "invalid object extent or content encoding",
            ));
        }
        let bytes = bounded_body(response, len).await?;
        if bytes.len() as u64 != len {
            return Err(integrity("range", "short range response"));
        }
        Ok(bytes)
    }
    async fn head(&self, key: &str) -> Result<ArtifactHead, ArtifactError> {
        let url = self.read_url(key)?;
        let response = self
            .send(|| async { Ok(self.authorize(self.client.head(url.clone()))) })
            .await?;
        status(&response)?;
        let size_bytes = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| integrity("head", "missing Content-Length"))?;
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(ArtifactHead { size_bytes, etag })
    }
    async fn delete(&self, key: &str) -> Result<(), ArtifactError> {
        let object_url = self.read_url(key)?;
        let mut url = self.config.api_url.clone();
        url.set_path(&format!("{}/delete", url.path().trim_end_matches('/')));
        let body = serde_json::json!({"urls": [object_url.as_str()]});
        let response = self
            .send(|| async { Ok(self.authorize(self.client.post(url.clone())).json(&body)) })
            .await?;
        match status(&response) {
            Err(ArtifactError::NotFound) => Ok(()),
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_after_seconds_and_http_date() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay(0, Some("17")), Duration::from_secs(17));
        assert_eq!(
            policy.delay(0, Some("Thu, 01 Jan 1970 00:00:00 GMT")),
            Duration::ZERO
        );
        let future = SystemTime::now() + Duration::from_secs(120);
        let delay = policy.delay(0, Some(&httpdate::fmt_http_date(future)));
        assert!(delay >= Duration::from_secs(118) && delay <= Duration::from_secs(120));
        assert_eq!(policy.delay(1, Some("invalid")), Duration::from_millis(500));
    }
}
