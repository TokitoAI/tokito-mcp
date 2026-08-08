//! FTS5-backed keyword search and structured capability search over the catalog.
//!
//! Search results union the two catalog tables — the CERN-derived `symbol`
//! table (source = `official`) and the `generated_symbol` table (source =
//! `generated`, restricted to `status = 'published'`). BM25 scores are
//! comparable across the two mirrors because both FTS5 indexes use the same
//! tokenizer and column layout.

use rusqlite::Connection;

use crate::{
    model::{Source, SymbolRef},
    Result,
};

pub struct SearchOpts<'a> {
    pub query: &'a str,
    pub limit: u32,
    pub lib_filter: Option<&'a str>,
}

/// Capability search — combines structured filters (pin count, footprint
/// pattern) with optional FTS5 keyword ranking.
///
/// When `query` is `None`, results are ordered by `(pin_count, lib, name)`
/// for determinism. When present, results are ranked by FTS5 BM25.
pub struct CompatibleOpts<'a> {
    pub pins: Option<u32>,
    /// Matched as `%pattern%` against both `fp_filters` and `footprint`.
    pub fp_pattern: Option<&'a str>,
    pub query: Option<&'a str>,
    pub limit: u32,
    pub lib_filter: Option<&'a str>,
}

pub fn search(conn: &Connection, opts: SearchOpts<'_>) -> Result<Vec<SymbolRef>> {
    // FTS5 expects the query already shaped — we pass it through to allow
    // boolean operators and column-scoped queries. Caller-level sanitisation
    // is a future concern (rate-limited public endpoint).
    let generated = crate::generated::search_available(conn)?;
    let sql = match (generated, opts.lib_filter) {
        (true, Some(_)) => SQL_WITH_LIB,
        (true, None) => SQL_ANY_LIB,
        (false, Some(_)) => SQL_OFFICIAL_WITH_LIB,
        (false, None) => SQL_OFFICIAL_ANY_LIB,
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows: Vec<SymbolRef> = if let Some(lib) = opts.lib_filter {
        stmt.query_map(rusqlite::params![opts.query, lib, opts.limit], row_to_ref)?
            .collect::<std::result::Result<_, _>>()?
    } else {
        stmt.query_map(rusqlite::params![opts.query, opts.limit], row_to_ref)?
            .collect::<std::result::Result<_, _>>()?
    };
    Ok(rows)
}

fn row_to_ref(r: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolRef> {
    let source_raw: String = r.get(7)?;
    let source = match source_raw.as_str() {
        "official" => Source::Official,
        "generated" => Source::Generated,
        // Column is emitted as a hard-coded literal in every SELECT branch, so
        // an unknown value would be a schema regression, not user input.
        // Surface as a SQL type error rather than silently defaulting.
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                format!("unknown source marker {other:?}").into(),
            ));
        }
    };
    Ok(SymbolRef {
        lib: r.get(0)?,
        name: r.get(1)?,
        ref_des: r.get(2)?,
        description: r.get(3)?,
        keywords: r.get(4)?,
        pin_count: r.get::<_, i64>(5)? as u16,
        score: r.get::<_, f64>(6)? as f32,
        source,
    })
}

// The two branches are unioned then re-ordered/limited outside so BM25 scores
// stay comparable and we don't emit `LIMIT` twice with different arg indices.
const SQL_ANY_LIB: &str = r#"
SELECT lib, name, ref_des, description, keywords, pin_count, score, source FROM (
    SELECT l.name AS lib, s.name AS name, s.ref_des, s.description, s.keywords,
           s.pin_count, bm25(symbol_fts) AS score, 'official' AS source
      FROM symbol_fts
      JOIN symbol s ON s.id = symbol_fts.rowid
      JOIN lib    l ON l.id = s.lib_id
     WHERE symbol_fts MATCH ?1
    UNION ALL
    SELECT l.name AS lib, g.name AS name, g.ref_des, g.description, g.keywords,
           g.pin_count, bm25(generated_symbol_fts) AS score, 'generated' AS source
      FROM generated_symbol_fts
      JOIN generated_symbol g ON g.id = generated_symbol_fts.rowid
      JOIN lib             l ON l.id = g.lib_id
     WHERE generated_symbol_fts MATCH ?1 AND g.status = 'published'
) ORDER BY score LIMIT ?2
"#;

const SQL_WITH_LIB: &str = r#"
SELECT lib, name, ref_des, description, keywords, pin_count, score, source FROM (
    SELECT l.name AS lib, s.name AS name, s.ref_des, s.description, s.keywords,
           s.pin_count, bm25(symbol_fts) AS score, 'official' AS source
      FROM symbol_fts
      JOIN symbol s ON s.id = symbol_fts.rowid
      JOIN lib    l ON l.id = s.lib_id
     WHERE symbol_fts MATCH ?1 AND l.name = ?2
    UNION ALL
    SELECT l.name AS lib, g.name AS name, g.ref_des, g.description, g.keywords,
           g.pin_count, bm25(generated_symbol_fts) AS score, 'generated' AS source
      FROM generated_symbol_fts
      JOIN generated_symbol g ON g.id = generated_symbol_fts.rowid
      JOIN lib             l ON l.id = g.lib_id
     WHERE generated_symbol_fts MATCH ?1 AND l.name = ?2 AND g.status = 'published'
) ORDER BY score LIMIT ?3
"#;

const SQL_OFFICIAL_ANY_LIB: &str = r#"
SELECT l.name AS lib, s.name, s.ref_des, s.description, s.keywords,
       s.pin_count, bm25(symbol_fts) AS score, 'official' AS source
  FROM symbol_fts
  JOIN symbol s ON s.id = symbol_fts.rowid
  JOIN lib    l ON l.id = s.lib_id
 WHERE symbol_fts MATCH ?1
 ORDER BY score LIMIT ?2
"#;

const SQL_OFFICIAL_WITH_LIB: &str = r#"
SELECT l.name AS lib, s.name, s.ref_des, s.description, s.keywords,
       s.pin_count, bm25(symbol_fts) AS score, 'official' AS source
  FROM symbol_fts
  JOIN symbol s ON s.id = symbol_fts.rowid
  JOIN lib    l ON l.id = s.lib_id
 WHERE symbol_fts MATCH ?1 AND l.name = ?2
 ORDER BY score LIMIT ?3
"#;

pub fn find_compatible(conn: &Connection, opts: CompatibleOpts<'_>) -> Result<Vec<SymbolRef>> {
    // Bind parameters in a stable order regardless of which filters are set.
    //
    // find_compatible today searches only the CERN-derived `symbol` catalog;
    // widening to include `generated_symbol` is tracked separately (pin_count
    // + fp_pattern filters need to apply symmetrically to both, and the
    // dynamic-SQL builder is easier to grow after Wave B.2 stabilizes).
    // The `'official'` literal keeps `row_to_ref`'s source column contract.
    let mut sql =
        String::from("SELECT l.name, s.name, s.ref_des, s.description, s.keywords, s.pin_count, ");
    if opts.query.is_some() {
        sql.push_str("bm25(symbol_fts) AS score, 'official' AS source FROM symbol_fts ");
        sql.push_str("JOIN symbol s ON s.id = symbol_fts.rowid ");
        sql.push_str("JOIN lib l ON l.id = s.lib_id WHERE symbol_fts MATCH ?1 ");
    } else {
        sql.push_str("0.0 AS score, 'official' AS source FROM symbol s ");
        sql.push_str("JOIN lib l ON l.id = s.lib_id WHERE 1=1 ");
    }

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(q) = opts.query {
        params.push(Box::new(q.to_string()));
    }
    if let Some(p) = opts.pins {
        sql.push_str(&format!("AND s.pin_count = ?{} ", params.len() + 1));
        params.push(Box::new(p as i64));
    }
    if let Some(pat) = opts.fp_pattern {
        // `pat` is a literal substring, not a LIKE pattern: escape `%`, `_` (and
        // the escape char itself) so they match literally rather than as
        // wildcards, and declare the escape char with `ESCAPE '\'`.
        sql.push_str(&format!(
            "AND (s.fp_filters LIKE ?{n} ESCAPE '\\' OR s.footprint LIKE ?{n} ESCAPE '\\') ",
            n = params.len() + 1
        ));
        params.push(Box::new(format!("%{}%", escape_like(pat))));
    }
    if let Some(lib) = opts.lib_filter {
        sql.push_str(&format!("AND l.name = ?{} ", params.len() + 1));
        params.push(Box::new(lib.to_string()));
    }

    sql.push_str(if opts.query.is_some() {
        "ORDER BY score "
    } else {
        "ORDER BY s.pin_count, l.name, s.name "
    });
    sql.push_str(&format!("LIMIT ?{}", params.len() + 1));
    params.push(Box::new(opts.limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(param_refs), row_to_ref)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Escape SQL `LIKE` metacharacters so a user-supplied substring matches
/// literally. Pairs with `ESCAPE '\'` in the query. The backslash must be
/// escaped first so we don't double-escape the escapes we add.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Cheap listing of library names + their symbol counts.
pub fn list_libraries(conn: &Connection) -> Result<Vec<LibInfo>> {
    let mut stmt = conn.prepare_cached(
        "SELECT l.name, COUNT(s.id) AS n FROM lib l \
         LEFT JOIN symbol s ON s.lib_id = l.id \
         GROUP BY l.id ORDER BY l.name",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LibInfo {
                name: r.get(0)?,
                symbol_count: r.get::<_, i64>(1)? as u32,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibInfo {
    pub name: String,
    pub symbol_count: u32,
}
