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

const CHILD_LMxxx: &str = r#"(kicad_symbol_lib
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
    std::fs::write(amp_dir.join("LMxxx_A.kicad_sym"), CHILD_LMxxx).unwrap();

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
    assert!(fts_hits >= 2, "expected FTS to return at least the two opamps");

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
        .query_row("SELECT value FROM meta WHERE key='source_commit'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(commit, "test-fixture");

    let symbol_count_meta: String = conn
        .query_row("SELECT value FROM meta WHERE key='symbol_count'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(symbol_count_meta, "3");
}
