//! Health / manifest / library-list handlers.

use axum::{extract::State, Json};
use tokito_symbols::search::LibInfo;

use crate::{error::AppError, state::{AppState, Manifest}};

pub async fn health() -> &'static str {
    "ok"
}

pub async fn manifest(State(s): State<AppState>) -> Json<Manifest> {
    Json((*s.manifest).clone())
}

pub async fn libraries(State(s): State<AppState>) -> Result<Json<Vec<LibInfo>>, AppError> {
    let conn = s.conn.clone();
    let libs = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap();
        tokito_symbols::search::list_libraries(&c)
    })
    .await??;
    Ok(Json(libs))
}
