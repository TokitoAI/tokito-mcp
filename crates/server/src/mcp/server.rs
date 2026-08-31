//! MCP server implementation. Tools wrap the same code REST uses:
//!
//!   - `search_symbols(query, limit?, lib?)`
//!   - `get_symbol(lib, name)`
//!   - `find_compatible(pins?, fp_pattern?, query?, lib?, limit?)`
//!   - `part_offer_query(symbol_id?, lib?, name?, value?, package?, market?)`
//!   - `resolve_by_mpn(manufacturer, mpn, package)`
//!   - `get_symbol_provenance(revision_id? | lib + name)`
//!   - `list_libraries()`
//!
//! Tool handlers run heavy SQL inside `spawn_blocking` so the tokio runtime
//! isn't held back by SQLite mutex acquisition.

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, ErrorCode, ErrorData as McpError, Implementation,
        ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokito_symbols::{generated, part_id::PartId, search};

use crate::{part_offer_query, state::AppState};

use super::capped::CappedSessionManager;

pub type McpService =
    rmcp::transport::streamable_http_server::StreamableHttpService<Tokito, CappedSessionManager>;

/// Re-exported via mod.rs so main.rs doesn't have to know rmcp internals.
///
/// `allowed_hosts: None` keeps rmcp's safe loopback-only default (DNS-rebinding
/// guard); `Some(list)` overrides it for public deployments. `allowed_origins`
/// enables MCP `Origin` validation when non-empty (empty = disabled, the rmcp
/// default). `max_sessions` bounds concurrent sessions via `CappedSessionManager`.
pub fn build_mcp_service(
    state: AppState,
    allowed_hosts: Option<Vec<String>>,
    allowed_origins: Vec<String>,
    max_sessions: usize,
) -> McpService {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };
    let mut config = StreamableHttpServerConfig::default();
    if let Some(hosts) = allowed_hosts {
        config.allowed_hosts = hosts;
    }
    config.allowed_origins = allowed_origins;
    StreamableHttpService::new(
        move || Ok(Tokito::new(state.clone())),
        Arc::new(CappedSessionManager::new(max_sessions)),
        config,
    )
}

#[derive(Clone)]
pub struct Tokito {
    state: AppState,
    // Built by the `#[tool_router]` macro; field is referenced by the generated
    // `tool_handler` impl, but the compiler can't see across the macro boundary.
    #[allow(dead_code)]
    tool_router: ToolRouter<Tokito>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// FTS5 query (supports prefix `*`, boolean AND/OR/NOT, column filters like `name:`).
    pub query: String,
    /// Max number of results to return. Defaults to 20, capped at 200.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Restrict to a single library by name (e.g. "MCU_Microchip_ATmega").
    #[serde(default)]
    pub lib: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetSymbolArgs {
    /// Library name (e.g. "Device", "MCU_Microchip_ATmega").
    pub lib: String,
    /// Symbol name within that library (e.g. "R", "ATmega328P-A").
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindCompatibleArgs {
    /// Exact pin count to match (e.g. 32 for a TQFP-32 part).
    #[serde(default)]
    pub pins: Option<u32>,
    /// Footprint pattern, matched anywhere in the symbol's `fp_filters`
    /// or `footprint` (e.g. "TQFP", "SOIC-8").
    #[serde(default)]
    pub fp_pattern: Option<String>,
    /// Optional FTS5 keyword query (e.g. "I2C low-power").
    #[serde(default)]
    pub query: Option<String>,
    /// Restrict to a single library (e.g. "MCU_Microchip_ATmega").
    #[serde(default)]
    pub lib: Option<String>,
    /// Max results to return. Defaults to 50, capped at 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ResolveByMpnArgs {
    /// Manufacturer name as printed on the datasheet, e.g. "STMicroelectronics".
    /// Normalized server-side (NFC + lowercase + collapse whitespace) before
    /// the DB lookup — see `tokito_symbols::part_id::PartId` for the exact
    /// rules.
    pub manufacturer: String,
    /// Exact manufacturer part number, case-sensitive.
    pub mpn: String,
    /// Package/variant string, e.g. "LQFP100", "SO-PowerPAD-8".
    pub package: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetSymbolProvenanceArgs {
    /// Library id, e.g. "generated:stmicroelectronics" or "official:MCU_ST_STM32H7".
    #[serde(default)]
    pub lib: Option<String>,
    /// Symbol name (typically the MPN for generated symbols).
    #[serde(default)]
    pub name: Option<String>,
    /// Exact immutable generated revision id. Use this instead of `(lib,
    /// name)` when validating a just-published ingestion response.
    #[serde(default)]
    pub revision_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PartOfferQueryArgs {
    /// Symbol id in `Library:Name` form, e.g. `Device:R`.
    #[serde(default)]
    pub symbol_id: Option<String>,
    /// Library name, alternative to `symbol_id`.
    #[serde(default)]
    pub lib: Option<String>,
    /// Symbol name, alternative to `symbol_id`.
    #[serde(default)]
    pub name: Option<String>,
    /// Schematic value, e.g. `330`, `10 uF`, `Red`.
    #[serde(default)]
    pub value: Option<String>,
    /// Package / footprint hint, e.g. `R_0603`.
    #[serde(default)]
    pub package: Option<String>,
    /// ISO country/market hint, e.g. `IN`.
    #[serde(default)]
    pub market: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchResponseJson {
    query: String,
    total: usize,
    items: Vec<tokito_symbols::model::SymbolRef>,
}

#[tool_router]
impl Tokito {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search the KiCad symbol library by keywords or part number. \
                       Ranked by BM25 over symbol name, description, keywords, and \
                       footprint filters. Returns up to `limit` results."
    )]
    async fn search_symbols(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.query.trim().is_empty() {
            return Err(McpError::new(
                ErrorCode::INVALID_PARAMS,
                "query is empty".to_string(),
                None,
            ));
        }
        check_len("query", &args.query, MAX_QUERY_LEN)?;
        if let Some(lib) = args.lib.as_deref() {
            check_len("lib", lib, MAX_LIB_NAME_LEN)?;
        }
        let limit = args.limit.unwrap_or(20).clamp(1, 200);
        let state = self.state.clone();
        let query = args.query.clone();
        let items = tokio::task::spawn_blocking(move || {
            state.search_catalogs(&args.query, limit, args.lib.as_deref())
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;

        let total = items.len();
        let response = SearchResponseJson {
            query,
            total,
            items,
        };
        ok_json(&response)
    }

    #[tool(
        description = "Fetch a single symbol, fully resolved (any `extends` parent \
                       body is merged in). Returns pins (number, name, electrical \
                       type, geometry), graphics (rectangles, polylines, arcs, \
                       circles, text), units, and properties."
    )]
    async fn get_symbol(
        &self,
        Parameters(args): Parameters<GetSymbolArgs>,
    ) -> Result<CallToolResult, McpError> {
        check_len("lib", &args.lib, MAX_LIB_NAME_LEN)?;
        check_len("name", &args.name, MAX_SYMBOL_NAME_LEN)?;
        let conn = self.state.connection_for_lib(&args.lib);
        let resolver = self.state.resolver.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            resolver.resolve(&c, &args.lib, &args.name)
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;

        ok_json(&*resolved)
    }

    #[tool(
        description = "Capability search — find symbols matching structured constraints. \
                       Combine `pins` (exact pin count), `fp_pattern` (footprint substring), \
                       and `query` (FTS5 keyword query) to express \"32-pin TQFP MCU with I2C\" \
                       and similar. At least one filter is required. Without `query`, results \
                       are sorted by pin count then name; with `query`, by FTS5 BM25 rank."
    )]
    async fn find_compatible(
        &self,
        Parameters(args): Parameters<FindCompatibleArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.pins.is_none() && args.fp_pattern.is_none() && args.query.is_none() {
            return Err(McpError::new(
                ErrorCode::INVALID_PARAMS,
                "at least one of `pins`, `fp_pattern`, or `query` must be set".to_string(),
                None,
            ));
        }
        if let Some(q) = args.query.as_deref() {
            check_len("query", q, MAX_QUERY_LEN)?;
        }
        if let Some(p) = args.fp_pattern.as_deref() {
            check_len("fp_pattern", p, MAX_FP_PATTERN_LEN)?;
        }
        if let Some(lib) = args.lib.as_deref() {
            check_len("lib", lib, MAX_LIB_NAME_LEN)?;
        }
        let limit = args.limit.unwrap_or(50).clamp(1, 200);
        let conn = self.state.conn.clone();
        let items = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            search::find_compatible(
                &c,
                search::CompatibleOpts {
                    pins: args.pins,
                    fp_pattern: args.fp_pattern.as_deref(),
                    query: args.query.as_deref(),
                    limit,
                    lib_filter: args.lib.as_deref(),
                },
            )
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;

        let total = items.len();
        ok_json(&serde_json::json!({
            "total": total,
            "items": items,
        }))
    }

    #[tool(
        description = "Build a distributor-search procurement query for a catalog symbol. \
                       Returns symbol metadata, datasheet hints when present, market-appropriate \
                       distributor domains, and a generic procurement_query. It does not return \
                       live pricing or stock."
    )]
    async fn part_offer_query(
        &self,
        Parameters(args): Parameters<PartOfferQueryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (lib, name) = symbol_keys(
            args.symbol_id.as_deref(),
            args.lib.as_deref(),
            args.name.as_deref(),
        )?;
        check_len("lib", &lib, MAX_LIB_NAME_LEN)?;
        check_len("name", &name, MAX_SYMBOL_NAME_LEN)?;
        if let Some(value) = args.value.as_deref() {
            check_len("value", value, MAX_QUERY_LEN)?;
        }
        if let Some(package) = args.package.as_deref() {
            check_len("package", package, MAX_FP_PATTERN_LEN)?;
        }
        if let Some(market) = args.market.as_deref() {
            check_len("market", market, 8)?;
        }

        let symbol_id = part_offer_query::symbol_id(&lib, &name);
        let conn = self.state.connection_for_lib(&lib);
        let resolver = self.state.resolver.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            resolver.resolve(&c, &lib, &name)
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;
        let response = part_offer_query::build_response(
            &symbol_id,
            args.value.as_deref(),
            args.package.as_deref(),
            args.market.as_deref(),
            Some(&resolved),
        );
        ok_json(&response)
    }

    #[tool(
        description = "Resolve a generated symbol by exact manufacturer + MPN + package \
                       identity. Returns the currently-published revision as a \
                       ResolvedSymbol when one exists, or `{ \"status\": \"not_found\" }` \
                       if the part is unknown or has no published revision yet. \
                       Read-only: the MCP surface never accepts writes to the \
                       generated store."
    )]
    async fn resolve_by_mpn(
        &self,
        Parameters(args): Parameters<ResolveByMpnArgs>,
    ) -> Result<CallToolResult, McpError> {
        check_len("manufacturer", &args.manufacturer, MAX_MANUFACTURER_LEN)?;
        check_len("mpn", &args.mpn, MAX_MPN_LEN)?;
        check_len("package", &args.package, MAX_PACKAGE_LEN)?;

        let part = PartId::new(&args.manufacturer, &args.mpn, &args.package)
            .map_err(|e| McpError::new(ErrorCode::INVALID_PARAMS, e.to_string(), None))?;

        let conn = self.state.generated_connection();
        let resolved = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            generated::resolve_by_mpn(&c, &part)
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;

        match resolved {
            Some(sym) => ok_json(&*sym),
            None => ok_json(&serde_json::json!({ "status": "not_found" })),
        }
    }

    #[tool(
        description = "Fetch the DS-ViRe provenance record for a generated symbol — \
                       datasheet identity, evidence region ids, extractor + compiler \
                       + retrieval versions, publication status, and content hash. \
                       Wire shape follows docs/CONTRACTS.md §5. Returns \
                       `{ \"status\": \"not_found\" }` when the symbol is not in \
                       the generated store or has no published revision."
    )]
    async fn get_symbol_provenance(
        &self,
        Parameters(args): Parameters<GetSymbolProvenanceArgs>,
    ) -> Result<CallToolResult, McpError> {
        let revision_id = args
            .revision_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let lib = args
            .lib
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let name = args
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let has_symbol_key = lib.is_some() || name.is_some();
        if revision_id.is_some() == has_symbol_key || lib.is_some() != name.is_some() {
            return Err(McpError::new(
                ErrorCode::INVALID_PARAMS,
                "provide either `revision_id` or both `lib` and `name`".to_string(),
                None,
            ));
        }
        if let Some(value) = revision_id.as_deref() {
            check_len("revision_id", value, MAX_REVISION_ID_LEN)?;
        }
        if let Some(value) = lib.as_deref() {
            check_len("lib", value, MAX_LIB_NAME_LEN)?;
        }
        if let Some(value) = name.as_deref() {
            check_len("name", value, MAX_SYMBOL_NAME_LEN)?;
        }
        let conn = self.state.generated_connection();
        let prov = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(revision_id) = revision_id.as_deref() {
                generated::provenance_for_revision(&c, revision_id)
            } else {
                generated::provenance_for_symbol(
                    &c,
                    lib.as_deref().expect("validated lib"),
                    name.as_deref().expect("validated name"),
                )
            }
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;
        match prov {
            Some(v) => ok_json(&v),
            None => ok_json(&serde_json::json!({ "status": "not_found" })),
        }
    }

    #[tool(
        description = "List every library in the catalog with its symbol count. \
                       Useful for browsing — e.g. to pick a domain (Amplifier_Operational, \
                       MCU_*, Connector_*) before narrowing a search."
    )]
    async fn list_libraries(&self) -> Result<CallToolResult, McpError> {
        let conn = self.state.conn.clone();
        let libs = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            search::list_libraries(&c)
        })
        .await
        .map_err(map_join)?
        .map_err(map_sym)?;

        ok_json(&libs)
    }
}

#[tool_handler]
impl ServerHandler for Tokito {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tokito-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "KiCad symbol catalog for AI-assisted PCB design. \
                 Search by keywords (`search_symbols`), browse by library \
                 (`list_libraries`), and fetch full symbol bodies including pin \
                 names + electrical types (`get_symbol`).",
            )
    }
}

// Length caps for user-supplied tool args. Mirrored on the REST face. The
// numbers are deliberately tight: KiCad library names top out at ~30 chars,
// symbol names at ~40, and a meaningful FTS5 query rarely needs more than
// a few dozen characters. Anything larger is almost certainly an attempt to
// exercise the FTS5 backend's worst-case parse time.
const MAX_QUERY_LEN: usize = 256;
const MAX_LIB_NAME_LEN: usize = 64;
const MAX_SYMBOL_NAME_LEN: usize = 128;
const MAX_FP_PATTERN_LEN: usize = 64;
const MAX_MANUFACTURER_LEN: usize = 128;
const MAX_MPN_LEN: usize = 96;
const MAX_PACKAGE_LEN: usize = 64;
const MAX_REVISION_ID_LEN: usize = 256;

fn check_len(field: &str, value: &str, max: usize) -> Result<(), McpError> {
    if value.len() > max {
        return Err(McpError::new(
            ErrorCode::INVALID_PARAMS,
            format!("`{field}` exceeds {max} bytes"),
            None,
        ));
    }
    Ok(())
}

fn symbol_keys(
    symbol_id: Option<&str>,
    lib: Option<&str>,
    name: Option<&str>,
) -> Result<(String, String), McpError> {
    if let Some(symbol_id) = symbol_id {
        let Some((lib, name)) = part_offer_query::split_symbol_id(symbol_id) else {
            return Err(McpError::new(
                ErrorCode::INVALID_PARAMS,
                "symbol_id must be in `Library:Name` form".to_string(),
                None,
            ));
        };
        return Ok((lib.to_string(), name.to_string()));
    }
    let Some(lib) = lib.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(McpError::new(
            ErrorCode::INVALID_PARAMS,
            "lib is required when symbol_id is absent".to_string(),
            None,
        ));
    };
    let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(McpError::new(
            ErrorCode::INVALID_PARAMS,
            "name is required when symbol_id is absent".to_string(),
            None,
        ));
    };
    Ok((lib.to_string(), name.to_string()))
}

fn ok_json<T: Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    let s = serde_json::to_string(v).map_err(|e| internal("serialize response", &e))?;
    Ok(CallToolResult::success(vec![Content::text(s)]))
}

fn map_sym(e: tokito_symbols::Error) -> McpError {
    match &e {
        // Client-safe: tells them the symbol they asked for doesn't exist.
        tokito_symbols::Error::SymbolNotFound { .. } => {
            McpError::new(ErrorCode::INVALID_PARAMS, e.to_string(), None)
        }
        // Client-safe: the caller's `query` isn't valid FTS5 syntax — a
        // client mistake, not a server fault (TokitoAI/tokito-mcp#106).
        tokito_symbols::Error::InvalidQuery(_) => {
            McpError::new(ErrorCode::INVALID_PARAMS, e.to_string(), None)
        }
        // Everything else can carry raw rusqlite/postcard detail — don't leak it.
        _ => internal("symbol lookup", &e),
    }
}

fn map_join(e: tokio::task::JoinError) -> McpError {
    internal("task join", &e)
}

/// Internal MCP error: log the detail server-side, return a generic message so
/// raw rusqlite/postcard/io strings never reach the client (mirrors the REST
/// face's 5xx handling).
fn internal(context: &str, detail: &dyn std::fmt::Display) -> McpError {
    tracing::error!(%context, %detail, "mcp internal error");
    McpError::new(
        ErrorCode::INTERNAL_ERROR,
        "internal server error".to_string(),
        None,
    )
}
