//! Edge middleware (API_CONTRACT §1, §4–§6; SECURITY_REVIEW §6.2, §7.1, §10).
//!
//! [`guard`] (outermost) assigns the request id, enforces the `Host` allowlist, the header and
//! body size limits, refuses compressed request bodies, applies the exact-origin CORS policy
//! (preflights included) and fetch-metadata isolation, caps requests in flight, applies the
//! request timeout, and adds the security and `Vary` headers to every response.
//!
//! [`egress`] accounts response bytes per class (anonymous / keyed) against daily budgets that
//! survive restarts. [`anon_cache`] serves repeated anonymous GETs from memory so public demo
//! traffic does not reach the database after warm-up (SEC-10).

use std::{
    collections::HashMap,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::{
    Shared,
    error::{ApiError, REQUEST_ID, seconds_to_utc_midnight},
};

/// Largest request body (API_CONTRACT §1).
pub const MAX_BODY_BYTES: usize = 1 << 20;
/// Largest total size of request headers (SEC-5).
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
const ALLOW_METHODS: &str = "GET, POST, PATCH, DELETE";
const ALLOW_HEADERS: &str = "Authorization, Content-Type, If-Match, If-None-Match, X-Request-Id";
const EXPOSE_HEADERS: &str =
    "ETag, X-Request-Id, Retry-After, X-RateLimit-Limit, X-RateLimit-Remaining, Location";
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const INFLIGHT_WAIT: Duration = Duration::from_secs(2);
/// Responses above this size are refused once a class's daily egress budget is spent.
const EGRESS_SMALL_RESPONSE: usize = 64 * 1024;
/// Egress is flushed to Postgres every this many bytes per class.
const EGRESS_FLUSH_BYTES: i64 = 5_000_000;

fn valid_request_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn host_name(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    if host.starts_with('[') {
        return host
            .split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or(host);
    }
    host.split(':').next().unwrap_or_default().to_string()
}

/// Whether `origin` may call the API: listed exactly, or the server's own origin (same-origin
/// requests from a UI served by this process still carry `Origin` on POST).
fn origin_allowed(state: &Shared, origin: &str, host: Option<&str>) -> bool {
    if state.cfg.cors_allowed_origins.iter().any(|o| o == origin) {
        return true;
    }
    match (origin.split_once("://"), host) {
        (Some((_, authority)), Some(host)) => authority.eq_ignore_ascii_case(host),
        _ => false,
    }
}

fn header_bytes(headers: &HeaderMap) -> usize {
    headers
        .iter()
        .map(|(k, v)| k.as_str().len() + v.as_bytes().len() + 4)
        .sum()
}

/// The outermost layer of every route.
pub async fn guard(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| valid_request_id(v))
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let header_id = HeaderValue::from_str(&id).ok();
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut response = REQUEST_ID
        .scope(id, guarded(state.clone(), request, next, origin.clone()))
        .await;
    let headers = response.headers_mut();
    if let Some(v) = header_id {
        headers.insert("x-request-id", v);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    headers.append(header::VARY, HeaderValue::from_static("Authorization"));
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

async fn guarded(state: Shared, request: Request, next: Next, origin: Option<String>) -> Response {
    let path = request.uri().path().to_string();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // Host allowlist (DNS rebinding, SEC-4). `/healthz` answers for any host.
    if path != "/healthz" {
        let ok = host
            .as_deref()
            .is_some_and(|h| state.cfg.allowed_hosts.contains(&host_name(h)));
        if !ok {
            return ApiError::forbidden("this host name is not served here").into_response();
        }
    }
    if header_bytes(request.headers()) > MAX_HEADER_BYTES {
        return ApiError::new(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "invalid_request",
            "request headers are too large",
        )
        .into_response();
    }
    if request.headers().contains_key(header::CONTENT_ENCODING) {
        return ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "compressed request bodies are not accepted",
        )
        .into_response();
    }
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return payload_too_large().into_response();
    }

    let allowed_origin = match &origin {
        None => None,
        Some(o) if origin_allowed(&state, o, host.as_deref()) => Some(o.clone()),
        Some(_) => {
            // Foreign origin: a preflight gets no CORS headers, anything else 403.
            return ApiError::forbidden("requests from this origin are not allowed")
                .into_response();
        }
    };
    let preflight = request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    if preflight {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(origin) = &allowed_origin {
            cors_headers(response.headers_mut(), origin, true);
        }
        return response;
    }

    // Fetch metadata (SEC-32): cross-site navigations and no-cors embeds never reach a handler.
    let site = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok());
    let mode = request
        .headers()
        .get("sec-fetch-mode")
        .and_then(|v| v.to_str().ok());
    if site == Some("cross-site") && matches!(mode, Some("no-cors" | "navigate")) {
        return ApiError::forbidden("cross-site embedding is not allowed").into_response();
    }

    let mutation = !matches!(*request.method(), Method::GET | Method::HEAD);
    let mut response = if path == "/healthz" {
        next.run(request).await
    } else {
        let permit =
            match tokio::time::timeout(INFLIGHT_WAIT, state.inflight.clone().acquire_owned()).await
            {
                Ok(Ok(permit)) => permit,
                _ => return ApiError::unavailable("busy", 2).into_response(),
            };
        let limit = if mutation {
            WRITE_TIMEOUT
        } else {
            READ_TIMEOUT
        };
        let result = tokio::time::timeout(limit, next.run(request)).await;
        drop(permit);
        match result {
            Ok(response) => response,
            Err(_) => ApiError::unavailable("busy", 5).into_response(),
        }
    };
    if let Some(origin) = &allowed_origin {
        cors_headers(response.headers_mut(), origin, false);
    }
    response
}

fn cors_headers(headers: &mut HeaderMap, origin: &str, preflight: bool) {
    if let Ok(v) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    if preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(ALLOW_METHODS),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOW_HEADERS),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
    } else {
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(EXPOSE_HEADERS),
        );
    }
}

pub fn payload_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request",
        "the request body exceeds 1 MiB",
    )
}

pub async fn no_such_endpoint(OriginalUri(uri): OriginalUri) -> ApiError {
    let _ = uri;
    ApiError::not_found("endpoint")
}

// ----- egress -----

/// Daily response-byte accounting per class, persisted every 5 MB (SECURITY_REVIEW §6.2).
#[derive(Default)]
pub struct Egress {
    anonymous: ClassCounter,
    keyed: ClassCounter,
}

#[derive(Default)]
struct ClassCounter {
    /// Bytes counted today (including the value loaded from Postgres).
    total: AtomicI64,
    /// Bytes not yet flushed.
    pending: AtomicI64,
    loaded: AtomicBool,
    day: Mutex<Option<chrono::NaiveDate>>,
}

impl ClassCounter {
    fn roll(&self) {
        let today = chrono::Utc::now().date_naive();
        let mut day = self.day.lock().unwrap_or_else(PoisonError::into_inner);
        if *day != Some(today) {
            *day = Some(today);
            self.total.store(0, Ordering::SeqCst);
            self.pending.store(0, Ordering::SeqCst);
            self.loaded.store(false, Ordering::SeqCst);
        }
    }
}

impl Egress {
    fn class(&self, anonymous: bool) -> (&ClassCounter, &'static str) {
        if anonymous {
            (&self.anonymous, "anonymous")
        } else {
            (&self.keyed, "keyed")
        }
    }

    /// Bytes served today in a class.
    pub fn today(&self, anonymous: bool) -> i64 {
        self.class(anonymous).0.total.load(Ordering::SeqCst)
    }
}

pub async fn egress(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let anonymous = !request.headers().contains_key(header::AUTHORIZATION);
    let (counter, class) = state.egress.class(anonymous);
    counter.roll();
    let budget = if anonymous {
        state.cfg.egress_anonymous_daily_bytes
    } else {
        state.cfg.egress_keyed_daily_bytes
    };
    let response = next.run(request).await;
    let size = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    if size as usize > EGRESS_SMALL_RESPONSE {
        // First heavy response after start: load today's persisted value once.
        if !counter.loaded.swap(true, Ordering::SeqCst)
            && let Ok(stored) = state.db.egress_today(class).await
        {
            counter.total.fetch_max(stored, Ordering::SeqCst);
        }
        if counter.total.load(Ordering::SeqCst) + size > budget {
            return ApiError::unavailable("busy", seconds_to_utc_midnight())
                .with_details(json!({ "reason": "busy", "budget": "egress" }))
                .into_response();
        }
    }
    counter.total.fetch_add(size, Ordering::SeqCst);
    let pending = counter.pending.fetch_add(size, Ordering::SeqCst) + size;
    if pending >= EGRESS_FLUSH_BYTES {
        let flush = counter.pending.swap(0, Ordering::SeqCst);
        if flush > 0 {
            let db = state.db.clone();
            tokio::spawn(async move {
                if let Err(e) = db.add_egress(class, flush).await {
                    tracing::warn!(error = %e, "egress counter not persisted");
                }
            });
        }
    }
    response
}

// ----- anonymous response cache -----

const ANON_TTL: Duration = Duration::from_secs(30);
const ANON_TTL_JOBS: Duration = Duration::from_secs(2);
const ANON_MAX_ENTRIES: usize = 512;
const ANON_MAX_BYTES: usize = 32 << 20;

#[derive(Clone)]
struct Cached {
    generation: u64,
    at: Instant,
    status: StatusCode,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Bytes,
}

/// Responses of anonymous GETs, valid while no state change happened (`WorkerHub::generation`)
/// and for at most 30 s (2 s for job status).
#[derive(Default)]
pub struct AnonCache {
    entries: Mutex<HashMap<String, Cached>>,
}

pub async fn anon_cache(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let cacheable = request.method() == Method::GET
        && !request.headers().contains_key(header::AUTHORIZATION)
        && request.uri().path().starts_with("/api/v1/");
    if !cacheable {
        return next.run(request).await;
    }
    let key = request.uri().to_string();
    let ttl = if request.uri().path().starts_with("/api/v1/jobs") {
        ANON_TTL_JOBS
    } else {
        ANON_TTL
    };
    let generation = state.hub.generation.load(Ordering::SeqCst);
    let hit = state
        .anon_cache
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .filter(|c| c.generation == generation && c.at.elapsed() < ttl)
        .cloned();
    if let Some(c) = hit {
        let mut response = Response::new(Body::from(c.body));
        *response.status_mut() = c.status;
        for (k, v) in c.headers {
            response.headers_mut().insert(k, v);
        }
        return response;
    }
    let response = next.run(request).await;
    if response.status() != StatusCode::OK {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, state.cfg.anonymous_max_response_bytes * 4).await
    else {
        return ApiError::internal("response body unreadable").into_response();
    };
    let headers: Vec<(HeaderName, HeaderValue)> = parts
        .headers
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "content-type"
                    | "cache-control"
                    | "etag"
                    | "content-disposition"
                    | "content-location"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    {
        let mut entries = state
            .anon_cache
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let used: usize = entries.values().map(|c| c.body.len()).sum();
        if entries.len() >= ANON_MAX_ENTRIES || used + bytes.len() > ANON_MAX_BYTES {
            entries.clear();
        }
        entries.insert(
            key,
            Cached {
                generation,
                at: Instant::now(),
                status: parts.status,
                headers,
                body: bytes.clone(),
            },
        );
    }
    Response::from_parts(parts, Body::from(bytes))
}
