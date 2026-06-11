//! Server-wide error type — wraps `tokito_symbols::Error` and other I/O,
//! turns into an HTTP response.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use tokito_symbols::Error as SymErr;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Symbols(#[from] SymErr),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("task join: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Serialize)]
struct ErrBody<'a> {
    error: ErrInner<'a>,
}

#[derive(Serialize)]
struct ErrInner<'a> {
    code: &'a str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::Symbols(SymErr::SymbolNotFound { .. }) => {
                (StatusCode::NOT_FOUND, "not_found")
            }
            AppError::Symbols(SymErr::SchemaVersionMismatch { .. }) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "schema_mismatch")
            }
            AppError::Symbols(SymErr::ExtendsDepthExceeded(_)) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "extends_depth")
            }
            AppError::Symbols(SymErr::UnknownBodyFormat(_)) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "body_format")
            }
            AppError::Symbols(_) => (StatusCode::INTERNAL_SERVER_ERROR, "symbols"),
            AppError::Sqlite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "sqlite"),
            AppError::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "io"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Join(_) => (StatusCode::INTERNAL_SERVER_ERROR, "join"),
        };
        let message = self.to_string();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(%code, %message, "internal error");
        }
        let body = Json(ErrBody {
            error: ErrInner { code, message },
        });
        (status, body).into_response()
    }
}
