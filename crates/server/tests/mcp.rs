//! MCP face — initialize handshake, tools/list, tools/call for each tool.
//!
//! Talks to the streamable-HTTP service through the assembled router. The
//! server responds with SSE (`text/event-stream`); we parse the first
//! `data: ...` line as a JSON-RPC message.

use axum::{
    body::Body,
    http::{HeaderValue, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokito_mcp_server::build_app;
use tower::ServiceExt;

mod common;

async fn mcp_request(
    body: Value,
    session_id: Option<&str>,
) -> (StatusCode, Option<String>, String) {
    let app = build_app(common::fixture_app_state());
    let mut req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(s) = session_id {
        req = req.header("mcp-session-id", HeaderValue::from_str(s).unwrap());
    }
    let resp = app
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, session, String::from_utf8_lossy(&body).to_string())
}

/// Pull the first `data: { ... }` line out of an SSE-style body and parse it.
fn parse_sse_message(s: &str) -> Value {
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if rest.trim().starts_with('{') {
                return serde_json::from_str(rest)
                    .unwrap_or_else(|e| panic!("SSE data not JSON: {e} :: {rest}"));
            }
        }
    }
    panic!("no SSE data message in body: {s}");
}

async fn initialize_session() -> String {
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.1"}
        }
    });
    let (status, session, body) = mcp_request(init, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "initialize response body was: {body}"
    );
    let msg = parse_sse_message(&body);
    assert_eq!(msg["result"]["serverInfo"]["name"], "tokito-mcp");
    assert_eq!(
        msg["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "MCP initialize must advertise the package version"
    );
    session.expect("server should issue an mcp-session-id")
}

#[tokio::test]
async fn initialize_returns_server_info_and_session() {
    let sid = initialize_session().await;
    assert!(!sid.is_empty());
}

#[tokio::test]
async fn initialize_handshake_returns_complete_tool_catalog() {
    let sid = initialize_session().await;

    // Send the "initialized" notification (no-op for our test, but valid)
    let _ = mcp_request(
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        Some(&sid),
    )
    .await;

    // The whole flow can't actually be done in one app instance because each
    // oneshot rebuilds the router (and the session manager is per-process).
    // So we open a fresh session and call tools/list immediately after init —
    // since the session is local to that app instance, both calls in the same
    // process work as long as the same app survives.
    //
    // Assert tools/list returns the full catalog by running it on a fresh
    // session (the request itself carries the session id we just opened).
    let app = build_app(common::fixture_app_state());

    // 1) initialize on this fresh app
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "t", "version": "0"}
        }
    });
    let init_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(init_req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = init_resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("session id");
    let _ = init_resp.into_body().collect().await.unwrap();

    // 2) tools/list on the same app + session
    let tools_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("mcp-session-id", sid.clone())
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = tools_resp.into_body().collect().await.unwrap().to_bytes();
    let msg = parse_sse_message(std::str::from_utf8(&body).unwrap());
    let tools = msg["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"search_symbols"));
    assert!(names.contains(&"get_symbol"));
    assert!(names.contains(&"list_libraries"));
    assert!(names.contains(&"find_compatible"));
    assert!(names.contains(&"part_offer_query"));
    assert!(names.contains(&"resolve_by_mpn"));
    assert!(names.contains(&"get_symbol_provenance"));
    assert_eq!(names.len(), 7);
}

async fn call_tool(app: &axum::Router, sid: &str, id: i64, name: &str, args: Value) -> Value {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("mcp-session-id", sid)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "tools/call",
                        "params": {"name": name, "arguments": args}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    parse_sse_message(std::str::from_utf8(&body).unwrap())
}

async fn open_session_on(app: &axum::Router) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "initialize",
                        "params": {
                            "protocolVersion": "2025-03-26",
                            "capabilities": {},
                            "clientInfo": {"name": "t", "version": "0"}
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let sid = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("session id");
    let _ = resp.into_body().collect().await.unwrap();
    sid
}

#[tokio::test]
async fn search_symbols_tool_returns_opamps() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "search_symbols",
        json!({"query": "opamp", "limit": 5}),
    )
    .await;
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    let inner: Value = serde_json::from_str(text).unwrap();
    assert!(inner["total"].as_u64().unwrap() >= 2);
    let names: Vec<&str> = inner["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ROOT_OP"));
}

#[tokio::test]
async fn get_symbol_tool_resolves_extending_child() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol",
        json!({"lib": "Amplifier_Op", "name": "LMxxx_A"}),
    )
    .await;
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    let inner: Value = serde_json::from_str(text).unwrap();
    assert_eq!(inner["name"], "LMxxx_A");
    let parent = inner["parent"].as_array().unwrap();
    assert_eq!(parent[1], "ROOT_OP");
    assert_eq!(inner["body"]["pins"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn find_compatible_tool_combines_pins_and_query() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "find_compatible",
        json!({"pins": 5, "query": "opamp"}),
    )
    .await;
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    let inner: Value = serde_json::from_str(text).unwrap();
    assert_eq!(inner["total"].as_u64().unwrap(), 2);
}

#[tokio::test]
async fn find_compatible_requires_a_filter() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(&app, &sid, 2, "find_compatible", json!({})).await;
    assert!(msg.get("error").is_some(), "expected error for no filters");
}

#[tokio::test]
async fn part_offer_query_tool_returns_procurement_hint() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "part_offer_query",
        json!({"symbol_id": "Device:R", "value": "330", "package": "R_0603", "market": "IN"}),
    )
    .await;
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    let inner: Value = serde_json::from_str(text).unwrap();
    assert_eq!(inner["procurement_query"], "330 resistor, 0603 package");
    assert_eq!(inner["exact_mpn"], Value::Null);
    assert!(inner["distributor_domains"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d == "digikey.in"));
}

#[tokio::test]
async fn list_libraries_tool_returns_both() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(&app, &sid, 2, "list_libraries", json!({})).await;
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    let inner: Value = serde_json::from_str(text).unwrap();
    let arr = inner.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}

#[tokio::test]
async fn get_symbol_with_unknown_returns_tool_error() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol",
        json!({"lib":"Device", "name":"NoSuch"}),
    )
    .await;
    // MCP returns a tool-level error as a JSON-RPC error response
    assert!(
        msg.get("error").is_some(),
        "expected JSON-RPC error: got {msg}"
    );
}

/// TokitoAI/tokito-mcp#106 review, round 2: the REST face already had
/// coverage for a malformed FTS5 query returning a client-safe 400 — the
/// MCP face shares the exact same `tokito_symbols::search::search` call and
/// `map_sym` classification, but had no test of its own.
#[tokio::test]
async fn search_symbols_tool_rejects_malformed_query_with_invalid_params() {
    let app = build_app(common::fixture_app_state());
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "search_symbols",
        json!({"query": "he\"llo", "limit": 5}),
    )
    .await;
    assert!(
        msg.get("error").is_some(),
        "expected JSON-RPC error for malformed FTS5 query: got {msg}"
    );
    // -32602 is JSON-RPC 2.0's standard "Invalid params" code — what
    // `ErrorCode::INVALID_PARAMS` serializes to.
    assert_eq!(msg["error"]["code"], -32602);
    assert_eq!(
        msg["error"]["message"],
        tokito_symbols::INVALID_QUERY_CLIENT_MESSAGE,
        "must not leak the raw rusqlite/FTS5 detail to the client: {msg}"
    );
}
