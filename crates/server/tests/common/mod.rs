//! Test fixture for the server crate — builds an in-memory `symbols.sqlite`
//! equivalent without invoking the packer, then wraps it in `AppState`.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use tokito_mcp_server::state::{AppState, Manifest};
use tokito_symbols::{
    generated,
    model::{
        Fill, Graphic, GraphicKind, Pin, PinElectrical, PinStyle, Point, PublicationStatus, Stroke,
        StrokeKind, SymbolBody, SymbolFlags, Unit,
    },
    part_id::PartId,
    resolver::Resolver,
    BODY_FORMAT_POSTCARD_V1, CURRENT_SCHEMA_VERSION, SCHEMA_SQL,
};

#[allow(dead_code)]
pub fn fixture_app_state() -> AppState {
    let conn = build_fixture_conn();
    let manifest = Manifest {
        source_commit: "test-fixture".into(),
        generator_version: "0.0.0".into(),
        schema_version: CURRENT_SCHEMA_VERSION,
        symbol_count: 3,
        lib_count: 2,
        generated_at: None,
    };
    AppState {
        conn: Arc::new(Mutex::new(conn)),
        generated_conn: None,
        resolver: Resolver::new(64),
        manifest: Arc::new(manifest),
    }
}

/// Same as [`fixture_app_state`] but with one published generated symbol
/// seeded so tests can exercise the resolve_by_mpn + provenance surfaces.
///
/// Seeded row:
///   * manufacturer "Texas Instruments" → normalized "texas instruments"
///   * mpn "TPS5430DDAR" (case-sensitive)
///   * package "SO-PowerPAD-8"
///   * lib "generated:texas_instruments"
///   * name "TPS5430DDAR" (matches MPN)
///   * status "published"
#[allow(dead_code)]
pub fn fixture_app_state_with_generated() -> AppState {
    let conn = build_fixture_conn();
    let generated_conn = build_fixture_conn();
    let body = SymbolBody {
        pins: (1..=8)
            .map(|n| Pin {
                number: n.to_string(),
                name: format!("P{n}"),
                electrical: PinElectrical::Passive,
                style: PinStyle::Line,
                x: 0,
                y: n * 100,
                rotation: 0,
                length: 100,
                unit: 1,
                body_style: 1,
            })
            .collect(),
        graphics: vec![],
        units: vec![Unit {
            unit: 1,
            body_style: 1,
        }],
        props_layout: vec![],
        flags: SymbolFlags::default(),
    };
    let body_bytes = postcard::to_stdvec(&body).unwrap();
    let part = PartId::new("Texas Instruments", "TPS5430DDAR", "SO-PowerPAD-8").unwrap();
    let revision_id = "gen_sha256_fixture_tps5430ddar";
    let provenance = serde_json::json!({
        "revision_id": revision_id,
        "part_id": {
            "manufacturer_norm": part.manufacturer_norm,
            "mpn": part.mpn,
            "package": part.package,
        },
        "library_id": {
            "lib": "generated:texas_instruments",
            "name": "TPS5430DDAR",
        },
        "evidence": {
            "datasheet_id": "ti-slvs632l",
            "content_sha256": "83074fc1265c8e5c6639511bdb9f83e96c6e6f993613deadea0d09c3a12a2c07",
            "region_ids": ["r_pinout_01", "r_pin_table_01"],
        },
        "pipeline": {
            "extractor_version": "fixture@0",
            "compiler_version": "fixture@0",
            "layout_policy_version": "fixture@0",
            "extractor_model": "fixture",
            "dsvire_index_version": "fixture@0",
            "dsvire_model_ids": ["fixture"],
        },
        "status": "published",
        "published_at": "2026-08-08T07:15:00Z",
        "content_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    });
    generated::insert_revision(
        &generated_conn,
        generated::NewRevision {
            revision_id,
            part: &part,
            lib: "generated:texas_instruments",
            name: "TPS5430DDAR",
            ref_des: "U",
            description: "TI TPS5430 3A step-down converter (fixture)",
            keywords: "buck regulator switching tps5430",
            fp_filters: "SOIC*",
            datasheet: "https://www.ti.com/lit/ds/symlink/tps5430.pdf",
            footprint: "",
            pin_count: 8,
            flags: 0,
            body: &body_bytes,
            body_format: BODY_FORMAT_POSTCARD_V1,
            symbol_text: "",
            provenance_json: &provenance.to_string(),
            status: PublicationStatus::Published,
            content_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            published_at: "2026-08-08T07:15:00Z",
        },
    )
    .unwrap();
    let manifest = Manifest {
        source_commit: "test-fixture-with-generated".into(),
        generator_version: "0.0.0".into(),
        schema_version: CURRENT_SCHEMA_VERSION,
        symbol_count: 4,
        lib_count: 3,
        generated_at: None,
    };
    AppState {
        conn: Arc::new(Mutex::new(conn)),
        generated_conn: Some(Arc::new(Mutex::new(generated_conn))),
        resolver: Resolver::new(64),
        manifest: Arc::new(manifest),
    }
}

fn build_fixture_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    conn.execute(
        "INSERT INTO meta(key,value) VALUES('schema_version', ?1)",
        params![CURRENT_SCHEMA_VERSION.to_string()],
    )
    .unwrap();

    conn.execute("INSERT INTO lib(name) VALUES('Device')", [])
        .unwrap();
    let device_id: i64 = conn.last_insert_rowid();
    conn.execute("INSERT INTO lib(name) VALUES('Amplifier_Op')", [])
        .unwrap();
    let op_id: i64 = conn.last_insert_rowid();

    let r_body = SymbolBody {
        pins: vec![
            Pin {
                number: "1".into(),
                name: "".into(),
                electrical: PinElectrical::Passive,
                style: PinStyle::Line,
                x: 0,
                y: 381,
                rotation: 270,
                length: 127,
                unit: 1,
                body_style: 1,
            },
            Pin {
                number: "2".into(),
                name: "".into(),
                electrical: PinElectrical::Passive,
                style: PinStyle::Line,
                x: 0,
                y: -381,
                rotation: 90,
                length: 127,
                unit: 1,
                body_style: 1,
            },
        ],
        graphics: vec![Graphic {
            unit: 0,
            body_style: 1,
            kind: GraphicKind::Rectangle {
                start: Point { x: -102, y: -254 },
                end: Point { x: 102, y: 254 },
            },
            stroke: Stroke {
                width: 25,
                kind: StrokeKind::Default,
            },
            fill: Fill::None,
        }],
        units: vec![
            Unit {
                unit: 0,
                body_style: 1,
            },
            Unit {
                unit: 1,
                body_style: 1,
            },
        ],
        props_layout: vec![],
        flags: SymbolFlags::default(),
    };
    insert_symbol(
        &conn,
        device_id,
        "R",
        "R",
        "Resistor",
        "R res resistor",
        "R_*",
        2,
        Some(&r_body),
        None,
    );

    let op_body = SymbolBody {
        pins: (1..=5)
            .map(|n| Pin {
                number: n.to_string(),
                name: format!("PIN{n}"),
                electrical: PinElectrical::Input,
                style: PinStyle::Line,
                x: 0,
                y: n * 100,
                rotation: 0,
                length: 100,
                unit: 1,
                body_style: 1,
            })
            .collect(),
        graphics: vec![],
        units: vec![Unit {
            unit: 1,
            body_style: 1,
        }],
        props_layout: vec![],
        flags: SymbolFlags::default(),
    };
    insert_symbol(
        &conn,
        op_id,
        "ROOT_OP",
        "U",
        "Operational amplifier root",
        "opamp operational amplifier",
        "DIP*",
        5,
        Some(&op_body),
        None,
    );
    insert_symbol(
        &conn,
        op_id,
        "LMxxx_A",
        "U",
        "Single low-noise opamp, DIP-8",
        "opamp low-noise single",
        "DIP-8*",
        0,
        None,
        Some("ROOT_OP"),
    );

    conn.execute(
        "UPDATE symbol SET pin_count = (SELECT pin_count FROM symbol p WHERE p.id = symbol.parent_id) \
         WHERE parent_id IS NOT NULL AND pin_count = 0",
        [],
    )
    .unwrap();

    conn
}

#[allow(clippy::too_many_arguments)]
fn insert_symbol(
    conn: &Connection,
    lib_id: i64,
    name: &str,
    ref_des: &str,
    description: &str,
    keywords: &str,
    fp_filters: &str,
    pin_count: i64,
    body: Option<&SymbolBody>,
    parent_name: Option<&str>,
) {
    let parent_id: Option<i64> = parent_name.map(|n| {
        conn.query_row(
            "SELECT id FROM symbol WHERE lib_id=?1 AND name=?2",
            params![lib_id, n],
            |r| r.get(0),
        )
        .unwrap()
    });
    let body_blob: Option<Vec<u8>> = body.map(|b| postcard::to_stdvec(b).unwrap());
    let body_format = body_blob.as_ref().map(|_| BODY_FORMAT_POSTCARD_V1);
    conn.execute(
        "INSERT INTO symbol(lib_id, name, ref_des, description, keywords, fp_filters, datasheet, footprint, parent_id, pin_count, flags, body, body_format) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, '', '', ?7, ?8, 0, ?9, ?10)",
        params![lib_id, name, ref_des, description, keywords, fp_filters, parent_id, pin_count, body_blob, body_format],
    )
    .unwrap();
}
