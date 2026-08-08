//! REST face â€” exercise every endpoint through the assembled router via
//! tower::ServiceExt::oneshot. No port binding, no async tasks beyond the
//! single request future.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokito_mcp_server::build_app;
use tower::ServiceExt;

mod common;

async fn request_json(uri: &str) -> (StatusCode, Value) {
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
    let json: Value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| panic!("non-JSON body for {uri}: {:?}", body));
    (status, json)
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let app = build_app(common::fixture_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"ok");
}

#[tokio::test]
async fn manifest_reports_fixture_counts() {
    let (status, v) = request_json("/v1/manifest").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["source_commit"], "test-fixture");
    assert_eq!(v["schema_version"], 2);
    assert_eq!(v["symbol_count"], 3);
    assert_eq!(v["lib_count"], 2);
}

#[tokio::test]
async fn libraries_returns_two_with_counts() {
    let (status, v) = request_json("/v1/libraries").await;
    assert_eq!(status, StatusCode::OK);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let by_name: std::collections::HashMap<_, _> = arr
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap(),
                r["symbol_count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(by_name["Device"], 1);
    assert_eq!(by_name["Amplifier_Op"], 2);
}

#[tokio::test]
async fn search_opamp_returns_both_op_symbols() {
    let (status, v) = request_json("/v1/search?q=opamp&limit=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["query"], "opamp");
    let items = v["items"].as_array().unwrap();
    assert!(items.len() >= 2);
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"ROOT_OP"));
    assert!(names.contains(&"LMxxx_A"));
}

#[tokio::test]
async fn search_scoped_by_lib_drops_others() {
    let (status, v) = request_json("/v1/search?q=opamp&lib=Device&limit=10").await;
    assert_eq!(status, StatusCode::OK);
    let items = v["items"].as_array().unwrap();
    assert!(items.is_empty(), "Device has no opamps");
}

#[tokio::test]
async fn search_empty_query_returns_400() {
    let (status, v) = request_json("/v1/search?q=&limit=10").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn get_root_symbol_returns_body() {
    let (status, v) = request_json("/v1/symbols/Device/R").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["name"], "R");
    assert!(v["parent"].is_null());
    assert_eq!(v["body"]["pins"].as_array().unwrap().len(), 2);
    assert_eq!(v["body"]["graphics"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn get_extending_child_returns_parent_body() {
    let (status, v) = request_json("/v1/symbols/Amplifier_Op/LMxxx_A").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["name"], "LMxxx_A");
    let parent = v["parent"].as_array().unwrap();
    assert_eq!(parent[0], "Amplifier_Op");
    assert_eq!(parent[1], "ROOT_OP");
    assert_eq!(
        v["body"]["pins"].as_array().unwrap().len(),
        5,
        "inherits parent pins"
    );
    assert_eq!(v["description"], "Single low-noise opamp, DIP-8");
    assert_eq!(v["fp_filters"], "DIP-8*");
}

#[tokio::test]
async fn get_missing_symbol_returns_404() {
    let (status, v) = request_json("/v1/symbols/Device/NoSuchSymbol").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
}

#[tokio::test]
async fn list_lib_symbols_paginates() {
    let (status, v) = request_json("/v1/libraries/Amplifier_Op/symbols?limit=1&offset=0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["lib"], "Amplifier_Op");
    assert_eq!(v["total"], 2);
    assert_eq!(v["limit"], 1);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn part_offer_query_returns_procurement_hint() {
    let (status, v) =
        request_json("/v1/part-offer-query?symbol_id=Device:R&value=330&package=R_0603&market=IN")
            .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["symbol_id"], "Device:R");
    assert_eq!(v["procurement_query"], "330 resistor, 0603 package");
    assert_eq!(v["exact_mpn"], Value::Null);
    let domains = v["distributor_domains"].as_array().unwrap();
    assert!(domains.iter().any(|d| d == "digikey.in"));
}

#[tokio::test]
async fn part_offer_query_requires_symbol_key() {
    let (status, v) = request_json("/v1/part-offer-query?value=330").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}
