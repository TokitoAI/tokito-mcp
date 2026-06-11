//! Full symbol fetch — resolves `extends` and returns pins + graphics.

use axum::{
    extract::{Path, State},
    Json,
};
use tokito_symbols::model::ResolvedSymbol;

use crate::{error::AppError, state::AppState};

pub async fn get_symbol(
    State(s): State<AppState>,
    Path((lib, name)): Path<(String, String)>,
) -> Result<Json<ResolvedSymbol>, AppError> {
    let conn = s.conn.clone();
    let resolver = s.resolver.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap();
        resolver.resolve(&c, &lib, &name)
    })
    .await??;
    Ok(Json((*resolved).clone()))
}
