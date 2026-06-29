//! Procurement-query hints derived from catalog symbols.

use serde::Serialize;
use tokito_symbols::model::ResolvedSymbol;

#[derive(Debug, Clone, Serialize)]
pub struct PartOfferQueryResponse {
    pub symbol_id: String,
    pub value: Option<String>,
    pub package: Option<String>,
    pub market: Option<String>,
    pub procurement_query: String,
    pub exact_mpn: Option<String>,
    pub datasheet: Option<String>,
    pub description: Option<String>,
    pub fp_filters: Option<String>,
    pub footprint: Option<String>,
    pub distributor_domains: Vec<String>,
    pub notes: Vec<String>,
}

pub fn build_response(
    symbol_id: &str,
    value: Option<&str>,
    package: Option<&str>,
    market: Option<&str>,
    resolved: Option<&ResolvedSymbol>,
) -> PartOfferQueryResponse {
    let value = clean(value).map(str::to_string);
    let package = clean(package)
        .map(str::to_string)
        .or_else(|| resolved.and_then(|s| clean(Some(s.footprint.as_str())).map(str::to_string)));
    let description =
        resolved.and_then(|s| clean(Some(s.description.as_str())).map(str::to_string));
    let fp_filters = resolved.and_then(|s| clean(Some(s.fp_filters.as_str())).map(str::to_string));
    let footprint = resolved.and_then(|s| clean(Some(s.footprint.as_str())).map(str::to_string));
    let datasheet = resolved.and_then(|s| clean(Some(s.datasheet.as_str())).map(str::to_string));
    let procurement_query = build_procurement_query(
        symbol_id,
        value.as_deref(),
        package.as_deref(),
        description.as_deref(),
    );
    let market = clean(market).map(|m| m.to_ascii_uppercase());
    let distributor_domains = distributor_domains_for_market(market.as_deref());

    PartOfferQueryResponse {
        symbol_id: symbol_id.to_string(),
        value,
        package,
        market,
        procurement_query,
        exact_mpn: None,
        datasheet,
        description,
        fp_filters,
        footprint,
        distributor_domains,
        notes: vec![
            "The symbol catalog does not contain live stock or pricing.".into(),
            "Use procurement_query with distributor web search, then verify electrical/package compatibility before committing a BOM offer.".into(),
        ],
    }
}

pub fn split_symbol_id(symbol_id: &str) -> Option<(&str, &str)> {
    let (lib, name) = symbol_id.split_once(':')?;
    let lib = lib.trim();
    let name = name.trim();
    (!lib.is_empty() && !name.is_empty()).then_some((lib, name))
}

pub fn symbol_id(lib: &str, name: &str) -> String {
    format!("{}:{}", lib.trim(), name.trim())
}

fn build_procurement_query(
    symbol_id: &str,
    value: Option<&str>,
    package: Option<&str>,
    description: Option<&str>,
) -> String {
    let symbol_key = symbol_id.trim().to_ascii_lowercase();
    let short = symbol_key.rsplit(':').next().unwrap_or(symbol_key.as_str());
    let desc = description.unwrap_or_default().to_ascii_lowercase();
    let kind = if matches!(symbol_key.as_str(), "device:r" | "device:r_us") {
        "resistor".to_string()
    } else if matches!(symbol_key.as_str(), "device:c" | "device:c_small") {
        "capacitor".to_string()
    } else if matches!(symbol_key.as_str(), "device:l" | "device:l_small") {
        "inductor".to_string()
    } else if symbol_key == "device:led" || short.contains("led") || desc.contains("led") {
        "LED".to_string()
    } else if symbol_key == "device:d" || short == "d" || short.contains("diode") {
        "diode".to_string()
    } else if short.contains("battery") || short == "bt" || short == "bat" {
        "battery connector or holder".to_string()
    } else if short.contains("sw_spst") || short.contains("spst") {
        "SPST switch".to_string()
    } else if short.contains("sw") || short.contains("switch") {
        "switch".to_string()
    } else {
        readable_symbol_name(short)
    };

    let mut query = match clean(value) {
        Some(value) if !value.eq_ignore_ascii_case(&kind) => format!("{value} {kind}"),
        _ => kind,
    };
    if let Some(package) = package.and_then(package_search_phrase) {
        if !query
            .to_ascii_lowercase()
            .contains(&package.to_ascii_lowercase())
        {
            query.push_str(", ");
            query.push_str(&package);
        }
    }
    query
}

fn clean(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "-" && *s != "???")
}

fn package_search_phrase(package: &str) -> Option<String> {
    let package = package.trim();
    if package.is_empty() {
        return None;
    }
    let simplified = package
        .rsplit(':')
        .next()
        .unwrap_or(package)
        .trim_start_matches("R_")
        .trim_start_matches("C_")
        .replace('_', " ");
    let simplified = simplified.trim();
    (!simplified.is_empty()).then(|| format!("{simplified} package"))
}

fn readable_symbol_name(short: &str) -> String {
    short
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn distributor_domains_for_market(market: Option<&str>) -> Vec<String> {
    let domains = match market.unwrap_or("US") {
        "IN" => vec![
            "digikey.in",
            "mouser.in",
            "in.element14.com",
            "rsdelivers.com",
            "arrow.com",
        ],
        "GB" => vec![
            "uk.farnell.com",
            "rs-online.com",
            "digikey.co.uk",
            "mouser.co.uk",
            "arrow.com",
        ],
        "AU" => vec![
            "au.element14.com",
            "au.rs-online.com",
            "digikey.com.au",
            "mouser.com",
            "arrow.com",
        ],
        "DE" | "FR" | "IT" | "ES" | "NL" | "BE" | "IE" | "FI" | "AT" | "PT" => vec![
            "farnell.com",
            "rs-online.com",
            "digikey.com",
            "mouser.com",
            "arrow.com",
        ],
        _ => vec![
            "digikey.com",
            "mouser.com",
            "arrow.com",
            "farnell.com",
            "rs-online.com",
        ],
    };
    domains.into_iter().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resistor_symbol_builds_generic_procurement_query() {
        let response = build_response("Device:R", Some("330"), Some("R_0603"), Some("IN"), None);

        assert_eq!(response.procurement_query, "330 resistor, 0603 package");
        assert_eq!(response.exact_mpn, None);
        assert!(response.distributor_domains.contains(&"digikey.in".into()));
    }
}
