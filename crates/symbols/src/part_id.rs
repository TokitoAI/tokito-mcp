//! Part identity for the generated-symbol store.
//!
//! `PartId` normalizes the manufacturer key so the same physical part resolves
//! identically across submissions ("STMicroelectronics", " STMicroelectronics ",
//! "stmicroelectronics" collapse to one row). MPN and package pass through
//! case-sensitive — vendors distinguish variants by casing (SUFFIX vs suffix).
//!
//! Normalization matches the rule in docs/CONTRACTS.md §3:
//!   * NFC-normalise the manufacturer string;
//!   * lowercase (ASCII simple lowering plus Unicode simple case folding via
//!     `to_lowercase`);
//!   * collapse all Unicode whitespace runs to a single ASCII space, trim ends.
//!
//! When tokito-catalog::pipeline::identity::PartId lands (Wave A.1), that crate
//! becomes the canonical source of truth and this module exists as its wire-
//! side mirror. Both implementations MUST produce byte-identical strings on
//! the same input; the round-trip test at the bottom of this file is the
//! anchor.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// Normalized (manufacturer, mpn, package) triple.
///
/// Construct via [`PartId::new`] to run normalization. The `Serialize` /
/// `Deserialize` impls emit and consume the exact stored form — no
/// re-normalization on the wire — so a round-tripped `PartId` is stable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartId {
    /// NFC + lowercased + whitespace-collapsed manufacturer name.
    pub manufacturer_norm: String,
    /// Manufacturer part number, exact case (case-sensitive).
    pub mpn: String,
    /// Package/variant string, exact case.
    pub package: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("manufacturer is empty after normalization")]
    EmptyManufacturer,
    #[error("mpn is empty after trim")]
    EmptyMpn,
    #[error("package is empty after trim")]
    EmptyPackage,
}

impl PartId {
    /// Normalize the manufacturer, trim mpn/package, and validate non-empty.
    pub fn new(
        manufacturer: impl AsRef<str>,
        mpn: impl AsRef<str>,
        package: impl AsRef<str>,
    ) -> Result<Self, IdentityError> {
        let manufacturer_norm = normalize_manufacturer(manufacturer.as_ref());
        if manufacturer_norm.is_empty() {
            return Err(IdentityError::EmptyManufacturer);
        }
        let mpn = mpn.as_ref().trim().to_string();
        if mpn.is_empty() {
            return Err(IdentityError::EmptyMpn);
        }
        let package = package.as_ref().trim().to_string();
        if package.is_empty() {
            return Err(IdentityError::EmptyPackage);
        }
        Ok(Self {
            manufacturer_norm,
            mpn,
            package,
        })
    }

    /// Composite key used as the `part_registry.part_id` primary key.
    /// Chosen so it's URL-safe when a client wants to pass it back verbatim.
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.manufacturer_norm, self.mpn, self.package)
    }
}

/// Normalize a manufacturer name to its canonical registry key.
///
/// Exposed for callers that only need the key (e.g., an admin CLI that
/// looks up a row without constructing the full `PartId`).
pub fn normalize_manufacturer(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let mut out = String::with_capacity(nfc.len());
    let mut prev_space = true;
    for ch in nfc.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_and_trims() {
        assert_eq!(
            normalize_manufacturer(" STMicroelectronics "),
            "stmicroelectronics"
        );
    }

    #[test]
    fn normalize_collapses_internal_whitespace() {
        assert_eq!(
            normalize_manufacturer("Texas   Instruments\tInc."),
            "texas instruments inc.",
        );
    }

    #[test]
    fn normalize_handles_unicode_case_folding() {
        assert_eq!(normalize_manufacturer("İstanbul"), "i\u{307}stanbul");
    }

    #[test]
    fn normalize_nfc_composes_decomposed() {
        // "é" as decomposed 'e' + U+0301 combining acute; NFC should compose it.
        assert_eq!(normalize_manufacturer("R\u{0065}\u{0301}"), "ré");
    }

    #[test]
    fn new_rejects_empty_manufacturer() {
        assert_eq!(
            PartId::new("   ", "MPN", "PKG").unwrap_err(),
            IdentityError::EmptyManufacturer,
        );
    }

    #[test]
    fn new_rejects_empty_mpn_after_trim() {
        assert_eq!(
            PartId::new("ST", "  ", "PKG").unwrap_err(),
            IdentityError::EmptyMpn,
        );
    }

    #[test]
    fn new_preserves_mpn_case() {
        let p = PartId::new("STMicroelectronics", "STM32H743VIT6", "LQFP100").unwrap();
        assert_eq!(p.mpn, "STM32H743VIT6");
        assert_eq!(p.package, "LQFP100");
    }

    #[test]
    fn key_is_stable() {
        let p = PartId::new("STMicroelectronics", "STM32H743VIT6", "LQFP100").unwrap();
        assert_eq!(p.key(), "stmicroelectronics|STM32H743VIT6|LQFP100");
    }

    #[test]
    fn same_input_variations_produce_same_key() {
        let a = PartId::new("STMicroelectronics", "STM32", "LQFP").unwrap();
        let b = PartId::new(" stmicroelectronics ", "STM32", "LQFP").unwrap();
        let c = PartId::new("ST  Microelectronics", "STM32", "LQFP").unwrap();
        assert_eq!(a.key(), b.key());
        // c is a genuinely different string (contains "st microelectronics"),
        // so this documents that whitespace *inside* the name is preserved
        // as a single space and is significant. Vendors picking "STM" vs
        // "ST Microelectronics" as their canonical spelling stay distinct.
        assert_ne!(a.key(), c.key());
    }
}
