//! MCP session cap (card #11) — `create_session` past `max_sessions` is
//! rejected, so a scripted `initialize` loop can't grow the session map
//! unbounded. Driven through the assembled router; the app (and its shared
//! `CappedSessionManager`) is cloned per request so sessions accumulate.

use axum::{body::Body, http::Request};
use serde_json::json;
use tokito_mcp_server::{build_app_with_config, ServerConfig};
use tower::ServiceExt;

mod common;

fn init_request() -> Request<Body> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "t", "version": "0" }
        }
    });
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn session_cap_rejects_excess_sessions() {
    let cfg = ServerConfig {
        max_sessions: 2,
        ..Default::default()
    };
    let app = build_app_with_config(common::fixture_app_state(), cfg);

    // Each `initialize` with no session id creates and retains a new session.
    let s1 = app.clone().oneshot(init_request()).await.unwrap().status();
    let s2 = app.clone().oneshot(init_request()).await.unwrap().status();
    let s3 = app.clone().oneshot(init_request()).await.unwrap().status();

    assert!(s1.is_success(), "1st session should succeed, got {s1}");
    assert!(s2.is_success(), "2nd session should succeed, got {s2}");
    assert!(
        !s3.is_success(),
        "3rd session past cap=2 must be rejected, got {s3}"
    );
}
