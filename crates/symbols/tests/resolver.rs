//! Resolver — root, extending child, not-found, caching.

mod common;

use tokito_symbols::resolver::Resolver;

#[test]
fn root_symbol_returns_own_body() {
    let conn = common::fixture_db();
    let r = Resolver::new(64);
    let s = r.resolve(&conn, "Device", "R").unwrap();
    assert_eq!(s.lib, "Device");
    assert_eq!(s.name, "R");
    assert_eq!(s.parent, None);
    assert_eq!(s.body.pins.len(), 2, "root carries its own pins");
    assert_eq!(s.body.graphics.len(), 1, "root carries its own graphics");
    assert_eq!(s.ref_des, "R");
    assert_eq!(s.description, "Resistor");
}

#[test]
fn extending_child_inherits_parent_body_and_keeps_own_metadata() {
    let conn = common::fixture_db();
    let r = Resolver::new(64);
    let s = r.resolve(&conn, "Amplifier_Op", "LMxxx_A").unwrap();
    assert_eq!(s.name, "LMxxx_A");
    assert_eq!(
        s.parent,
        Some(("Amplifier_Op".into(), "ROOT_OP".into())),
        "parent (lib,name) preserved on resolved output"
    );
    assert_eq!(s.body.pins.len(), 5, "child inherits parent's pins");
    assert_eq!(
        s.body.pins[0].name, "PIN1",
        "pin names come from parent body"
    );
    assert_eq!(
        s.description, "Single low-noise opamp, DIP-8",
        "child keeps its own description (not parent's)"
    );
    assert_eq!(s.fp_filters, "DIP-8*", "child keeps its own fp filters");
}

#[test]
fn missing_symbol_returns_typed_error() {
    let conn = common::fixture_db();
    let r = Resolver::new(64);
    let err = r.resolve(&conn, "Device", "NoSuchSymbol").unwrap_err();
    match err {
        tokito_symbols::Error::SymbolNotFound { lib, name } => {
            assert_eq!(lib, "Device");
            assert_eq!(name, "NoSuchSymbol");
        }
        other => panic!("expected SymbolNotFound, got {other:?}"),
    }
}

#[test]
fn cache_hits_return_same_arc() {
    let conn = common::fixture_db();
    let r = Resolver::new(64);
    let a = r.resolve(&conn, "Device", "R").unwrap();
    let b = r.resolve(&conn, "Device", "R").unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "second resolve should return the cached Arc verbatim"
    );
}

#[test]
fn pin_count_backfill_visible_via_resolver() {
    // The extending child's pin_count was UPDATEd from the parent at fixture
    // build time. After resolver materialises the body, the body's pins.len()
    // matches the catalog's pin_count, end to end.
    let conn = common::fixture_db();
    let pin_count: i64 = conn
        .query_row(
            "SELECT s.pin_count FROM symbol s JOIN lib l ON l.id=s.lib_id \
             WHERE l.name='Amplifier_Op' AND s.name='LMxxx_A'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pin_count, 5);
}
