use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{bail, Context};
use rusqlite::{params, Connection};
use sha2::Digest;
use tokito_symbols::footprints::{FootprintId, MAX_SOURCE_BYTES};

pub fn pack(conn: &mut Connection, root: &Path, revision: &str) -> anyhow::Result<usize> {
    fn git(root: &Path, args: &[&str]) -> anyhow::Result<String> {
        let output = Command::new("git").current_dir(root).args(args).output()?;
        if !output.status.success() {
            bail!(
                "cannot verify footprint checkout: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(String::from_utf8(output.stdout)?)
    }
    if revision.len() != 40 || git(root, &["rev-parse", "HEAD"])?.trim() != revision {
        bail!("footprint checkout does not match the pinned commit");
    }
    // Read only committed regular blobs. Ignored or untracked files, symlinks
    // and working-tree edits must never be published under the pinned commit.
    let listing = git(
        root,
        &["-c", "core.quotePath=false", "ls-tree", "-rz", "HEAD"],
    )?;
    let tx = conn.transaction()?;
    let mut child = Command::new("git")
        .current_dir(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().context("missing Git batch input")?;
    let mut output = BufReader::new(child.stdout.take().context("missing Git batch output")?);
    let result = (|| -> anyhow::Result<usize> {
        let mut insert = tx.prepare("INSERT INTO footprint_source(library, name, revision, source, sha256) VALUES(?1, ?2, ?3, ?4, ?5)")?;
        let mut count = 0;
        for entry in listing.split_terminator('\0') {
            let (metadata, path) = entry.split_once('\t').context("invalid Git tree record")?;
            let Some((library, name)) = path.split_once('/') else {
                continue;
            };
            let (Some(library), Some(name)) = (
                library.strip_suffix(".pretty"),
                name.strip_suffix(".kicad_mod"),
            ) else {
                continue;
            };
            if name.contains('/') {
                continue;
            }
            let id = FootprintId::new(library, name)
                .with_context(|| format!("unsupported footprint ID: {path}"))?;
            if !metadata.starts_with("100644 blob ") && !metadata.starts_with("100755 blob ") {
                bail!("footprint is not a regular committed file: {path}");
            }
            let expected = metadata.rsplit_once(' ').context("missing blob hash")?.1;
            writeln!(input, "{expected}")?;
            let mut header = String::new();
            output.read_line(&mut header)?;
            let length: usize = header
                .trim_end()
                .strip_prefix(expected)
                .and_then(|rest| rest.strip_prefix(" blob "))
                .context("Git did not return the requested footprint blob")?
                .parse()?;
            if !(1..=MAX_SOURCE_BYTES).contains(&length) {
                bail!("invalid footprint size: {path}");
            }
            let mut source = vec![0; length];
            output.read_exact(&mut source)?;
            let mut delimiter = [0];
            output.read_exact(&mut delimiter)?;
            if delimiter != [b'\n'] {
                bail!("invalid Git batch delimiter");
            }
            let digest = hex::encode(sha2::Sha256::digest(&source));
            insert.execute(params![
                id.library,
                id.name,
                revision,
                String::from_utf8(source)?,
                digest
            ])?;
            count += 1;
        }
        drop(insert);
        if count == 0 {
            bail!("footprint commit contains no admitted entries");
        }
        Ok(count)
    })();
    drop(input);
    drop(output);
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let count = result?;
    if !status.success() {
        bail!("Git footprint blob reader failed");
    }
    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn packs_committed_bytes_under_exact_case_sensitive_identity() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "core.autocrlf", "false"]);
        fs::create_dir(root.join("Test.pretty")).unwrap();
        let path = root.join("Test.pretty/Chip.kicad_mod");
        let original = "(footprint \"Chip\")\n";
        fs::write(&path, original).unwrap();
        git(&["add", "--all"]);
        git(&[
            "-c",
            "user.name=Tokito Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ]);
        let revision = git(&["rev-parse", "HEAD"]);
        fs::write(&path, "local edit").unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(tokito_symbols::SCHEMA_SQL).unwrap();
        assert!(pack(&mut conn, root, &"0".repeat(40)).is_err());
        assert_eq!(pack(&mut conn, root, &revision).unwrap(), 1);
        let lookup = |id| {
            tokito_symbols::footprints::lookup(&conn, &FootprintId::parse(id).unwrap()).unwrap()
        };
        assert!(lookup("Test:chip").is_none());
        let record = lookup("Test:Chip").unwrap();
        assert_eq!(record.source, original);
        assert_eq!(
            record.sha256,
            hex::encode(sha2::Sha256::digest(original.as_bytes()))
        );
        assert_eq!(record.revision, revision);
    }
}
