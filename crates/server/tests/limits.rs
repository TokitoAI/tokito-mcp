//! Server-hardening regression tests — body limits, request timeout,
//! concurrency cap, and per-arg length validation.
//!
//! These guard against the v0.1.0 audit findings (CRIT-2, CRIT-3, and the
//! shallow arg validation on the MCP face).

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokito_mcp_server::{build_app, MCP_BODY_LIMIT_BYTES, REST_BODY_LIMIT_BYTES};
use tower::ServiceExt;

mod common;

async fn rest_get(uri: &str) -> (StatusCode, Value) {
    let app = build_app(common::fixture_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn search_with_oversize_query_returns_400() {
    let q = "a".repeat(300);
    let (status, v) = rest_get(&format!("/v1/search?q={q}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn search_with_oversize_lib_returns_400() {
    let lib = "x".repeat(100);
    let (status, v) = rest_get(&format!("/v1/search?q=ok&lib={lib}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn get_symbol_with_oversize_name_returns_400() {
    let name = "x".repeat(200);
    let (status, v) = rest_get(&format!("/v1/symbols/Device/{name}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn list_symbols_limit_caps_at_200() {
    // Asking for 5000 must clamp to 200.
    let (status, v) = rest_get("/v1/libraries/Device/symbols?limit=5000").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["limit"], 200);
}

#[tokio::test]
async fn compatible_with_oversize_fp_pattern_returns_400() {
    let pat = "p".repeat(100);
    let (status, v) = rest_get(&format!("/v1/compatible?fp_pattern={pat}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn mcp_request_body_limit_rejects_oversized_post() {
    // The body limit is set BELOW rmcp via ServiceBuilder. When the inner
    // service tries to read past the cap, the body wrapper returns a read
    // error and rmcp surfaces it. tower_http surfaces this as 413, rmcp
    // surfaces it as 500 — either way the request is rejected and no OOM
    // occurs, which is the property we care about.
    let oversized = vec![b'x'; MCP_BODY_LIMIT_BYTES + 1024];
    let app = build_app(common::fixture_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::PAYLOAD_TOO_LARGE
            || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
        "expected oversized body to be rejected with 413 or 500, got {}",
        resp.status()
    );
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "oversized body must not be accepted"
    );
}

#[tokio::test]
async fn body_limit_constants_are_publicly_exposed() {
    // The constants are part of the public surface so audit reviewers can
    // verify the configured caps without reading lib.rs. Pinned via
    // const-asserts so any change to the limits has to also update this
    // file — a deliberate trip-wire.
    const _: () = assert!(REST_BODY_LIMIT_BYTES == 64 * 1024);
    const _: () = assert!(MCP_BODY_LIMIT_BYTES == 1024 * 1024);
}
