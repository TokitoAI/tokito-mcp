//! tokito-mcp-server CLI bootstrap. Library lives in `lib.rs`.

use std::path::PathBuf;

use clap::Parser;
use tokito_mcp_server::{build_app, state::AppState};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "tokito-mcp-server", version)]
struct Args {
    /// Path to the symbols.sqlite artifact.
    #[arg(long, env = "TOKITO_MCP_DB")]
    db: PathBuf,

    /// Bind address.
    #[arg(long, default_value = "127.0.0.1:8090", env = "TOKITO_MCP_ADDR")]
    addr: String,

    /// Resolver LRU capacity (resolved symbols cached per process).
    #[arg(long, default_value_t = 2048, env = "TOKITO_MCP_CACHE")]
    cache: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("tokito_mcp_server=info,tower_http=info")),
        )
        .init();

    let args = Args::parse();
    tracing::info!(?args.db, addr = %args.addr, cache = args.cache, "starting tokito-mcp-server");

    let state = AppState::open(&args.db, args.cache)?;
    tracing::info!(
        commit = %state.manifest.source_commit,
        symbols = state.manifest.symbol_count,
        libs = state.manifest.lib_count,
        "artifact loaded"
    );

    let app = build_app(state).layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    tracing::info!(addr = %args.addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}
