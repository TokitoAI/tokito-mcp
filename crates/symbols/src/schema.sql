-- Canonical schema for symbols.sqlite — owned by the tokito-symbols crate.
-- Bump `meta.schema_version` and update MIN_COMPATIBLE_VERSION/CURRENT_VERSION
-- in lib.rs on any structural change.

PRAGMA page_size = 8192;
PRAGMA journal_mode = OFF;        -- read-only artifact at runtime; builder runs offline
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS lib (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS symbol (
    id              INTEGER PRIMARY KEY,
    lib_id          INTEGER NOT NULL REFERENCES lib(id),
    name            TEXT    NOT NULL,
    ref_des         TEXT    NOT NULL DEFAULT '',
    description     TEXT    NOT NULL DEFAULT '',
    keywords        TEXT    NOT NULL DEFAULT '',
    fp_filters      TEXT    NOT NULL DEFAULT '',
    datasheet       TEXT    NOT NULL DEFAULT '',
    footprint       TEXT    NOT NULL DEFAULT '',
    parent_id       INTEGER REFERENCES symbol(id),  -- NULL = root symbol
    pin_count       INTEGER NOT NULL DEFAULT 0,
    flags           INTEGER NOT NULL DEFAULT 0,
    body            BLOB,                            -- NULL when parent_id IS NOT NULL
    body_format     TEXT,                            -- 'postcard-v1' when body is set
    UNIQUE(lib_id, name)
);

CREATE INDEX IF NOT EXISTS idx_symbol_parent ON symbol(parent_id);

-- FTS5 over the searchable text fields. Triggers keep it in sync (used by tests
-- and the builder's incremental updates; the shipped artifact is built once).
CREATE VIRTUAL TABLE IF NOT EXISTS symbol_fts USING fts5(
    name, description, keywords, fp_filters,
    content='symbol',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS symbol_ai AFTER INSERT ON symbol BEGIN
    INSERT INTO symbol_fts(rowid, name, description, keywords, fp_filters)
    VALUES (new.id, new.name, new.description, new.keywords, new.fp_filters);
END;
CREATE TRIGGER IF NOT EXISTS symbol_ad AFTER DELETE ON symbol BEGIN
    INSERT INTO symbol_fts(symbol_fts, rowid, name, description, keywords, fp_filters)
    VALUES ('delete', old.id, old.name, old.description, old.keywords, old.fp_filters);
END;
CREATE TRIGGER IF NOT EXISTS symbol_au AFTER UPDATE ON symbol BEGIN
    INSERT INTO symbol_fts(symbol_fts, rowid, name, description, keywords, fp_filters)
    VALUES ('delete', old.id, old.name, old.description, old.keywords, old.fp_filters);
    INSERT INTO symbol_fts(rowid, name, description, keywords, fp_filters)
    VALUES (new.id, new.name, new.description, new.keywords, new.fp_filters);
END;

-- Capability (semantic) search — reserved for a future hosted sqlite-vec
-- index. Not yet populated by the builder and read by no consumer today;
-- the table ships empty (negligible size) until that lands.
CREATE TABLE IF NOT EXISTS symbol_embedding (
    symbol_id   INTEGER PRIMARY KEY REFERENCES symbol(id),
    model       TEXT NOT NULL,
    vec         BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- Generated symbols
-- ---------------------------------------------------------------------------
-- Produced from DS-ViRe evidence via the tokito-ai symbol-extractor and the
-- tokito-catalog symbol compiler; sync'd in by `tokito-mcp-pack --generated`.
-- Read-only at runtime like `symbol`. Writes only happen offline in the packer
-- so the MCP read surface never becomes an unauthenticated write path.
--
-- Layout mirrors `symbol` (same catalog columns + body/body_format) so the
-- existing resolver and search-result mappers keep a single row shape. Extra
-- columns track revision identity, publication lifecycle, and provenance.

CREATE TABLE IF NOT EXISTS part_registry (
    part_id            TEXT PRIMARY KEY,        -- "<manufacturer_norm>|<mpn>|<package>"
    manufacturer_norm  TEXT NOT NULL,           -- NFC + lowercase + whitespace-collapsed
    mpn                TEXT NOT NULL,
    package            TEXT NOT NULL,
    UNIQUE(manufacturer_norm, mpn, package)
);

CREATE INDEX IF NOT EXISTS idx_part_registry_mpn ON part_registry(mpn);

CREATE TABLE IF NOT EXISTS generated_symbol (
    id                 INTEGER PRIMARY KEY,
    revision_id        TEXT    NOT NULL UNIQUE,          -- "gen_sha256_<hex>"
    part_id            TEXT    NOT NULL REFERENCES part_registry(part_id),
    lib_id             INTEGER NOT NULL REFERENCES lib(id),
    name               TEXT    NOT NULL,
    ref_des            TEXT    NOT NULL DEFAULT '',
    description        TEXT    NOT NULL DEFAULT '',
    keywords           TEXT    NOT NULL DEFAULT '',
    fp_filters         TEXT    NOT NULL DEFAULT '',
    datasheet          TEXT    NOT NULL DEFAULT '',
    footprint          TEXT    NOT NULL DEFAULT '',
    pin_count          INTEGER NOT NULL DEFAULT 0,
    flags              INTEGER NOT NULL DEFAULT 0,
    body               BLOB    NOT NULL,                 -- generated symbols never extend
    body_format        TEXT    NOT NULL,                 -- 'postcard-v1'
    symbol_text         TEXT    NOT NULL DEFAULT '',      -- exact compiler-emitted .tokito_sym bytes
    provenance_json    TEXT    NOT NULL,                 -- CONTRACTS.md §5 record
    status             TEXT    NOT NULL
                       CHECK (status IN ('draft','validating','verified','published','superseded','quarantined')),
    content_hash       TEXT    NOT NULL,                 -- 'sha256:<hex>' of canonical .tokito_sym bytes
    published_at       TEXT    NOT NULL                  -- ISO8601 UTC, e.g. '2026-08-08T07:15:00Z'
);

CREATE INDEX IF NOT EXISTS idx_generated_part_status ON generated_symbol(part_id, status, published_at DESC);
CREATE INDEX IF NOT EXISTS idx_generated_lib_name ON generated_symbol(lib_id, name);
CREATE INDEX IF NOT EXISTS idx_generated_content_hash ON generated_symbol(content_hash);

-- FTS5 mirror for generated symbols. Kept as a separate virtual table so the
-- two catalogs UNION ALL at query time without either mirror needing to know
-- about the other's rowids.
CREATE VIRTUAL TABLE IF NOT EXISTS generated_symbol_fts USING fts5(
    name, description, keywords, fp_filters,
    content='generated_symbol',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS generated_symbol_ai AFTER INSERT ON generated_symbol BEGIN
    INSERT INTO generated_symbol_fts(rowid, name, description, keywords, fp_filters)
    VALUES (new.id, new.name, new.description, new.keywords, new.fp_filters);
END;
CREATE TRIGGER IF NOT EXISTS generated_symbol_ad AFTER DELETE ON generated_symbol BEGIN
    INSERT INTO generated_symbol_fts(generated_symbol_fts, rowid, name, description, keywords, fp_filters)
    VALUES ('delete', old.id, old.name, old.description, old.keywords, old.fp_filters);
END;
CREATE TRIGGER IF NOT EXISTS generated_symbol_au AFTER UPDATE ON generated_symbol BEGIN
    INSERT INTO generated_symbol_fts(generated_symbol_fts, rowid, name, description, keywords, fp_filters)
    VALUES ('delete', old.id, old.name, old.description, old.keywords, old.fp_filters);
    INSERT INTO generated_symbol_fts(rowid, name, description, keywords, fp_filters)
    VALUES (new.id, new.name, new.description, new.keywords, new.fp_filters);
END;
