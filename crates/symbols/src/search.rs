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
    if let Some(lib) = opts.lib_filter {
        run_match_query(&mut stmt, rusqlite::params![query, lib, opts.limit])
    } else {
        run_match_query(&mut stmt, rusqlite::params![query, opts.limit])
    }
}

/// Executes a prepared `... MATCH ?1 ...` statement and collects the rows.
/// The SQL text at every call site is a fixed, trusted template — the only
/// caller-controlled input is the `MATCH` argument bound into it — but the
/// *rows* it reads back are not equally trusted: a corrupt catalog can fail
/// to decode a column just as easily on a query that happens to hit FTS5 as
/// on one that doesn't. So only a `rusqlite::Error` that is specifically
/// FTS5/SQLite rejecting the query's *syntax* becomes
/// [`Error::InvalidQuery`] (→ 400); everything else — a row-decode failure,
/// corruption, I/O, whatever — stays a generic internal fault (→ 500), the
/// same as it would for a query that never touched `MATCH` at all. See
/// [`is_query_syntax_error`] (TokitoAI/tokito-mcp#106 review, round 2: an
/// earlier version of this function mapped *every* error from this call —
/// including row-decode failures on a corrupt `description` blob — to
/// `InvalidQuery`, which both hid the internal fault from the server log
/// and leaked the raw rusqlite message to the client).
fn run_match_query<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> Result<Vec<SymbolRef>> {
    stmt.query_map(params, row_to_ref)
        .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
        .map_err(query_error)
}

/// Classifies an error from executing a `MATCH` statement: a query-syntax
/// problem becomes [`Error::InvalidQuery`], anything else passes through as
/// [`Error::Sql`] unchanged (so it gets the usual 500 treatment upstream).
fn query_error(e: rusqlite::Error) -> crate::Error {
    if is_query_syntax_error(&e) {
        crate::Error::InvalidQuery(e.to_string())
    } else {
        crate::Error::Sql(e)
    }
}

/// True when `e` is FTS5/SQLite rejecting a `MATCH` argument's syntax (a bad
/// column filter, unbalanced quotes, a bare `AND`/`OR`/`NOT` with no
/// operand, ...) — narrowly, by primary SQLite result code, not by message
/// text (version-dependent and free-form) or by "any error from this call
/// site" (too broad — see the doc comment on [`run_match_query`]).
///
/// SQLite reports every one of these as a generic `SQLITE_ERROR`, which
/// rusqlite classifies as `ErrorCode::Unknown` (it has no dedicated variant
/// for "generic error" — every *other* primary result code SQLite defines
/// gets its own named `ErrorCode`). Checking the enum variant is therefore
/// enough to exclude, structurally, the failure modes this must never
/// swallow:
///   - `rusqlite::Error::FromSqlConversionFailure` /
///     `InvalidColumnType` / `InvalidColumnIndex` / `Utf8Error` — row-decode
///     failures. These aren't even `SqliteFailure`, so the outer `matches!`
///     already excludes them regardless of `ErrorCode`.
///   - `SqliteFailure` with `ErrorCode::DatabaseCorrupt` / `SystemIoFailure`
///     / `DatabaseBusy` / `NotADatabase` (SQLite's `CORRUPT`/`IOERR`/`BUSY`/
///     `NOTADB`) — each has its own dedicated `ErrorCode` variant distinct
///     from `Unknown`, so these fall through to `Error::Sql` (500) too.
fn is_query_syntax_error(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::Unknown,
                ..
            },
            _,
        )
    )
}

/// Reshapes a raw user query into one that means what the caller intended
/// once it reaches FTS5 `MATCH`. Runs ahead of every FTS5 `MATCH`, so
/// `search_symbols`, `find_compatible`, and the REST `/v1/search` face all
/// benefit — and are all equally exposed if this rewrite ever produces
/// something FTS5 can't parse, which is why bullet 0 below exists.
///
/// 0. **Syntax passthrough (TokitoAI/tokito-mcp#106 review).** A query that
///    already carries FTS5 query-grammar markers — a column filter
///    (`fp_filters:Connector*`), a quoted phrase, `(`/`)` grouping (also
///    used by `NEAR(...)`), a prefix wildcard (`*`), or a standalone
///    `AND`/`OR`/`NOT`/`NEAR` operator token — is passed through completely
///    unmodified. Two reasons: rewriting risks turning a well-formed query
///    into a malformed one (desugaring `AND_gate` below would resurrect
///    `AND` as a real operator with no left operand; desugaring
///    `fp_filters:Connector*` would split the column name into a bareword
///    `fp` plus a bogus `filters:` reference), and a caller who already
///    knows FTS5 syntax should get exactly their pre-#105 behavior back.
/// 1. **Underscore desugaring.** FTS5's query grammar treats a
///    punctuation-free run of characters (a "bareword") as a single term —
///    but if *tokenizing* that bareword produces more than one token (which
///    underscores do, since `unicode61` treats `_` as a separator), FTS5
///    silently reinterprets it as an implicit **phrase**: the sub-tokens
///    must appear immediately adjacent, in that order, in a single column.
///    A copy-pasted symbol-style query like `Pin_Header_1x02` becomes the
///    phrase `pin` → `header` → `1x02`, which can never match a row whose
///    "pin header" synonym lives in `keywords` and whose `01x02` lives in
///    `name` — different columns can't be phrase-adjacent. Splitting on `_`
///    and joining the pieces with explicit `AND` instead turns that into
///    three independent terms — exactly what a name-shaped query is
///    supposed to mean. (Terms are joined with an explicit `AND` rather
///    than left space-separated because bullet 2 below can turn a term into
///    a parenthesized `(a OR b)` group, and FTS5's grammar rejects bare
///    juxtaposition — `foo (a OR b) bar` — next to a group; only `foo AND
///    (a OR b) AND bar` parses.)
/// 2. **Row/column padding, both forms.** KiCad's generic connector
///    families name pin counts with zero-padded two-digit numbers joined by
///    `x` (`Conn_01x02`, `Screw_Terminal_02x05`), but people type the count
///    without the padding (`1x02`, `2x5`). FTS5 `MATCH` is exact token
///    equality, not prefix or fuzzy matching, so an unpadded query token
///    never lines up with the indexed `01x02` token. But plenty of *other*
///    real symbols index the count literally unpadded — character LCDs
///    (`16x2`, `20x4`), keypad and LED matrices (`4x4`, `8x8`) — so
///    unconditionally padding would silently lose those hits (review
///    finding on the original #105 fix). A whole term shaped like `<1-2
///    digits>x<1-2 digits>` is therefore rewritten to match *both* forms:
///    `1x02` becomes `(1x02 OR 01x02)`. When the term is already padded
///    (`01x02`), both forms are identical and it passes through unchanged.
pub(crate) fn normalize_query(query: &str) -> String {
    if has_fts5_syntax(query) {
        return query.to_string();
    }
    let terms: Vec<String> = query
        .replace('_', " ")
        .split_whitespace()
        .map(normalize_term)
        .collect();
    if terms.is_empty() {
        // Desugaring can strip an already-degenerate query (e.g. a lone
        // `_`, which `unicode61` tokenizes to nothing) down to pure
        // whitespace. FTS5's grammar rejects an empty `MATCH` argument
        // outright — but happily accepts the original underscore-only text
        // as a zero-token query, so fall back to that rather than sending
        // FTS5 something that can never parse.
        return query.to_string();
    }
    terms.join(" AND ")
}

/// True when `query` contains an FTS5 query-grammar marker: a column-filter
/// colon, a quote, parens (also covers `NEAR(...)`), a prefix-wildcard
/// `*`, or a standalone `AND`/`OR`/`NOT`/`NEAR` operator token (checked
/// case-sensitively and word-bounded — FTS5 only recognizes these as
/// operators in exactly that form, per the grammar; splitting on any
/// non-alphanumeric character also catches an operator hiding behind an
/// underscore, e.g. `AND_gate`, since desugaring would otherwise resurrect
/// it as a real operator).
fn has_fts5_syntax(query: &str) -> bool {
    if query.contains([':', '"', '(', ')', '*']) {
        return true;
    }
    query
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| matches!(word, "AND" | "OR" | "NOT" | "NEAR"))
}

/// Normalizes one whitespace-delimited term. A term shaped exactly like a
/// KiCad row/column count (`<1-2 digits>x<1-2 digits>`, matched in full —
/// not as a substring, so `R1x02` and `100x200` pass through untouched) is
/// rewritten to an FTS5 OR-group matching both the as-typed and the
/// zero-padded form; anything else is returned unchanged.
fn normalize_term(term: &str) -> String {
    match padded_row_col(term) {
        Some(padded) if padded != term => format!("({term} OR {padded})"),
        _ => term.to_string(),
    }
}

/// Parses `term` as a whole `<1-2 digits>x<1-2 digits>` row/column token and
/// returns its zero-padded form, or `None` if `term` isn't shaped like one —
/// extra characters before/after the digits, or a digit run longer than 2
/// (KiCad's row/column counts never exceed two digits, so e.g. `100x200` or
/// `1x100` is some other kind of numeric token — a part number, a
/// voltage — not a pin count, and must pass through untouched).
fn padded_row_col(term: &str) -> Option<String> {
    let chars: Vec<char> = term.chars().collect();
    let (padded, consumed) = match_row_col(&chars)?;
    (consumed == chars.len()).then_some(padded)
}

/// Tries to parse a `<digits>x<digits>` run at the start of `chars`. Returns
/// the zero-padded replacement and how many source chars it consumed.
/// Digit runs are capped at 2 on each side — see [`padded_row_col`].
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
    use super::{has_fts5_syntax, normalize_query, padded_row_col};

    // --- padded_row_col: the row/col pattern matcher ---

    #[test]
    fn pads_bare_row_col_term() {
        assert_eq!(padded_row_col("1x02").as_deref(), Some("01x02"));
        assert_eq!(padded_row_col("1x2").as_deref(), Some("01x02"));
        assert_eq!(padded_row_col("01x2").as_deref(), Some("01x02"));
        assert_eq!(padded_row_col("2x5").as_deref(), Some("02x05"));
    }

    #[test]
    fn already_padded_round_trips() {
        assert_eq!(padded_row_col("01x02").as_deref(), Some("01x02"));
    }

    #[test]
    fn does_not_touch_longer_digit_runs() {
        // Not a row/col pattern — a 3rd digit on either side means this is
        // some other numeric token (part number, voltage, etc), not a pin
        // count, so it must pass through untouched.
        assert_eq!(padded_row_col("100x200"), None);
        assert_eq!(padded_row_col("1x100"), None);
    }

    #[test]
    fn must_match_the_whole_term() {
        // "R1x02" — glued to a letter, not a standalone row/col term — is
        // left alone rather than guessed at.
        assert_eq!(padded_row_col("R1x02"), None);
        assert_eq!(padded_row_col("resistor"), None);
    }

    // --- has_fts5_syntax: the passthrough detector ---

    #[test]
    fn detects_syntax_markers() {
        for q in [
            "fp_filters:Connector*",
            "he\"llo",
            "(pin OR header)",
            "NEAR(pin header)",
            "1x02*",
        ] {
            assert!(
                has_fts5_syntax(q),
                "{q:?} should be detected as FTS5 syntax"
            );
        }
    }

    #[test]
    fn detects_standalone_boolean_operators_even_behind_an_underscore() {
        for q in [
            "AND_gate",
            "OR_gate",
            "NOT_gate",
            "foo AND bar",
            "NEAR thing",
        ] {
            assert!(
                has_fts5_syntax(q),
                "{q:?} should be detected as FTS5 syntax"
            );
        }
    }

    #[test]
    fn plain_queries_are_not_flagged_as_syntax() {
        for q in [
            "resistor",
            "1x02",
            "Pin_Header_1x02",
            "header 2 pin",
            "_",
            "__",
        ] {
            assert!(!has_fts5_syntax(q), "{q:?} should not be flagged");
        }
    }

    // --- normalize_query: the full pipeline ---

    #[test]
    fn desugars_underscores_and_and_joins() {
        // `Pin_Header_1x02` must become independent AND'd terms, not an
        // implicit phrase FTS5 can never satisfy across columns.
        assert_eq!(
            normalize_query("Pin_Header_1x02"),
            "Pin AND Header AND (1x02 OR 01x02)"
        );
        assert_eq!(normalize_query("Conn_01x02"), "Conn AND 01x02");
    }

    #[test]
    fn pads_both_forms_via_or_group() {
        assert_eq!(normalize_query("1x02"), "(1x02 OR 01x02)");
        assert_eq!(
            normalize_query("header 2x5 jumper"),
            "header AND (2x5 OR 02x05) AND jumper"
        );
    }

    #[test]
    fn already_padded_query_has_no_or_group() {
        assert_eq!(
            normalize_query("header 01x02 jumper"),
            "header AND 01x02 AND jumper"
        );
    }

    // --- TokitoAI/tokito-mcp#106 review: hostile-input hardening ---
    //
    // Every query below must come back from `normalize_query` still valid
    // FTS5 syntax — a query that already means something in FTS5 grammar
    // must be passed straight through unmodified (bullet 0 of the doc
    // comment), never rewritten into something FTS5 can't parse.

    #[test]
    fn syntax_bearing_queries_pass_through_unmodified() {
        for q in [
            "fp_filters:Connector*",
            "AND_gate",
            "OR_gate",
            "NOT_gate",
            "he\"llo",
            "unterminated \"quote",
            "(pin OR header)",
            "NEAR(pin header)",
        ] {
            assert_eq!(normalize_query(q), q, "{q:?} must be passed through as-is");
        }
    }

    #[test]
    fn underscore_only_query_falls_back_to_the_original_text() {
        // Desugaring "_" alone collapses to pure whitespace, which FTS5's
        // grammar rejects outright — but the original text, sent unmodified,
        // is a query FTS5 happily parses as zero tokens (0 rows, no error).
        // The empty-after-normalization check must catch this and fall back
        // rather than forwarding whitespace.
        assert_eq!(normalize_query("_"), "_");
        assert_eq!(normalize_query("__"), "__");
    }

    #[test]
    fn unicode_query_is_not_flagged_as_syntax_and_survives_normalization() {
        let q = "コネクタ";
        assert!(!has_fts5_syntax(q));
        assert_eq!(normalize_query(q), q);
    }
}

// TokitoAI/tokito-mcp#106 review, round 2: `is_query_syntax_error` /
// `query_error` must classify a genuine FTS5 query-syntax rejection as
// `Error::InvalidQuery` (→ 400) but leave every other kind of
// `rusqlite::Error` — row-decode failures in particular — as `Error::Sql`
// (→ 500), never the other way around. `run_match_query`'s doc comment
// explains why: an earlier version mapped *every* error from that call
// site to `InvalidQuery`, which turned a corrupt catalog into a
// misleading 400 with a leaked raw rusqlite message and no server log.
#[cfg(test)]
mod query_error_tests {
    use super::{is_query_syntax_error, query_error};
    use rusqlite::{ffi, Error as SqlError, ErrorCode};

    /// Behind a function call so the `invalid_from_utf8` lint doesn't flag
    /// the (deliberately) invalid literal at the `from_utf8` call site.
    fn invalid_utf8_bytes() -> Vec<u8> {
        vec![0xff]
    }

    fn sqlite_failure(code: ErrorCode) -> SqlError {
        SqlError::SqliteFailure(
            ffi::Error {
                code,
                extended_code: 1,
            },
            Some("synthetic".to_string()),
        )
    }

    #[test]
    fn generic_sqlite_error_is_a_query_syntax_error() {
        // What FTS5 actually reports for every syntax rejection: unbalanced
        // quotes, a bad column filter, a bare boolean operator, ... — no
        // dedicated SQLite result code, so rusqlite classifies it Unknown.
        let e = sqlite_failure(ErrorCode::Unknown);
        assert!(is_query_syntax_error(&e));
        assert!(matches!(query_error(e), crate::Error::InvalidQuery(_)));
    }

    #[test]
    fn row_decode_failures_are_never_query_syntax_errors() {
        // None of these are even `SqliteFailure` — a corrupt catalog fails
        // to decode a column, which is nothing FTS5's query grammar had any
        // say in.
        let cases = [
            SqlError::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, "synthetic".into()),
            SqlError::InvalidColumnType(3, "description".into(), rusqlite::types::Type::Blob),
            SqlError::InvalidColumnIndex(3),
            SqlError::Utf8Error(3, std::str::from_utf8(&invalid_utf8_bytes()).unwrap_err()),
        ];
        for e in cases {
            assert!(
                !is_query_syntax_error(&e),
                "{e:?} must not be a query-syntax error"
            );
            let mapped = query_error(e);
            assert!(
                matches!(mapped, crate::Error::Sql(_)),
                "must stay Error::Sql (→ 500), got {mapped:?}"
            );
        }
    }

    #[test]
    fn other_sqlite_result_codes_are_never_query_syntax_errors() {
        // Each of these has its own dedicated `ErrorCode` distinct from
        // `Unknown` — corruption, I/O, contention, and "not a database
        // file" are catalog/infra problems, not something a caller's query
        // text could ever cause.
        for code in [
            ErrorCode::DatabaseCorrupt,
            ErrorCode::SystemIoFailure,
            ErrorCode::DatabaseBusy,
            ErrorCode::NotADatabase,
        ] {
            let e = sqlite_failure(code);
            assert!(
                !is_query_syntax_error(&e),
                "{code:?} must not be a query-syntax error"
            );
            assert!(matches!(query_error(e), crate::Error::Sql(_)));
        }
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
    let query_map_result = stmt
        .query_map(rusqlite::params_from_iter(param_refs), row_to_ref)
        .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>());
    // Only the `query.is_some()` branch touches `symbol_fts MATCH`, so only
    // that branch's failures can even possibly be FTS5 rejecting caller
    // syntax — `query_error` still narrows further by result code, so a
    // row-decode failure on this same branch stays a 500 rather than
    // silently becoming a 400 — see `run_match_query`'s doc comment.
    if opts.query.is_some() {
        query_map_result.map_err(query_error)
    } else {
        Ok(query_map_result?)
    }
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
