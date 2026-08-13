//! tokito-mcp-server CLI bootstrap. Library lives in `lib.rs`.

use std::path::PathBuf;

use clap::Parser;
use tokito_mcp_server::{build_app_with_config, state::AppState, ServerConfig};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "tokito-mcp-server", version)]
struct Args {
    /// Path to the symbols.sqlite artifact.
    #[arg(long, env = "TOKITO_MCP_DB")]
    db: PathBuf,

    /// Optional live generated-symbol database, opened read-only. Ingestion
    /// owns writes; setting this avoids baking new revisions into every MCP
    /// image release.
    #[arg(long, env = "TOKITO_MCP_GENERATED_DB")]
    generated_db: Option<PathBuf>,

    /// Bind address.
    #[arg(long, default_value = "127.0.0.1:8090", env = "TOKITO_MCP_ADDR")]
    addr: String,

    /// Resolver cache capacity (resolved symbols cached per process).
    #[arg(long, default_value_t = 2048, env = "TOKITO_MCP_CACHE")]
    cache: u64,

    /// Comma-separated `Host` authorities allowed on the MCP face (DNS-rebinding
    /// guard). Empty keeps the safe default (loopback only). Public deployments
    /// set their real host(s), e.g. "mcp.tokito.dev,mcp.tokito.dev:9443".
    #[arg(long, value_delimiter = ',', env = "TOKITO_MCP_ALLOWED_HOSTS")]
    allowed_hosts: Vec<String>,

    /// Comma-separated browser origins allowed for REST CORS and MCP `Origin`
    /// validation. Empty disables both. e.g. "https://app.tokito.dev".
    #[arg(long, value_delimiter = ',', env = "TOKITO_MCP_ALLOWED_ORIGINS")]
    allowed_origins: Vec<String>,

    /// Maximum concurrent MCP sessions. `initialize` past this is rejected, so a
    /// scripted session loop can't grow the session map / task count unbounded.
    #[arg(long, default_value_t = tokito_mcp_server::DEFAULT_MAX_SESSIONS, env = "TOKITO_MCP_MAX_SESSIONS")]
    max_sessions: usize,
}

fn normalize_allowlists(args: &mut Args) {
    args.allowed_hosts.retain(|value| !value.trim().is_empty());
    args.allowed_origins
        .retain(|value| !value.trim().is_empty());
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("tokito_mcp_server=info,tower_http=info")),
        )
        .init();

    let mut args = Args::parse();
    normalize_allowlists(&mut args);
    tracing::info!(?args.db, ?args.generated_db, addr = %args.addr, cache = args.cache, "starting tokito-mcp-server");

    let state = AppState::open_with_generated(&args.db, args.generated_db.as_deref(), args.cache)?;
    tracing::info!(
        commit = %state.manifest.source_commit,
        symbols = state.manifest.symbol_count,
        libs = state.manifest.lib_count,
        "artifact loaded"
    );

    let cfg = ServerConfig {
        allowed_hosts: (!args.allowed_hosts.is_empty()).then_some(args.allowed_hosts),
        allowed_origins: args.allowed_origins,
        max_sessions: args.max_sessions,
    };
    tracing::info!(
        allowed_hosts = ?cfg.allowed_hosts,
        allowed_origins = ?cfg.allowed_origins,
        max_sessions = cfg.max_sessions,
        "exposure config (allowed_hosts None = loopback-only default)"
    );

    let app = build_app_with_config(state, cfg).layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    tracing::info!(addr = %args.addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_environment_values_stay_disabled() {
        let mut args = Args::try_parse_from([
            "tokito-mcp-server",
            "--db",
            "symbols.sqlite",
            "--allowed-hosts",
            "",
            "--allowed-origins",
            "",
        ])
        .expect("parse empty compose values");
        normalize_allowlists(&mut args);
        assert!(args.allowed_hosts.is_empty());
        assert!(args.allowed_origins.is_empty());
    }

    #[test]
    fn normalization_preserves_nonempty_allowlist_entries() {
        let mut args = Args::try_parse_from([
            "tokito-mcp-server",
            "--db",
            "symbols.sqlite",
            "--allowed-hosts",
            "mcp.tokito.dev,,api.tokito.dev",
            "--allowed-origins",
            ",https://app.tokito.dev",
        ])
        .expect("parse allowlists");
        normalize_allowlists(&mut args);
        assert_eq!(args.allowed_hosts, ["mcp.tokito.dev", "api.tokito.dev"]);
        assert_eq!(args.allowed_origins, ["https://app.tokito.dev"]);
    }
}
