//! Error surface (card #8) — internal (5xx) errors must NOT leak raw
//! rusqlite/postcard/io detail to the client; client-facing 4xx keep their
//! descriptive message. Drives `AppError::into_response` directly.

use axum::response::IntoResponse;
use http_body_util::BodyExt;
use serde_json::Value;
use tokito_mcp_server::error::AppError;

async fn body_json(err: AppError) -> (axum::http::StatusCode, Value) {
    let resp = err.into_response();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn internal_error_body_is_generic_and_does_not_leak_detail() {
    let secret = "near /private/keys/symbols.sqlite: disk I/O error (code 5274)";
    let (status, v) = body_json(AppError::Io(std::io::Error::other(secret))).await;

    assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(v["error"]["code"], "io"); // stable category preserved
    assert_eq!(v["error"]["message"], "internal server error");
    // The raw detail must be nowhere in the serialized response.
    assert!(
        !v.to_string().contains("disk I/O error"),
        "internal detail leaked to client: {v}"
    );
}

#[tokio::test]
async fn bad_request_message_is_preserved() {
    let (status, v) = body_json(AppError::BadRequest("query must not be empty".into())).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
    assert_eq!(
        v["error"]["message"],
        "bad request: query must not be empty"
    );
}
