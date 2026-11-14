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
        LazyLock, Mutex, PoisonError,
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
use utoipa::openapi::{RefOr, path::ParameterIn};

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
/// Non-error responses above this size read today's persisted egress counter first and are
/// refused once a class's daily egress budget is spent; tiny ones and errors are always served
/// (and counted).
const EGRESS_TINY_RESPONSE: usize = 2 * 1024;
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
    // Creating a comparison may load and align two runs: it has its own bounded timeout
    // (`BB_COMPARISON_TIMEOUT_S`); every other mutation keeps the short write timeout.
    let comparison = request.method() == Method::POST && path == "/api/v1/comparisons";
    let mut response = if path == "/healthz" {
        next.run(request).await
    } else {
        let permit =
            match tokio::time::timeout(INFLIGHT_WAIT, state.inflight.clone().acquire_owned()).await
            {
                Ok(Ok(permit)) => permit,
                _ => return ApiError::unavailable("busy", 2).into_response(),
            };
        let limit = if comparison {
            state.cfg.comparison_timeout
        } else if mutation {
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
    /// Today's persisted value has been added to `total`.
    loaded: AtomicBool,
    /// A flush that also loads today's persisted value is in flight.
    loading: AtomicBool,
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
            self.loading.store(false, Ordering::SeqCst);
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

/// The refusal of a response body above the caller's cap (API_CONTRACT §2, §7): 422
/// `response_too_large` with `details = {limit_bytes, hint}`. Handlers bound their output first
/// (pages, analytics series downsampled to fit the cap), so a client sees it only when even the
/// smallest representation does not fit; never silently truncated JSON.
pub fn response_too_large(cap: usize, anonymous: bool) -> ApiError {
    tracing::warn!(request_id = %super::error::request_id(), cap, anonymous, "response body above the cap refused");
    let hint = if anonymous {
        "request less (a page, a narrower window, or one series via its /analytics/series path), \
         or send an API key for the larger keyed limit"
    } else {
        "request less (a page, a narrower window, or one series via its /analytics/series path)"
    };
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "response_too_large",
        format!("the response would exceed the {cap}-byte limit for this caller"),
    )
    .with_details(json!({ "limit_bytes": cap, "hint": hint }))
}

/// Buffers `response` (every handler builds its body in memory) and returns it with its exact
/// body length; `Err` when the body is longer than `cap`. Bundle exports
/// (`application/gzip`, keyed only, bounded by the output limit) are measured but not capped.
async fn measured(
    response: Response,
    cap: usize,
    anonymous: bool,
) -> Result<(Response, usize), ApiError> {
    let (parts, body) = response.into_parts();
    let export = parts
        .headers
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes() == b"application/gzip");
    let limit = if export { usize::MAX } else { cap };
    let bytes = axum::body::to_bytes(body, limit)
        .await
        .map_err(|_| response_too_large(cap, anonymous))?;
    let size = bytes.len();
    Ok((Response::from_parts(parts, Body::from(bytes)), size))
}

/// The single response-size path of every route: enforces the per-class body cap (anonymous
/// 1 MiB, keyed `BB_MAX_RESPONSE_BYTES`) on the serialized body itself, and adds the exact number
/// of body bytes sent to the daily egress counter of the class (cached bodies included; it sits
/// outside [`anon_cache`]).
///
/// Today's persisted counter is added to the in-memory total once per class and day: read before
/// the first refusable response (a non-error body above 2 KiB), or taken from the first flush
/// (`add_egress` returns the new stored total). Until then only that one loading flush may run,
/// so the persisted value is added exactly once and the total stays exact across restarts. A
/// refusable response is refused (503 `unavailable`) while the stored value cannot be read (fail
/// closed), and once the budget is spent (503 `busy`), so tiny pages cannot keep draining it; it
/// reserves its bytes atomically against the budget, so concurrent responses cannot overshoot it.
pub async fn egress(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let anonymous = !request.headers().contains_key(header::AUTHORIZATION);
    let head = request.method() == Method::HEAD;
    let (counter, class) = state.egress.class(anonymous);
    counter.roll();
    let (budget, cap) = if anonymous {
        (
            state.cfg.egress_anonymous_daily_bytes,
            state.cfg.anonymous_max_response_bytes,
        )
    } else {
        (
            state.cfg.egress_keyed_daily_bytes,
            state.cfg.max_response_bytes,
        )
    };
    let response = next.run(request).await;
    let (response, size) = match measured(response, cap, anonymous).await {
        Ok(measured) => measured,
        Err(refusal) => measured(refusal.into_response(), cap, anonymous)
            .await
            .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR.into_response(), 0)),
    };
    // A HEAD response carries no body on the wire.
    let size = if head { 0 } else { size as i64 };
    let refusable = size as usize > EGRESS_TINY_RESPONSE
        && !response.status().is_client_error()
        && !response.status().is_server_error();
    if refusable && !counter.loaded.load(Ordering::SeqCst) {
        match state.db.egress_today(class).await {
            Ok(stored) => {
                if !counter.loaded.swap(true, Ordering::SeqCst) {
                    counter.total.fetch_add(stored, Ordering::SeqCst);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, class, "egress counter not loaded; response refused");
                return ApiError::unavailable("database", 5).into_response();
            }
        }
    }
    let total = counter.total.fetch_add(size, Ordering::SeqCst) + size;
    if refusable && total > budget {
        counter.total.fetch_sub(size, Ordering::SeqCst);
        return ApiError::unavailable("busy", seconds_to_utc_midnight())
            .with_details(json!({ "reason": "busy", "budget": "egress" }))
            .into_response();
    }
    let pending = counter.pending.fetch_add(size, Ordering::SeqCst) + size;
    if pending < EGRESS_FLUSH_BYTES {
        return response;
    }
    let loads = !counter.loaded.load(Ordering::SeqCst);
    if loads && counter.loading.swap(true, Ordering::SeqCst) {
        return response;
    }
    let flush = counter.pending.swap(0, Ordering::SeqCst);
    if flush == 0 {
        counter.loading.store(false, Ordering::SeqCst);
        return response;
    }
    crate::metrics::Metrics::inc(&state.metrics.egress_flushes_total);
    let state = state.clone();
    tokio::spawn(async move {
        let (counter, class) = state.egress.class(anonymous);
        match state.db.add_egress(class, flush).await {
            Ok(stored) => {
                if loads && !counter.loaded.swap(true, Ordering::SeqCst) {
                    counter.total.fetch_add(stored - flush, Ordering::SeqCst);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "egress counter not persisted");
                counter.pending.fetch_add(flush, Ordering::SeqCst);
            }
        }
        if loads {
            counter.loading.store(false, Ordering::SeqCst);
        }
    });
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
    /// Last time it was stored or served (eviction order).
    used: Instant,
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

impl AnonCache {
    /// Stores `entry`, first dropping entries of an older generation or past the longest TTL,
    /// then the least recently used ones until it fits in 512 entries / 32 MiB.
    fn insert(&self, key: String, entry: Cached) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.remove(&key);
        entries.retain(|_, c| c.generation == entry.generation && c.at.elapsed() < ANON_TTL);
        let mut used: usize = entries.values().map(|c| c.body.len()).sum();
        while entries.len() >= ANON_MAX_ENTRIES || used + entry.body.len() > ANON_MAX_BYTES {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, c)| c.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(c) = entries.remove(&oldest) {
                used -= c.body.len();
            }
        }
        entries.insert(key, entry);
    }
}

/// Query parameter names of every documented GET route, from the OpenAPI document: the path
/// split at `/` (a `{…}` segment matches any one segment) and its query parameter names.
static ROUTE_QUERY_PARAMS: LazyLock<Vec<(Vec<String>, Vec<String>)>> = LazyLock::new(|| {
    let (_, api) = super::api_router().split_for_parts();
    api.paths
        .paths
        .into_iter()
        .filter_map(|(path, item)| {
            let names = item
                .parameters
                .into_iter()
                .chain(item.get?.parameters)
                .flatten()
                .filter_map(|p| match p {
                    RefOr::T(p) if p.parameter_in == ParameterIn::Query => Some(p.name),
                    _ => None,
                })
                .collect();
            Some((path.split('/').map(str::to_string).collect(), names))
        })
        .collect()
});

/// The query parameter names the GET route(s) matching `path` read; `None` for an undocumented
/// path.
fn known_query_params(path: &str) -> Option<Vec<&'static str>> {
    let segments: Vec<&str> = path.split('/').collect();
    let mut known: Option<Vec<&'static str>> = None;
    for (template, names) in ROUTE_QUERY_PARAMS.iter() {
        let matches = template.len() == segments.len()
            && template
                .iter()
                .zip(&segments)
                .all(|(t, s)| t == s || (t.starts_with('{') && !s.is_empty()));
        if matches {
            known
                .get_or_insert_with(Vec::new)
                .extend(names.iter().map(String::as_str));
        }
    }
    known
}

/// The cache key of an anonymous GET: the path and the route's known query pairs in a canonical
/// order (stable by parameter name, so repeated parameters keep their meaning), without empty
/// pairs. Reordered or padded query strings and unknown parameters (which the handlers ignore)
/// share one entry. `None` (not cached) for an undocumented path or an encoded parameter name,
/// which could decode to a known one.
fn cache_key(uri: &axum::http::Uri) -> Option<String> {
    let known = known_query_params(uri.path())?;
    let mut pairs = Vec::new();
    for pair in uri.query().unwrap_or("").split('&') {
        let name = pair.split('=').next().unwrap_or(pair);
        if name.contains(['%', '+']) {
            return None;
        }
        if !pair.is_empty() && known.contains(&name) {
            pairs.push(pair);
        }
    }
    pairs.sort_by_key(|p| p.split('=').next().unwrap_or(p));
    Some(if pairs.is_empty() {
        uri.path().to_string()
    } else {
        format!("{}?{}", uri.path(), pairs.join("&"))
    })
}

pub async fn anon_cache(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let cacheable = request.method() == Method::GET
        && !request.headers().contains_key(header::AUTHORIZATION)
        && request.uri().path().starts_with("/api/v1/");
    let Some(key) = cacheable.then(|| cache_key(request.uri())).flatten() else {
        return next.run(request).await;
    };
    let ttl = if request.uri().path().starts_with("/api/v1/jobs") {
        ANON_TTL_JOBS
    } else {
        ANON_TTL
    };
    let generation = state.data_generation();
    let hit = state
        .anon_cache
        .entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_mut(&key)
        .filter(|c| c.generation == generation && c.at.elapsed() < ttl)
        .map(|c| {
            c.used = Instant::now();
            c.clone()
        });
    if let Some(c) = hit {
        let mut response = Response::new(Body::from(c.body));
        *response.status_mut() = c.status;
        for (k, v) in c.headers {
            response.headers_mut().insert(k, v);
        }
        return response;
    }
    let response = next.run(request).await;
    // Only bodies within the anonymous cap are cached; a longer one passes through to `egress`,
    // which refuses it.
    let within_cap = axum::body::HttpBody::size_hint(response.body())
        .exact()
        .is_some_and(|n| n <= state.cfg.anonymous_max_response_bytes as u64);
    if response.status() != StatusCode::OK || !within_cap {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, state.cfg.anonymous_max_response_bytes).await else {
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
    let now = Instant::now();
    state.anon_cache.insert(
        key,
        Cached {
            generation,
            at: now,
            used: now,
            status: parts.status,
            headers,
            body: bytes.clone(),
        },
    );
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::{body::Bytes, http::StatusCode};

    use super::{ANON_MAX_ENTRIES, AnonCache, Cached, cache_key};

    #[test]
    fn anonymous_cache_keys_are_normalized() {
        let key = |s: &str| cache_key(&s.parse().unwrap()).unwrap();
        assert_eq!(key("/api/v1/runs"), "/api/v1/runs");
        assert_eq!(key("/api/v1/runs?&&"), "/api/v1/runs");
        assert_eq!(
            key("/api/v1/runs?sort=net_pnl_desc&&limit=5"),
            key("/api/v1/runs?limit=5&sort=net_pnl_desc")
        );
        // Repeated parameters keep their order: the first `limit` wins in the handlers.
        assert_ne!(
            key("/api/v1/runs?limit=1&limit=2"),
            key("/api/v1/runs?limit=2&limit=1")
        );
        assert_eq!(
            key("/api/v1/jobs?state=failed&b=1&state=completed"),
            "/api/v1/jobs?state=failed&state=completed"
        );
    }

    #[test]
    fn anonymous_cache_keys_hold_only_the_routes_known_parameters() {
        let key = |s: &str| cache_key(&s.parse().unwrap());
        // Unknown parameters share the entry of the request without them.
        assert_eq!(
            key("/api/v1/runs/r1/timeline?x=1&t0_ns=5&cursor=c&y").unwrap(),
            "/api/v1/runs/r1/timeline?cursor=c&t0_ns=5"
        );
        assert_eq!(key("/api/v1/whoami?x=2").unwrap(), "/api/v1/whoami");
        // A known parameter of another route is unknown here.
        assert_eq!(key("/api/v1/status?limit=3").unwrap(), "/api/v1/status");
        // Encoded names may decode to a known one; undocumented paths are not cached.
        assert_eq!(key("/api/v1/runs?li%6Dit=1"), None);
        assert_eq!(key("/api/v1/runs?a+b=1"), None);
        assert_eq!(key("/api/v1/no/such/path"), None);
    }

    #[test]
    fn anonymous_cache_evicts_the_least_recently_used_entry() {
        let cache = AnonCache::default();
        let start = Instant::now();
        let entry = |i: u64| Cached {
            generation: 1,
            at: start,
            used: start + Duration::from_millis(i),
            status: StatusCode::OK,
            headers: Vec::new(),
            body: Bytes::from_static(b"{}"),
        };
        for i in 0..ANON_MAX_ENTRIES as u64 {
            cache.insert(format!("k{i}"), entry(i + 1));
        }
        // `k0` was served most recently: `k1` goes, the others stay.
        cache.entries.lock().unwrap().get_mut("k0").unwrap().used = start + Duration::from_secs(10);
        cache.insert("new".into(), entry(10_000));
        let entries = cache.entries.lock().unwrap();
        assert_eq!(entries.len(), ANON_MAX_ENTRIES);
        assert!(entries.contains_key("k0") && entries.contains_key("new"));
        assert!(!entries.contains_key("k1"));
    }
}
