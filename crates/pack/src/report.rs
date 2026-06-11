//! Emit `manifest.json` + `build.log` alongside the SQLite artifacts.
//!
//! These are the human-reviewable artifacts when CERN updates roll in: the
//! manifest pins source commit + sizes + checksums; the build log is the
//! text reviewers actually diff in PRs.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::emit::Stats;

#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub source_commit: String,
    pub generator_version: String,
    pub schema_version: u32,
    pub generated_at: String,
    pub lib_count: u64,
    pub symbol_count: u64,
    pub root_count: u64,
    pub extends_count: u64,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub name: String,
    pub size_bytes: u64,
    pub blake3: String,
}

pub struct Report {
    pub manifest: Manifest,
    pub top_libs: Vec<(String, u64)>,
}

impl Report {
    pub fn new(
        stats: &Stats,
        source_commit: String,
        generator_version: String,
        generated_at: String,
        schema_version: u32,
    ) -> Self {
        Self {
            manifest: Manifest {
                source_commit,
                generator_version,
                schema_version,
                generated_at,
                lib_count: stats.lib_count as u64,
                symbol_count: stats.symbol_count as u64,
                root_count: stats.root_count as u64,
                extends_count: stats.extends_count as u64,
                artifacts: vec![],
            },
            top_libs: vec![],
        }
    }
}

/// Compute size + blake3 for one file and add it to the manifest.
pub fn record_artifact(manifest: &mut Manifest, path: &Path) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    let hash = blake3::hash(&bytes);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    manifest.artifacts.push(Artifact {
        name,
        size_bytes: bytes.len() as u64,
        blake3: hash.to_hex().to_string(),
    });
    Ok(())
}

/// Query the catalog for the largest libraries by symbol count.
pub fn collect_top_libs(
    conn: &rusqlite::Connection,
    limit: u32,
) -> rusqlite::Result<Vec<(String, u64)>> {
    let mut stmt = conn.prepare(
        "SELECT l.name, COUNT(s.id) AS n FROM lib l \
         LEFT JOIN symbol s ON s.lib_id = l.id \
         GROUP BY l.id ORDER BY n DESC, l.name LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn write_manifest(out_dir: &Path, manifest: &Manifest) -> std::io::Result<PathBuf> {
    let path = out_dir.join("manifest.json");
    let text = serde_json::to_string_pretty(manifest)?;
    std::fs::write(&path, text)?;
    Ok(path)
}

pub fn write_build_log(
    out_dir: &Path,
    report: &Report,
    dangling_extends: &[(String, String, String)],
) -> std::io::Result<PathBuf> {
    let path = out_dir.join("build.log");
    let mut f = std::fs::File::create(&path)?;
    let m = &report.manifest;

    writeln!(f, "tokito-mcp-pack build report")?;
    writeln!(f, "=====================================")?;
    writeln!(f)?;
    writeln!(
        f,
        "Source:     gitlab.com/kicad/libraries/kicad-symbols @ {}",
        m.source_commit
    )?;
    writeln!(f, "Generated:  {}", m.generated_at)?;
    writeln!(f, "Generator:  tokito-mcp-pack {}", m.generator_version)?;
    writeln!(f, "Schema:     v{}", m.schema_version)?;
    writeln!(f)?;
    writeln!(f, "Totals")?;
    writeln!(f, "------")?;
    writeln!(f, "  Libraries:        {}", m.lib_count)?;
    writeln!(
        f,
        "  Symbols:          {}  ({} root + {} extending)",
        m.symbol_count, m.root_count, m.extends_count
    )?;
    writeln!(f, "  Dangling extends: {}", dangling_extends.len())?;
    writeln!(f)?;

    if !report.top_libs.is_empty() {
        writeln!(f, "Top libraries by symbol count")?;
        writeln!(f, "-----------------------------")?;
        for (name, n) in &report.top_libs {
            writeln!(f, "  {:35}  {:>6}", name, n)?;
        }
        writeln!(f)?;
    }

    if !m.artifacts.is_empty() {
        writeln!(f, "Artifacts")?;
        writeln!(f, "---------")?;
        for a in &m.artifacts {
            let mb = a.size_bytes as f64 / 1_048_576.0;
            writeln!(
                f,
                "  {:24}  {:>7.2} MB   blake3:{}…",
                a.name,
                mb,
                &a.blake3[..16]
            )?;
        }
        writeln!(f)?;
    }

    if !dangling_extends.is_empty() {
        writeln!(f, "Dangling extends ({}):", dangling_extends.len())?;
        for (lib, name, parent) in dangling_extends.iter().take(50) {
            writeln!(f, "  - {}:{} -> {}", lib, name, parent)?;
        }
        if dangling_extends.len() > 50 {
            writeln!(f, "  … ({} more)", dangling_extends.len() - 50)?;
        }
        writeln!(f)?;
    }

    Ok(path)
}
