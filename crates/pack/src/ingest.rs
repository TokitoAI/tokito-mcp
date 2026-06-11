//! Walk a kicad-symbols checkout, parse every `.kicad_sym` in parallel.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::{kicad, sexpr};

/// One parsed symbol with its library name (the `.kicad_symdir` directory it
/// came from, minus the suffix).
#[derive(Debug)]
pub struct Ingested {
    pub lib: String,
    pub symbol: kicad::ParsedSymbol,
    pub source: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("io reading {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("parse {0}: {1}")]
    Parse(PathBuf, sexpr::ParseError),
    #[error("extract {0}: {1}")]
    Extract(PathBuf, kicad::ExtractError),
    #[error("file is outside a `.kicad_symdir`: {0}")]
    NotInLibDir(PathBuf),
}

/// Collect every `*.kicad_sym` file under `root` and parse it. Returns one
/// `Ingested` per top-level symbol in each file (almost always exactly one in
/// the KiCad library, since the format is one-symbol-per-file).
pub fn ingest_all(root: &Path) -> (Vec<Ingested>, Vec<IngestError>) {
    let files: Vec<PathBuf> = WalkDir::new(root)
        .min_depth(2) // skip root + lib dirs themselves
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().map(|s| s == "kicad_sym").unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect();

    tracing::info!(count = files.len(), "discovered .kicad_sym files");

    let (oks, errs): (Vec<_>, Vec<_>) = files
        .par_iter()
        .map(parse_one)
        .partition(Result::is_ok);

    let ingested: Vec<Ingested> = oks.into_iter().flat_map(Result::unwrap).collect();
    let errors: Vec<IngestError> = errs.into_iter().map(|r| r.unwrap_err()).collect();

    (ingested, errors)
}

fn parse_one(path: &PathBuf) -> Result<Vec<Ingested>, IngestError> {
    let text = std::fs::read_to_string(path).map_err(|e| IngestError::Io(path.clone(), e))?;
    let tree = sexpr::parse(&text).map_err(|e| IngestError::Parse(path.clone(), e))?;
    let syms = kicad::extract_lib(&tree).map_err(|e| IngestError::Extract(path.clone(), e))?;
    let lib = lib_name_from_path(path).ok_or_else(|| IngestError::NotInLibDir(path.clone()))?;
    Ok(syms
        .into_iter()
        .map(|s| Ingested {
            lib: lib.clone(),
            symbol: s,
            source: path.clone(),
        })
        .collect())
}

/// `…/Device.kicad_symdir/R.kicad_sym` → `"Device"`.
fn lib_name_from_path(path: &Path) -> Option<String> {
    let parent = path.parent()?.file_name()?.to_str()?;
    parent.strip_suffix(".kicad_symdir").map(|s| s.to_string())
}
