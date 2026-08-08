//! REST mirrors for the generated-symbol MCP tools.
//!
//! `GET /v1/generated/resolve` — resolve_by_mpn.
//! `GET /v1/generated/:lib/:name/provenance` — get_symbol_provenance.
//!
//! Both endpoints are read-only. The generated store is populated by the
//! offline packer (`tokito-mcp-pack --generated`); the MCP/REST faces never
//! accept writes into it.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tokito_symbols::{generated, model::ResolvedSymbol, part_id::PartId};

use crate::{error::AppError, state::AppState};

/// Untagged so `Found` serializes as a bare ResolvedSymbol (matching the
/// existing `GET /v1/symbols/:lib/:name` shape) while `NotFound` carries a
/// tiny sentinel object. Clients discriminate by presence of `body`.
/// `ResolvedSymbol` is ~340B so the large variant is boxed to keep the enum
/// size small on the happy-path stack.
#[derive(Serialize)]
#[serde(untagged)]
pub enum ResolveResponse {
    Found(Box<ResolvedSymbol>),
    NotFound { status: &'static str },
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ProvenanceResponse {
    Found(serde_json::Value),
    NotFound { status: &'static str },
}

// Mirrored from the MCP tool caps in `mcp/server.rs`.
const MAX_LIB_NAME_LEN: usize = 64;
const MAX_SYMBOL_NAME_LEN: usize = 128;
const MAX_MANUFACTURER_LEN: usize = 128;
const MAX_MPN_LEN: usize = 96;
const MAX_PACKAGE_LEN: usize = 64;

#[derive(Debug, Deserialize)]
pub struct ResolveByMpnQuery {
    pub manufacturer: String,
    pub mpn: String,
    pub package: String,
}

/// `GET /v1/generated/resolve?manufacturer=...&mpn=...&package=...`
pub async fn resolve_by_mpn(
    State(s): State<AppState>,
    Query(q): Query<ResolveByMpnQuery>,
) -> Result<Json<ResolveResponse>, AppError> {
    check_len("manufacturer", &q.manufacturer, MAX_MANUFACTURER_LEN)?;
    check_len("mpn", &q.mpn, MAX_MPN_LEN)?;
    check_len("package", &q.package, MAX_PACKAGE_LEN)?;

    let part = PartId::new(&q.manufacturer, &q.mpn, &q.package)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let conn = s.conn.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());
        generated::resolve_by_mpn(&c, &part)
    })
    .await??;

    Ok(Json(match resolved {
        Some(sym) => ResolveResponse::Found(Box::new((*sym).clone())),
        None => ResolveResponse::NotFound {
            status: "not_found",
        },
    }))
}

/// `GET /v1/generated/:lib/:name/provenance`
pub async fn get_symbol_provenance(
    State(s): State<AppState>,
    Path((lib, name)): Path<(String, String)>,
) -> Result<Json<ProvenanceResponse>, AppError> {
    check_len("lib", &lib, MAX_LIB_NAME_LEN)?;
    check_len("name", &name, MAX_SYMBOL_NAME_LEN)?;

    let conn = s.conn.clone();
    let prov = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());
        generated::provenance_for_symbol(&c, &lib, &name)
    })
    .await??;

    Ok(Json(match prov {
        Some(v) => ProvenanceResponse::Found(v),
        None => ProvenanceResponse::NotFound {
            status: "not_found",
        },
    }))
}

fn check_len(field: &str, value: &str, max: usize) -> Result<(), AppError> {
    if value.len() > max {
        return Err(AppError::BadRequest(format!(
            "`{field}` exceeds {max} bytes"
        )));
    }
    Ok(())
}
