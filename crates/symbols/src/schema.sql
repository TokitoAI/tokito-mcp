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

-- Capability search (hosted artifact only) — populated by the builder when
-- sqlite-vec is available. Desktop slim catalog ships without this table.
CREATE TABLE IF NOT EXISTS symbol_embedding (
    symbol_id   INTEGER PRIMARY KEY REFERENCES symbol(id),
    model       TEXT NOT NULL,
    vec         BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL
);
