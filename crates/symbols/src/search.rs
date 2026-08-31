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
    let query = normalize_query(opts.query);
    let generated = crate::generated::search_available(conn)?;
    let sql = match (generated, opts.lib_filter) {
        (true, Some(_)) => SQL_WITH_LIB,
        (true, None) => SQL_ANY_LIB,
        (false, Some(_)) => SQL_OFFICIAL_WITH_LIB,
        (false, None) => SQL_OFFICIAL_ANY_LIB,
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows: Vec<SymbolRef> = if let Some(lib) = opts.lib_filter {
        stmt.query_map(rusqlite::params![query, lib, opts.limit], row_to_ref)?
            .collect::<std::result::Result<_, _>>()?
    } else {
        stmt.query_map(rusqlite::params![query, opts.limit], row_to_ref)?
            .collect::<std::result::Result<_, _>>()?
    };
    Ok(rows)
}

/// Reshapes a raw user query into one that means what the caller intended
/// once it reaches FTS5 `MATCH`. Two independent fixes, both from
/// TokitoAI/tokito-mcp#105:
///
/// 1. **Underscore desugaring.** FTS5's query grammar treats a
///    punctuation-free run of characters (a "bareword") as a single term —
///    but if *tokenizing* that bareword produces more than one token (which
///    underscores do, since `unicode61` treats `_` as a separator), FTS5
///    silently reinterprets it as an implicit **phrase**: the sub-tokens
///    must appear immediately adjacent, in that order, in a single column.
///    A copy-pasted symbol-style query like `Pin_Header_1x02` becomes the
///    phrase `pin` → `header` → `1x02`, which can never match a row whose
///    "pin header" synonym lives in `keywords` and whose `01x02` lives in
///    `name` — different columns can't be phrase-adjacent. Replacing `_`
///    with a space before the query reaches FTS5 turns that into three
///    independent barewords, which the grammar ANDs together instead —
///    exactly what a name-shaped query is supposed to mean.
/// 2. **Row/column zero-padding.** KiCad's generic connector families name
///    pin counts with zero-padded two-digit numbers joined by `x`
///    (`Conn_01x02`, `Screw_Terminal_02x05`), but people type the count
///    without the padding (`1x02`, `2x5`). FTS5 `MATCH` is exact token
///    equality, not prefix or fuzzy matching, so an unpadded query token
///    never lines up with the indexed `01x02` token. Scan for `<1-2
///    digits>x<1-2 digits>` runs bounded by non-alphanumeric characters (or
///    the string edges) and zero-pad each side to width 2, leaving the rest
///    of the query untouched.
///
/// Runs ahead of every FTS5 `MATCH`, so `search_symbols`, `find_compatible`,
/// and the REST `/v1/search` face all benefit uniformly.
pub(crate) fn normalize_query(query: &str) -> String {
    normalize_row_col_tokens(&query.replace('_', " "))
}

fn normalize_row_col_tokens(query: &str) -> String {
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::with_capacity(query.len());
    let mut i = 0;
    while i < chars.len() {
        let boundary_ok = i == 0 || !chars[i - 1].is_alphanumeric();
        if boundary_ok {
            if let Some((padded, consumed)) = match_row_col(&chars[i..]) {
                out.push_str(&padded);
                i += consumed;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Tries to parse a `<digits>x<digits>` run at the start of `chars`. Returns
/// the zero-padded replacement and how many source chars it consumed.
/// Digit runs are capped at 2 on each side — KiCad's row/column counts never
/// exceed two digits, so a longer run (or a trailing digit after the second
/// group) means this isn't the pattern we're after and we bail out.
fn match_row_col(chars: &[char]) -> Option<(String, usize)> {
    let mut idx = 0;
    while idx < chars.len() && idx < 2 && chars[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == 0 || (idx < chars.len() && chars[idx].is_ascii_digit()) {
        return None; // no leading digits, or a 3rd digit (run too long)
    }
    let first: String = chars[..idx].iter().collect();

    if idx >= chars.len() || (chars[idx] != 'x' && chars[idx] != 'X') {
        return None;
    }
    idx += 1;

    let second_start = idx;
    while idx < chars.len() && idx - second_start < 2 && chars[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == second_start || (idx < chars.len() && chars[idx].is_ascii_digit()) {
        return None; // no trailing digits, or a 3rd digit (run too long)
    }
    let second: String = chars[second_start..idx].iter().collect();

    Some((format!("{first:0>2}x{second:0>2}"), idx))
}

#[cfg(test)]
mod normalize_tests {
    use super::{normalize_query, normalize_row_col_tokens};

    #[test]
    fn pads_bare_row_col_query() {
        assert_eq!(normalize_row_col_tokens("1x02"), "01x02");
        assert_eq!(normalize_row_col_tokens("1x2"), "01x02");
        assert_eq!(normalize_row_col_tokens("01x2"), "01x02");
        assert_eq!(normalize_row_col_tokens("2x5"), "02x05");
    }

    #[test]
    fn pads_inside_a_longer_query() {
        assert_eq!(
            normalize_row_col_tokens("Pin_Header_1x02"),
            "Pin_Header_01x02"
        );
        assert_eq!(
            normalize_row_col_tokens("header 2x5 jumper"),
            "header 02x05 jumper"
        );
    }

    #[test]
    fn already_padded_is_unchanged() {
        assert_eq!(normalize_row_col_tokens("Conn_01x02"), "Conn_01x02");
    }

    #[test]
    fn leaves_unrelated_queries_alone() {
        assert_eq!(normalize_row_col_tokens("resistor"), "resistor");
        assert_eq!(normalize_row_col_tokens("i2c"), "i2c");
        assert_eq!(normalize_row_col_tokens("STM32F4"), "STM32F4");
    }

    #[test]
    fn does_not_touch_longer_digit_runs() {
        // Not a row/col pattern — a 3rd digit on either side means this is
        // some other numeric token (part number, voltage, etc), not a pin
        // count, so it must pass through untouched.
        assert_eq!(normalize_row_col_tokens("100x200"), "100x200");
        assert_eq!(normalize_row_col_tokens("1x100"), "1x100");
    }

    #[test]
    fn normalize_query_desugars_underscores_to_spaces() {
        // `Pin_Header_1x02` must become independent AND'd barewords, not an
        // implicit phrase FTS5 can never satisfy across columns.
        assert_eq!(normalize_query("Pin_Header_1x02"), "Pin Header 01x02");
        assert_eq!(normalize_query("Conn_01x02"), "Conn 01x02");
    }

    #[test]
    fn normalize_query_composes_both_fixes() {
        assert_eq!(normalize_query("header 2 pin"), "header 2 pin");
        assert_eq!(normalize_query("1x02"), "01x02");
    }

    #[test]
    fn boundary_must_be_non_alphanumeric() {
        // "R1x02" — the digit run doesn't start at a word boundary (it's
        // glued to a letter), so this is left alone rather than guessed at.
        assert_eq!(normalize_row_col_tokens("R1x02"), "R1x02");
    }
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
        params.push(Box::new(normalize_query(q)));
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
