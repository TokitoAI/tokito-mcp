//! Tests for the generated-symbol store: resolve_by_mpn, provenance
//! lookup, resolver dispatch on `generated:*` libs, and insert idempotency.

mod common;

use tokito_symbols::{
    generated,
    model::{Pin, PinElectrical, PinStyle, PublicationStatus, SymbolBody, SymbolFlags},
    part_id::PartId,
    resolver::Resolver,
    BODY_FORMAT_POSTCARD_V1,
};

fn tiny_body(pin_count: u8) -> SymbolBody {
    SymbolBody {
        pins: (1..=pin_count)
            .map(|n| Pin {
                number: n.to_string(),
                name: format!("P{n}"),
                electrical: PinElectrical::Passive,
                style: PinStyle::Line,
                x: 0,
                y: (n as i32) * 100,
                rotation: 0,
                length: 100,
                unit: 1,
                body_style: 1,
            })
            .collect(),
        graphics: vec![],
        units: vec![],
        props_layout: vec![],
        flags: SymbolFlags::default(),
    }
}

#[test]
fn resolve_by_mpn_returns_published_revision() {
    let conn = common::fixture_db();
    let body = tiny_body(8);
    let part = common::insert_generated_fixture(
        &conn,
        "Texas Instruments",
        "TPS5430DDAR",
        "SO-PowerPAD-8",
        "generated:texas_instruments",
        "TPS5430DDAR",
        &body,
    );

    let resolved = generated::resolve_by_mpn(&conn, &part)
        .unwrap()
        .expect("published revision must resolve");
    assert_eq!(resolved.lib, "generated:texas_instruments");
    assert_eq!(resolved.name, "TPS5430DDAR");
    assert_eq!(resolved.body.pins.len(), 8);
}

#[test]
fn resolve_by_mpn_normalizes_manufacturer_case_and_whitespace() {
    let conn = common::fixture_db();
    let body = tiny_body(3);
    common::insert_generated_fixture(
        &conn,
        "Texas Instruments",
        "TPS5430DDAR",
        "SO-PowerPAD-8",
        "generated:texas_instruments",
        "TPS5430DDAR",
        &body,
    );

    // Same physical part with sloppy manufacturer casing/whitespace.
    let part = PartId::new("  TEXAS   Instruments  ", "TPS5430DDAR", "SO-PowerPAD-8").unwrap();
    let resolved = generated::resolve_by_mpn(&conn, &part).unwrap();
    assert!(
        resolved.is_some(),
        "manufacturer normalization must let sloppy inputs land on the canonical row"
    );
}

#[test]
fn resolve_by_mpn_returns_none_for_unknown_part() {
    let conn = common::fixture_db();
    let part = PartId::new("Unknown Corp", "XYZ", "PKG").unwrap();
    assert!(generated::resolve_by_mpn(&conn, &part).unwrap().is_none());
}

#[test]
fn resolve_by_mpn_ignores_non_published_revisions() {
    // Insert a quarantined revision and assert it's not resolvable.
    let conn = common::fixture_db();
    let body = tiny_body(2);
    let body_bytes = postcard::to_stdvec(&body).unwrap();
    let part = PartId::new("Vendor Inc", "PART-1", "SOIC-8").unwrap();
    generated::insert_revision(
        &conn,
        generated::NewRevision {
            revision_id: "gen_sha256_deadbeef",
            part: &part,
            lib: "generated:vendor_inc",
            name: "PART-1",
            ref_des: "U",
            description: "quarantined",
            keywords: "",
            fp_filters: "",
            datasheet: "",
            footprint: "",
            pin_count: 2,
            flags: 0,
            body: &body_bytes,
            body_format: BODY_FORMAT_POSTCARD_V1,
            provenance_json: "{}",
            status: PublicationStatus::Quarantined,
            content_hash: "sha256:00",
            published_at: "2026-08-08T07:00:00Z",
        },
    )
    .unwrap();
    assert!(generated::resolve_by_mpn(&conn, &part).unwrap().is_none());
}

#[test]
fn resolver_dispatches_on_generated_prefix() {
    let conn = common::fixture_db();
    let body = tiny_body(4);
    common::insert_generated_fixture(
        &conn,
        "STMicroelectronics",
        "STM32F103C8T6",
        "LQFP48",
        "generated:stmicroelectronics",
        "STM32F103C8T6",
        &body,
    );

    let resolver = Resolver::new(16);
    let sym = resolver
        .resolve(&conn, "generated:stmicroelectronics", "STM32F103C8T6")
        .expect("dispatch to generated store");
    assert_eq!(sym.body.pins.len(), 4);
}

#[test]
fn resolver_falls_through_to_official_for_non_generated_libs() {
    // The fixture ships Device:R and Amplifier_Op:ROOT_OP as official symbols;
    // dispatch must NOT hit the generated path for those.
    let conn = common::fixture_db();
    let resolver = Resolver::new(16);
    let sym = resolver
        .resolve(&conn, "Device", "R")
        .expect("official lookup must still work");
    assert_eq!(sym.name, "R");
}

#[test]
fn provenance_returns_stored_json_for_published_symbol() {
    let conn = common::fixture_db();
    let body = tiny_body(2);
    common::insert_generated_fixture(
        &conn,
        "Texas Instruments",
        "TPS5430DDAR",
        "SO-PowerPAD-8",
        "generated:texas_instruments",
        "TPS5430DDAR",
        &body,
    );

    let p = generated::provenance_for_symbol(&conn, "generated:texas_instruments", "TPS5430DDAR")
        .unwrap()
        .expect("provenance present");
    assert_eq!(p["part_id"]["mpn"], "TPS5430DDAR");
    assert_eq!(p["status"], "published");
    assert!(p["revision_id"]
        .as_str()
        .unwrap()
        .starts_with("gen_sha256_"));
}

#[test]
fn provenance_none_for_missing_symbol() {
    let conn = common::fixture_db();
    let p = generated::provenance_for_symbol(&conn, "generated:nonesuch", "GHOST").unwrap();
    assert!(p.is_none());
}

#[test]
fn insert_revision_is_idempotent_on_same_body() {
    let conn = common::fixture_db();
    let body = tiny_body(2);
    let body_bytes = postcard::to_stdvec(&body).unwrap();
    let part = PartId::new("Acme", "A-1", "SOIC-8").unwrap();
    let new = || generated::NewRevision {
        revision_id: "gen_sha256_stable",
        part: &part,
        lib: "generated:acme",
        name: "A-1",
        ref_des: "U",
        description: "",
        keywords: "",
        fp_filters: "",
        datasheet: "",
        footprint: "",
        pin_count: 2,
        flags: 0,
        body: &body_bytes,
        body_format: BODY_FORMAT_POSTCARD_V1,
        provenance_json: "{}",
        status: PublicationStatus::Published,
        content_hash: "sha256:aa",
        published_at: "2026-08-08T07:00:00Z",
    };
    let id1 = generated::insert_revision(&conn, new()).unwrap();
    let id2 = generated::insert_revision(&conn, new()).unwrap();
    assert_eq!(id1, id2, "idempotent reinsert returns the same row id");
}

#[test]
fn insert_revision_rejects_body_mismatch_on_same_id() {
    let conn = common::fixture_db();
    let part = PartId::new("Acme", "A-1", "SOIC-8").unwrap();
    let body_a = postcard::to_stdvec(&tiny_body(2)).unwrap();
    let body_b = postcard::to_stdvec(&tiny_body(3)).unwrap();
    generated::insert_revision(
        &conn,
        generated::NewRevision {
            revision_id: "gen_sha256_conflict",
            part: &part,
            lib: "generated:acme",
            name: "A-1",
            ref_des: "U",
            description: "",
            keywords: "",
            fp_filters: "",
            datasheet: "",
            footprint: "",
            pin_count: 2,
            flags: 0,
            body: &body_a,
            body_format: BODY_FORMAT_POSTCARD_V1,
            provenance_json: "{}",
            status: PublicationStatus::Published,
            content_hash: "sha256:aa",
            published_at: "2026-08-08T07:00:00Z",
        },
    )
    .unwrap();

    let err = generated::insert_revision(
        &conn,
        generated::NewRevision {
            revision_id: "gen_sha256_conflict",
            part: &part,
            lib: "generated:acme",
            name: "A-1",
            ref_des: "U",
            description: "",
            keywords: "",
            fp_filters: "",
            datasheet: "",
            footprint: "",
            pin_count: 3,
            flags: 0,
            body: &body_b,
            body_format: BODY_FORMAT_POSTCARD_V1,
            provenance_json: "{}",
            status: PublicationStatus::Published,
            content_hash: "sha256:bb",
            published_at: "2026-08-08T07:00:00Z",
        },
    )
    .unwrap_err();
    match err {
        tokito_symbols::Error::RevisionBodyMismatch { revision_id } => {
            assert_eq!(revision_id, "gen_sha256_conflict");
        }
        other => panic!("expected RevisionBodyMismatch, got {other:?}"),
    }
}
