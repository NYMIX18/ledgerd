use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

use crate::domain::DomainError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(String),
    #[error("insufficient funds in account {0}")]
    InsufficientFunds(Uuid),
    #[error("missing or invalid credentials")]
    Unauthorized,
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl From<DomainError> for AppError {
    fn from(e: DomainError) -> Self {
        AppError::Validation(e.to_string())
    }
}

impl AppError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            AppError::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "validation_error"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::InsufficientFunds(_) => (StatusCode::UNPROCESSABLE_ENTITY, "insufficient_funds"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        if let AppError::Internal(e) = &self {
            // Never leak internals to clients; log the full chain instead.
            tracing::error!(error = ?e, "internal error");
        }
        let body = Json(json!({ "error": { "code": code, "message": self.to_string() } }));
        (status, body).into_response()
    }
}

/// True if the error is a Postgres unique-constraint violation.
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}
