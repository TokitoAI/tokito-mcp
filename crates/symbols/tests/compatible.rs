//! find_compatible — structured filters with and without an FTS5 query.

mod common;

use tokito_symbols::search::{self, CompatibleOpts};

#[test]
fn requires_no_filter_is_caller_concern() {
    // The function itself returns whatever matches; "no filter" returns
    // everything up to limit. The REST/MCP layer enforces at-least-one.
    let conn = common::fixture_db();
    let all = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: None,
            fp_pattern: None,
            query: None,
            limit: 100,
            lib_filter: None,
        },
    )
    .unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn filters_by_pin_count() {
    let conn = common::fixture_db();
    let hits = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: Some(2),
            fp_pattern: None,
            query: None,
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "R");
}

#[test]
fn filters_by_footprint_pattern() {
    let conn = common::fixture_db();
    let hits = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: None,
            fp_pattern: Some("DIP"),
            query: None,
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    let names: Vec<&str> = hits.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"ROOT_OP"));
    assert!(names.contains(&"LMxxx_A"));
    assert!(!names.contains(&"R"));
}

#[test]
fn combines_query_and_structured_filter() {
    let conn = common::fixture_db();
    let hits = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: Some(5),
            fp_pattern: None,
            query: Some("opamp"),
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    // ROOT_OP has pin_count=5 + matches FTS "opamp"; LMxxx_A has pin_count=5
    // (backfilled from parent) + matches FTS "opamp"; should return both.
    assert_eq!(hits.len(), 2);
}

#[test]
fn lib_filter_is_respected() {
    let conn = common::fixture_db();
    let hits = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: Some(5),
            fp_pattern: None,
            query: None,
            limit: 10,
            lib_filter: Some("Device"),
        },
    )
    .unwrap();
    assert!(hits.is_empty(), "Device lib has no 5-pin parts");
}

#[test]
fn fp_pattern_escapes_like_wildcards() {
    let conn = common::fixture_db();

    // `_` is a LIKE single-char wildcard; escaped it must match a literal
    // underscore. Only R's fp_filters ("R_*") contains one — unescaped, `%_%`
    // would match all three fixtures.
    let underscore = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: None,
            fp_pattern: Some("_"),
            query: None,
            limit: 100,
            lib_filter: None,
        },
    )
    .unwrap();
    let names: Vec<&str> = underscore.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["R"],
        "`_` must match literally, not as a wildcard"
    );

    // `%` as a wildcard matches everything; escaped it matches a literal `%`,
    // which no fixture footprint contains.
    let percent = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: None,
            fp_pattern: Some("%"),
            query: None,
            limit: 100,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(
        percent.is_empty(),
        "`%` must match literally, not as a wildcard; got {percent:?}"
    );
}

#[test]
fn no_query_orders_deterministically_by_pin_count() {
    let conn = common::fixture_db();
    let hits = search::find_compatible(
        &conn,
        CompatibleOpts {
            pins: None,
            fp_pattern: None,
            query: None,
            limit: 100,
            lib_filter: None,
        },
    )
    .unwrap();
    let pin_counts: Vec<u16> = hits.iter().map(|h| h.pin_count).collect();
    let mut sorted = pin_counts.clone();
    sorted.sort();
    assert_eq!(pin_counts, sorted, "expected ascending pin_count order");
}
