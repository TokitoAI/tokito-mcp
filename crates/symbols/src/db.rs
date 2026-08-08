//! Database open / migration / version-check.

use rusqlite::{Connection, OpenFlags};
use std::path::Path;

use crate::{Error, Result, CURRENT_SCHEMA_VERSION, MIN_COMPATIBLE_VERSION, SCHEMA_SQL};

/// Open the artifact read-only with mmap enabled. Used by the server at boot
/// and by the desktop's catalog loader (`tokito-catalog`).
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "mmap_size", 268_435_456_i64)?; // 256 MB
    conn.pragma_update(None, "query_only", true)?;
    check_schema_version(&conn)?;
    Ok(conn)
}

/// Open (or create) a database for writing — used by the builder. Applies the
/// canonical schema if the tables don't exist.
pub fn open_for_build(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', ?1)",
        rusqlite::params![CURRENT_SCHEMA_VERSION.to_string()],
    )?;
    Ok(conn)
}

fn check_schema_version(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )?
        .parse()
        .unwrap_or(0);
    if !(MIN_COMPATIBLE_VERSION..=CURRENT_SCHEMA_VERSION).contains(&v) {
        return Err(Error::SchemaVersionMismatch {
            artifact: v,
            min: MIN_COMPATIBLE_VERSION,
            current: CURRENT_SCHEMA_VERSION,
        });
    }
    Ok(())
}
