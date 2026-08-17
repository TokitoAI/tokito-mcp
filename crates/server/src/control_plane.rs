//! Materialize a validated, immutable control-plane view for the existing pack compiler.

use std::path::Path;

use anyhow::Context;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use rusqlite::{params, Connection};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const SOURCE_SCHEMA: &str = r#"
CREATE TABLE generated_revision(
 revision_id TEXT PRIMARY KEY, manufacturer_norm TEXT NOT NULL, mpn TEXT NOT NULL,
 package TEXT NOT NULL, lib TEXT NOT NULL, name TEXT NOT NULL,
 reference_prefix TEXT NOT NULL, description TEXT NOT NULL, keywords TEXT NOT NULL,
 datasheet TEXT NOT NULL, footprint TEXT NOT NULL, pin_count INTEGER NOT NULL,
 symbol_text TEXT NOT NULL, content_hash TEXT NOT NULL, status TEXT NOT NULL,
 provenance_json TEXT NOT NULL, published_at TEXT NOT NULL
);
"#;

#[derive(Debug, Deserialize)]
struct Manifest {
    schema_version: String,
    items: Vec<ManifestItem>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestItem {
    revision_id: String,
    content_hash: String,
}

#[derive(Debug, Deserialize)]
struct Revision {
    revision_id: String,
    status: String,
    content_hash: String,
    part_id: PartId,
    library_id: LibraryId,
    reference_prefix: String,
    description: String,
    keywords: String,
    datasheet: String,
    footprint: String,
    pin_count: i64,
    published_at: String,
    symbol_tokito_sym: String,
    provenance: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct PartId {
    manufacturer_norm: String,
    mpn: String,
    package: String,
}

#[derive(Debug, Deserialize)]
struct LibraryId {
    lib: String,
    name: String,
}

pub async fn materialize(base_url: &str, bearer: &str, output: &Path) -> anyhow::Result<usize> {
    let client = reqwest::Client::builder()
        .https_only(base_url.starts_with("https://"))
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let parent = output
        .parent()
        .context("generated source has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut cursor = String::new();
    let mut all_revisions = Vec::new();
    loop {
        let manifest: Manifest = client
            .get(format!(
                "{}/v1/generated-manifest",
                base_url.trim_end_matches('/')
            ))
            .query(&[("after", cursor.as_str()), ("limit", "250")])
            .header(AUTHORIZATION, format!("Bearer {bearer}"))
            .header(
                USER_AGENT,
                concat!("tokito-mcp/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if manifest.schema_version != "tokito.generated-manifest.v1" {
            anyhow::bail!("unsupported generated manifest schema");
        }
        for item in &manifest.items {
            let revision: Revision = client
                .get(format!(
                    "{}/v1/generated/{}",
                    base_url.trim_end_matches('/'),
                    item.revision_id
                ))
                .header(AUTHORIZATION, format!("Bearer {bearer}"))
                .header(
                    USER_AGENT,
                    concat!("tokito-mcp/", env!("CARGO_PKG_VERSION")),
                )
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            validate(&revision, item)?;
            all_revisions.push(revision);
        }
        match manifest.next {
            Some(next) if !manifest.items.is_empty() && next != cursor => cursor = next,
            None => break,
            _ => anyhow::bail!("generated manifest pagination did not advance"),
        }
    }
    let staging = output.with_extension("sqlite.part");
    let _ = std::fs::remove_file(&staging);
    let mut database = Connection::open(&staging)?;
    database.execute_batch(SOURCE_SCHEMA)?;
    let transaction = database.transaction()?;
    for revision in &all_revisions {
        transaction.execute(
            "INSERT INTO generated_revision VALUES(
             ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                revision.revision_id,
                revision.part_id.manufacturer_norm,
                revision.part_id.mpn,
                revision.part_id.package,
                revision.library_id.lib,
                revision.library_id.name,
                revision.reference_prefix,
                revision.description,
                revision.keywords,
                revision.datasheet,
                revision.footprint,
                revision.pin_count,
                revision.symbol_tokito_sym,
                revision.content_hash,
                revision.status,
                serde_json::to_string(&revision.provenance)?,
                revision.published_at,
            ],
        )?;
    }
    transaction.commit()?;
    database.execute_batch("PRAGMA optimize; VACUUM;")?;
    drop(database);
    replace(&staging, output)?;
    Ok(all_revisions.len())
}

fn replace(staging: &Path, output: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    return std::fs::rename(staging, output);
    #[cfg(windows)]
    {
        let previous = output.with_extension("sqlite.previous");
        let _ = std::fs::remove_file(&previous);
        if output.exists() {
            std::fs::rename(output, &previous)?;
        }
        if let Err(error) = std::fs::rename(staging, output) {
            let _ = std::fs::rename(&previous, output);
            return Err(error);
        }
        let _ = std::fs::remove_file(previous);
        Ok(())
    }
}

fn validate(revision: &Revision, item: &ManifestItem) -> anyhow::Result<()> {
    if revision.revision_id != item.revision_id
        || revision.content_hash != item.content_hash
        || revision.status != "published"
    {
        anyhow::bail!("control-plane revision metadata mismatch");
    }
    let digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(revision.symbol_tokito_sym.as_bytes()))
    );
    if digest != revision.content_hash {
        anyhow::bail!("control-plane revision content hash mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> Revision {
        let symbol = "(symbol \"TEST\")\n".to_string();
        let hash = format!("sha256:{}", hex::encode(Sha256::digest(symbol.as_bytes())));
        Revision {
            revision_id: "gen_sha256_test".into(),
            status: "published".into(),
            content_hash: hash,
            part_id: PartId {
                manufacturer_norm: "test".into(),
                mpn: "TEST".into(),
                package: "DIP-8".into(),
            },
            library_id: LibraryId {
                lib: "generated:test".into(),
                name: "TEST".into(),
            },
            reference_prefix: "U".into(),
            description: String::new(),
            keywords: String::new(),
            datasheet: String::new(),
            footprint: String::new(),
            pin_count: 8,
            published_at: "2026-08-17T00:00:00Z".into(),
            symbol_tokito_sym: symbol,
            provenance: serde_json::json!({"status":"published"}),
        }
    }

    #[test]
    fn revision_must_match_manifest_and_bytes() {
        let mut revision = revision();
        let item = ManifestItem {
            revision_id: revision.revision_id.clone(),
            content_hash: revision.content_hash.clone(),
        };
        assert!(validate(&revision, &item).is_ok());
        revision.symbol_tokito_sym.push_str("drift");
        assert!(validate(&revision, &item).is_err());
    }
}
