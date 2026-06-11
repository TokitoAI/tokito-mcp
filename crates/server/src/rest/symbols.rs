//! Full symbol fetch — resolves `extends` and returns pins + graphics.

use axum::{
    extract::{Path, State},
    Json,
};
use tokito_symbols::model::ResolvedSymbol;

use crate::{error::AppError, state::AppState};

// Length caps matching the search-handler caps.
const MAX_LIB_NAME_LEN: usize = 64;
const MAX_SYMBOL_NAME_LEN: usize = 128;

pub async fn get_symbol(
    State(s): State<AppState>,
    Path((lib, name)): Path<(String, String)>,
) -> Result<Json<ResolvedSymbol>, AppError> {
    if lib.len() > MAX_LIB_NAME_LEN {
        return Err(AppError::BadRequest(format!(
            "`lib` exceeds {MAX_LIB_NAME_LEN} bytes"
        )));
    }
    if name.len() > MAX_SYMBOL_NAME_LEN {
        return Err(AppError::BadRequest(format!(
            "`name` exceeds {MAX_SYMBOL_NAME_LEN} bytes"
        )));
    }
    let conn = s.conn.clone();
    let resolver = s.resolver.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());
        resolver.resolve(&c, &lib, &name)
    })
    .await??;
    Ok(Json((*resolved).clone()))
}
