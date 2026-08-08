//! Tests for `generated::sync_from` — the offline merge path used by
//! `tokito-mcp-pack generated` to pull tokito-ai's `generated.sqlite`
//! into the served `symbols.sqlite`.

mod common;

use rusqlite::Connection;
use tempfile::TempDir;
use tokito_symbols::{
    generated,
    model::{Pin, PinElectrical, PinStyle, PublicationStatus, SymbolBody, SymbolFlags},
    part_id::PartId,
    BODY_FORMAT_POSTCARD_V1, SCHEMA_SQL,
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

/// Build a file-backed source DB with the canonical schema and one published
/// generated revision. Returns the tempdir (keep it alive) and the source path.
fn source_db_with_one_published(mpn: &str, pin_count: u8) -> (TempDir, std::path::PathBuf, PartId) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    let body = tiny_body(pin_count);
    let part = common::insert_generated_fixture(
        &conn,
        "Texas Instruments",
        mpn,
        "SO-PowerPAD-8",
        "generated:texas_instruments",
        mpn,
        &body,
    );
    drop(conn);
    (dir, path, part)
}

#[test]
fn sync_from_copies_revisions_into_empty_target() {
    let (_dir, source_path, part) = source_db_with_one_published("TPS5430DDAR", 8);

    let target = common::fixture_db();
    let merged = generated::sync_from(&target, &source_path).unwrap();
    assert_eq!(merged, 1);

    let resolved = generated::resolve_by_mpn(&target, &part)
        .unwrap()
        .expect("merged revision resolves in the target");
    assert_eq!(resolved.lib, "generated:texas_instruments");
    assert_eq!(resolved.body.pins.len(), 8);
}

#[test]
fn sync_from_is_idempotent_when_run_twice() {
    let (_dir, source_path, _part) = source_db_with_one_published("TPS5430DDAR", 8);
    let target = common::fixture_db();

    let first = generated::sync_from(&target, &source_path).unwrap();
    let second = generated::sync_from(&target, &source_path).unwrap();
    assert_eq!(first, 1);
    assert_eq!(
        second, 1,
        "sync_from re-inserts the same revision id — insert_revision is idempotent"
    );

    let count: i64 = target
        .query_row("SELECT COUNT(*) FROM generated_symbol", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "no duplicate rows after re-sync");
}

#[test]
fn sync_from_rejects_source_with_conflicting_body_for_existing_revision() {
    // Simulates a corrupted or forked source that reuses a revision id for a
    // different body. This must fail loudly rather than silently overwrite.
    let target = common::fixture_db();
    let part = PartId::new("Acme", "A-1", "SOIC-8").unwrap();
    let body_a = postcard::to_stdvec(&tiny_body(2)).unwrap();
    generated::insert_revision(
        &target,
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

    // Build a source with the same revision_id but a different body.
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("generated.sqlite");
    let source = Connection::open(&source_path).unwrap();
    source.execute_batch(SCHEMA_SQL).unwrap();
    let body_b = postcard::to_stdvec(&tiny_body(3)).unwrap();
    generated::insert_revision(
        &source,
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
    .unwrap();
    drop(source);

    let err = generated::sync_from(&target, &source_path).unwrap_err();
    assert!(
        matches!(err, tokito_symbols::Error::RevisionBodyMismatch { .. }),
        "expected RevisionBodyMismatch, got {err:?}"
    );
}

#[test]
fn sync_from_rejects_unknown_status_string() {
    // Defensive path: if a future or forked schema.sql widens the allowed
    // status set, sync_from must fail loudly rather than silently coerce.
    // The canonical schema enforces the same set via a CHECK, so we build
    // the source with the CHECK dropped to simulate a status this build
    // does not know about.
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("generated.sqlite");
    let source = Connection::open(&source_path).unwrap();
    source.execute_batch(SCHEMA_SQL).unwrap();

    let part = PartId::new("Acme", "A-1", "SOIC-8").unwrap();
    let body = postcard::to_stdvec(&tiny_body(2)).unwrap();
    generated::insert_revision(
        &source,
        generated::NewRevision {
            revision_id: "gen_sha256_alien",
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
            body: &body,
            body_format: BODY_FORMAT_POSTCARD_V1,
            provenance_json: "{}",
            status: PublicationStatus::Published,
            content_hash: "sha256:aa",
            published_at: "2026-08-08T07:00:00Z",
        },
    )
    .unwrap();
    // Bypass the CHECK constraint on `status` by rewriting the column value
    // through sqlite_master. This mirrors a source built against a future
    // schema that admits new status strings we haven't taught this build yet.
    source
        .pragma_update(None, "writable_schema", true)
        .unwrap();
    source
        .execute(
            "UPDATE sqlite_master SET sql = replace(sql, \
                  'status IN (''draft'',''validating'',''verified'',''published'',''superseded'',''quarantined'')', \
                  'status IS NOT NULL') \
             WHERE type = 'table' AND name = 'generated_symbol'",
            [],
        )
        .unwrap();
    source
        .pragma_update(None, "writable_schema", false)
        .unwrap();
    drop(source);

    // Reopen so sqlite re-parses the schema, then rewrite the row.
    let source = Connection::open(&source_path).unwrap();
    source
        .execute(
            "UPDATE generated_symbol SET status = 'martian' WHERE revision_id = 'gen_sha256_alien'",
            [],
        )
        .unwrap();
    drop(source);

    let target = common::fixture_db();
    let err = generated::sync_from(&target, &source_path).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("martian"),
        "error should quote the offending status, got: {msg}"
    );
}

#[test]
fn sync_from_errors_when_source_file_missing() {
    let target = common::fixture_db();
    let err = generated::sync_from(&target, std::path::Path::new("/nonexistent/generated.sqlite"))
        .unwrap_err();
    assert!(
        matches!(err, tokito_symbols::Error::Sql(_)),
        "expected sqlite open error, got {err:?}"
    );
}

#[test]
fn sync_from_preserves_source_ordering_stability() {
    // Two revisions in the source; both must land in the target and be
    // resolvable independently. Guards against a bug where the row iterator
    // silently drops rows after the first.
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("generated.sqlite");
    let source = Connection::open(&source_path).unwrap();
    source.execute_batch(SCHEMA_SQL).unwrap();
    let part_a = common::insert_generated_fixture(
        &source,
        "Texas Instruments",
        "TPS5430DDAR",
        "SO-PowerPAD-8",
        "generated:texas_instruments",
        "TPS5430DDAR",
        &tiny_body(8),
    );
    let part_b = common::insert_generated_fixture(
        &source,
        "STMicroelectronics",
        "STM32F103C8T6",
        "LQFP48",
        "generated:stmicroelectronics",
        "STM32F103C8T6",
        &tiny_body(48),
    );
    drop(source);

    let target = common::fixture_db();
    let merged = generated::sync_from(&target, &source_path).unwrap();
    assert_eq!(merged, 2);

    assert!(generated::resolve_by_mpn(&target, &part_a).unwrap().is_some());
    assert!(generated::resolve_by_mpn(&target, &part_b).unwrap().is_some());
}
