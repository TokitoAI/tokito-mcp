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
            // The caller's `q`/`query` isn't valid FTS5 syntax (unbalanced
            // quotes, a bad column filter, a bare boolean operator with no
            // operand, ...) — a client mistake, not a server fault. See
            // `tokito_symbols::search::run_match_query` (TokitoAI/tokito-mcp#106).
            AppError::Symbols(SymErr::InvalidQuery(_)) => (StatusCode::BAD_REQUEST, "bad_request"),
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
        // Never leak internal detail (raw rusqlite / postcard / io strings) to
        // clients: on 5xx, log the full error server-side and return a generic
        // message. The stable `code` still tells the client the category. 4xx
        // (bad_request / not_found) carry their descriptive, client-safe message.
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(%code, detail = %self, "internal error");
            "internal server error".to_string()
        } else {
            self.to_string()
        };
        let body = Json(ErrBody {
            error: ErrInner { code, message },
        });
        (status, body).into_response()
    }
}
