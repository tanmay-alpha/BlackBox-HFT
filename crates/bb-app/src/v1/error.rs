//! The `/api/v1` error envelope (API_CONTRACT §2): stable `code`, human `message`, the request
//! id and structured `details`; always `Cache-Control: no-store`.
//!
//! Internal failures (database, storage, I/O, engine errors, panics) never reach the client:
//! they are logged with the request id and answered with a generic message (SEC-17).

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::error::AppError;

tokio::task_local! {
    /// The id of the request being handled (set by the edge middleware).
    pub static REQUEST_ID: String;
}

/// The id of the current request, or `-` outside a request.
pub fn request_id() -> String {
    REQUEST_ID
        .try_with(|id| id.clone())
        .unwrap_or_else(|_| "-".to_string())
}

/// Most problems reported for one invalid configuration (SEC-7); the rest are counted.
pub const MAX_PROBLEMS: usize = 50;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub details: Value,
    pub retry_after: Option<u64>,
    pub www_authenticate: Option<&'static str>,
}

/// Body of every non-2xx response.
#[derive(Serialize, ToSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    /// Stable machine-readable code, e.g. `budget_exceeded`.
    pub code: String,
    pub message: String,
    pub request_id: String,
    /// Code-specific structured details (API_CONTRACT §2); `null` when there are none.
    #[schema(value_type = Object)]
    pub details: Value,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: Value::Null,
            retry_after: None,
            www_authenticate: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn invalid_field(field: &str, message: impl Into<String>) -> Self {
        Self::invalid_request(message).with_details(json!({ "field": field }))
    }

    /// 401. `key_sent` distinguishes a bad key from a missing one in `WWW-Authenticate`.
    pub fn unauthorized(key_sent: bool) -> Self {
        let mut e = Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            if key_sent {
                "the API key is malformed, unknown, revoked or expired"
            } else {
                "this request needs an API key (Authorization: Bearer bbk_...)"
            },
        );
        e.www_authenticate = Some(if key_sent {
            "Bearer error=\"invalid_token\""
        } else {
            "Bearer"
        });
        e
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn missing_scope(scope: &str) -> Self {
        Self::forbidden(format!("this API key lacks the `{scope}` scope"))
            .with_details(json!({ "required_scope": scope }))
    }

    /// 404 whose body is identical for invisible and nonexistent resources (SEC-2).
    pub fn not_found(resource: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("{resource} not found"),
        )
        .with_details(json!({ "resource": resource }))
    }

    pub fn conflict(reason: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
            .with_details(json!({ "reason": reason }))
    }

    pub fn invalid_config(problems: Vec<String>) -> Self {
        let total = problems.len();
        let shown: Vec<String> = problems
            .into_iter()
            .take(MAX_PROBLEMS)
            .map(|p| p.chars().take(300).collect())
            .collect();
        let message = if total > shown.len() {
            format!(
                "invalid configuration: {}; and {} more problems",
                shown.join("; "),
                total - shown.len()
            )
        } else {
            format!("invalid configuration: {}", shown.join("; "))
        };
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_config", message)
            .with_details(json!({ "problems": shown, "problem_count": total }))
    }

    pub fn budget_exceeded(message: impl Into<String>, details: Value) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "budget_exceeded", message)
            .with_details(details)
    }

    pub fn rate_limited(retry_after_s: u64) -> Self {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests; retry later",
        )
        .with_details(json!({ "retry_after_s": retry_after_s }));
        e.retry_after = Some(retry_after_s);
        e
    }

    pub fn quota_exceeded(scope: &str, limit: i64, used: i64) -> Self {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "quota_exceeded",
            format!("the `{scope}` quota ({limit}) is used up"),
        )
        .with_details(json!({
            "scope": scope, "limit": limit, "used": used,
            "resets_at_ns": quota_reset(scope),
        }));
        e.retry_after = Some(seconds_to_utc_midnight());
        e
    }

    pub fn integrity_failed(run_id: &str, object: &str, expected_sha256: &str) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "integrity_failed",
            format!(
                "run {run_id} failed verification ({object}); it is quarantined and will be \
                 re-simulated on the next identical submission"
            ),
        )
        .with_details(json!({
            "run_id": run_id, "object": object, "expected_sha256": expected_sha256,
        }))
    }

    pub fn platform_mismatch(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "platform_mismatch", message)
    }

    pub fn unavailable(reason: &str, retry_after_s: u64) -> Self {
        let mut e = Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            match reason {
                "draining" => "the server is shutting down; retry shortly",
                "database" => "the database is unreachable; retry shortly",
                "storage" => "artefact storage is unavailable or its budget is spent",
                _ => "the server is busy; retry shortly",
            },
        )
        .with_details(json!({ "reason": reason }));
        e.retry_after = Some(retry_after_s);
        e
    }

    /// Logs `detail` under the request id and answers with a generic message.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!(request_id = %request_id(), error = %detail, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error; quote the request id when reporting it",
        )
    }
}

fn quota_reset(scope: &str) -> Value {
    if scope.ends_with("_per_day") {
        let now = chrono::Utc::now();
        let midnight = (now + chrono::Duration::days(1))
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .map(|t| t.and_utc());
        midnight
            .map(|t| Value::String(crate::wire::at_ns(t)))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

/// Seconds until the next UTC midnight (at least 1).
pub fn seconds_to_utc_midnight() -> u64 {
    let now = chrono::Utc::now();
    let next = (now + chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|t| t.and_utc());
    next.map(|t| (t - now).num_seconds().max(1) as u64)
        .unwrap_or(3600)
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code.to_string(),
                message: self.message,
                request_id: request_id(),
                details: self.details,
            },
        };
        let mut response = (self.status, Json(body)).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(seconds) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            headers.insert(header::RETRY_AFTER, value);
        }
        if let Some(challenge) = self.www_authenticate {
            headers.insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(challenge),
            );
        }
        response
    }
}

impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        match err {
            AppError::NotFound(message) => {
                ApiError::new(StatusCode::NOT_FOUND, "not_found", message)
                    .with_details(json!({ "resource": "resource" }))
            }
            AppError::InvalidRequest(message) => ApiError::invalid_request(message),
            AppError::InvalidConfig(problems) => ApiError::invalid_config(problems),
            AppError::BudgetExceeded(message) => {
                ApiError::budget_exceeded(message, json!({ "profile": null }))
            }
            AppError::Cancelled(message) => ApiError::conflict("cancelled", message),
            AppError::PlatformMismatch(message) => ApiError::platform_mismatch(message),
            AppError::SimulationFailed(detail) | AppError::Internal(detail) => {
                ApiError::internal(detail)
            }
        }
    }
}

impl From<bb_db::Error> for ApiError {
    fn from(err: bb_db::Error) -> Self {
        match err {
            bb_db::Error::NotFound => ApiError::not_found("resource"),
            bb_db::Error::Conflict => {
                ApiError::conflict("version_mismatch", "the resource changed; reload and retry")
            }
            bb_db::Error::Invalid(message) => ApiError::invalid_request(message),
            bb_db::Error::Quota(scope) => ApiError::quota_exceeded(scope, 0, 0),
            bb_db::Error::Database(e) if is_unreachable(&e) => {
                tracing::error!(request_id = %request_id(), error = %e, "database unreachable");
                ApiError::unavailable("database", 5)
            }
            other => ApiError::internal(other),
        }
    }
}

impl From<bb_artifacts::ArtifactError> for ApiError {
    fn from(err: bb_artifacts::ArtifactError) -> Self {
        ApiError::internal(format!("artifact error: {err}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::internal(format!("json error: {err}"))
    }
}

fn is_unreachable(e: &sqlx::Error) -> bool {
    matches!(
        e,
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::PoolClosed
    )
}

/// Database errors outside a request (worker, registry): a stable `AppError`.
pub fn internal_db(err: bb_db::Error) -> AppError {
    match err {
        bb_db::Error::NotFound => AppError::NotFound("resource not found".into()),
        bb_db::Error::Invalid(message) => AppError::InvalidRequest(message),
        other => AppError::Internal(format!("database: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn internal_error_classes_do_not_leak_diagnostics_into_http_bodies() {
        let detail = "panicked sentinel.rs:42 SELECT bb.secrets /tmp/file https://private.example";
        let errors: Vec<ApiError> = vec![
            bb_db::Error::Database(sqlx::Error::Protocol(detail.into())).into(),
            bb_db::Error::Migration(sqlx::migrate::MigrateError::Execute(sqlx::Error::Protocol(
                detail.into(),
            )))
            .into(),
            bb_artifacts::ArtifactError::Io(std::io::Error::other(detail)).into(),
            AppError::Internal(detail.into()).into(),
            AppError::SimulationFailed(detail.into()).into(),
        ];
        for error in errors {
            let response = error.into_response();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert!(value["error"]["request_id"].is_string());
            let body = String::from_utf8(body.to_vec()).unwrap();
            for forbidden in ["panicked", ".rs:", "SELECT", "bb.", "/tmp", "https://"] {
                assert!(!body.contains(forbidden), "{body}");
            }
        }
    }
}
