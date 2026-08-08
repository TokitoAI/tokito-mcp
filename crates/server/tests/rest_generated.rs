//! REST-face tests for the generated-symbol endpoints:
//!
//!   GET /v1/generated/resolve?manufacturer=...&mpn=...&package=...
//!   GET /v1/generated/:lib/:name/provenance
//!
//! Exercised through the assembled router via `tower::ServiceExt::oneshot`.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokito_mcp_server::{build_app, state::AppState};
use tower::ServiceExt;

mod common;

async fn request_json(state: AppState, uri: &str) -> (StatusCode, Value) {
    let app = build_app(state);
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
    let v: Value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| panic!("non-JSON body for {uri}: {body:?}"));
    (status, v)
}

#[tokio::test]
async fn resolve_by_mpn_returns_symbol_when_published() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/generated/resolve\
         ?manufacturer=Texas%20Instruments\
         &mpn=TPS5430DDAR\
         &package=SO-PowerPAD-8",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["lib"], "generated:texas_instruments");
    assert_eq!(v["name"], "TPS5430DDAR");
    assert_eq!(v["body"]["pins"].as_array().unwrap().len(), 8);
    assert!(
        v["status"].is_null(),
        "found response has no `status` field"
    );
}

#[tokio::test]
async fn resolve_by_mpn_normalizes_manufacturer_casing() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/generated/resolve\
         ?manufacturer=TEXAS%20INSTRUMENTS\
         &mpn=TPS5430DDAR\
         &package=SO-PowerPAD-8",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["name"], "TPS5430DDAR");
}

#[tokio::test]
async fn resolve_by_mpn_returns_not_found_for_unknown_part() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/generated/resolve\
         ?manufacturer=Nobody\
         &mpn=NOPE-1\
         &package=DIP-8",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["status"], "not_found");
}

#[tokio::test]
async fn resolve_by_mpn_rejects_missing_query_arg() {
    // axum's Query extractor emits a plain-text body when a required field is
    // missing, so we assert the status only; the AppError JSON envelope only
    // wraps errors that reach our handler.
    let app = build_app(common::fixture_app_state_with_generated());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/generated/resolve?manufacturer=TI&mpn=TPS5430DDAR")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn resolve_by_mpn_rejects_empty_manufacturer() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/generated/resolve\
         ?manufacturer=\
         &mpn=TPS5430DDAR\
         &package=SO-PowerPAD-8",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn get_symbol_provenance_returns_expected_shape() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/generated/generated:texas_instruments/TPS5430DDAR/provenance",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["part_id"]["mpn"], "TPS5430DDAR");
    assert_eq!(v["part_id"]["manufacturer_norm"], "texas instruments");
    assert_eq!(v["status"], "published");
    assert_eq!(
        v["evidence"]["region_ids"].as_array().unwrap().len(),
        2,
        "seeded fixture has two evidence regions"
    );
}

#[tokio::test]
async fn get_symbol_provenance_returns_not_found_for_missing() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/generated/generated:nobody/GHOST/provenance",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["status"], "not_found");
}

#[tokio::test]
async fn get_symbol_endpoint_dispatches_to_generated_lib() {
    // Existing GET /v1/symbols/:lib/:name endpoint must transparently route
    // generated:* lookups to the generated store.
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/symbols/generated:texas_instruments/TPS5430DDAR",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["name"], "TPS5430DDAR");
    assert_eq!(v["body"]["pins"].as_array().unwrap().len(), 8);
}

#[tokio::test]
async fn search_returns_generated_hits_with_source_marker() {
    let (status, v) = request_json(
        common::fixture_app_state_with_generated(),
        "/v1/search?q=tps5430",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = v["items"].as_array().unwrap();
    let generated: Vec<_> = items
        .iter()
        .filter(|i| i["source"] == "generated")
        .collect();
    assert!(
        !generated.is_empty(),
        "search must surface published generated symbols"
    );
    assert_eq!(generated[0]["name"], "TPS5430DDAR");
}
