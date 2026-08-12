//! Import the real `tokito-ai` generated-revision database into the served
//! catalog. The producer stores canonical `.tokito_sym` text; this importer
//! parses and validates that artifact once, then stores both its compact body
//! and its exact bytes so downstream clients never reconstruct a lossy copy.

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use tokito_symbols::{
    generated::{self, NewRevision},
    model::PublicationStatus,
    part_id::{normalize_manufacturer, PartId},
    BODY_FORMAT_POSTCARD_V1,
};

use crate::{emit, kicad, sexpr};

const MAX_SYMBOL_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROVENANCE_BYTES: usize = 256 * 1024;

pub(crate) fn sync_from_ingestion(
    target: &Connection,
    source: &std::path::Path,
) -> anyhow::Result<usize> {
    ensure_symbol_text_column(target)?;
    let src = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let has_real_schema = src
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='generated_revision'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_real_schema {
        anyhow::bail!(
            "source is not a tokito-ai generated.sqlite: generated_revision table missing"
        );
    }

    let mut statement = src.prepare(
        "SELECT revision_id, manufacturer_norm, mpn, package, lib, name, \
                reference_prefix, description, keywords, datasheet, footprint, pin_count, \
                symbol_text, content_hash, status, provenance_json, published_at \
           FROM generated_revision ORDER BY published_at, revision_id",
    )?;
    let rows = statement.query_map([], SourceRevision::from_row)?;
    let mut inserted = 0usize;
    for row in rows {
        let row = row?;
        let prepared = row.validate_and_prepare()?;
        let existed = target
            .query_row(
                "SELECT 1 FROM generated_symbol WHERE revision_id=?1",
                [&row.revision_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        generated::insert_revision(target, prepared.as_revision())?;
        if !existed {
            inserted += 1;
        }
    }
    Ok(inserted)
}

fn ensure_symbol_text_column(conn: &Connection) -> rusqlite::Result<()> {
    let present = conn
        .prepare("PRAGMA table_info(generated_symbol)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "symbol_text");
    if !present {
        conn.execute(
            "ALTER TABLE generated_symbol ADD COLUMN symbol_text TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

struct SourceRevision {
    revision_id: String,
    manufacturer_norm: String,
    mpn: String,
    package: String,
    lib: String,
    name: String,
    reference_prefix: String,
    description: String,
    keywords: String,
    datasheet: String,
    footprint: String,
    pin_count: i64,
    symbol_text: String,
    content_hash: String,
    status: String,
    provenance_json: String,
    published_at: String,
}

impl SourceRevision {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            revision_id: row.get(0)?,
            manufacturer_norm: row.get(1)?,
            mpn: row.get(2)?,
            package: row.get(3)?,
            lib: row.get(4)?,
            name: row.get(5)?,
            reference_prefix: row.get(6)?,
            description: row.get(7)?,
            keywords: row.get(8)?,
            datasheet: row.get(9)?,
            footprint: row.get(10)?,
            pin_count: row.get(11)?,
            symbol_text: row.get(12)?,
            content_hash: row.get(13)?,
            status: row.get(14)?,
            provenance_json: row.get(15)?,
            published_at: row.get(16)?,
        })
    }

    fn validate_and_prepare(&self) -> anyhow::Result<PreparedRevision<'_>> {
        if self.symbol_text.is_empty() || self.symbol_text.len() > MAX_SYMBOL_TEXT_BYTES {
            anyhow::bail!(
                "revision {} has invalid symbol_text size {}",
                self.revision_id,
                self.symbol_text.len()
            );
        }
        let actual_hash = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(self.symbol_text.as_bytes()))
        );
        if actual_hash != self.content_hash {
            anyhow::bail!("revision {} content hash mismatch", self.revision_id);
        }
        if self.provenance_json.len() > MAX_PROVENANCE_BYTES
            || !serde_json::from_str::<serde_json::Value>(&self.provenance_json)?.is_object()
        {
            anyhow::bail!("revision {} has invalid provenance JSON", self.revision_id);
        }
        let status = PublicationStatus::from_str(&self.status).ok_or_else(|| {
            anyhow::anyhow!(
                "revision {} has invalid status {:?}",
                self.revision_id,
                self.status
            )
        })?;
        let part = PartId::new(&self.manufacturer_norm, &self.mpn, &self.package)?;
        if part.manufacturer_norm != self.manufacturer_norm || !self.lib.starts_with("generated:") {
            anyhow::bail!("revision {} has non-canonical identity", self.revision_id);
        }
        if !(0..=u16::MAX as i64).contains(&self.pin_count) {
            anyhow::bail!(
                "revision {} has invalid pin count {}",
                self.revision_id,
                self.pin_count
            );
        }

        let tree = sexpr::parse(&self.symbol_text)?;
        let mut symbols = kicad::extract_lib(&tree)?;
        if symbols.len() != 1 {
            anyhow::bail!(
                "revision {} must contain exactly one symbol",
                self.revision_id
            );
        }
        let parsed = symbols.pop().expect("length checked");
        if parsed.name != self.name || parsed.pins.len() != self.pin_count as usize {
            anyhow::bail!(
                "revision {} symbol identity or pin count drift",
                self.revision_id
            );
        }
        let property = |key: &str| {
            parsed
                .properties
                .get(key)
                .map(|p| p.value.as_str())
                .unwrap_or("")
        };
        let identities_match = property("Value") == self.name
            && property("MPN") == self.mpn
            && normalize_manufacturer(property("Manufacturer")) == self.manufacturer_norm
            && property("package") == self.package
            && property("Reference") == self.reference_prefix
            && property("Datasheet") == self.datasheet
            && property("Description") == self.description
            && property("Footprint") == self.footprint;
        if !identities_match {
            anyhow::bail!(
                "revision {} denormalized fields disagree with canonical symbol properties",
                self.revision_id
            );
        }

        let body = emit::build_body(&parsed);
        let body_bytes = postcard::to_stdvec(&body)?;
        let flags = emit::pack_flags(&parsed.flags) as u32;
        Ok(PreparedRevision {
            source: self,
            part,
            body_bytes,
            fp_filters: property("ki_fp_filters").to_string(),
            flags,
            status,
        })
    }
}

struct PreparedRevision<'a> {
    source: &'a SourceRevision,
    part: PartId,
    body_bytes: Vec<u8>,
    fp_filters: String,
    flags: u32,
    status: PublicationStatus,
}

impl PreparedRevision<'_> {
    fn as_revision(&self) -> NewRevision<'_> {
        NewRevision {
            revision_id: &self.source.revision_id,
            part: &self.part,
            lib: &self.source.lib,
            name: &self.source.name,
            ref_des: &self.source.reference_prefix,
            description: &self.source.description,
            keywords: &self.source.keywords,
            fp_filters: &self.fp_filters,
            datasheet: &self.source.datasheet,
            footprint: &self.source.footprint,
            pin_count: self.source.pin_count as u16,
            flags: self.flags,
            body: &self.body_bytes,
            body_format: BODY_FORMAT_POSTCARD_V1,
            symbol_text: &self.source.symbol_text,
            provenance_json: &self.source.provenance_json,
            status: self.status,
            content_hash: &self.source.content_hash,
            published_at: &self.source.published_at,
        }
    }
}
