//! Generated-symbol store: resolve by MPN, read provenance, and offline
//! insert (used by `tokito-mcp-pack --generated`).
//!
//! Storage lives in the `part_registry` and `generated_symbol` tables. The
//! server holds the DB read-only at runtime; every mutation in this module is
//! either invoked by the packer or by an in-memory test fixture. See
//! docs/HACKATHON_SLICE.md §3.3 and docs/CONTRACTS.md §5 for the contract.

use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

use crate::{
    model::{PublicationStatus, ResolvedSymbol, SymbolBody},
    part_id::PartId,
    Error, Result, BODY_FORMAT_POSTCARD_V1,
};

/// Resolve the currently-published generated symbol for a normalized part
/// identity. Returns `Ok(None)` when the part is unknown or has no published
/// revision (draft, quarantined, or superseded state).
pub fn resolve_by_mpn(conn: &Connection, part: &PartId) -> Result<Option<Arc<ResolvedSymbol>>> {
    let row = conn
        .prepare_cached(SQL_LATEST_PUBLISHED_BY_PART)?
        .query_row(
            params![part.manufacturer_norm, part.mpn, part.package],
            row_to_generated,
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some(g) => Ok(Some(Arc::new(g.into_resolved()?))),
    }
}

/// Resolve the currently-published generated symbol by its catalog identity
/// (`generated:<lib>` + `name`). Powers the dispatch path in `Resolver::resolve`
/// for the `generated:*` library namespace.
pub fn resolve_current_by_lib_name(
    conn: &Connection,
    lib: &str,
    name: &str,
) -> Result<Option<Arc<ResolvedSymbol>>> {
    let row = conn
        .prepare_cached(SQL_LATEST_PUBLISHED_BY_LIB_NAME)?
        .query_row(params![lib, name], row_to_generated)
        .optional()?;
    match row {
        None => Ok(None),
        Some(g) => Ok(Some(Arc::new(g.into_resolved()?))),
    }
}

/// Fetch the provenance JSON for the currently-published revision of a
/// generated symbol identified by `(lib, name)`. Returns `Ok(None)` if the
/// symbol is not in the generated store or has no published revision.
pub fn provenance_for_symbol(
    conn: &Connection,
    lib: &str,
    name: &str,
) -> Result<Option<serde_json::Value>> {
    let raw: Option<String> = conn
        .prepare_cached(SQL_PROVENANCE_BY_LIB_NAME)?
        .query_row(params![lib, name], |r| r.get::<_, String>(0))
        .optional()?;
    match raw {
        None => Ok(None),
        Some(s) => Ok(Some(
            serde_json::from_str::<serde_json::Value>(&s).map_err(Error::from_json)?,
        )),
    }
}

/// Fetch the provenance JSON for an exact revision id.
pub fn provenance_for_revision(
    conn: &Connection,
    revision_id: &str,
) -> Result<Option<serde_json::Value>> {
    let raw: Option<String> = conn
        .prepare_cached(SQL_PROVENANCE_BY_REVISION)?
        .query_row(params![revision_id], |r| r.get::<_, String>(0))
        .optional()?;
    match raw {
        None => Ok(None),
        Some(s) => Ok(Some(
            serde_json::from_str::<serde_json::Value>(&s).map_err(Error::from_json)?,
        )),
    }
}

/// Insert a generated-symbol revision. Idempotent on `revision_id`: repeated
/// calls with the same id and identical body are a no-op; different bodies
/// under the same id are a hard error (revision ids are content-hash-derived
/// and must be immutable per docs/CONTRACTS.md §4).
///
/// The transaction inserts (or reuses) the `part_registry` row, resolves the
/// `lib_id`, and writes the `generated_symbol` row. Callers are responsible
/// for wrapping this in a broader transaction if they need atomicity across
/// multiple revisions.
pub fn insert_revision<'a>(conn: &Connection, r: NewRevision<'a>) -> Result<i64> {
    // Existing revision with the same id must have byte-identical body.
    if let Some(existing_body) = conn
        .prepare_cached("SELECT body FROM generated_symbol WHERE revision_id = ?1")?
        .query_row(params![r.revision_id], |row| row.get::<_, Vec<u8>>(0))
        .optional()?
    {
        if existing_body == r.body {
            return conn
                .prepare_cached("SELECT id FROM generated_symbol WHERE revision_id = ?1")?
                .query_row(params![r.revision_id], |row| row.get::<_, i64>(0))
                .map_err(Error::from);
        }
        return Err(Error::RevisionBodyMismatch {
            revision_id: r.revision_id.to_string(),
        });
    }

    conn.execute(
        "INSERT OR IGNORE INTO part_registry(part_id, manufacturer_norm, mpn, package) \
         VALUES(?1, ?2, ?3, ?4)",
        params![
            r.part.key(),
            r.part.manufacturer_norm,
            r.part.mpn,
            r.part.package,
        ],
    )?;

    let lib_id: i64 = conn
        .prepare_cached("SELECT id FROM lib WHERE name = ?1")?
        .query_row(params![r.lib], |row| row.get(0))
        .optional()?
        .map_or_else(
            || {
                conn.execute("INSERT INTO lib(name) VALUES(?1)", params![r.lib])?;
                Ok::<i64, rusqlite::Error>(conn.last_insert_rowid())
            },
            Ok,
        )?;

    conn.execute(
        "INSERT INTO generated_symbol( \
             revision_id, part_id, lib_id, name, ref_des, description, keywords, \
             fp_filters, datasheet, footprint, pin_count, flags, body, body_format, \
             provenance_json, status, content_hash, published_at \
         ) VALUES( \
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18 \
         )",
        params![
            r.revision_id,
            r.part.key(),
            lib_id,
            r.name,
            r.ref_des,
            r.description,
            r.keywords,
            r.fp_filters,
            r.datasheet,
            r.footprint,
            r.pin_count as i64,
            r.flags as i64,
            r.body,
            r.body_format,
            r.provenance_json,
            r.status.as_str(),
            r.content_hash,
            r.published_at,
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Insert-payload for a single generated-symbol revision.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct NewRevision<'a> {
    pub revision_id: &'a str,
    pub part: &'a PartId,
    pub lib: &'a str,
    pub name: &'a str,
    pub ref_des: &'a str,
    pub description: &'a str,
    pub keywords: &'a str,
    pub fp_filters: &'a str,
    pub datasheet: &'a str,
    pub footprint: &'a str,
    pub pin_count: u16,
    pub flags: u32,
    /// postcard-encoded `SymbolBody` bytes. Length is not implicitly bounded
    /// here — the packer enforces size caps upstream.
    pub body: &'a [u8],
    pub body_format: &'a str,
    /// Serialized provenance JSON (see docs/CONTRACTS.md §5).
    pub provenance_json: &'a str,
    pub status: PublicationStatus,
    pub content_hash: &'a str,
    pub published_at: &'a str,
}

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

const SQL_LATEST_PUBLISHED_BY_PART: &str = r#"
SELECT g.id, g.revision_id, g.part_id, l.name AS lib, g.name,
       g.ref_des, g.description, g.keywords, g.fp_filters,
       g.datasheet, g.footprint, g.pin_count, g.body, g.body_format,
       g.provenance_json, g.status, g.content_hash, g.published_at
  FROM generated_symbol g
  JOIN part_registry p ON p.part_id = g.part_id
  JOIN lib           l ON l.id       = g.lib_id
 WHERE p.manufacturer_norm = ?1
   AND p.mpn               = ?2
   AND p.package           = ?3
   AND g.status            = 'published'
 ORDER BY g.published_at DESC
 LIMIT 1
"#;

const SQL_LATEST_PUBLISHED_BY_LIB_NAME: &str = r#"
SELECT g.id, g.revision_id, g.part_id, l.name AS lib, g.name,
       g.ref_des, g.description, g.keywords, g.fp_filters,
       g.datasheet, g.footprint, g.pin_count, g.body, g.body_format,
       g.provenance_json, g.status, g.content_hash, g.published_at
  FROM generated_symbol g
  JOIN lib           l ON l.id = g.lib_id
 WHERE l.name  = ?1
   AND g.name  = ?2
   AND g.status = 'published'
 ORDER BY g.published_at DESC
 LIMIT 1
"#;

const SQL_PROVENANCE_BY_LIB_NAME: &str = r#"
SELECT g.provenance_json
  FROM generated_symbol g
  JOIN lib l ON l.id = g.lib_id
 WHERE l.name = ?1
   AND g.name = ?2
   AND g.status = 'published'
 ORDER BY g.published_at DESC
 LIMIT 1
"#;

const SQL_PROVENANCE_BY_REVISION: &str = r#"
SELECT provenance_json
  FROM generated_symbol
 WHERE revision_id = ?1
"#;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

struct GeneratedRow {
    lib: String,
    name: String,
    ref_des: String,
    description: String,
    keywords: String,
    fp_filters: String,
    datasheet: String,
    footprint: String,
    body: Vec<u8>,
    body_format: String,
}

impl GeneratedRow {
    fn into_resolved(self) -> Result<ResolvedSymbol> {
        if self.body_format != BODY_FORMAT_POSTCARD_V1 {
            return Err(Error::UnknownBodyFormat(self.body_format));
        }
        let body: SymbolBody = postcard::from_bytes(&self.body)?;
        Ok(ResolvedSymbol {
            lib: self.lib,
            name: self.name,
            ref_des: self.ref_des,
            description: self.description,
            keywords: self.keywords,
            fp_filters: self.fp_filters,
            datasheet: self.datasheet,
            footprint: self.footprint,
            parent: None,
            body,
        })
    }
}

fn row_to_generated(r: &rusqlite::Row<'_>) -> rusqlite::Result<GeneratedRow> {
    // Columns from SQL_LATEST_PUBLISHED_BY_PART, indexed in select order.
    Ok(GeneratedRow {
        lib: r.get::<_, String>(3)?,
        name: r.get::<_, String>(4)?,
        ref_des: r.get::<_, String>(5)?,
        description: r.get::<_, String>(6)?,
        keywords: r.get::<_, String>(7)?,
        fp_filters: r.get::<_, String>(8)?,
        datasheet: r.get::<_, String>(9)?,
        footprint: r.get::<_, String>(10)?,
        body: r.get::<_, Vec<u8>>(12)?,
        body_format: r.get::<_, String>(13)?,
    })
}
