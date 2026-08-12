-- Contract fixture mirroring the immutable producer table in
-- TokitoAI/tokito-ai migrations/generated_0001_init.sql. Keep every column the
-- MCP importer reads plus the producer's idempotency and immutability rules.
CREATE TABLE generated_revision (
    revision_id            TEXT PRIMARY KEY,
    manufacturer_norm      TEXT NOT NULL,
    mpn                    TEXT NOT NULL,
    package                TEXT NOT NULL,
    lib                    TEXT NOT NULL,
    name                   TEXT NOT NULL,
    reference_prefix       TEXT NOT NULL,
    description            TEXT NOT NULL DEFAULT '',
    keywords               TEXT NOT NULL DEFAULT '',
    datasheet              TEXT NOT NULL DEFAULT '',
    footprint              TEXT NOT NULL DEFAULT '',
    pin_count              INTEGER NOT NULL,
    symbol_text            TEXT NOT NULL,
    content_hash           TEXT NOT NULL,
    status                 TEXT NOT NULL CHECK (status IN (
                               'draft','validating','verified','published',
                               'superseded','quarantined')),
    spec_json              TEXT NOT NULL,
    evidence_json          TEXT NOT NULL,
    provenance_json        TEXT NOT NULL,
    idempotency_key        TEXT NOT NULL UNIQUE,
    source_hash            TEXT NOT NULL,
    extractor_version      TEXT NOT NULL,
    compiler_version       TEXT NOT NULL,
    layout_policy_version  TEXT NOT NULL,
    published_at           TEXT NOT NULL,
    ingested_by            TEXT NOT NULL
);

CREATE TRIGGER generated_revision_no_update
BEFORE UPDATE ON generated_revision BEGIN
    SELECT RAISE(ABORT, 'generated_revision rows are immutable');
END;

CREATE TRIGGER generated_revision_no_delete
BEFORE DELETE ON generated_revision BEGIN
    SELECT RAISE(ABORT, 'generated_revision rows are immutable');
END;
