//! Response and request plumbing shared by the handlers: wire-encoded JSON with caching
//! headers, byte-bounded pages, opaque cursors and the strict JSON body extractor.

use std::sync::OnceLock;

use axum::{
    body::Body,
    extract::{FromRequest, FromRequestParts, Request},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::{auth::Caller, edge, error::ApiError};
use crate::wire;

/// `Cache-Control` classes of API_CONTRACT §6.
pub enum Cache {
    /// Job status, `/status`, `/whoami`, system routes.
    NoStore,
    /// Mutable workspace objects; validator = `ETag`.
    Revalidate,
    /// Completed-run data and dataset versions: content-addressed.
    Immutable,
}

/// Serializes `value`, converts it to the wire encoding and attaches caching headers. The
/// `ETag` is a digest of the body, so `If-None-Match` revalidation is exact.
pub fn json<T: Serialize>(
    status: StatusCode,
    value: &T,
    cache: Cache,
) -> Result<Response, ApiError> {
    let mut v = serde_json::to_value(value)?;
    wire::to_wire(&mut v).map_err(ApiError::internal)?;
    json_value(status, &v, cache)
}

/// Like [`json`] for an already wire-encoded value.
pub fn json_value(status: StatusCode, v: &Value, cache: Cache) -> Result<Response, ApiError> {
    Ok(json_bytes(status, serde_json::to_vec(v)?, cache))
}

/// Like [`json`] for an already serialized wire-encoded body. The body has an exact length,
/// which `edge::egress` measures and counts.
pub fn json_bytes(status: StatusCode, bytes: Vec<u8>, cache: Cache) -> Response {
    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&bytes))[..32]);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    apply_cache(h, cache, Some(&etag));
    response
}

pub fn apply_cache(h: &mut HeaderMap, cache: Cache, etag: Option<&str>) {
    let value = match cache {
        Cache::NoStore => "no-store",
        Cache::Revalidate => "private, no-cache",
        Cache::Immutable => "private, max-age=31536000, immutable",
    };
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    if !matches!(cache, Cache::NoStore)
        && let Some(etag) = etag
        && let Ok(v) = HeaderValue::from_str(etag)
    {
        h.insert(header::ETAG, v);
    }
}

/// Answers 304 when the request's `If-None-Match` matches the response `ETag`.
pub fn conditional(request_headers: &HeaderMap, response: Response) -> Response {
    let matches = response.headers().get(header::ETAG).is_some_and(|etag| {
        let etag = etag.to_str().unwrap_or("");
        request_headers
            .get_all(header::IF_NONE_MATCH)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|tag| {
                let tag = tag.trim();
                tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag
            })
    });
    if matches {
        let mut not_modified = Response::new(Body::empty());
        *not_modified.status_mut() = StatusCode::NOT_MODIFIED;
        for name in [header::ETAG, header::CACHE_CONTROL] {
            if let Some(v) = response.headers().get(&name) {
                not_modified.headers_mut().insert(name, v.clone());
            }
        }
        not_modified
    } else {
        response
    }
}

/// Why a page ended before the end of the data.
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TruncatedBy {
    Limit,
    Bytes,
}

/// A cursor page (API_CONTRACT §7).
#[derive(Serialize, ToSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Opaque; pass back as `cursor` with the same filters.
    pub next_cursor: Option<String>,
    pub truncated_by: Option<TruncatedBy>,
}

/// Response byte cap for this caller (SEC-12: anonymous pages ≤ 1 MiB).
pub fn byte_cap(state: &super::AppState, caller: &Caller) -> usize {
    if caller.is_anonymous() {
        state.cfg.anonymous_max_response_bytes
    } else {
        state.cfg.max_response_bytes
    }
}

/// Room left for items after the page envelope.
pub const ENVELOPE_RESERVE: usize = 4096;

/// One item as a wire value and its serialized size plus a separating comma.
pub fn wire_item<T: Serialize>(item: &T) -> Result<(Value, usize), ApiError> {
    let mut v = serde_json::to_value(item)?;
    wire::to_wire(&mut v).map_err(ApiError::internal)?;
    let size = serde_json::to_vec(&v)?.len() + 1;
    Ok((v, size))
}

/// Converts items to wire values until `cap` bytes; returns the values and whether the byte
/// cap ended the page early. A single item larger than the cap is an internal error.
pub fn bounded<T: Serialize>(items: &[T], cap: usize) -> Result<(Vec<Value>, bool), ApiError> {
    let budget = cap.saturating_sub(ENVELOPE_RESERVE);
    let mut out = Vec::with_capacity(items.len());
    let mut used = 0usize;
    for item in items {
        let (v, size) = wire_item(item)?;
        if used + size > budget {
            if out.is_empty() {
                return Err(ApiError::internal("a single item exceeds the response cap"));
            }
            return Ok((out, true));
        }
        used += size;
        out.push(v);
    }
    Ok((out, false))
}

/// Key of the cursor MACs, derived from the API key pepper by [`set_cursor_key`] when the state
/// is built (one pepper per process). Without it (unit tests of pure page functions) the MAC is
/// keyed with zeros: still bound to the position and the filters, but forgeable.
static CURSOR_KEY: OnceLock<[u8; 32]> = OnceLock::new();

/// Keys every cursor with `secret` (the API key pepper); the first call wins.
pub fn set_cursor_key(secret: &[u8]) {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(b"blackbox cursor v1");
    let _ = CURSOR_KEY.set(mac.finalize().into_bytes().into());
}

/// HMAC-SHA-256 of a cursor's position bytes and the filters it belongs to.
fn cursor_mac(position: &[u8], filters: &str) -> Hmac<Sha256> {
    let key = CURSOR_KEY.get().copied().unwrap_or_default();
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC accepts any key length");
    mac.update(&(position.len() as u64).to_be_bytes());
    mac.update(position);
    mac.update(filters.as_bytes());
    mac
}

/// Opaque cursor: base64url JSON of a position, `.`, and the base64url HMAC-SHA-256 (keyed with
/// a server secret) of that position and the filters it belongs to, so a cursor can be neither
/// forged nor reused with other filters (callers include the resource id in `filters`).
pub fn encode_cursor<P: Serialize>(position: &P, filters: &str) -> String {
    let data = serde_json::to_vec(position).unwrap_or_default();
    let tag = cursor_mac(&data, filters).finalize().into_bytes();
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&data),
        URL_SAFE_NO_PAD.encode(tag)
    )
}

pub fn decode_cursor<P: DeserializeOwned>(cursor: &str, filters: &str) -> Result<P, ApiError> {
    let invalid = || ApiError::invalid_field("cursor", "invalid cursor");
    if cursor.len() > 1024 {
        return Err(invalid());
    }
    let (data, tag) = cursor.split_once('.').ok_or_else(invalid)?;
    let data = URL_SAFE_NO_PAD.decode(data).map_err(|_| invalid())?;
    let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| invalid())?;
    if cursor_mac(&data, filters).verify_slice(&tag).is_err() {
        return Err(ApiError::invalid_field(
            "cursor",
            "the cursor belongs to different filters",
        ));
    }
    serde_json::from_slice(&data).map_err(|_| invalid())
}

/// A query extractor whose deserialization failures use the API error envelope, not Axum's
/// plain-text rejection. Oversized integers, duplicate fields and missing fields are 400s.
pub struct Query<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Query<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|query| Self(query.0))
            .map_err(|_| {
                ApiError::invalid_request("the query parameters are malformed or out of range")
            })
    }
}

/// A JSON request body: ≤ 1 MiB, `application/json`, nanosecond strings accepted, unknown
/// fields refused by the target type.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, ApiError> {
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !content_type.starts_with("application/json") {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_request",
                "the request body must be application/json",
            ));
        }
        let bytes = axum::body::to_bytes(request.into_body(), edge::MAX_BODY_BYTES)
            .await
            .map_err(|e| {
                if format!("{e:?}").contains("LengthLimit")
                    || e.to_string().contains("length limit")
                {
                    edge::payload_too_large()
                } else {
                    ApiError::invalid_request("the request body could not be read")
                }
            })?;
        let mut value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::invalid_request(format!("malformed JSON: {e}")))?;
        wire::from_wire(&mut value);
        serde_json::from_value(value)
            .map(ApiJson)
            .map_err(|e| ApiError::invalid_request(format!("invalid request body: {e}")))
    }
}

/// `If-Match: "v<version>"` (required for PATCH).
pub fn if_match(headers: &HeaderMap, required: bool) -> Result<Option<i64>, ApiError> {
    match headers.get(header::IF_MATCH) {
        None if required => Err(ApiError::invalid_field(
            "If-Match",
            "this request needs an If-Match: \"v<version>\" header",
        )),
        None => Ok(None),
        Some(v) => v
            .to_str()
            .ok()
            .and_then(|v| v.trim().trim_matches('"').strip_prefix('v'))
            .and_then(|v| v.parse().ok())
            .map(Some)
            .ok_or_else(|| ApiError::invalid_field("If-Match", "If-Match must be \"v<version>\"")),
    }
}

/// Labels and titles: ≤ 100 characters, no control characters (SEC-7).
pub fn check_label(field: &str, label: &str, allow_empty: bool) -> Result<String, ApiError> {
    let label = label.trim();
    if label.chars().count() > 100 {
        return Err(ApiError::invalid_config(vec![format!(
            "{field} is longer than 100 characters"
        )]));
    }
    if label.chars().any(char::is_control) {
        return Err(ApiError::invalid_config(vec![format!(
            "{field} contains control characters"
        )]));
    }
    if label.is_empty() && !allow_empty {
        return Err(ApiError::invalid_config(vec![format!("{field} is empty")]));
    }
    Ok(label.to_string())
}

/// Parses a decimal i64 query value (`*_ns`).
pub fn ns_param(field: &str, value: Option<&str>) -> Result<Option<i64>, ApiError> {
    value
        .map(|v| {
            v.parse::<i64>().map_err(|_| {
                ApiError::invalid_field(field, format!("{field} must be a decimal integer"))
            })
        })
        .transpose()
}

/// Epoch-ns query value → timestamp.
pub fn at_param(
    field: &str,
    value: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    Ok(ns_param(field, value)?.map(chrono::DateTime::from_timestamp_nanos))
}

/// Parses a UUID path segment; anything else is a 404 like an unknown id.
pub fn uuid_param(resource: &str, id: &str) -> Result<uuid::Uuid, ApiError> {
    id.parse().map_err(|_| ApiError::not_found(resource))
}

/// Validates a page size.
pub fn limit_param(limit: Option<i64>, default: i64, max: i64) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(default);
    if !(1..=max).contains(&limit) {
        return Err(ApiError::invalid_field(
            "limit",
            format!("limit must be between 1 and {max}"),
        ));
    }
    Ok(limit)
}
