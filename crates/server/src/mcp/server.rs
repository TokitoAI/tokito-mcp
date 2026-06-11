//! MCP server implementation. Three tools wrap the same code REST uses:
//!
//!   - `search_symbols(query, limit?, lib?)`
//!   - `get_symbol(lib, name)`
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
use tokito_symbols::search;

use crate::state::AppState;

pub type McpService = rmcp::transport::streamable_http_server::StreamableHttpService<
    Tokito,
    rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
>;

/// Re-exported via mod.rs so main.rs doesn't have to know rmcp internals.
pub fn build_mcp_service(state: AppState) -> McpService {
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };
    StreamableHttpService::new(
        move || Ok(Tokito::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
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
        let conn = self.state.conn.clone();
        let query = args.query.clone();
        let items = tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|p| p.into_inner());
            search::search(
                &c,
                search::SearchOpts {
                    query: &args.query,
                    limit,
                    lib_filter: args.lib.as_deref(),
                },
            )
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
        let conn = self.state.conn.clone();
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

fn ok_json<T: Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    let s = serde_json::to_string(v).map_err(|e| {
        McpError::new(
            ErrorCode::INTERNAL_ERROR,
            format!("serialize response: {e}"),
            None,
        )
    })?;
    Ok(CallToolResult::success(vec![Content::text(s)]))
}

fn map_sym(e: tokito_symbols::Error) -> McpError {
    let code = match &e {
        tokito_symbols::Error::SymbolNotFound { .. } => ErrorCode::INVALID_PARAMS,
        _ => ErrorCode::INTERNAL_ERROR,
    };
    McpError::new(code, e.to_string(), None)
}

fn map_join(e: tokio::task::JoinError) -> McpError {
    McpError::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None)
}
