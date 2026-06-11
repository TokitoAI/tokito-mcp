//! tokito-mcp-server — library surface used by `main.rs` and integration tests.
//!
//! Holds the axum router assembly. Keeping it in a library makes the same
//! routes testable end-to-end without binding a TCP port.

pub mod error;
pub mod mcp;
pub mod rest;
pub mod state;

use std::time::Duration;

use axum::Router;
use state::AppState;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

/// Caps on what the public service will accept. Picked deliberately tight so a
/// single hostile client can't head-of-line every other request.
///
/// REST body limit is small because the largest legitimate REST body is a
/// nonexistent thing — every endpoint is GET. The MCP envelope is JSON-RPC
/// over POST so the cap needs headroom for batched tool calls.
pub const REST_BODY_LIMIT_BYTES: usize = 64 * 1024;
pub const MCP_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Wall-clock budget for any one request. SQLite read-only mmap queries are
/// sub-millisecond; FTS5 on hostile input can spend seconds. 5s is generous
/// without leaving room for a head-of-line attack.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum requests in flight across the whole service. Higher than this and
/// a slow query through the single Mutex<Connection> will queue requests
/// indefinitely; better to return 503 fast and let the client retry/back off.
pub const MAX_CONCURRENT_REQUESTS: usize = 64;

/// Build the fully-assembled router with both REST and MCP faces.
///
/// Layer ordering: timeout + concurrency-limit wrap the outer router so they
/// apply to BOTH faces; the MCP service gets its own body limit because
/// `nest_service` bypasses axum's `DefaultBodyLimit` and our outer REST limit
/// is too tight for MCP envelopes.
pub fn build_app(state: AppState) -> Router {
    let mcp_service = ServiceBuilder::new()
        .layer(RequestBodyLimitLayer::new(MCP_BODY_LIMIT_BYTES))
        .service(mcp::build_mcp_service(state.clone()));

    Router::new()
        .merge(rest::routes())
        .with_state(state)
        .nest_service("/mcp", mcp_service)
        .layer(RequestBodyLimitLayer::new(REST_BODY_LIMIT_BYTES))
        .layer(TimeoutLayer::new(REQUEST_TIMEOUT))
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            MAX_CONCURRENT_REQUESTS,
        ))
}
