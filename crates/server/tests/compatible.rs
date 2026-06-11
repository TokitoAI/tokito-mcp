//! REST /v1/compatible — capability search.

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
        .unwrap_or_else(|_| panic!("non-JSON body for {uri}: {:?}", &body));
    (status, json)
}

#[tokio::test]
async fn requires_at_least_one_filter() {
    let (status, v) = request_json("/v1/compatible").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn filters_by_pin_count() {
    let (status, v) = request_json("/v1/compatible?pins=2").await;
    assert_eq!(status, StatusCode::OK);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "R");
}

#[tokio::test]
async fn filters_by_footprint_pattern() {
    let (status, v) = request_json("/v1/compatible?fp_pattern=DIP").await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ROOT_OP"));
    assert!(names.contains(&"LMxxx_A"));
}

#[tokio::test]
async fn combines_pins_and_query() {
    let (status, v) = request_json("/v1/compatible?pins=5&query=opamp").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
}
