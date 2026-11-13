//! The application error type and its mapping onto the HTTP error contract.

use axum::{
    Json,
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::api::{ErrorBody, ErrorDetail};

/// One variant per API error code. Payloads are plain strings so the error is `Send + Sync` no
/// matter what the originating crate's error type holds, which lets it cross worker threads.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// `not_found` (404).
    #[error("{0}")]
    NotFound(String),
    /// `invalid_request` (400): the request itself is malformed.
    #[error("{0}")]
    InvalidRequest(String),
    /// `invalid_config` (422): the configuration parsed but cannot be simulated. Carries every
    /// problem, not just the first.
    #[error("invalid configuration: {}", .0.join("; "))]
    InvalidConfig(Vec<String>),
    /// `simulation_failed` (500).
    #[error("simulation failed: {0}")]
    SimulationFailed(String),
    /// `internal` (500).
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    /// The `error.code` of the HTTP error body.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "not_found",
            AppError::InvalidRequest(_) => "invalid_request",
            AppError::InvalidConfig(_) => "invalid_config",
            AppError::SimulationFailed(_) => "simulation_failed",
            AppError::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            AppError::InvalidConfig(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::SimulationFailed(_) | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// The documented error body for this error.
    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            error: ErrorDetail {
                code: self.code().to_string(),
                message: self.to_string(),
            },
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status(), Json(self.body())).into_response()
    }
}

impl From<bb_data::DataError> for AppError {
    fn from(err: bb_data::DataError) -> Self {
        match err {
            // The caller supplied something unusable (for example a malformed dataset id).
            bb_data::DataError::Invalid(message) => AppError::InvalidRequest(message),
            bb_data::DataError::NotFound(id) => {
                AppError::NotFound(format!("dataset '{id}' not found"))
            }
            // Unreadable, corrupt or inconsistent dataset files are a server-side condition.
            other => AppError::Internal(format!("dataset error: {other}")),
        }
    }
}

impl From<bb_sim::SimError> for AppError {
    fn from(err: bb_sim::SimError) -> Self {
        match err {
            bb_sim::SimError::InvalidConfig(problems) => AppError::InvalidConfig(problems),
            other => AppError::SimulationFailed(other.to_string()),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Internal(format!("i/o error: {err}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Internal(format!("json error: {err}"))
    }
}

// Extractor rejections are rewritten so that even a malformed path, query string or body gets
// the documented `{"error": {...}}` shape instead of axum's plain-text default.

impl From<PathRejection> for AppError {
    fn from(rejection: PathRejection) -> Self {
        AppError::InvalidRequest(rejection.body_text())
    }
}

impl From<QueryRejection> for AppError {
    fn from(rejection: QueryRejection) -> Self {
        AppError::InvalidRequest(rejection.body_text())
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        AppError::InvalidRequest(rejection.body_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_statuses_follow_the_api_contract() {
        let cases = [
            (AppError::NotFound("x".into()), "not_found", 404),
            (AppError::InvalidRequest("x".into()), "invalid_request", 400),
            (
                AppError::InvalidConfig(vec!["x".into()]),
                "invalid_config",
                422,
            ),
            (
                AppError::SimulationFailed("x".into()),
                "simulation_failed",
                500,
            ),
            (AppError::Internal("x".into()), "internal", 500),
        ];
        for (err, code, status) in cases {
            assert_eq!(err.code(), code);
            assert_eq!(err.status().as_u16(), status);
            assert_eq!(err.into_response().status().as_u16(), status);
        }
    }

    #[test]
    fn invalid_config_message_lists_every_problem() {
        let err = AppError::InvalidConfig(vec!["first problem".into(), "second problem".into()]);
        let body = err.body();
        assert_eq!(body.error.code, "invalid_config");
        assert!(body.error.message.contains("first problem"));
        assert!(body.error.message.contains("second problem"));
        let json = serde_json::to_value(&body).unwrap();
        assert!(json["error"]["message"].is_string());
    }
}
