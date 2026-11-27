use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use tokito_symbols::footprints::{lookup, FootprintId};

use crate::{error::AppError, state::AppState};

#[derive(Serialize)]
pub(super) struct FootprintResponse {
    footprint_id: String,
    source_revision: String,
    source_sha256: String,
    source: String,
}

pub(super) async fn get_footprint(
    State(state): State<AppState>,
    Path((lib, name)): Path<(String, String)>,
) -> Result<Result<Json<FootprintResponse>, StatusCode>, AppError> {
    let found = tokio::task::spawn_blocking(move || {
        let id = FootprintId::new(&lib, &name)
            .ok_or_else(|| AppError::BadRequest("invalid footprint identity".into()))?;
        let conn = state.conn.lock().unwrap_or_else(|p| p.into_inner());
        Ok::<_, AppError>(lookup(&conn, &id)?.map(|record| FootprintResponse {
            footprint_id: format!("{lib}:{name}"),
            source_revision: record.revision,
            source_sha256: record.sha256,
            source: record.source,
        }))
    })
    .await??;
    Ok(found.map(Json).ok_or(StatusCode::NOT_FOUND))
}
