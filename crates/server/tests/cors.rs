//! Exposure config — MCP `Host` allowlist (DNS-rebinding guard) and REST CORS,
//! both plumbed via `ServerConfig` (card #9). Driven through the assembled
//! router with `oneshot`, no port binding.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use tokito_mcp_server::{build_app, build_app_with_config, ServerConfig};
use tower::ServiceExt;

mod common;

async fn mcp_init_status(cfg: ServerConfig, host: &str) -> StatusCode {
    let app = build_app_with_config(common::fixture_app_state(), cfg);
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "t", "version": "0" }
        }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn mcp_rejects_host_not_in_allowlist() {
    // With a custom allowlist, the previously-default loopback host is rejected.
    let cfg = ServerConfig {
        allowed_hosts: Some(vec!["example.com".into()]),
        allowed_origins: vec![],
    };
    assert_eq!(
        mcp_init_status(cfg, "localhost").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn mcp_accepts_configured_host() {
    // This is the fix for the proxied-deployment "Forbidden: Host" rejection.
    let cfg = ServerConfig {
        allowed_hosts: Some(vec!["example.com".into()]),
        allowed_origins: vec![],
    };
    let st = mcp_init_status(cfg, "example.com").await;
    assert!(
        st.is_success(),
        "expected success for allowed host, got {st}"
    );
}

#[tokio::test]
async fn mcp_default_still_allows_loopback() {
    // No override → rmcp's safe loopback default is preserved.
    let st = mcp_init_status(ServerConfig::default(), "localhost").await;
    assert!(
        st.is_success(),
        "expected loopback to pass by default, got {st}"
    );
}

#[tokio::test]
async fn rest_sets_cors_header_for_allowed_origin() {
    let cfg = ServerConfig {
        allowed_hosts: None,
        allowed_origins: vec!["https://app.example.com".into()],
    };
    let app = build_app_with_config(common::fixture_app_state(), cfg);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health")
                .header("origin", "https://app.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let acao = resp
        .headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok());
    assert_eq!(acao, Some("https://app.example.com"));
}

#[tokio::test]
async fn rest_no_cors_header_when_unconfigured() {
    // Default config adds no CORS layer, so no ACAO header is emitted.
    let app = build_app(common::fixture_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/health")
                .header("origin", "https://app.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.headers().get("access-control-allow-origin").is_none());
}
