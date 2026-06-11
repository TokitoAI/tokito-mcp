//! tokito-mcp-pack — builds `symbols.sqlite` from a checkout of CERN's
//! kicad-symbols repo, optionally also a slim `catalog.sqlite` and a
//! `manifest.json` + `build.log` for PR review.

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod emit;
mod ingest;
mod kicad;
mod report;
mod sexpr;

#[derive(Debug, Parser)]
#[command(name = "tokito-mcp-pack", version)]
struct Args {
    /// Path to the cloned kicad-symbols repo (the directory containing
    /// `*.kicad_symdir/` library directories).
    #[arg(long)]
    src: PathBuf,

    /// Output path for the built `symbols.sqlite`. Overwritten if it exists.
    #[arg(long)]
    out: PathBuf,

    /// Optional output path for the slim `catalog.sqlite` (same schema, body
    /// columns NULLed, embeddings table dropped — the desktop bundle).
    #[arg(long)]
    slim_out: Option<PathBuf>,

    /// CERN git SHA the source was checked out at. Stored in `meta` and
    /// `manifest.json`.
    #[arg(long)]
    source_commit: String,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    if args.out.exists() {
        std::fs::remove_file(&args.out)?;
    }

    let t_total = Instant::now();
    let (items, errors) = ingest::ingest_all(&args.src);
    tracing::info!(
        symbols = items.len(),
        errors = errors.len(),
        secs = t_total.elapsed().as_secs_f32(),
        "ingest done"
    );
    for e in errors.iter().take(20) {
        tracing::warn!(error = %e, "ingest error");
    }
    if errors.len() > 20 {
        tracing::warn!(
            more = errors.len() - 20,
            "additional ingest errors suppressed"
        );
    }

    let t_emit = Instant::now();
    let mut conn = tokito_symbols::db::open_for_build(&args.out)?;
    let stats = emit::build(&mut conn, items)?;

    // Write generator metadata into the meta table.
    let generated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let generator_version = env!("CARGO_PKG_VERSION");
    write_meta(&conn, "source_commit", &args.source_commit)?;
    write_meta(&conn, "generator_version", generator_version)?;
    write_meta(&conn, "generated_at", &generated_at)?;
    write_meta(&conn, "symbol_count", &stats.symbol_count.to_string())?;
    write_meta(&conn, "lib_count", &stats.lib_count.to_string())?;

    // Compact the file so the released artifact is minimal.
    conn.execute_batch("VACUUM;")?;

    // Top libs for the build log (do it before slim — slim has the same
    // metadata, but easier to query once here).
    let top_libs = report::collect_top_libs(&conn, 15)?;
    drop(conn);

    tracing::info!(
        libs = stats.lib_count,
        symbols = stats.symbol_count,
        roots = stats.root_count,
        extending = stats.extends_count,
        dangling = stats.dangling_extends.len(),
        secs = t_emit.elapsed().as_secs_f32(),
        "emit done"
    );
    for (lib, name, parent) in stats.dangling_extends.iter().take(20) {
        tracing::warn!(child = %format!("{lib}:{name}"), parent = %parent, "dangling extends");
    }

    // --- slim catalog ---
    if let Some(slim_path) = args.slim_out.as_ref() {
        build_slim_catalog(&args.out, slim_path)?;
        tracing::info!(out = ?slim_path, "slim catalog written");
    }

    // --- report ---
    let mut rep = report::Report::new(
        &stats,
        args.source_commit.clone(),
        generator_version.to_string(),
        generated_at.clone(),
        tokito_symbols::CURRENT_SCHEMA_VERSION,
    );
    rep.top_libs = top_libs;
    report::record_artifact(&mut rep.manifest, &args.out)?;
    if let Some(slim_path) = args.slim_out.as_ref() {
        report::record_artifact(&mut rep.manifest, slim_path)?;
    }

    let out_dir = args
        .out
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = report::write_manifest(&out_dir, &rep.manifest)?;
    let log_path = report::write_build_log(&out_dir, &rep, &stats.dangling_extends)?;
    tracing::info!(manifest = ?manifest_path, log = ?log_path, "report written");

    let total = t_total.elapsed().as_secs_f32();
    let size = std::fs::metadata(&args.out)?.len();
    tracing::info!(
        out = ?args.out,
        size_mb = size as f32 / 1_048_576.0,
        total_secs = total,
        "build complete"
    );

    Ok(())
}

fn write_meta(conn: &rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

fn build_slim_catalog(full: &PathBuf, slim: &PathBuf) -> anyhow::Result<()> {
    if slim.exists() {
        std::fs::remove_file(slim)?;
    }
    let src = rusqlite::Connection::open(full)?;
    let slim_str = slim
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("slim out path is not UTF-8: {slim:?}"))?;
    src.execute_batch(&format!("VACUUM INTO '{}';", escape_sql(slim_str)))?;
    drop(src);

    let dst = rusqlite::Connection::open(slim)?;
    dst.execute_batch(
        r#"
        UPDATE symbol SET body = NULL, body_format = NULL;
        DROP TABLE IF EXISTS symbol_embedding;
        VACUUM;
        "#,
    )?;
    Ok(())
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}
