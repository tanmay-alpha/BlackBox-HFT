//! Authentication, scopes and request-rate limits (API_CONTRACT §3, §5; SECURITY_REVIEW §9).
//!
//! Verification order: header shape → key check tag (HMAC with `BB_API_KEY_PEPPER`, constant
//! time, no database) → digest lookup → scopes. A forged or random `bbk_` key is refused before
//! any database access (SEC-31). No header = anonymous = the public demo workspace, read-only;
//! an anonymous mutation is answered 401 before its body is read (SEC-1). Keys in the query
//! string are ignored.

use std::{
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
    http::{HeaderMap, Method, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bb_db::{ApiKey, Scope, Workspace};
use governor::{
    DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Quota, RateLimiter, clock::Clock,
};
use uuid::Uuid;

use super::{Shared, error::ApiError};
use crate::config::AppConfig;

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

    /// 401 without a key, 403 when the key lacks `scope`.
    pub fn require(&self, scope: Scope) -> Result<&ApiKey, ApiError> {
        match &self.key {
            None => Err(ApiError::unauthorized(false)),
            Some(key) if !key.permits(scope) => Err(ApiError::missing_scope(scope.as_str())),
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

/// Resolves a presented key (tag check, then one digest lookup).
pub async fn resolve_key(state: &Shared, key: &str) -> Result<ApiKey, ApiError> {
    if !bb_db::key_tag_valid(key, state.cfg.api_key_pepper.expose().as_bytes()) {
        return Err(ApiError::unauthorized(true));
    }
    match state.db.authenticate(key).await? {
        Some(key) => Ok(key),
        None => Err(ApiError::unauthorized(true)),
    }
}

fn is_mutation(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Resolves the caller, applies rate limits and the anonymous-mutation rule, and stores the
/// [`Caller`] for the handlers. `/healthz` is not subject to it.
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
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    match resolve(&state, &headers, method, peer).await {
        Ok((caller, limit)) => {
            request.extensions_mut().insert(caller);
            let mut response = next.run(request).await;
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
    method: Method,
    peer: Option<SocketAddr>,
) -> Result<(Caller, u32), ApiError> {
    let demo = state.workspaces.demo(state).await?;
    match bearer(headers)? {
        None => {
            let (limit, _) = state.limiters.check_anonymous(client_ip(headers, peer))?;
            if is_mutation(&method) {
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
            // Forged keys are rate limited per IP like anonymous traffic before the tag check.
            if !bb_db::key_tag_valid(presented, state.cfg.api_key_pepper.expose().as_bytes()) {
                state.limiters.check_anonymous(client_ip(headers, peer))?;
                return Err(ApiError::unauthorized(true));
            }
            let key = resolve_key(state, presented).await?;
            let (limit, _) = state.limiters.check_keyed(key.id)?;
            // Every mutation needs at least `run`; refuse before any body is read (SEC-1).
            if is_mutation(&method) && !key.permits(Scope::Run) {
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
}
