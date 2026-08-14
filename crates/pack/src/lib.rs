//! Validated catalog-pack construction shared by the offline CLI and server.

use std::path::{Path, PathBuf};

#[allow(dead_code)]
mod emit;
mod generated_source;
#[allow(dead_code)]
mod ingest;
mod kicad;
mod sexpr;

/// Build a fresh served catalog from an immutable official base plus the
/// writer-side Tokito Cloud ingestion database.
pub fn publish_generated_pack(base: &Path, source: &Path, output: &Path) -> anyhow::Result<usize> {
    if !base.is_file() {
        anyhow::bail!("official catalog {base:?} is not a regular file");
    }
    if !source.is_file() {
        anyhow::bail!("ingestion database {source:?} is not a regular file");
    }
    let parent = output
        .parent()
        .ok_or_else(|| anyhow::anyhow!("generated pack output has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = temporary_path(output);
    let result = (|| {
        std::fs::copy(base, &temporary)?;
        let mut conn = rusqlite::Connection::open(&temporary)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.execute_batch(tokito_symbols::SCHEMA_SQL)?;
        let tx = conn.transaction()?;
        let inserted = generated_source::sync_from_ingestion(&tx, source)?;
        tx.commit()?;
        let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            anyhow::bail!("generated pack integrity check failed: {integrity}");
        }
        conn.execute_batch("VACUUM;")?;
        drop(conn);
        tokito_symbols::db::open_read_only(&temporary)?;
        replace_file(&temporary, output)?;
        Ok(inserted)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    output.with_file_name(name)
}

#[cfg(unix)]
fn replace_file(source: &Path, output: &Path) -> std::io::Result<()> {
    std::fs::rename(source, output)
}

#[cfg(windows)]
fn replace_file(source: &Path, output: &Path) -> std::io::Result<()> {
    if output.exists() {
        std::fs::remove_file(output)?;
    }
    std::fs::rename(source, output)
}

#[cfg(test)]
mod tests {
    use super::publish_generated_pack;

    #[test]
    fn invalid_source_preserves_last_known_good_pack() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("base.sqlite");
        let source = root.path().join("writer.sqlite");
        let output = root.path().join("generated-pack.sqlite");
        tokito_symbols::db::open_for_build(&base).unwrap();
        rusqlite::Connection::open(&source).unwrap();
        std::fs::write(&output, b"last-known-good").unwrap();

        assert!(publish_generated_pack(&base, &source, &output).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"last-known-good");
    }
}
