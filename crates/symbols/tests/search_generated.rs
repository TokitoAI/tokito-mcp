//! Tests: search results union `symbol` and published `generated_symbol`,
//! carrying a `source` marker that distinguishes the two catalogs.

mod common;

use tokito_symbols::{
    model::{Pin, PinElectrical, PinStyle, PublicationStatus, Source, SymbolBody, SymbolFlags},
    part_id::PartId,
    search::{self, SearchOpts},
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
fn search_includes_generated_symbols_with_source_marker() {
    let conn = common::fixture_db();
    let body = tiny_body(8);
    common::insert_generated_fixture(
        &conn,
        "Texas Instruments",
        "TPS5430DDAR",
        "SO-PowerPAD-8",
        "generated:texas_instruments",
        "TPS5430DDAR",
        &body,
    );
    let hits = search::search(
        &conn,
        SearchOpts {
            query: "TPS5430DDAR",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(!hits.is_empty(), "generated symbol must be searchable");
    assert!(hits
        .iter()
        .any(|h| matches!(h.source, Source::Generated) && h.name == "TPS5430DDAR"));
}

#[test]
fn search_official_hits_carry_official_source_marker() {
    let conn = common::fixture_db();
    let hits = search::search(
        &conn,
        SearchOpts {
            query: "resistor",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        assert!(
            matches!(h.source, Source::Official),
            "official row was tagged as {:?}",
            h.source
        );
    }
}

#[test]
fn search_legacy_official_artifact_without_generated_tables() {
    let conn = common::fixture_db();
    conn.execute_batch(
        "DROP TABLE generated_symbol_fts; \
         DROP TABLE generated_symbol; \
         DROP TABLE part_registry;",
    )
    .unwrap();
    let hits = search::search(
        &conn,
        SearchOpts {
            query: "resistor",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(hits.iter().any(|hit| hit.name == "R"));
    assert!(hits.iter().all(|hit| hit.source == Source::Official));
}

#[test]
fn search_excludes_non_published_generated_revisions() {
    let conn = common::fixture_db();
    let body = tiny_body(2);
    let body_bytes = postcard::to_stdvec(&body).unwrap();
    let part = PartId::new("Vendor Inc", "QUARANTINE-1", "SOIC-8").unwrap();
    tokito_symbols::generated::insert_revision(
        &conn,
        tokito_symbols::generated::NewRevision {
            revision_id: "gen_sha256_q1",
            part: &part,
            lib: "generated:vendor_inc",
            name: "QUARANTINE-1",
            ref_des: "U",
            description: "quarantined revision",
            keywords: "quarantined",
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

    // FTS5 treats `-` as NOT; use a hyphen-free tokenizable name for the query.
    let hits = search::search(
        &conn,
        SearchOpts {
            query: "quarantined",
            limit: 10,
            lib_filter: None,
        },
    )
    .unwrap();
    assert!(
        hits.iter().all(|h| !matches!(h.source, Source::Generated)),
        "quarantined generated symbols must not surface in search"
    );
}

#[test]
fn search_lib_filter_scopes_generated_correctly() {
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

    // Filter to a lib that does NOT include the generated one — must exclude it.
    let hits = search::search(
        &conn,
        SearchOpts {
            query: "TPS5430DDAR",
            limit: 10,
            lib_filter: Some("Device"),
        },
    )
    .unwrap();
    assert!(hits.iter().all(|h| h.name != "TPS5430DDAR"));

    // Filter to the generated lib — must include it.
    let hits = search::search(
        &conn,
        SearchOpts {
            query: "TPS5430DDAR",
            limit: 10,
            lib_filter: Some("generated:texas_instruments"),
        },
    )
    .unwrap();
    assert!(hits.iter().any(|h| h.name == "TPS5430DDAR"));
}
