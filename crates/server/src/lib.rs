//! tokito-mcp-server — library surface used by `main.rs` and integration tests.
//!
//! Holds the axum router assembly. Keeping it in a library makes the same
//! routes testable end-to-end without binding a TCP port.

pub mod error;
pub mod mcp;
pub mod part_offer_query;
pub mod rest;
pub mod state;

use std::time::Duration;

use axum::http::{header, HeaderValue, Method};
use axum::Router;
use state::AppState;
use tower::ServiceBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};
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

/// Default cap on concurrent MCP sessions (see `ServerConfig::max_sessions`).
pub const DEFAULT_MAX_SESSIONS: usize = 256;

/// Network-exposure config plumbed from the CLI / env (see `main.rs`).
///
/// Both faces are public-facing, so each guards a different vector:
/// `allowed_hosts` is the MCP DNS-rebinding `Host` allowlist; `allowed_origins`
/// gates browser `Origin` (MCP `Origin` validation + the REST CORS layer);
/// `max_sessions` bounds the MCP session map against an `initialize`-loop DoS.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// MCP `Host` authorities. `None` keeps rmcp's safe default (loopback only);
    /// `Some(list)` overrides it — public deployments set their real host(s).
    pub allowed_hosts: Option<Vec<String>>,
    /// Browser origins allowed for REST CORS *and* MCP `Origin` validation.
    /// Empty = no CORS layer and MCP `Origin` checking stays disabled.
    pub allowed_origins: Vec<String>,
    /// Maximum concurrent MCP sessions; `create_session` past this is rejected.
    pub max_sessions: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            allowed_hosts: None,
            allowed_origins: Vec::new(),
            max_sessions: DEFAULT_MAX_SESSIONS,
        }
    }
}

/// Build the router with default exposure config (loopback-only MCP host
/// allowlist, no CORS). Convenience for tests and local runs.
pub fn build_app(state: AppState) -> Router {
    build_app_with_config(state, ServerConfig::default())
}

/// Build the fully-assembled router with both REST and MCP faces.
///
/// Layer ordering: timeout + concurrency-limit wrap the outer router so they
/// apply to BOTH faces; the MCP service gets its own body limit because
/// `nest_service` bypasses axum's `DefaultBodyLimit` and our outer REST limit
/// is too tight for MCP envelopes. CORS is applied to the REST face only — the
/// MCP service does its own `Origin` validation internally.
pub fn build_app_with_config(state: AppState, cfg: ServerConfig) -> Router {
    let mcp_service = ServiceBuilder::new()
        .layer(RequestBodyLimitLayer::new(MCP_BODY_LIMIT_BYTES))
        .service(mcp::build_mcp_service(
            state.clone(),
            cfg.allowed_hosts.clone(),
            cfg.allowed_origins.clone(),
            cfg.max_sessions,
        ));

    let mut rest = rest::routes().with_state(state);
    if !cfg.allowed_origins.is_empty() {
        rest = rest.layer(cors_layer(&cfg.allowed_origins));
    }

    Router::new()
        .merge(rest)
        .nest_service("/mcp", mcp_service)
        .layer(RequestBodyLimitLayer::new(REST_BODY_LIMIT_BYTES))
        .layer(TimeoutLayer::new(REQUEST_TIMEOUT))
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            MAX_CONCURRENT_REQUESTS,
        ))
}

/// CORS for the REST face. REST is read-only (GET), so we allow only GET and the
/// `Content-Type` header, scoped to the configured origins. Unparseable origin
/// entries are dropped (a misconfig narrows access rather than widening it).
fn cors_layer(origins: &[String]) -> CorsLayer {
    let list: Vec<HeaderValue> = origins.iter().filter_map(|o| o.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(list))
        .allow_methods([Method::GET])
        .allow_headers([header::CONTENT_TYPE])
}
