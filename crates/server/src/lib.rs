//! tokito-mcp-server — library surface used by `main.rs` and integration tests.
//!
//! Holds the axum router assembly. Keeping it in a library makes the same
//! routes testable end-to-end without binding a TCP port.

pub mod error;
pub mod mcp;
pub mod rest;
pub mod state;

use axum::Router;
use state::AppState;

/// Build the fully-assembled router with both REST and MCP faces.
pub fn build_app(state: AppState) -> Router {
    let mcp_service = mcp::build_mcp_service(state.clone());
    Router::new()
        .merge(rest::routes())
        .with_state(state)
        .nest_service("/mcp", mcp_service)
}
