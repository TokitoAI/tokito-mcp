//! MCP face — exposes the same data the REST face does, but over the
//! Model Context Protocol (JSON-RPC 2.0 over Streamable HTTP).

mod capped;
mod server;

pub use server::build_mcp_service;
