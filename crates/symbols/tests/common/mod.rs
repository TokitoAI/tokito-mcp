//! Shared test fixture: build a tiny in-memory `symbols.sqlite` with a
//! root + extending child + a couple of unrelated symbols for search.

use rusqlite::{params, Connection};
use tokito_symbols::{
    generated,
    model::{
        Fill, Graphic, GraphicKind, Pin, PinElectrical, PinStyle, Point, PublicationStatus, Stroke,
        StrokeKind, SymbolBody, SymbolFlags, Unit,
    },
    part_id::PartId,
    BODY_FORMAT_POSTCARD_V1, CURRENT_SCHEMA_VERSION, SCHEMA_SQL,
};

/// Build an in-memory DB with the canonical schema and a small fixture.
///
/// Layout:
///   lib `Device`         → `R` (root, 2 pins, 1 rectangle)
///   lib `Amplifier_Op`   → `ROOT_OP` (root, 5 pins) + `LMxxx_A` (extends ROOT_OP)
pub fn fixture_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    conn.execute(
        "INSERT INTO meta(key,value) VALUES('schema_version', ?1)",
        params![CURRENT_SCHEMA_VERSION.to_string()],
    )
    .unwrap();

    // libs
    conn.execute("INSERT INTO lib(name) VALUES('Device')", [])
        .unwrap();
    let device_id: i64 = conn.last_insert_rowid();
    conn.execute("INSERT INTO lib(name) VALUES('Amplifier_Op')", [])
        .unwrap();
    let op_id: i64 = conn.last_insert_rowid();

    // Device:R (root)
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

    // Amplifier_Op:ROOT_OP (root, simple body with 5 pins)
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

    // Amplifier_Op:LMxxx_A — extends ROOT_OP, body is NULL, own properties.
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

    // pin_count backfill for the extending child — mirror what emit::build does.
    conn.execute(
        "UPDATE symbol SET pin_count = (SELECT pin_count FROM symbol p WHERE p.id = symbol.parent_id) \
         WHERE parent_id IS NOT NULL AND pin_count = 0",
        [],
    )
    .unwrap();

    conn
}

/// `fixture_db()` plus the TokitoAI/tokito-mcp#105 generic-connector /
/// pin-header regression fixtures, kept in a separate builder so the many
/// other tests sharing `fixture_db()` don't have to account for extra libs,
/// symbols, and pin counts they never asked for.
///
/// Keywords mirror exactly what `tokito_mcp_pack::emit::enrich_keywords`
/// produces for these symbols at pack time (see `crates/pack/src/emit.rs`),
/// so these tests exercise the search-layer normalization in isolation from
/// the packer (which has its own unit tests for the enrichment itself).
#[allow(dead_code)]
pub fn fixture_db_with_connectors() -> Connection {
    let conn = fixture_db();

    conn.execute("INSERT INTO lib(name) VALUES('Connector_Generic')", [])
        .unwrap();
    let conn_generic_id: i64 = conn.last_insert_rowid();
    conn.execute("INSERT INTO lib(name) VALUES('Connector')", [])
        .unwrap();
    let connector_id: i64 = conn.last_insert_rowid();

    insert_symbol(
        &conn,
        conn_generic_id,
        "Conn_01x02",
        "J",
        "Generic connector, single row, 01x02, script generated (kicad-library-utils/schlib/autogen/connector/)",
        "connector pin header 2-pin",
        // The *real* packed value (TokitoAI/tokito-mcp#106 review: an
        // earlier version of this fixture used a placeholder that leaked a
        // literal, unpadded "1x02" token into the index via this column,
        // which let the `resolves_conn_01x02_via_all_four_issue_queries`
        // test pass even with the padding fix deleted). This tokenizes to
        // {"connector", "1x"} — no "1x02"/"01x02" token at all — so the
        // "01x02" token that makes these tests pass can only come from the
        // `name` column, which is exactly what pins the padding logic.
        "Connector*:*_1x??_*",
        2,
        None,
        None,
    );
    insert_symbol(
        &conn,
        conn_generic_id,
        "Conn_01x04",
        "J",
        "Generic connector, single row, 01x04, script generated (kicad-library-utils/schlib/autogen/connector/)",
        "connector pin header 4-pin",
        "Connector*:*_1x??_*",
        4,
        None,
        None,
    );
    insert_symbol(
        &conn,
        connector_id,
        "Screw_Terminal_01x02",
        "J",
        "Generic screw terminal, single row, 01x02",
        "screw terminal",
        "TerminalBlock*:*", // real packed value
        2,
        None,
        None,
    );
    insert_symbol(
        &conn,
        connector_id,
        "Barrel_Jack",
        "J",
        "Barrel connector, coaxial power jack",
        "power connector barrel jack",
        "BarrelJack*", // real packed value
        3,
        None,
        None,
    );
    insert_symbol(
        &conn,
        connector_id,
        "Jumper_2_Open",
        "JP",
        "Jumper, 2-pole, open/populate",
        "jumper open",
        "Jumper* TestPoint*2Pads* TestPoint*Bridge*", // real packed value
        2,
        None,
        None,
    );

    // TokitoAI/tokito-mcp#106 review (P2a): real symbols that index a
    // row/column-shaped count *literally unpadded* — character LCDs and
    // keypad/LED matrices are named this way in the wild, unlike KiCad's
    // generic connector family. Padding "16x2" to "16x02" unconditionally
    // would silently lose these hits, so `normalize_query` must match both
    // forms via an OR group rather than rewriting in place.
    conn.execute("INSERT INTO lib(name) VALUES('Display_Character')", [])
        .unwrap();
    let display_char_id: i64 = conn.last_insert_rowid();
    conn.execute("INSERT INTO lib(name) VALUES('Switch')", [])
        .unwrap();
    let switch_id: i64 = conn.last_insert_rowid();
    conn.execute("INSERT INTO lib(name) VALUES('Display_LED')", [])
        .unwrap();
    let display_led_id: i64 = conn.last_insert_rowid();

    insert_symbol(
        &conn,
        display_char_id,
        "HD44780_16x2",
        "U",
        "16x2 character LCD display module, HD44780 controller",
        "lcd display 16x2 character",
        "LCD*16x2*",
        16,
        None,
        None,
    );
    insert_symbol(
        &conn,
        display_char_id,
        "HD44780_20x4",
        "U",
        "20x4 character LCD display module, HD44780 controller",
        "lcd display 20x4 character",
        "LCD*20x4*",
        18,
        None,
        None,
    );
    insert_symbol(
        &conn,
        switch_id,
        "Keypad_4x4",
        "SW",
        "4x4 matrix keypad, 16 keys",
        "keypad matrix 4x4",
        "Keypad*4x4*",
        8,
        None,
        None,
    );
    insert_symbol(
        &conn,
        display_led_id,
        "LED_Matrix_8x8",
        "DS",
        "8x8 LED dot matrix display, common cathode",
        "led matrix display 8x8",
        "LED*Matrix*8x8*",
        16,
        None,
        None,
    );

    conn
}

/// Insert a published generated-symbol revision into the fixture DB.
///
/// Uses the crate's own `generated::insert_revision` so the code path is the
/// same as what the packer will exercise. Returns the `PartId` used so callers
/// can round-trip lookups.
#[allow(dead_code)]
pub fn insert_generated_fixture(
    conn: &Connection,
    manufacturer: &str,
    mpn: &str,
    package: &str,
    lib: &str,
    name: &str,
    body: &SymbolBody,
) -> PartId {
    let part = PartId::new(manufacturer, mpn, package).unwrap();
    let body_bytes = postcard::to_stdvec(body).unwrap();
    let revision_id = format!("gen_sha256_{}", blake3_hex(&body_bytes));
    let content_hash = format!("sha256:{}", sha256_hex(&body_bytes));
    let provenance = serde_json::json!({
        "revision_id": revision_id,
        "part_id": {
            "manufacturer_norm": part.manufacturer_norm,
            "mpn": part.mpn,
            "package": part.package,
        },
        "library_id": { "lib": lib, "name": name },
        "evidence": {
            "datasheet_id": "fixture-datasheet",
            "content_sha256": "0".repeat(64),
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
        "content_hash": content_hash,
    });
    generated::insert_revision(
        conn,
        generated::NewRevision {
            revision_id: &revision_id,
            part: &part,
            lib,
            name,
            ref_des: "U",
            description: "fixture generated symbol",
            keywords: "fixture generated",
            fp_filters: "",
            datasheet: "https://example.test/ds.pdf",
            footprint: "",
            pin_count: body.pins.len() as u16,
            flags: 0,
            body: &body_bytes,
            body_format: BODY_FORMAT_POSTCARD_V1,
            symbol_text: "",
            provenance_json: &provenance.to_string(),
            status: PublicationStatus::Published,
            content_hash: &content_hash,
            published_at: "2026-08-08T07:15:00Z",
        },
    )
    .unwrap();
    part
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
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
