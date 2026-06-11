//! Extends resolver — walks `parent_id` until it finds a non-NULL body, then
//! materialises the catalog's property columns on top.
//!
//! Audit findings against the CERN library (2026-06-10, 22,756 symbols):
//!   - 12,222 (53.7%) use `(extends ...)`
//!   - Zero extending children carry their own pins, graphics, or sub-symbols
//!   - Max chain depth = 4; no cycles
//!
//! So the model is: child contributes only the catalog's property columns
//! (Reference, Value, Description, …); body is 100% the parent's.

use moka::sync::Cache;
use rusqlite::Connection;
use std::sync::Arc;

use crate::{
    model::{ResolvedSymbol, SymbolBody},
    Error, Result, BODY_FORMAT_POSTCARD_V1, MAX_EXTENDS_DEPTH,
};

/// Caches resolved symbols by `symbol.id`. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct Resolver {
    cache: Cache<i64, Arc<ResolvedSymbol>>,
}

impl Resolver {
    pub fn new(capacity: u64) -> Self {
        Self {
            cache: Cache::new(capacity),
        }
    }

    /// Materialise a symbol by `(lib, name)`. Borrows `&Connection` short.
    pub fn resolve(
        &self,
        conn: &Connection,
        lib: &str,
        name: &str,
    ) -> Result<Arc<ResolvedSymbol>> {
        let id = lookup_id(conn, lib, name)?;
        self.resolve_by_id(conn, id)
    }

    pub fn resolve_by_id(&self, conn: &Connection, id: i64) -> Result<Arc<ResolvedSymbol>> {
        if let Some(hit) = self.cache.get(&id) {
            return Ok(hit);
        }

        // Walk the chain via the recursive CTE — same shape as the docs in
        // the design synth. Returns rows in (child → parent → root) order; we
        // want the root's body and the leaf's metadata.
        let mut chain: Vec<ChainRow> = conn
            .prepare_cached(CHAIN_CTE)?
            .query_map([id], ChainRow::from_row)?
            .collect::<std::result::Result<_, _>>()?;

        if chain.is_empty() {
            return Err(Error::SymbolNotFound {
                lib: String::new(),
                name: String::new(),
            });
        }
        if chain.len() as u32 > MAX_EXTENDS_DEPTH {
            return Err(Error::ExtendsDepthExceeded(MAX_EXTENDS_DEPTH));
        }

        let leaf = chain.first().cloned().unwrap();
        let body_blob = chain
            .iter()
            .rev()
            .find_map(|row| row.body.as_ref().map(|b| (row.body_format.as_deref(), b)));

        let body = match body_blob {
            Some((Some(BODY_FORMAT_POSTCARD_V1), bytes)) => postcard::from_bytes::<SymbolBody>(bytes)?,
            Some((Some(other), _)) => return Err(Error::UnknownBodyFormat(other.to_string())),
            Some((None, _)) | None => SymbolBody {
                pins: vec![],
                graphics: vec![],
                units: vec![],
                props_layout: vec![],
                flags: Default::default(),
            },
        };

        let parent = if chain.len() > 1 {
            let p = chain.last().unwrap();
            Some((p.lib_name.clone(), p.name.clone()))
        } else {
            None
        };

        let resolved = ResolvedSymbol {
            lib: leaf.lib_name.clone(),
            name: leaf.name.clone(),
            ref_des: leaf.ref_des.clone(),
            description: leaf.description.clone(),
            keywords: leaf.keywords.clone(),
            fp_filters: leaf.fp_filters.clone(),
            datasheet: leaf.datasheet.clone(),
            footprint: leaf.footprint.clone(),
            parent,
            body,
        };

        let arc = Arc::new(resolved);
        self.cache.insert(id, arc.clone());
        // suppress unused warning on chain
        let _ = &mut chain;
        Ok(arc)
    }
}

fn lookup_id(conn: &Connection, lib: &str, name: &str) -> Result<i64> {
    let row = conn
        .prepare_cached(
            "SELECT s.id FROM symbol s JOIN lib l ON s.lib_id = l.id WHERE l.name = ?1 AND s.name = ?2",
        )?
        .query_row(rusqlite::params![lib, name], |r| r.get::<_, i64>(0))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Error::SymbolNotFound {
                lib: lib.into(),
                name: name.into(),
            },
            other => Error::Sql(other),
        })?;
    Ok(row)
}

#[derive(Clone)]
struct ChainRow {
    name: String,
    lib_name: String,
    ref_des: String,
    description: String,
    keywords: String,
    fp_filters: String,
    datasheet: String,
    footprint: String,
    body: Option<Vec<u8>>,
    body_format: Option<String>,
}

impl ChainRow {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            name: r.get(0)?,
            lib_name: r.get(1)?,
            ref_des: r.get(2)?,
            description: r.get(3)?,
            keywords: r.get(4)?,
            fp_filters: r.get(5)?,
            datasheet: r.get(6)?,
            footprint: r.get(7)?,
            body: r.get(8)?,
            body_format: r.get(9)?,
        })
    }
}

/// Recursive CTE: starting from `?1`, walk parent_id up to root. Returned in
/// child-first order; depth-capped to MAX_EXTENDS_DEPTH so a malformed pack
/// can't blow the stack.
const CHAIN_CTE: &str = r#"
WITH RECURSIVE chain(id, depth) AS (
    SELECT ?1, 0
    UNION ALL
    SELECT s.parent_id, c.depth + 1
      FROM symbol s
      JOIN chain c ON s.id = c.id
     WHERE s.parent_id IS NOT NULL
       AND c.depth < 8
)
SELECT s.name, l.name AS lib,
       s.ref_des, s.description, s.keywords, s.fp_filters,
       s.datasheet, s.footprint,
       s.body, s.body_format
  FROM chain c
  JOIN symbol s ON s.id = c.id
  JOIN lib    l ON l.id = s.lib_id
 ORDER BY c.depth ASC
"#;
