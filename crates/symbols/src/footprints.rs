//! Exact, case-sensitive footprint identities shared by the packer and server.
//! A supplier package description is not a KiCad library identity.

use rusqlite::{params, Connection, OptionalExtension};

pub const MAX_SOURCE_BYTES: usize = 512 * 1024;

pub struct FootprintSource {
    pub revision: String,
    pub source: String,
    pub sha256: String,
}

/// `None` means the artifact has no matching row, including legacy packs.
pub fn lookup(
    conn: &Connection,
    id: &FootprintId<'_>,
) -> rusqlite::Result<Option<FootprintSource>> {
    let present: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'footprint_source')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        return Ok(None);
    }
    conn.query_row(
        "SELECT revision, source, sha256 FROM footprint_source WHERE library = ?1 AND name = ?2",
        params![id.library, id.name],
        |row| {
            Ok(FootprintSource {
                revision: row.get(0)?,
                source: row.get(1)?,
                sha256: row.get(2)?,
            })
        },
    )
    .optional()
}

#[derive(Debug, PartialEq, Eq)]
pub struct FootprintId<'a> {
    pub library: &'a str,
    pub name: &'a str,
}

impl<'a> FootprintId<'a> {
    pub fn parse(id: &'a str) -> Option<Self> {
        let (library, name) = id.split_once(':')?;
        Self::new(library, name)
    }

    pub fn new(library: &'a str, name: &'a str) -> Option<Self> {
        fn valid(segment: &str) -> bool {
            !segment.is_empty()
                && segment.len() <= 240
                && !segment.starts_with('.')
                && !segment.ends_with('.')
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'))
        }
        (valid(library) && valid(name)).then_some(Self { library, name })
    }
}

#[cfg(test)]
mod tests {
    use super::FootprintId;

    #[test]
    fn accepts_exact_kicad_ids_without_rewriting_them() {
        let id = FootprintId::parse("RF_Module:ESP32-WROOM-32").unwrap();
        assert_eq!(id.library, "RF_Module");
        assert_eq!(id.name, "ESP32-WROOM-32");
        assert!(FootprintId::parse("Resistor_SMD:R_0603_1608Metric").is_some());
    }

    #[test]
    fn rejects_package_descriptions_and_noncanonical_paths() {
        for id in [
            "0603",
            "QFN: QFN-32",
            "Lib:../a",
            "Lib:a\\b",
            "Lib:a:b",
            "Lib:%2f",
            ":a",
            "Lib:",
        ] {
            assert!(FootprintId::parse(id).is_none(), "accepted {id}");
        }
    }
}
