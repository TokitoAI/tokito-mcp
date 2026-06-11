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
    assert!(any.len() > scoped.len(), "scoped should drop non-Device hits");
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
