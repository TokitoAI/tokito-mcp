//! Search — FTS5 ranks, library filter, list_libraries.

mod common;

use tokito_symbols::search;

#[test]
fn fts_finds_opamp_via_description_and_keywords() {
    let conn = common::fixture_db();
    let hits = search::search(
        &conn,
        search::SearchOpts {
            query: "opamp",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(!hits.is_empty(), "opamp query should match the fixture");
    let names: Vec<String> = hits.iter().map(|r| r.name.clone()).collect();
    assert!(names.iter().any(|n| n == "ROOT_OP"));
    assert!(names.iter().any(|n| n == "LMxxx_A"));
}

#[test]
fn fts_filters_by_library() {
    let conn = common::fixture_db();
    let any = search::search(
        &conn,
        search::SearchOpts {
            query: "opamp",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    let scoped = search::search(
        &conn,
        search::SearchOpts {
            query: "opamp",
            limit: 10,
            lib_filter: Some("Device"),
        },
    )
    .unwrap();
    assert!(
        any.len() > scoped.len(),
        "scoped should drop non-Device hits"
    );
    for h in &scoped {
        assert_eq!(h.lib, "Device");
    }
}

#[test]
fn fts_returns_resistor_for_keyword_match() {
    let conn = common::fixture_db();
    let hits = search::search(
        &conn,
        search::SearchOpts {
            query: "resistor",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "R");
    assert_eq!(hits[0].pin_count, 2);
}

// --- TokitoAI/tokito-mcp#105: generic connector / pin header resolution ---
//
// `Connector_Generic:Conn_01x02` was always in the catalog; the four query
// forms from the issue came back empty because of two independent
// search-layer bugs, not a missing-from-db gap:
//   1. FTS5 `MATCH` is exact-token equality, so an unpadded "1x02" never
//      lined up with the indexed "01x02" token.
//   2. A single underscore-joined bareword like "Pin_Header_1x02" tokenizes
//      into more than one token, which FTS5 silently reinterprets as an
//      implicit *phrase* (adjacent, same column) rather than an AND of
//      independent terms — impossible to satisfy across the `name` and
//      `keywords` columns.
// See `crates/symbols/src/search.rs::normalize_query` and
// `crates/pack/src/emit.rs::enrich_keywords` for the fixes.

fn hit_names(hits: &[tokito_symbols::model::SymbolRef]) -> Vec<String> {
    hits.iter()
        .map(|r| format!("{}:{}", r.lib, r.name))
        .collect()
}

#[test]
fn resolves_conn_01x02_via_all_four_issue_queries() {
    let conn = common::fixture_db_with_connectors();
    for query in ["1x02", "Pin_Header_1x02", "header 2 pin", "Conn_01x02"] {
        let hits = search::search(
            &conn,
            search::SearchOpts {
                query,
                limit: 20,
                lib_filter: None,
            },
        )
        .unwrap();
        let names = hit_names(&hits);
        assert!(
            names.contains(&"Connector_Generic:Conn_01x02".to_string()),
            "query {query:?} should resolve Connector_Generic:Conn_01x02, got {names:?}"
        );
    }
}

#[test]
fn resolves_conn_01x04_family_too() {
    let conn = common::fixture_db_with_connectors();
    for query in ["1x04", "Pin_Header_1x04", "Conn_01x04"] {
        let hits = search::search(
            &conn,
            search::SearchOpts {
                query,
                limit: 20,
                lib_filter: None,
            },
        )
        .unwrap();
        let names = hit_names(&hits);
        assert!(
            names.contains(&"Connector_Generic:Conn_01x04".to_string()),
            "query {query:?} should resolve Connector_Generic:Conn_01x04, got {names:?}"
        );
    }
}

#[test]
fn header_2_pin_query_picks_the_2_pin_variant_not_just_any_header() {
    let conn = common::fixture_db_with_connectors();
    let hits = search::search(
        &conn,
        search::SearchOpts {
            query: "header 2 pin",
            limit: 20,
            lib_filter: None,
        },
    )
    .unwrap();
    let names = hit_names(&hits);
    assert!(names.contains(&"Connector_Generic:Conn_01x02".to_string()));
    // The 4-pin sibling only carries a "4-pin" token, so a "2 pin" query
    // must not conflate it with the part actually asked for.
    assert!(!names.contains(&"Connector_Generic:Conn_01x04".to_string()));
}

#[test]
fn resistor_query_does_not_rank_generic_connectors() {
    let conn = common::fixture_db_with_connectors();
    let hits = search::search(
        &conn,
        search::SearchOpts {
            query: "resistor",
            limit: 20,
            lib_filter: None,
        },
    )
    .unwrap();
    let names = hit_names(&hits);
    assert_eq!(names, vec!["Device:R".to_string()]);
}

#[test]
fn screw_terminal_barrel_jack_and_jumper_families_resolve() {
    let conn = common::fixture_db_with_connectors();

    let screw = search::search(
        &conn,
        search::SearchOpts {
            query: "screw terminal",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hit_names(&screw).contains(&"Connector:Screw_Terminal_01x02".to_string()));

    // Bonus: the same underscore/padding normalization generalizes to this
    // family too, since it isn't special-cased to `Conn_*` names.
    let screw_by_name = search::search(
        &conn,
        search::SearchOpts {
            query: "Screw_Terminal_1x02",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hit_names(&screw_by_name).contains(&"Connector:Screw_Terminal_01x02".to_string()));

    let jack = search::search(
        &conn,
        search::SearchOpts {
            query: "barrel jack",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hit_names(&jack).contains(&"Connector:Barrel_Jack".to_string()));

    let jumper = search::search(
        &conn,
        search::SearchOpts {
            query: "jumper",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hit_names(&jumper).contains(&"Connector:Jumper_2_Open".to_string()));
}

#[test]
fn find_compatible_also_benefits_from_query_normalization() {
    let conn = common::fixture_db_with_connectors();
    let hits = search::find_compatible(
        &conn,
        search::CompatibleOpts {
            pins: None,
            fp_pattern: None,
            query: Some("Pin_Header_1x02"),
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hit_names(&hits).contains(&"Connector_Generic:Conn_01x02".to_string()));
}

// --- TokitoAI/tokito-mcp#106 review (P2a): padding must not lose literal
// unpadded hits ---
//
// Character LCDs, keypad matrices, and LED matrices are real symbols named
// with a row/column-shaped count that KiCad does *not* zero-pad — unlike
// the generic connector family. Padding "16x2" to "16x02" unconditionally
// would silently return zero hits for all of these; `normalize_query` must
// match both the as-typed and padded forms.

#[test]
fn padding_does_not_lose_literal_unpadded_lcd_and_matrix_hits() {
    let conn = common::fixture_db_with_connectors();
    let cases = [
        ("16x2", "Display_Character:HD44780_16x2"),
        ("20x4", "Display_Character:HD44780_20x4"),
        ("4x4", "Switch:Keypad_4x4"),
        ("8x8", "Display_LED:LED_Matrix_8x8"),
    ];
    for (query, expected) in cases {
        let hits = search::search(
            &conn,
            search::SearchOpts {
                query,
                limit: 10,
                lib_filter: None,
            },
        )
        .unwrap();
        let names = hit_names(&hits);
        assert!(
            names.contains(&expected.to_string()),
            "query {query:?} should still resolve {expected}, got {names:?}"
        );
    }
}

#[test]
fn padding_still_resolves_the_padded_connector_form_alongside_literal_hits() {
    // Both fixture families coexist in the same catalog; padding one must
    // not come at the expense of the other.
    let conn = common::fixture_db_with_connectors();
    let hits = search::search(
        &conn,
        search::SearchOpts {
            query: "1x02",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hit_names(&hits).contains(&"Connector_Generic:Conn_01x02".to_string()));
}

// --- TokitoAI/tokito-mcp#106 review (P1): hostile-input hardening ---
//
// None of these may return an `Err` from `search::search` at all other than
// `Error::InvalidQuery` — no panics, no generic SQL failures that would
// surface as a 500 through the REST/MCP layer (see
// `crates/server/tests/search_hostile_input.rs` for the HTTP-status-level
// version of this same probe table).

#[test]
fn hostile_queries_never_produce_an_unclassified_sql_error() {
    let conn = common::fixture_db_with_connectors();
    let probes = [
        "fp_filters:Connector*",
        "_",
        "__",
        "AND_gate",
        "OR_gate",
        "NOT_gate",
        "he\"llo",
        "unterminated \"quote",
        "(pin OR header)",
        "NEAR(pin header)",
        "コネクタ",
    ];
    for query in probes {
        let result = search::search(
            &conn,
            search::SearchOpts {
                query,
                limit: 10,
                lib_filter: None,
            },
        );
        match result {
            Ok(_) => {}
            Err(tokito_symbols::Error::InvalidQuery(_)) => {}
            Err(other) => panic!("query {query:?} produced an unclassified error: {other:?}"),
        }
    }
}

#[test]
fn list_libraries_returns_expected_counts() {
    let conn = common::fixture_db();
    let libs = search::list_libraries(&conn).unwrap();
    assert_eq!(libs.len(), 2);
    let amp = libs.iter().find(|l| l.name == "Amplifier_Op").unwrap();
    assert_eq!(amp.symbol_count, 2);
    let dev = libs.iter().find(|l| l.name == "Device").unwrap();
    assert_eq!(dev.symbol_count, 1);
}
