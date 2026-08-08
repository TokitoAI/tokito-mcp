//! MCP integration tests for the generated-symbol tools:
//! `resolve_by_mpn` and `get_symbol_provenance`.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokito_mcp_server::build_app;
use tower::ServiceExt;

mod common;

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
    assert_eq!(resp.status(), StatusCode::OK);
    let sid = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("session id");
    let _ = resp.into_body().collect().await.unwrap();
    sid
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

fn call_tool_payload(msg: &Value) -> Value {
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

fn app_with_generated() -> axum::Router {
    build_app(common::fixture_app_state_with_generated())
}

#[tokio::test]
async fn resolve_by_mpn_tool_returns_symbol_body_when_published() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "resolve_by_mpn",
        json!({
            "manufacturer": "Texas Instruments",
            "mpn": "TPS5430DDAR",
            "package": "SO-PowerPAD-8"
        }),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["lib"], "generated:texas_instruments");
    assert_eq!(inner["name"], "TPS5430DDAR");
    assert_eq!(inner["body"]["pins"].as_array().unwrap().len(), 8);
}

#[tokio::test]
async fn resolve_by_mpn_tool_normalizes_manufacturer_case() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "resolve_by_mpn",
        json!({
            "manufacturer": "  TEXAS   Instruments  ",
            "mpn": "TPS5430DDAR",
            "package": "SO-PowerPAD-8"
        }),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["name"], "TPS5430DDAR");
}

#[tokio::test]
async fn resolve_by_mpn_tool_returns_not_found_sentinel() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "resolve_by_mpn",
        json!({"manufacturer": "Nobody", "mpn": "NOPE", "package": "DIP-8"}),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["status"], "not_found");
}

#[tokio::test]
async fn resolve_by_mpn_tool_rejects_empty_manufacturer() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "resolve_by_mpn",
        json!({"manufacturer": "", "mpn": "TPS5430DDAR", "package": "SO-PowerPAD-8"}),
    )
    .await;
    assert!(msg.get("error").is_some(), "expected INVALID_PARAMS: {msg}");
}

#[tokio::test]
async fn get_symbol_provenance_tool_returns_expected_shape() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol_provenance",
        json!({"lib": "generated:texas_instruments", "name": "TPS5430DDAR"}),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["part_id"]["mpn"], "TPS5430DDAR");
    assert_eq!(inner["status"], "published");
    assert_eq!(inner["evidence"]["region_ids"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn get_symbol_provenance_accepts_exact_revision_id() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol_provenance",
        json!({"revision_id": "gen_sha256_fixture_tps5430ddar"}),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["part_id"]["mpn"], "TPS5430DDAR");
    assert_eq!(inner["status"], "published");
}

#[tokio::test]
async fn get_symbol_provenance_rejects_ambiguous_identity() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol_provenance",
        json!({
            "revision_id": "gen_sha256_fixture_tps5430ddar",
            "lib": "generated:texas_instruments",
            "name": "TPS5430DDAR"
        }),
    )
    .await;
    assert!(msg.get("error").is_some(), "expected invalid params: {msg}");
}

#[tokio::test]
async fn get_symbol_provenance_tool_returns_not_found_sentinel() {
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol_provenance",
        json!({"lib": "generated:nobody", "name": "GHOST"}),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["status"], "not_found");
}

#[tokio::test]
async fn get_symbol_tool_dispatches_generated_prefix() {
    // The existing `get_symbol` tool must transparently route generated:* libs
    // to the generated store — validates the dispatch inside Resolver::resolve.
    let app = app_with_generated();
    let sid = open_session_on(&app).await;
    let msg = call_tool(
        &app,
        &sid,
        2,
        "get_symbol",
        json!({"lib": "generated:texas_instruments", "name": "TPS5430DDAR"}),
    )
    .await;
    let inner = call_tool_payload(&msg);
    assert_eq!(inner["name"], "TPS5430DDAR");
    assert_eq!(inner["body"]["pins"].as_array().unwrap().len(), 8);
}
