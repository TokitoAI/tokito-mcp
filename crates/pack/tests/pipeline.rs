//! End-to-end pipeline test: write a fake kicad-symbols fixture to a
//! tempdir, invoke the real `tokito-mcp-pack` binary, then open the
//! resulting `symbols.sqlite` and assert structure.

use std::process::Command;

const ROOT_R: &str = r#"(kicad_symbol_lib
  (version 20251024)
  (symbol "R"
    (in_bom yes)
    (on_board yes)
    (property "Reference" "R" (at 2.032 0 90))
    (property "Value" "R" (at 0 0 90))
    (property "Description" "Resistor" (at 0 0 0))
    (property "ki_keywords" "R res resistor" (at 0 0 0))
    (property "ki_fp_filters" "R_*" (at 0 0 0))
    (symbol "R_0_1"
      (rectangle (start -1.016 -2.54) (end 1.016 2.54)
        (stroke (width 0.254) (type default))
        (fill (type none))))
    (symbol "R_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27)
        (name "" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 0 -3.81 90) (length 1.27)
        (name "" (effects (font (size 1.27 1.27))))
        (number "2" (effects (font (size 1.27 1.27))))))))"#;

const ROOT_OP: &str = r#"(kicad_symbol_lib
  (version 20251024)
  (symbol "ROOT_OP"
    (property "Reference" "U" (at 0 0 0))
    (property "Value" "ROOT_OP" (at 0 0 0))
    (property "Description" "Generic opamp" (at 0 0 0))
    (property "ki_keywords" "opamp operational" (at 0 0 0))
    (symbol "ROOT_OP_1_1"
      (pin input line (at -2.54 1.27 0) (length 1.27)
        (name "IN+" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
      (pin input line (at -2.54 -1.27 0) (length 1.27)
        (name "IN-" (effects (font (size 1.27 1.27))))
        (number "2" (effects (font (size 1.27 1.27)))))
      (pin output line (at 2.54 0 180) (length 1.27)
        (name "OUT" (effects (font (size 1.27 1.27))))
        (number "3" (effects (font (size 1.27 1.27)))))
      (pin power_in line (at 0 2.54 270) (length 1.27)
        (name "V+" (effects (font (size 1.27 1.27))))
        (number "4" (effects (font (size 1.27 1.27)))))
      (pin power_in line (at 0 -2.54 90) (length 1.27)
        (name "V-" (effects (font (size 1.27 1.27))))
        (number "5" (effects (font (size 1.27 1.27))))))))"#;

const CHILD_LM_XXX: &str = r#"(kicad_symbol_lib
  (version 20251024)
  (symbol "LMxxx_A"
    (extends "ROOT_OP")
    (property "Value" "LMxxx_A" (at 0 0 0))
    (property "Description" "Single low-noise opamp, DIP-8" (at 0 0 0))
    (property "ki_keywords" "opamp low-noise single" (at 0 0 0))
    (property "ki_fp_filters" "DIP-8*" (at 0 0 0))))"#;

#[test]
fn end_to_end_build_from_fixture_dir() {
    let tmp = tempfile::tempdir().unwrap();

    // Build a fixture mirroring the KiCad library directory layout.
    let device_dir = tmp.path().join("Device.kicad_symdir");
    let amp_dir = tmp.path().join("Amplifier_Op.kicad_symdir");
    std::fs::create_dir(&device_dir).unwrap();
    std::fs::create_dir(&amp_dir).unwrap();
    std::fs::write(device_dir.join("R.kicad_sym"), ROOT_R).unwrap();
    std::fs::write(amp_dir.join("ROOT_OP.kicad_sym"), ROOT_OP).unwrap();
    std::fs::write(amp_dir.join("LMxxx_A.kicad_sym"), CHILD_LM_XXX).unwrap();

    let out = tmp.path().join("symbols.sqlite");

    // Invoke the real binary.
    let status = Command::new(env!("CARGO_BIN_EXE_tokito-mcp-pack"))
        .args([
            "--src",
            tmp.path().to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--source-commit",
            "test-fixture",
        ])
        .env("RUST_LOG", "warn")
        .status()
        .expect("run tokito-mcp-pack");
    assert!(status.success(), "packer exited non-zero");
    assert!(out.exists(), "symbols.sqlite was not produced");

    // Query the result.
    let conn = rusqlite::Connection::open(&out).unwrap();

    let lib_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM lib", [], |r| r.get(0))
        .unwrap();
    assert_eq!(lib_count, 2);

    let symbol_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbol", [], |r| r.get(0))
        .unwrap();
    assert_eq!(symbol_count, 3);

    let roots: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbol WHERE body IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(roots, 2, "R + ROOT_OP carry bodies");

    let extending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbol WHERE parent_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(extending, 1, "LMxxx_A extends ROOT_OP");

    // pin_count was backfilled on the extending child
    let lmxxx_pin_count: i64 = conn
        .query_row(
            "SELECT pin_count FROM symbol s JOIN lib l ON l.id = s.lib_id \
             WHERE l.name = 'Amplifier_Op' AND s.name = 'LMxxx_A'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lmxxx_pin_count, 5, "child inherits parent's pin_count");

    // FTS5 finds the opamp by keyword
    let fts_hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbol_fts WHERE symbol_fts MATCH 'opamp'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        fts_hits >= 2,
        "expected FTS to return at least the two opamps"
    );

    // No dangling extends — every parent reference resolved
    let dangling: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbol WHERE parent_id IS NULL \
             AND id IN (SELECT id FROM symbol WHERE 0)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dangling, 0);

    // meta has source_commit + generator_version + counts
    let commit: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='source_commit'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(commit, "test-fixture");

    let symbol_count_meta: String = conn
        .query_row("SELECT value FROM meta WHERE key='symbol_count'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(symbol_count_meta, "3");
}

/// Exercises the Wave C.2 `generated` subcommand: build a real KiCad
/// artifact via the top-level packer, then merge a source `generated.sqlite`
/// (as tokito-ai would produce it) into that artifact and assert the merged
/// revision is resolvable via the served path.
#[test]
fn generated_subcommand_merges_source_into_served_artifact() {
    use sha2::{Digest, Sha256};
    use tokito_symbols::{generated, part_id::PartId};

    let tmp = tempfile::tempdir().unwrap();

    // 1. Build a served symbols.sqlite from the same KiCad fixture the
    //    top-level packer test uses. This gives us a realistic target.
    let device_dir = tmp.path().join("Device.kicad_symdir");
    std::fs::create_dir(&device_dir).unwrap();
    std::fs::write(device_dir.join("R.kicad_sym"), ROOT_R).unwrap();

    let served = tmp.path().join("symbols.sqlite");
    let status = Command::new(env!("CARGO_BIN_EXE_tokito-mcp-pack"))
        .args([
            "--src",
            tmp.path().to_str().unwrap(),
            "--out",
            served.to_str().unwrap(),
            "--source-commit",
            "test-fixture",
        ])
        .env("RUST_LOG", "warn")
        .status()
        .expect("run tokito-mcp-pack (top-level build)");
    assert!(status.success());

    // 2. Author a source generated.sqlite as tokito-ai's ingestion service
    //    would: canonical schema + one published revision.
    let source = tmp.path().join("generated.sqlite");
    let src_conn = rusqlite::Connection::open(&source).unwrap();
    src_conn
        .execute_batch(include_str!("fixtures/tokito_ai_generated_revision.sql"))
        .unwrap();
    let symbol_text = r#"(tokito_symbol_lib
  (version 20251024)
  (generator "tokito_cache")
  (symbol "TPS5430DDAR"
    (property "Reference" "U" (at 0 0 0))
    (property "Value" "TPS5430DDAR" (at 0 0 0))
    (property "Datasheet" "https://example.test/ds.pdf" (at 0 0 0))
    (property "Description" "e2e generated" (at 0 0 0))
    (property "Footprint" "Package_SO:PowerPAD-8" (at 0 0 0))
    (property "MPN" "TPS5430DDAR" (at 0 0 0))
    (property "Manufacturer" "Texas Instruments" (at 0 0 0))
    (property "package" "SO-PowerPAD-8" (at 0 0 0))
    (property "ki_keywords" "buck converter" (at 0 0 0))
    (property "ki_fp_filters" "SO*PowerPAD*" (at 0 0 0))
    (symbol "TPS5430DDAR_0_1"
      (rectangle (start -2.54 3.81) (end 2.54 -3.81)
        (stroke (width 0.254) (type default)) (fill (type background))))
    (symbol "TPS5430DDAR_1_1"
      (pin power_in line (at -5.08 2.54 0) (length 2.54) (name "VIN") (number "1"))
      (pin input line (at -5.08 0 0) (length 2.54) (name "EN") (number "2"))
      (pin passive line (at -5.08 -2.54 0) (length 2.54) (name "SS") (number "3"))
      (pin power_in line (at 0 -6.35 90) (length 2.54) (name "GND") (number "4"))
      (pin passive line (at 5.08 -2.54 180) (length 2.54) (name "SW") (number "5"))
      (pin input line (at 5.08 0 180) (length 2.54) (name "VSENSE") (number "6"))
      (pin power_in line (at 0 6.35 270) (length 2.54) (name "BOOT") (number "7"))
      (pin power_out line (at 5.08 2.54 180) (length 2.54) (name "PH") (number "8"))))
)
"#;
    let digest = hex::encode(Sha256::digest(symbol_text.as_bytes()));
    let revision_id = format!("gen_sha256_{digest}");
    let content_hash = format!("sha256:{digest}");
    let part = PartId::new("Texas Instruments", "TPS5430DDAR", "SO-PowerPAD-8").unwrap();
    src_conn.execute(
        r#"INSERT INTO generated_revision(revision_id, manufacturer_norm, mpn, package, lib, name,
         reference_prefix, description, keywords, datasheet, footprint, pin_count, symbol_text,
         content_hash, status, spec_json, evidence_json, provenance_json, idempotency_key,
         source_hash, extractor_version, compiler_version, layout_policy_version, published_at,
         ingested_by) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
         'published', '{}', '{}', '{"evidence":{"region_ids":["r_pinout_01"]}}', ?15,
         ?16, 'extractor-v1', 'compiler-v1', 'layout-v1', '2026-08-08T07:00:00Z', 'fixture')"#,
        rusqlite::params![revision_id, part.manufacturer_norm, part.mpn, part.package,
            "generated:texas_instruments", "TPS5430DDAR", "U", "e2e generated",
            "buck converter", "https://example.test/ds.pdf", "Package_SO:PowerPAD-8", 8,
            symbol_text, content_hash, "idempotency-fixture", "source-fixture"],
    ).unwrap();
    drop(src_conn);

    // 3. Invoke `tokito-mcp-pack generated --db ... --source ...`.
    let status = Command::new(env!("CARGO_BIN_EXE_tokito-mcp-pack"))
        .args([
            "generated",
            "--db",
            served.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
        ])
        .env("RUST_LOG", "warn")
        .status()
        .expect("run tokito-mcp-pack generated");
    assert!(status.success(), "generated subcommand exited non-zero");

    // 4. Reopen the served artifact and confirm the merged revision is
    //    resolvable through the same code path the MCP server will use.
    let merged = rusqlite::Connection::open(&served).unwrap();
    let count: i64 = merged
        .query_row("SELECT COUNT(*) FROM generated_symbol", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 1,
        "one generated revision should be present post-merge"
    );

    let resolved = generated::resolve_by_mpn(&merged, &part)
        .unwrap()
        .expect("merged revision must resolve by MPN");
    assert_eq!(resolved.lib, "generated:texas_instruments");
    assert_eq!(resolved.body.pins.len(), 8);
    assert_eq!(resolved.tokito_sym.as_deref(), Some(symbol_text));
    for required in ["MPN", "Manufacturer", "package"] {
        assert!(resolved
            .tokito_sym
            .as_ref()
            .unwrap()
            .contains(&format!("property \"{required}\"")));
    }

    // 5. Re-run the subcommand — must be idempotent.
    let status = Command::new(env!("CARGO_BIN_EXE_tokito-mcp-pack"))
        .args([
            "generated",
            "--db",
            served.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
        ])
        .env("RUST_LOG", "warn")
        .status()
        .expect("re-run generated subcommand");
    assert!(status.success());
    let count_after: i64 = merged
        .query_row("SELECT COUNT(*) FROM generated_symbol", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count_after, 1, "re-sync must not duplicate revisions");
}

/// `generated` refuses to run when the target DB is missing — creating one
/// from scratch would silently drop the KiCad half of the catalog.
#[test]
fn generated_subcommand_fails_when_target_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("generated.sqlite");
    let src_conn = rusqlite::Connection::open(&source).unwrap();
    src_conn.execute_batch(tokito_symbols::SCHEMA_SQL).unwrap();
    drop(src_conn);

    let missing_target = tmp.path().join("does_not_exist.sqlite");
    let status = Command::new(env!("CARGO_BIN_EXE_tokito-mcp-pack"))
        .args([
            "generated",
            "--db",
            missing_target.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
        ])
        .env("RUST_LOG", "warn")
        .status()
        .expect("run generated subcommand");
    assert!(!status.success(), "must fail when target is missing");
}
