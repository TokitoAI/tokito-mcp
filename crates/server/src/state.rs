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
use std::collections::HashSet;
use tokito_symbols::model::{Source, SymbolRef};

use crate::error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    /// Optional live generated-symbol catalog. In production the ingestion
    /// service owns this file and MCP opens it read-only; ordinary catalog
    /// queries continue to use the immutable release artifact in `conn`.
    pub generated_conn: Option<Arc<Mutex<Connection>>>,
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
        Self::open_with_generated(db_path, None, cache_capacity)
    }

    pub fn open_with_generated(
        db_path: &Path,
        generated_db_path: Option<&Path>,
        cache_capacity: u64,
    ) -> Result<Self, AppError> {
        let conn = tokito_symbols::db::open_read_only(db_path)?;
        let generated_conn = generated_db_path.and_then(open_optional_generated_catalog);
        let manifest = load_manifest(&conn);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            generated_conn,
            resolver: tokito_symbols::resolver::Resolver::new(cache_capacity),
            manifest: Arc::new(manifest),
        })
    }

    /// Select the live generated catalog when configured. This keeps the
    /// public server read-only while allowing committed ingestion revisions to
    /// become visible without rebuilding or restarting the MCP image.
    pub fn generated_connection(&self) -> Arc<Mutex<Connection>> {
        self.generated_conn
            .clone()
            .unwrap_or_else(|| self.conn.clone())
    }

    pub fn connection_for_lib(&self, lib: &str) -> Arc<Mutex<Connection>> {
        if lib.starts_with(tokito_symbols::resolver::GENERATED_LIB_PREFIX) {
            self.generated_connection()
        } else {
            self.conn.clone()
        }
    }

    /// Search the immutable catalog plus the live generated catalog, when one
    /// is configured. Generated rows in the baked catalog are ignored in that
    /// mode so superseded runtime data cannot leak through stale image state.
    pub fn search_catalogs(
        &self,
        query: &str,
        limit: u32,
        lib_filter: Option<&str>,
    ) -> tokito_symbols::Result<Vec<SymbolRef>> {
        let mut items = {
            let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
            tokito_symbols::search::search(
                &conn,
                tokito_symbols::search::SearchOpts {
                    query,
                    limit,
                    lib_filter,
                },
            )?
        };

        if let Some(generated_conn) = &self.generated_conn {
            items.retain(|item| item.source == Source::Official);
            let conn = generated_conn.lock().unwrap_or_else(|p| p.into_inner());
            let mut generated = tokito_symbols::search::search(
                &conn,
                tokito_symbols::search::SearchOpts {
                    query,
                    limit,
                    lib_filter,
                },
            )?;
            generated.retain(|item| item.source == Source::Generated);
            items.extend(generated);
        }

        items.sort_by(|a, b| {
            a.score
                .total_cmp(&b.score)
                .then_with(|| a.lib.cmp(&b.lib))
                .then_with(|| a.name.cmp(&b.name))
        });
        let mut seen = HashSet::new();
        items.retain(|item| seen.insert((item.lib.clone(), item.name.clone())));
        items.truncate(limit as usize);
        Ok(items)
    }
}

/// The live generated catalog is an optional production overlay. An ingestion
/// rollout may create its SQLite file before publishing its schema, so a bad
/// optional overlay must not take down the immutable catalog service.
fn open_optional_generated_catalog(path: &Path) -> Option<Arc<Mutex<Connection>>> {
    match tokito_symbols::db::open_read_only(path) {
        Ok(connection) => Some(Arc::new(Mutex::new(connection))),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "ignoring unavailable generated catalog");
            None
        }
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

#[cfg(test)]
mod tests {
    use super::open_optional_generated_catalog;

    #[test]
    fn uninitialized_optional_generated_catalog_is_ignored() {
        let file = tempfile::NamedTempFile::new().unwrap();
        rusqlite::Connection::open(file.path()).unwrap();

        assert!(open_optional_generated_catalog(file.path()).is_none());
    }
}
