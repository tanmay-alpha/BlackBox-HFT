//! Request admission for the local server.
//!
//! The API has no accounts, so the threat is not a remote attacker but a web page open in the
//! user's own browser. Two checks close that:
//!
//! * **Host allowlist.** A request whose `Host` header is not one of the server's own names is
//!   refused. This defeats DNS rebinding, where an attacker's hostname is re-pointed at
//!   127.0.0.1 so the browser treats the attack page and the API as one origin.
//! * **Origin allowlist.** Browsers attach `Origin` to every cross-origin request and to
//!   same-origin non-GET requests. A request carrying an `Origin` that is not one of the
//!   server's own origins (or an explicitly allowed development origin) is refused, so another
//!   site cannot read results, start simulations or delete runs.
//!
//! No CORS headers are emitted at all: the bundled UI is served from the same origin, and the
//! Vite development server proxies `/api` (its origin must be passed with `--allow-origin`).

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};

use crate::api::{ErrorBody, ErrorDetail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPolicy {
    /// Lower-case `host:port` authorities this server answers to.
    hosts: Vec<String>,
    /// Lower-case `scheme://host:port` origins allowed to call it.
    origins: Vec<String>,
}

impl AccessPolicy {
    /// The policy for a server listening on `addr`: its loopback names on that port, plus any
    /// extra authorities and origins the operator names explicitly.
    pub fn for_listener(
        addr: SocketAddr,
        extra_hosts: &[String],
        extra_origins: &[String],
    ) -> Self {
        let port = addr.port();
        let mut hosts = vec![
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        if !addr.ip().is_unspecified() && !addr.ip().is_loopback() {
            hosts.push(addr.to_string());
        }
        hosts.extend(extra_hosts.iter().map(|h| h.trim().to_ascii_lowercase()));
        hosts.sort();
        hosts.dedup();
        let mut origins: Vec<String> = hosts.iter().map(|h| format!("http://{h}")).collect();
        origins.extend(
            extra_origins
                .iter()
                .map(|o| o.trim().trim_end_matches('/').to_ascii_lowercase()),
        );
        origins.sort();
        origins.dedup();
        Self { hosts, origins }
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    /// `Ok(())` when the request may proceed, otherwise the reason it may not.
    pub fn check(&self, headers: &HeaderMap) -> Result<(), String> {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_ascii_lowercase);
        match host {
            Some(h) if self.hosts.contains(&h) => {}
            Some(h) => {
                return Err(format!(
                    "requests addressed to host '{h}' are refused; this server answers to {}. \
                     Start it with --allow-host <host:port> to add a name",
                    self.hosts.join(", ")
                ));
            }
            None => return Err("requests without a Host header are refused".into()),
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            let origin = origin.to_str().unwrap_or("").to_ascii_lowercase();
            if !self.origins.contains(&origin) {
                return Err(format!(
                    "requests from origin '{origin}' are refused. Start the server with \
                     --allow-origin <origin> to allow a development front end"
                ));
            }
        }
        Ok(())
    }
}

async fn admit(policy: Arc<AccessPolicy>, request: Request, next: Next) -> Response {
    match policy.check(request.headers()) {
        Ok(()) => next.run(request).await,
        Err(message) => (
            StatusCode::FORBIDDEN,
            Json(ErrorBody {
                error: ErrorDetail {
                    code: "forbidden".into(),
                    message,
                },
            }),
        )
            .into_response(),
    }
}

/// Wraps every route of `app` (API and static files) in the policy.
pub fn guard(app: Router, policy: AccessPolicy) -> Router {
    let policy = Arc::new(policy);
    app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let policy = Arc::clone(&policy);
        admit(policy, request, next)
    }))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(host: Option<&str>, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = host {
            h.insert(header::HOST, HeaderValue::from_str(v).unwrap());
        }
        if let Some(v) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn loopback() -> AccessPolicy {
        AccessPolicy::for_listener("127.0.0.1:8787".parse().unwrap(), &[], &[])
    }

    #[test]
    fn own_names_are_admitted() {
        let p = loopback();
        for host in [
            "127.0.0.1:8787",
            "localhost:8787",
            "LOCALHOST:8787",
            "[::1]:8787",
        ] {
            assert!(p.check(&headers(Some(host), None)).is_ok(), "{host}");
        }
        assert!(
            p.check(&headers(
                Some("localhost:8787"),
                Some("http://localhost:8787")
            ))
            .is_ok()
        );
    }

    #[test]
    fn dns_rebinding_host_is_refused() {
        let p = loopback();
        let err = p
            .check(&headers(Some("attacker.example:8787"), None))
            .unwrap_err();
        assert!(err.contains("attacker.example"));
        assert!(p.check(&headers(Some("localhost:9999"), None)).is_err());
        assert!(p.check(&headers(None, None)).is_err());
    }

    #[test]
    fn foreign_origin_is_refused_even_with_a_valid_host() {
        let p = loopback();
        assert!(
            p.check(&headers(
                Some("127.0.0.1:8787"),
                Some("https://evil.example")
            ))
            .is_err()
        );
        assert!(
            p.check(&headers(Some("127.0.0.1:8787"), Some("null")))
                .is_err()
        );
    }

    #[test]
    fn explicit_additions_are_honoured() {
        let p = AccessPolicy::for_listener(
            "0.0.0.0:8787".parse().unwrap(),
            &["blackbox.local:9000".into()],
            &["http://localhost:5173/".into()],
        );
        assert!(p.check(&headers(Some("blackbox.local:9000"), None)).is_ok());
        assert!(
            p.check(&headers(
                Some("127.0.0.1:8787"),
                Some("http://localhost:5173")
            ))
            .is_ok()
        );
        // The unspecified address itself is never a valid authority.
        assert!(!p.hosts().iter().any(|h| h.starts_with("0.0.0.0")));
    }
}
