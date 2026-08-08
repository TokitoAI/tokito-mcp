//! tokito-symbols — shared crate for the KiCad symbol catalog.
//!
//! Storage: SQLite (+ FTS5) for catalog and search; postcard-encoded BLOBs
//! for symbol bodies. Extends children carry NULL body — the resolver walks
//! `parent_id` until a non-NULL body is found, then overlays the child's
//! property columns onto the parent's body.

pub mod db;
pub mod generated;
pub mod model;
pub mod part_id;
pub mod resolver;
pub mod search;

/// Embedded canonical schema. The builder runs this on a fresh database;
/// the server expects it to already exist.
pub const SCHEMA_SQL: &str = include_str!("schema.sql");

/// Bump on any breaking schema or body-format change. Version 2 introduces
/// the `part_registry` and `generated_symbol` tables plus their FTS mirror.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Oldest schema version this build still understands. v1 artifacts are
/// missing the generated-symbol tables; the server still accepts them and
/// the generated-symbol code paths return "not found" until a v2 pack is
/// deployed.
pub const MIN_COMPATIBLE_VERSION: u32 = 1;

/// Maximum `extends` chain depth the resolver will walk before giving up.
/// Max observed depth in the CERN library is 4; this is 2× headroom.
pub const MAX_EXTENDS_DEPTH: u32 = 8;

/// Current body-blob format tag. Stored in `symbol.body_format`.
pub const BODY_FORMAT_POSTCARD_V1: &str = "postcard-v1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("postcard decode: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("provenance json decode: {0}")]
    ProvenanceJson(String),
    #[error("schema version mismatch: artifact={artifact} supported={min}..={current}")]
    SchemaVersionMismatch {
        artifact: u32,
        min: u32,
        current: u32,
    },
    #[error("symbol {lib:?}:{name:?} not found")]
    SymbolNotFound { lib: String, name: String },
    #[error("extends chain exceeds depth cap of {0}")]
    ExtendsDepthExceeded(u32),
    #[error("body has unknown format tag: {0:?}")]
    UnknownBodyFormat(String),
    #[error("revision {revision_id:?} already exists with a different body — revision ids are immutable")]
    RevisionBodyMismatch { revision_id: String },
}

impl Error {
    pub(crate) fn from_json(e: serde_json::Error) -> Self {
        Self::ProvenanceJson(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
