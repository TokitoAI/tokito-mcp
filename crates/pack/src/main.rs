//! tokito-mcp-pack — builds `symbols.sqlite` from a checkout of CERN's
//! kicad-symbols repo, plus a `manifest.json` + `build.log` for PR review.
//!
//! `symbols.sqlite` is the single shipped catalog: symbol bodies (geometry)
//! together with the FTS index. It is the one artifact consumed by both the
//! hosted server and the desktop app (`tokito-catalog/build.rs`).

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod emit;
mod ingest;
mod kicad;
mod report;
mod sexpr;

#[derive(Debug, Parser)]
#[command(name = "tokito-mcp-pack", version)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    // ---- Flat build args (legacy invocation without a subcommand) ----
    //
    // Older CI and dev workflows invoke `tokito-mcp-pack --src ... --out ...
    // --source-commit ...` without a subcommand; those still work. New
    // features live under subcommands.
    /// Path to the cloned kicad-symbols repo. Legacy top-level flag.
    #[arg(long, global = false)]
    src: Option<PathBuf>,

    /// Output path for the built `symbols.sqlite`. Legacy top-level flag.
    #[arg(long, global = false)]
    out: Option<PathBuf>,

    /// CERN git SHA the source was checked out at. Legacy top-level flag.
    #[arg(long, global = false)]
    source_commit: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Merge generated symbols from a source `symbols.sqlite` (typically
    /// tokito-ai's `generated.sqlite`, populated by the ingestion service
    /// in Wave C.1) into a target `symbols.sqlite`.
    ///
    /// The target file is opened read-write. Each revision from the source
    /// is inserted via `tokito_symbols::generated::insert_revision`, which
    /// is idempotent on `revision_id` + body match and rejects body drift
    /// with a hard error. No writes go through the MCP surface.
    #[command(name = "generated")]
    Generated {
        /// Target `symbols.sqlite` (opened read-write).
        #[arg(long)]
        db: PathBuf,

        /// Source `symbols.sqlite` (opened read-only).
        #[arg(long)]
        source: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    if let Some(Command::Generated { db, source }) = &args.command {
        return run_generated_sync(db, source);
    }

    let args = LegacyArgs::try_from(args)?;
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

    // Top libs for the build log.
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

/// Legacy top-level args coalesced from the flat CLI form.
struct LegacyArgs {
    src: PathBuf,
    out: PathBuf,
    source_commit: String,
}

impl TryFrom<Args> for LegacyArgs {
    type Error = anyhow::Error;
    fn try_from(a: Args) -> Result<Self, Self::Error> {
        let src = a
            .src
            .ok_or_else(|| anyhow::anyhow!("--src is required for the top-level KiCad build"))?;
        let out = a
            .out
            .ok_or_else(|| anyhow::anyhow!("--out is required for the top-level KiCad build"))?;
        let source_commit = a.source_commit.ok_or_else(|| {
            anyhow::anyhow!("--source-commit is required for the top-level KiCad build")
        })?;
        Ok(Self {
            src,
            out,
            source_commit,
        })
    }
}

fn run_generated_sync(db: &std::path::Path, source: &std::path::Path) -> anyhow::Result<()> {
    if !source.exists() {
        anyhow::bail!("source generated.sqlite {source:?} does not exist");
    }
    if !db.exists() {
        anyhow::bail!(
            "target symbols.sqlite {db:?} does not exist; run the top-level pack first"
        );
    }
    let t = Instant::now();
    let mut conn = rusqlite::Connection::open(db)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let tx = conn.transaction()?;
    let count = tokito_symbols::generated::sync_from(&tx, source)?;
    tx.commit()?;
    tracing::info!(
        merged = count,
        secs = t.elapsed().as_secs_f32(),
        source = ?source,
        target = ?db,
        "generated sync done"
    );
    Ok(())
}
