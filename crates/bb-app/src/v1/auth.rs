//! Authentication, scopes and request-rate limits (API_CONTRACT §3, §5; SECURITY_REVIEW §9).
//!
//! Verification order: header shape → key check tag (HMAC with `BB_API_KEY_PEPPER`, constant
//! time, no database) → digest lookup → scopes. A forged or random `bbk_` key is refused before
//! any database access (SEC-31). No header = anonymous = the public demo workspace, read-only;
//! an anonymous mutation is answered 401 before its body is read (SEC-1). Keys in the query
//! string are ignored.
//!
//! A refused key that resolves to a workspace (revoked, expired, or lacking the route's scope)
//! writes one `auth.denied` audit row, at most one per key per minute (SECURITY_REVIEW §15,
//! SEC-25). Anonymous and forged-key traffic never writes audit rows.

use std::{
    cell::Cell,
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, Method, StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bb_db::{ApiKey, KeyLookup, ReadClass, Scope, Workspace};
use governor::{
    DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Quota, RateLimiter, clock::Clock,
};
use uuid::Uuid;

use super::{Shared, error::ApiError};
use crate::config::AppConfig;

/// The caller of the request being handled, visible below the handlers (dataset registration
/// attribution, the Blob read class) and collecting a scope refusal for its audit row.
struct RequestScope {
    key: Option<Uuid>,
    workspace: Uuid,
    denied_scope: Cell<Option<&'static str>>,
}

tokio::task_local! {
    static REQUEST: RequestScope;
}

/// Who issued the request being handled.
#[derive(Clone, Copy, Debug)]
pub struct RequestActor {
    /// `None` for an anonymous request.
    pub key: Option<Uuid>,
    /// The key's workspace, or the demo workspace for anonymous requests.
    pub workspace: Uuid,
}

/// The caller of the current request, or `None` outside a request (CLI, worker, seeding).
pub fn request_actor() -> Option<RequestActor> {
    REQUEST
        .try_with(|r| RequestActor {
            key: r.key,
            workspace: r.workspace,
        })
        .ok()
}

/// The Blob read class of the current task (SECURITY_REVIEW §6.3): anonymous or keyed inside a
/// request, `System` outside one (the worker).
pub fn read_class() -> ReadClass {
    match REQUEST.try_with(|r| r.key.is_some()) {
        Ok(true) => ReadClass::Keyed,
        Ok(false) => ReadClass::Anonymous,
        Err(_) => ReadClass::System,
    }
}

/// The authenticated (or anonymous) caller of one request.
#[derive(Clone, Debug)]
pub struct Caller {
    pub key: Option<ApiKey>,
    /// The key's workspace, or the demo workspace for anonymous callers.
    pub workspace: Workspace,
    pub demo: Workspace,
}

impl Caller {
    pub fn is_anonymous(&self) -> bool {
        self.key.is_none()
    }

    pub fn key_id(&self) -> Option<Uuid> {
        self.key.as_ref().map(|k| k.id)
    }

    /// The actor of a mutation of `workspace` for its transactional audit row (SEC-25).
    pub fn audit(&self, workspace: &Workspace) -> bb_db::Audit {
        bb_db::Audit {
            workspace: Some(workspace.id),
            key: self.key_id(),
            request_id: Some(super::error::request_id()),
        }
    }

    /// 401 without a key, 403 when the key lacks `scope` (recorded for the `auth.denied` row).
    pub fn require(&self, scope: Scope) -> Result<&ApiKey, ApiError> {
        match &self.key {
            None => Err(ApiError::unauthorized(false)),
            Some(key) if !key.permits(scope) => {
                let _ = REQUEST.try_with(|r| r.denied_scope.set(Some(scope.as_str())));
                Err(ApiError::missing_scope(scope.as_str()))
            }
            Some(key) => Ok(key),
        }
    }

    /// The workspace a listing reads: own (default), or `demo`. Any other slug is 404, whether
    /// or not it exists (SEC-2).
    pub fn read_workspace(&self, param: Option<&str>) -> Result<&Workspace, ApiError> {
        match param {
            None => Ok(&self.workspace),
            Some(slug) if slug == self.workspace.slug => Ok(&self.workspace),
            Some(slug) if slug == self.demo.slug => Ok(&self.demo),
            Some(_) => Err(ApiError::not_found("workspace")),
        }
    }

    /// The workspace a mutation writes: needs `run`; the demo workspace additionally needs
    /// `admin` (also for keys that belong to it).
    pub fn write_workspace(&self, param: Option<&str>) -> Result<&Workspace, ApiError> {
        self.require(Scope::Run)?;
        let target = self.read_workspace(param)?;
        if target.kind == "demo" {
            self.require(Scope::Admin)?;
        }
        Ok(target)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, ApiError> {
        parts
            .extensions
            .get::<Caller>()
            .cloned()
            .ok_or_else(|| ApiError::internal("caller missing: route outside the auth layer"))
    }
}

/// Workspace rows by id (keyed callers) and the demo workspace, cached in memory so anonymous
/// requests need no database access (SEC-10).
#[derive(Default)]
pub struct WorkspaceCache {
    demo: Mutex<Option<Workspace>>,
    by_id: Mutex<HashMap<Uuid, (Workspace, Instant)>>,
}

const WORKSPACE_TTL: Duration = Duration::from_secs(60);

impl WorkspaceCache {
    pub async fn demo(&self, state: &Shared) -> Result<Workspace, ApiError> {
        if let Some(ws) = self
            .demo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Ok(ws);
        }
        let ws = state.db.workspace("demo").await?;
        *self.demo.lock().unwrap_or_else(PoisonError::into_inner) = Some(ws.clone());
        Ok(ws)
    }

    pub async fn by_id(&self, state: &Shared, id: Uuid) -> Result<Workspace, ApiError> {
        if let Some((ws, at)) = self
            .by_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
            && at.elapsed() < WORKSPACE_TTL
        {
            return Ok(ws);
        }
        let ws = state.db.workspace_by_id(id).await?;
        let mut map = self.by_id.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() > 1024 {
            map.clear();
        }
        map.insert(id, (ws.clone(), Instant::now()));
        Ok(ws)
    }
}

/// Request-rate limiters (GCRA via `governor`).
pub struct Limiters {
    anonymous_ip: DefaultKeyedRateLimiter<IpAddr>,
    anonymous_global: DefaultDirectRateLimiter,
    keyed: DefaultKeyedRateLimiter<Uuid>,
    anonymous_limit: u32,
    keyed_limit: u32,
    calls: AtomicU64,
    denials: DenialThrottle,
}

fn quota(per_min: u32, burst: u32) -> Quota {
    let per_min = NonZeroU32::new(per_min.max(1)).expect("positive");
    let burst = NonZeroU32::new(burst.max(1)).expect("positive");
    Quota::per_minute(per_min).allow_burst(burst)
}

impl Limiters {
    pub fn new(cfg: &AppConfig) -> Self {
        let r = cfg.rate_limits;
        Self {
            anonymous_ip: RateLimiter::keyed(quota(r.anonymous_per_min, r.anonymous_burst)),
            anonymous_global: RateLimiter::direct(quota(
                cfg.anonymous_global_per_min,
                cfg.anonymous_global_per_min / 4,
            )),
            keyed: RateLimiter::keyed(quota(r.keyed_per_min, r.keyed_burst)),
            anonymous_limit: r.anonymous_per_min,
            keyed_limit: r.keyed_per_min,
            calls: AtomicU64::new(0),
            denials: DenialThrottle::default(),
        }
    }

    fn housekeeping(&self) {
        // Keyed state grows with distinct clients; drop idle entries now and then.
        if self
            .calls
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(4096)
        {
            self.anonymous_ip.retain_recent();
            self.keyed.retain_recent();
        }
    }

    fn retry_after(not_until: governor::NotUntil<governor::clock::QuantaInstant>) -> u64 {
        let clock = governor::clock::DefaultClock::default();
        not_until.wait_time_from(clock.now()).as_secs().max(1)
    }

    /// Anonymous: per client IP and a global anonymous budget (forged `X-Forwarded-For` values
    /// cannot escape the global cap).
    fn check_anonymous(&self, ip: IpAddr) -> Result<(u32, u32), ApiError> {
        self.housekeeping();
        self.anonymous_global
            .check()
            .map_err(|n| ApiError::rate_limited(Self::retry_after(n)))?;
        self.anonymous_ip
            .check_key(&ip)
            .map_err(|n| ApiError::rate_limited(Self::retry_after(n)))?;
        Ok((self.anonymous_limit, 0))
    }

    fn check_keyed(&self, key: Uuid) -> Result<(u32, u32), ApiError> {
        self.housekeeping();
        self.keyed
            .check_key(&key)
            .map_err(|n| ApiError::rate_limited(Self::retry_after(n)))?;
        Ok((self.keyed_limit, 0))
    }
}

/// Window of the `auth.denied` throttle: at most one row per key per window.
const DENIAL_WINDOW: Duration = Duration::from_secs(60);
/// Keys the throttle remembers at once. Only real keys (valid tag, existing row) reach it.
const DENIAL_KEYS: usize = 4096;

/// In-process `auth.denied` throttle (SEC-25): the last row time per key, bounded in memory.
#[derive(Default)]
struct DenialThrottle {
    last: Mutex<HashMap<Uuid, Instant>>,
}

impl DenialThrottle {
    /// Claims the row of `key` at `now`: `false` when the key already has one in the window, or
    /// when the map is full of keys still inside their windows (the row is then skipped rather
    /// than letting memory or the write rate grow).
    fn claim(&self, key: Uuid, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = last.get(&key) {
            if now.saturating_duration_since(*at) < DENIAL_WINDOW {
                return false;
            }
        } else if last.len() >= DENIAL_KEYS {
            last.retain(|_, at| now.saturating_duration_since(*at) < DENIAL_WINDOW);
            if last.len() >= DENIAL_KEYS {
                return false;
            }
        }
        last.insert(key, now);
        true
    }

    /// Gives back a claim whose row could not be written.
    fn release(&self, key: Uuid, claimed: Instant) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.get(&key) == Some(&claimed) {
            last.remove(&key);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Why a key that resolved to a workspace was refused.
#[derive(Clone, Copy)]
enum Denial {
    Inactive(&'static str),
    Scope(&'static str),
}

/// Writes the throttled `auth.denied` row of `key` (SECURITY_REVIEW §15): its workspace, key id,
/// request id, client address and safe metadata (reason, required scope, method, path; never the
/// key). A failed write is returned, like the admission denial rows.
async fn audit_denied(
    state: &Shared,
    key: &ApiKey,
    denial: Denial,
    method: &Method,
    path: &str,
    ip: IpAddr,
) -> Result<(), ApiError> {
    let now = Instant::now();
    if !state.limiters.denials.claim(key.id, now) {
        return Ok(());
    }
    let mut detail = serde_json::json!({
        "method": method.as_str(),
        "path": path.chars().take(200).collect::<String>(),
    });
    match denial {
        Denial::Inactive(reason) => detail["reason"] = reason.into(),
        Denial::Scope(scope) => {
            detail["reason"] = "scope".into();
            detail["required_scope"] = scope.into();
        }
    }
    let request_id = super::error::request_id();
    let ip = ip.to_string();
    let target = key.id.to_string();
    let written = state
        .db
        .audit(bb_db::NewAuditEvent {
            workspace: Some(key.workspace_id),
            key: Some(key.id),
            action: "auth.denied",
            outcome: "denied",
            target_type: Some("api_key"),
            target_id: Some(&target),
            request_id: Some(&request_id),
            client_ip: Some(&ip),
            detail: &detail,
        })
        .await;
    if let Err(e) = written {
        state.limiters.denials.release(key.id, now);
        return Err(e.into());
    }
    Ok(())
}

/// The client address: the **last** `X-Forwarded-For` entry, i.e. the one appended by the
/// nearest proxy (Render's edge); entries further left are client-supplied and ignored. Without
/// the header, the socket peer.
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|v| v.trim().parse().ok())
        .or(peer.map(|p| p.ip()))
        .unwrap_or(IpAddr::from([0, 0, 0, 0]))
}

/// The presented key: `None` without a header, `Err` for a malformed header.
pub fn bearer(headers: &HeaderMap) -> Result<Option<&str>, ApiError> {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| ApiError::unauthorized(true))?;
    if value.len() > 128 {
        return Err(ApiError::unauthorized(true));
    }
    let key = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized(true))?;
    let well_formed = key.len() == 47
        && key.starts_with("bbk_")
        && key[4..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !well_formed {
        return Err(ApiError::unauthorized(true));
    }
    Ok(Some(key))
}

fn is_mutation(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Resolves the caller, applies rate limits and the anonymous-mutation rule, and stores the
/// [`Caller`] for the handlers. `/healthz` is not subject to it. The handlers run inside the
/// request scope ([`request_actor`], [`read_class`]); a keyed request a handler refused for a
/// missing scope writes its throttled `auth.denied` row here.
pub async fn authenticate(
    State(state): State<Shared>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }
    let headers = request.headers().clone();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let ip = client_ip(&headers, peer);
    match resolve(&state, &headers, &method, &path, ip).await {
        Ok((caller, limit)) => {
            let key = caller.key.clone();
            let scope = RequestScope {
                key: caller.key_id(),
                workspace: caller.workspace.id,
                denied_scope: Cell::new(None),
            };
            request.extensions_mut().insert(caller);
            let (mut response, denied) = REQUEST
                .scope(scope, async move {
                    let response = next.run(request).await;
                    (response, REQUEST.with(|r| r.denied_scope.get()))
                })
                .await;
            if let (Some(key), Some(scope)) = (key, denied)
                && response.status() == StatusCode::FORBIDDEN
                && let Err(e) =
                    audit_denied(&state, &key, Denial::Scope(scope), &method, &path, ip).await
            {
                return e.into_response();
            }
            if let Ok(v) = limit.to_string().parse() {
                response.headers_mut().insert("x-ratelimit-limit", v);
            }
            response
        }
        Err(e) => e.into_response(),
    }
}

async fn resolve(
    state: &Shared,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
    ip: IpAddr,
) -> Result<(Caller, u32), ApiError> {
    let demo = state.workspaces.demo(state).await?;
    match bearer(headers)? {
        None => {
            let (limit, _) = state.limiters.check_anonymous(ip)?;
            if is_mutation(method) {
                return Err(ApiError::unauthorized(false));
            }
            Ok((
                Caller {
                    key: None,
                    workspace: demo.clone(),
                    demo,
                },
                limit,
            ))
        }
        Some(presented) => {
            // Forged keys are rate limited per IP like anonymous traffic before the tag check,
            // and refused without any database access or audit row.
            if !bb_db::key_tag_valid(presented, state.cfg.api_key_pepper.expose().as_bytes()) {
                state.limiters.check_anonymous(ip)?;
                return Err(ApiError::unauthorized(true));
            }
            let key = match state.db.lookup_key(presented).await? {
                KeyLookup::Active(key) => key,
                KeyLookup::Inactive { key, reason } => {
                    audit_denied(state, &key, Denial::Inactive(reason), method, path, ip).await?;
                    return Err(ApiError::unauthorized(true));
                }
                KeyLookup::Unknown => return Err(ApiError::unauthorized(true)),
            };
            let (limit, _) = state.limiters.check_keyed(key.id)?;
            // Every mutation needs at least `run`; refuse before any body is read (SEC-1).
            if is_mutation(method) && !key.permits(Scope::Run) {
                audit_denied(state, &key, Denial::Scope("run"), method, path, ip).await?;
                return Err(ApiError::missing_scope("run"));
            }
            let workspace = state.workspaces.by_id(state, key.workspace_id).await?;
            Ok((
                Caller {
                    key: Some(key),
                    workspace,
                    demo,
                },
                limit,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_rate_limits_bound_one_ip_and_rotating_forwarded_ips() {
        let mut cfg = AppConfig::from_lookup("data".into(), |_| None).unwrap();
        cfg.rate_limits.anonymous_per_min = 120;
        cfg.rate_limits.anonymous_burst = 30;
        cfg.anonymous_global_per_min = 1200;
        let limits = Limiters::new(&cfg);
        let ip = "192.0.2.1".parse().unwrap();
        for _ in 0..30 {
            assert!(limits.check_anonymous(ip).is_ok());
        }
        let refusal = limits.check_anonymous(ip).unwrap_err();
        assert_eq!(refusal.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert!(refusal.retry_after.is_some_and(|seconds| seconds >= 1));

        let limits = Limiters::new(&cfg);
        let mut denied = 0;
        for i in 0..1000u16 {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-forwarded-for",
                format!("198.51.100.1, 192.0.{}.{}", i / 256, i % 256)
                    .parse()
                    .unwrap(),
            );
            if limits.check_anonymous(client_ip(&headers, None)).is_err() {
                denied += 1;
            }
        }
        assert!(denied >= 690, "the global burst is 300, denied {denied}");
    }

    #[test]
    fn denial_throttle_admits_one_row_per_key_per_minute_in_bounded_memory() {
        let throttle = DenialThrottle::default();
        let start = Instant::now();
        let key = Uuid::new_v4();
        assert!(throttle.claim(key, start));
        assert!(!throttle.claim(key, start + Duration::from_secs(59)));
        assert!(throttle.claim(key, start + DENIAL_WINDOW));
        // A failed write gives its claim back; a stale release does not undo a newer claim.
        let later = start + DENIAL_WINDOW;
        throttle.release(key, later);
        assert!(throttle.claim(key, later));
        throttle.release(key, start);
        assert!(!throttle.claim(key, later));

        // Full of keys inside their window: new keys are refused, the map does not grow.
        let throttle = DenialThrottle::default();
        for _ in 0..DENIAL_KEYS {
            assert!(throttle.claim(Uuid::new_v4(), start));
        }
        assert!(!throttle.claim(Uuid::new_v4(), start + Duration::from_secs(30)));
        assert_eq!(throttle.len(), DENIAL_KEYS);
        // Once their windows pass, expired entries are dropped to admit a new key.
        assert!(throttle.claim(Uuid::new_v4(), start + DENIAL_WINDOW));
        assert_eq!(throttle.len(), 1);
    }
}
