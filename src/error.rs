use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors returned by the store and the HTTP layer.
///
/// `NotFound` is also used when the caller is not allowed to see a resource,
/// so that the existence of private entries is not revealed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }
    pub fn internal(msg: impl std::fmt::Display) -> Self {
        Self::Internal(msg.to_string())
    }

    pub fn status(&self) -> StatusCode {
        self.status_and_code().0
    }

    /// Errors that a person can fix by changing what they sent (shown next to the form).
    pub fn is_correctable(&self) -> bool {
        matches!(
            self,
            Error::BadRequest(_) | Error::Forbidden(_) | Error::TooManyRequests(_) | Error::PayloadTooLarge(_)
        )
    }

    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Error::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Error::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Error::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Error::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            Error::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Error::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            Error::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &e
            && db.is_unique_violation()
        {
            return Error::Conflict("already exists".into());
        }
        Error::Internal(e.to_string())
    }
}

impl From<sqlx::migrate::MigrateError> for Error {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        Error::Internal(e.to_string())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let message = match &self {
            Error::Internal(detail) => {
                tracing::error!("internal error: {detail}");
                "internal server error".to_string()
            }
            other => other.to_string(),
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}
