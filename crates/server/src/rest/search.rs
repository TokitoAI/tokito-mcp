//! Search + per-library listing.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tokito_symbols::{model::SymbolRef, search};

use crate::{error::AppError, part_offer_query, state::AppState};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 200;

// Length caps for user-supplied query / lib / fp_pattern, mirroring the
// MCP face. Anything bigger is almost certainly hostile input headed at
// FTS5's worst-case parse path.
const MAX_QUERY_LEN: usize = 256;
const MAX_LIB_NAME_LEN: usize = 64;
const MAX_FP_PATTERN_LEN: usize = 64;

fn check_len(field: &str, value: &str, max: usize) -> Result<(), AppError> {
    if value.len() > max {
        return Err(AppError::BadRequest(format!(
            "`{field}` exceeds {max} bytes"
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<u32>,
    pub lib: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub total: usize,
    pub items: Vec<SymbolRef>,
}

pub async fn search(
    State(s): State<AppState>,
    Query(p): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    if p.q.trim().is_empty() {
        return Err(AppError::BadRequest("query parameter `q` is empty".into()));
    }
    check_len("q", &p.q, MAX_QUERY_LEN)?;
    if let Some(lib) = p.lib.as_deref() {
        check_len("lib", lib, MAX_LIB_NAME_LEN)?;
    }
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let query = p.q.clone();

    let conn = s.conn.clone();
    let items: Vec<SymbolRef> = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());
        search::search(
            &c,
            search::SearchOpts {
                query: &p.q,
                limit,
                lib_filter: p.lib.as_deref(),
            },
        )
    })
    .await??;

    Ok(Json(SearchResponse {
        query,
        total: items.len(),
        items,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct LibSymbol {
    pub name: String,
    pub ref_des: String,
    pub description: String,
    pub keywords: String,
    pub pin_count: u32,
    pub has_parent: bool,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub lib: String,
    pub total: u64,
    pub offset: u32,
    pub limit: u32,
    pub items: Vec<LibSymbol>,
}

#[derive(Debug, Deserialize)]
pub struct CompatibleParams {
    /// Exact pin count.
    pub pins: Option<u32>,
    /// Footprint pattern, matched anywhere in `fp_filters` or `footprint`.
    pub fp_pattern: Option<String>,
    /// Optional FTS5 keyword query (e.g. "I2C").
    pub query: Option<String>,
    /// Restrict to a single library.
    pub lib: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CompatibleResponse {
    pub total: usize,
    pub items: Vec<SymbolRef>,
}

#[derive(Debug, Deserialize)]
pub struct PartOfferQueryParams {
    /// Symbol id in `Library:Name` form, e.g. `Device:R`.
    pub symbol_id: Option<String>,
    /// Library name, alternative to `symbol_id`.
    pub lib: Option<String>,
    /// Symbol name, alternative to `symbol_id`.
    pub name: Option<String>,
    /// Schematic value, e.g. `330`, `10 uF`, `Red`.
    pub value: Option<String>,
    /// Package / footprint hint, e.g. `R_0603`.
    pub package: Option<String>,
    /// ISO country/market hint, e.g. `IN`.
    pub market: Option<String>,
}

pub async fn find_compatible(
    State(s): State<AppState>,
    Query(p): Query<CompatibleParams>,
) -> Result<Json<CompatibleResponse>, AppError> {
    if p.pins.is_none() && p.fp_pattern.is_none() && p.query.is_none() {
        return Err(AppError::BadRequest(
            "at least one of `pins`, `fp_pattern`, or `query` is required".into(),
        ));
    }
    if let Some(q) = p.query.as_deref() {
        check_len("query", q, MAX_QUERY_LEN)?;
    }
    if let Some(pat) = p.fp_pattern.as_deref() {
        check_len("fp_pattern", pat, MAX_FP_PATTERN_LEN)?;
    }
    if let Some(lib) = p.lib.as_deref() {
        check_len("lib", lib, MAX_LIB_NAME_LEN)?;
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 200);

    let conn = s.conn.clone();
    let items: Vec<SymbolRef> = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());
        search::find_compatible(
            &c,
            search::CompatibleOpts {
                pins: p.pins,
                fp_pattern: p.fp_pattern.as_deref(),
                query: p.query.as_deref(),
                limit,
                lib_filter: p.lib.as_deref(),
            },
        )
    })
    .await??;

    Ok(Json(CompatibleResponse {
        total: items.len(),
        items,
    }))
}

pub async fn part_offer_query(
    State(s): State<AppState>,
    Query(p): Query<PartOfferQueryParams>,
) -> Result<Json<part_offer_query::PartOfferQueryResponse>, AppError> {
    let (lib, name) = symbol_keys(p.symbol_id.as_deref(), p.lib.as_deref(), p.name.as_deref())?;
    check_len("lib", &lib, MAX_LIB_NAME_LEN)?;
    check_len("name", &name, 128)?;
    if let Some(value) = p.value.as_deref() {
        check_len("value", value, MAX_QUERY_LEN)?;
    }
    if let Some(package) = p.package.as_deref() {
        check_len("package", package, MAX_FP_PATTERN_LEN)?;
    }
    if let Some(market) = p.market.as_deref() {
        check_len("market", market, 8)?;
    }

    let symbol_id = part_offer_query::symbol_id(&lib, &name);
    let conn = s.conn.clone();
    let resolver = s.resolver.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());
        resolver.resolve(&c, &lib, &name)
    })
    .await??;
    let response = part_offer_query::build_response(
        &symbol_id,
        p.value.as_deref(),
        p.package.as_deref(),
        p.market.as_deref(),
        Some(&resolved),
    );
    Ok(Json(response))
}

pub async fn list_symbols(
    State(s): State<AppState>,
    Path(lib): Path<String>,
    Query(p): Query<ListParams>,
) -> Result<Json<ListResponse>, AppError> {
    check_len("lib", &lib, MAX_LIB_NAME_LEN)?;
    // Lowered from 2000 — audit flagged the previous cap as a 10x outlier
    // that let a single client pull multi-MB JSON per request and hold the
    // global mutex for the duration of the stream.
    let limit = p.limit.unwrap_or(200).clamp(1, 200);
    // Cap offset too so an attacker can't sweep ascending offsets to bust caches.
    let offset = p.offset.unwrap_or(0).min(50_000);
    let lib_filter = lib.clone();

    let conn = s.conn.clone();
    let (total, items): (u64, Vec<LibSymbol>) = tokio::task::spawn_blocking(move || {
        let c = conn.lock().unwrap_or_else(|p| p.into_inner());

        let total: u64 = c.query_row(
            "SELECT COUNT(*) FROM symbol s JOIN lib l ON l.id = s.lib_id WHERE l.name = ?1",
            rusqlite::params![&lib_filter],
            |r| r.get::<_, i64>(0).map(|n| n as u64),
        )?;

        let mut stmt = c.prepare(
            "SELECT s.name, s.ref_des, s.description, s.keywords, s.pin_count, s.parent_id IS NOT NULL \
             FROM symbol s JOIN lib l ON l.id = s.lib_id \
             WHERE l.name = ?1 ORDER BY s.name LIMIT ?2 OFFSET ?3",
        )?;
        let items = stmt
            .query_map(rusqlite::params![&lib_filter, limit, offset], |r| {
                Ok(LibSymbol {
                    name: r.get(0)?,
                    ref_des: r.get(1)?,
                    description: r.get(2)?,
                    keywords: r.get(3)?,
                    pin_count: r.get::<_, i64>(4)? as u32,
                    has_parent: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok::<_, AppError>((total, items))
    })
    .await??;

    Ok(Json(ListResponse {
        lib,
        total,
        offset,
        limit,
        items,
    }))
}

fn symbol_keys(
    symbol_id: Option<&str>,
    lib: Option<&str>,
    name: Option<&str>,
) -> Result<(String, String), AppError> {
    if let Some(symbol_id) = symbol_id {
        let (lib, name) = part_offer_query::split_symbol_id(symbol_id).ok_or_else(|| {
            AppError::BadRequest("symbol_id must be in `Library:Name` form".into())
        })?;
        return Ok((lib.to_string(), name.to_string()));
    }
    let lib = lib
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("lib is required when symbol_id is absent".into()))?;
    let name = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("name is required when symbol_id is absent".into()))?;
    Ok((lib.to_string(), name.to_string()))
}
