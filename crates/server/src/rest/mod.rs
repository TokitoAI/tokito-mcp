//! REST face — JSON endpoints for the desktop renderer and any non-AI client.

use axum::{routing::get, Router};

use crate::state::AppState;

mod meta;
mod search;
mod symbols;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/health", get(meta::health))
        .route("/v1/manifest", get(meta::manifest))
        .route("/v1/libraries", get(meta::libraries))
        .route("/v1/libraries/:lib/symbols", get(search::list_symbols))
        .route("/v1/search", get(search::search))
        .route("/v1/compatible", get(search::find_compatible))
        .route("/v1/symbols/:lib/:name", get(symbols::get_symbol))
}
