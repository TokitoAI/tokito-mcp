//! App state shared across handlers.
//!
//! The Connection lives behind a `std::sync::Mutex` because rusqlite's
//! handle is `Send` (with bundled SQLite in serialized mode) but not `Sync`.
//! Handlers move the conn into `spawn_blocking` so the tokio runtime
//! isn't blocked on SQL.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::Serialize;

use crate::error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub resolver: tokito_symbols::resolver::Resolver,
    pub manifest: Arc<Manifest>,
}

/// What `/v1/manifest` returns. Loaded from the `meta` table at boot.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Manifest {
    pub source_commit: String,
    pub generator_version: String,
    pub schema_version: u32,
    pub symbol_count: u64,
    pub lib_count: u64,
    pub generated_at: Option<String>,
}

impl AppState {
    pub fn open(db_path: &Path, cache_capacity: u64) -> Result<Self, AppError> {
        let conn = tokito_symbols::db::open_read_only(db_path)?;
        let manifest = load_manifest(&conn);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            resolver: tokito_symbols::resolver::Resolver::new(cache_capacity),
            manifest: Arc::new(manifest),
        })
    }
}

fn load_manifest(conn: &Connection) -> Manifest {
    let mut m = Manifest::default();
    let mut stmt = match conn.prepare("SELECT key, value FROM meta") {
        Ok(s) => s,
        Err(_) => return m,
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    if let Ok(rows) = rows {
        for r in rows.flatten() {
            match r.0.as_str() {
                "source_commit" => m.source_commit = r.1,
                "generator_version" => m.generator_version = r.1,
                "schema_version" => m.schema_version = r.1.parse().unwrap_or(0),
                "symbol_count" => m.symbol_count = r.1.parse().unwrap_or(0),
                "lib_count" => m.lib_count = r.1.parse().unwrap_or(0),
                "generated_at" => m.generated_at = Some(r.1),
                _ => {}
            }
        }
    }
    m
}
