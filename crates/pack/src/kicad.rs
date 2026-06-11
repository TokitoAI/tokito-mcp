//! KiCad symbol AST: walks an `Sexpr` from a `.kicad_sym` file and produces
//! a `ParsedSymbol` per symbol in the file.
//!
//! Coordinates: source files use f32 mm; we convert to i32 1/100 mm at parse
//! time so downstream code never sees floats.
//!
//! Multi-unit symbols decompose into nested `(symbol "ROOT_n_m" ...)` blocks
//! where (n, m) = (unit, body_style). Pins and graphics get tagged with that
//! tuple so the renderer can group them.

use std::collections::BTreeMap;

use crate::sexpr::Sexpr;

#[derive(Debug, Default, Clone)]
pub struct ParsedSymbol {
    pub name: String,
    pub extends: Option<String>,
    /// Indexed by property key (`Reference`, `Value`, `ki_keywords`, …).
    pub properties: BTreeMap<String, ParsedProperty>,
    pub pins: Vec<ParsedPin>,
    pub graphics: Vec<ParsedGraphic>,
    /// Set of (unit, body_style) tuples discovered from nested sub-symbols.
    pub units: Vec<(u8, u8)>,
    pub flags: ParsedFlags,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedFlags {
    pub in_bom: bool,
    pub on_board: bool,
    pub in_pos_files: bool,
    pub exclude_from_sim: bool,
    pub hide_pin_numbers: bool,
    pub duplicate_pin_numbers_are_jumpers: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedProperty {
    pub value: String,
    pub x: i32,
    pub y: i32,
    pub rotation: i16,
    pub hide: bool,
    pub show_name: bool,
    pub font_size: i32,
    pub italic: bool,
    pub bold: bool,
    pub justify: String, // raw, normalised by emit.rs
}

#[derive(Debug, Clone)]
pub struct ParsedPin {
    pub number: String,
    pub name: String,
    pub electrical: String,
    pub style: String,
    pub x: i32,
    pub y: i32,
    pub rotation: i16,
    pub length: i32,
    pub unit: u8,
    pub body_style: u8,
}

#[derive(Debug, Clone)]
pub struct ParsedGraphic {
    pub unit: u8,
    pub body_style: u8,
    pub kind: ParsedGraphicKind,
    pub stroke_width: i32,
    pub stroke_kind: String,
    pub fill: String,
}

#[derive(Debug, Clone)]
pub enum ParsedGraphicKind {
    Rectangle { sx: i32, sy: i32, ex: i32, ey: i32 },
    Circle { cx: i32, cy: i32, radius: i32 },
    Arc { sx: i32, sy: i32, mx: i32, my: i32, ex: i32, ey: i32 },
    Polyline { points: Vec<(i32, i32)> },
    Bezier { points: Vec<(i32, i32)> },
    Text { x: i32, y: i32, rotation: i16, content: String, italic: bool, bold: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("expected kicad_symbol_lib root, got something else")]
    NotLibFile,
    #[error("symbol block missing name")]
    UnnamedSymbol,
    #[error("malformed value for {key}: {detail}")]
    BadValue { key: &'static str, detail: String },
}

/// Extract every top-level `(symbol ...)` from a parsed `.kicad_sym` or
/// `.tokito_sym` file. Both formats are syntactically identical at the
/// container level, only the root tag and a couple of property names differ.
pub fn extract_lib(root: &Sexpr) -> Result<Vec<ParsedSymbol>, ExtractError> {
    let (head, rest) = root.list_head().ok_or(ExtractError::NotLibFile)?;
    if head != "kicad_symbol_lib" && head != "tokito_symbol_lib" {
        return Err(ExtractError::NotLibFile);
    }
    let mut out = Vec::new();
    for item in rest {
        if let Some(("symbol", _)) = item.list_head() {
            out.push(extract_symbol(item)?);
        }
    }
    Ok(out)
}

fn extract_symbol(node: &Sexpr) -> Result<ParsedSymbol, ExtractError> {
    let items = node.as_list().ok_or(ExtractError::UnnamedSymbol)?;
    if items.len() < 2 {
        return Err(ExtractError::UnnamedSymbol);
    }
    let name = items[1].as_text().ok_or(ExtractError::UnnamedSymbol)?.to_string();
    let mut sym = ParsedSymbol {
        name,
        ..Default::default()
    };

    for child in &items[2..] {
        let Some((head, rest)) = child.list_head() else { continue };
        match head {
            "extends" => {
                if let Some(p) = rest.first().and_then(Sexpr::as_text) {
                    sym.extends = Some(p.to_string());
                }
            }
            "property" => {
                if let Some((k, p)) = extract_property(rest)? {
                    sym.properties.insert(k, p);
                }
            }
            "pin_numbers" => {
                sym.flags.hide_pin_numbers = find_yes_no(rest, "hide").unwrap_or(false);
            }
            "in_bom" => sym.flags.in_bom = first_yes_no(rest),
            "on_board" => sym.flags.on_board = first_yes_no(rest),
            "in_pos_files" => sym.flags.in_pos_files = first_yes_no(rest),
            "exclude_from_sim" => sym.flags.exclude_from_sim = first_yes_no(rest),
            "duplicate_pin_numbers_are_jumpers" => {
                sym.flags.duplicate_pin_numbers_are_jumpers = first_yes_no(rest)
            }
            "symbol" => {
                // Nested sub-symbol: pulls its (unit, body_style) from the
                // suffix on its name, then walks its children for pins/graphics.
                let parent_name = sym.name.clone();
                extract_sub_symbol(&parent_name, child, &mut sym)?;
            }
            _ => {}
        }
    }
    Ok(sym)
}

fn extract_sub_symbol(
    parent_name: &str,
    node: &Sexpr,
    out: &mut ParsedSymbol,
) -> Result<(), ExtractError> {
    let items = node.as_list().unwrap();
    let sub_name = items.get(1).and_then(Sexpr::as_text).unwrap_or("");
    let (unit, body_style) = parse_unit_suffix(sub_name, parent_name).unwrap_or((0, 1));
    if !out.units.iter().any(|&u| u == (unit, body_style)) {
        out.units.push((unit, body_style));
    }

    for child in &items[2..] {
        let Some((head, rest)) = child.list_head() else { continue };
        match head {
            "pin" => {
                if let Some(p) = extract_pin(rest, unit, body_style)? {
                    out.pins.push(p);
                }
            }
            "rectangle" | "circle" | "arc" | "polyline" | "bezier" | "text" => {
                if let Some(g) = extract_graphic(head, rest, unit, body_style)? {
                    out.graphics.push(g);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn extract_property(rest: &[Sexpr]) -> Result<Option<(String, ParsedProperty)>, ExtractError> {
    if rest.len() < 2 {
        return Ok(None);
    }
    let key = rest[0].as_text().unwrap_or_default().to_string();
    let value = rest[1].as_text().unwrap_or_default().to_string();

    let mut p = ParsedProperty {
        value,
        x: 0,
        y: 0,
        rotation: 0,
        hide: false,
        show_name: false,
        font_size: 0,
        italic: false,
        bold: false,
        justify: String::new(),
    };
    for child in &rest[2..] {
        let Some((head, r)) = child.list_head() else { continue };
        match head {
            "at" => {
                let (x, y, rot) = read_at(r);
                p.x = x;
                p.y = y;
                p.rotation = rot;
            }
            "hide" => p.hide = first_yes_no(r),
            "show_name" => p.show_name = first_yes_no(r),
            "effects" => read_effects(r, &mut p.font_size, &mut p.italic, &mut p.bold, &mut p.justify),
            _ => {}
        }
    }
    Ok(Some((key, p)))
}

fn extract_pin(
    rest: &[Sexpr],
    unit: u8,
    body_style: u8,
) -> Result<Option<ParsedPin>, ExtractError> {
    if rest.len() < 2 {
        return Ok(None);
    }
    let electrical = rest[0].as_text().unwrap_or("unspecified").to_string();
    let style = rest[1].as_text().unwrap_or("line").to_string();

    let mut pin = ParsedPin {
        number: String::new(),
        name: String::new(),
        electrical,
        style,
        x: 0,
        y: 0,
        rotation: 0,
        length: 0,
        unit,
        body_style,
    };
    for child in &rest[2..] {
        let Some((head, r)) = child.list_head() else { continue };
        match head {
            "at" => {
                let (x, y, rot) = read_at(r);
                pin.x = x;
                pin.y = y;
                pin.rotation = rot;
            }
            "length" => {
                if let Some(t) = r.first().and_then(Sexpr::as_text) {
                    pin.length = parse_mm_to_i32(t);
                }
            }
            "name" => {
                if let Some(t) = r.first().and_then(Sexpr::as_text) {
                    pin.name = t.to_string();
                }
            }
            "number" => {
                if let Some(t) = r.first().and_then(Sexpr::as_text) {
                    pin.number = t.to_string();
                }
            }
            _ => {}
        }
    }
    Ok(Some(pin))
}

fn extract_graphic(
    kind_tag: &str,
    rest: &[Sexpr],
    unit: u8,
    body_style: u8,
) -> Result<Option<ParsedGraphic>, ExtractError> {
    let mut stroke_width = 0;
    let mut stroke_kind = String::new();
    let mut fill = String::new();
    let mut sx = 0;
    let mut sy = 0;
    let mut ex = 0;
    let mut ey = 0;
    let mut mx = 0;
    let mut my = 0;
    let mut cx = 0;
    let mut cy = 0;
    let mut radius = 0;
    let mut points: Vec<(i32, i32)> = Vec::new();
    let mut text_x = 0;
    let mut text_y = 0;
    let mut text_rot = 0;
    let mut italic = false;
    let mut bold = false;
    let mut font_size = 0;
    let mut justify = String::new();

    // text's content is at position 0 of rest
    let text_content = if kind_tag == "text" {
        rest.first().and_then(Sexpr::as_text).unwrap_or("").to_string()
    } else {
        String::new()
    };

    let scan_start = if kind_tag == "text" { 1 } else { 0 };

    for child in &rest[scan_start..] {
        let Some((head, r)) = child.list_head() else { continue };
        match head {
            "start" => {
                let (x, y, _) = read_at(r);
                sx = x;
                sy = y;
            }
            "end" => {
                let (x, y, _) = read_at(r);
                ex = x;
                ey = y;
            }
            "mid" => {
                let (x, y, _) = read_at(r);
                mx = x;
                my = y;
            }
            "center" => {
                let (x, y, _) = read_at(r);
                cx = x;
                cy = y;
            }
            "radius" => {
                if let Some(t) = r.first().and_then(Sexpr::as_text) {
                    radius = parse_mm_to_i32(t);
                }
            }
            "pts" => {
                for p in r {
                    let Some((h, pr)) = p.list_head() else { continue };
                    if h == "xy" {
                        let (x, y, _) = read_at(pr);
                        points.push((x, y));
                    }
                }
            }
            "stroke" => {
                for c in r {
                    let Some((sh, sr)) = c.list_head() else { continue };
                    match sh {
                        "width" => {
                            if let Some(t) = sr.first().and_then(Sexpr::as_text) {
                                stroke_width = parse_mm_to_i32(t);
                            }
                        }
                        "type" => {
                            if let Some(t) = sr.first().and_then(Sexpr::as_text) {
                                stroke_kind = t.to_string();
                            }
                        }
                        _ => {}
                    }
                }
            }
            "fill" => {
                for c in r {
                    if let Some(("type", tr)) = c.list_head() {
                        if let Some(t) = tr.first().and_then(Sexpr::as_text) {
                            fill = t.to_string();
                        }
                    }
                }
            }
            "at" if kind_tag == "text" => {
                let (x, y, rot) = read_at(r);
                text_x = x;
                text_y = y;
                text_rot = rot;
            }
            "effects" if kind_tag == "text" => {
                read_effects(r, &mut font_size, &mut italic, &mut bold, &mut justify);
            }
            _ => {}
        }
    }

    let kind = match kind_tag {
        "rectangle" => ParsedGraphicKind::Rectangle { sx, sy, ex, ey },
        "circle" => ParsedGraphicKind::Circle { cx, cy, radius },
        "arc" => ParsedGraphicKind::Arc { sx, sy, mx, my, ex, ey },
        "polyline" => ParsedGraphicKind::Polyline { points },
        "bezier" => ParsedGraphicKind::Bezier { points },
        "text" => ParsedGraphicKind::Text {
            x: text_x,
            y: text_y,
            rotation: text_rot,
            content: text_content,
            italic,
            bold,
        },
        _ => return Ok(None),
    };

    Ok(Some(ParsedGraphic {
        unit,
        body_style,
        kind,
        stroke_width,
        stroke_kind,
        fill,
    }))
}

// ---------- helpers ----------

fn parse_mm_to_f64(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0)
}

fn parse_mm_to_i32(s: &str) -> i32 {
    (parse_mm_to_f64(s) * 100.0).round() as i32
}

fn read_at(rest: &[Sexpr]) -> (i32, i32, i16) {
    let x = rest.first().and_then(Sexpr::as_text).map(parse_mm_to_i32).unwrap_or(0);
    let y = rest.get(1).and_then(Sexpr::as_text).map(parse_mm_to_i32).unwrap_or(0);
    let rot = rest
        .get(2)
        .and_then(Sexpr::as_text)
        .and_then(|t| t.parse::<f32>().ok())
        .map(|f| f.round() as i16)
        .unwrap_or(0);
    (x, y, rot)
}

fn first_yes_no(rest: &[Sexpr]) -> bool {
    rest.first()
        .and_then(Sexpr::as_text)
        .map(|t| t == "yes")
        .unwrap_or(false)
}

fn find_yes_no(rest: &[Sexpr], key: &str) -> Option<bool> {
    for item in rest {
        if let Some((h, r)) = item.list_head() {
            if h == key {
                return Some(first_yes_no(r));
            }
        }
    }
    None
}

fn read_effects(
    rest: &[Sexpr],
    font_size: &mut i32,
    italic: &mut bool,
    bold: &mut bool,
    justify: &mut String,
) {
    for child in rest {
        let Some((head, r)) = child.list_head() else { continue };
        match head {
            "font" => {
                for c in r {
                    let Some((fh, fr)) = c.list_head() else { continue };
                    match fh {
                        "size" => {
                            if let Some(t) = fr.first().and_then(Sexpr::as_text) {
                                *font_size = parse_mm_to_i32(t);
                            }
                        }
                        "italic" => *italic = first_yes_no(fr),
                        "bold" => *bold = first_yes_no(fr),
                        _ => {}
                    }
                }
            }
            "justify" => {
                let parts: Vec<&str> = r.iter().filter_map(Sexpr::as_text).collect();
                *justify = parts.join("_");
            }
            _ => {}
        }
    }
}

/// Splits a sub-symbol name like `R_0_1` into (root="R", unit=0, body_style=1).
/// Falls back to None if the suffix doesn't match — caller treats as (0, 1).
fn parse_unit_suffix(sub_name: &str, parent_name: &str) -> Option<(u8, u8)> {
    // Strip the parent name + underscore from the front.
    let suffix = sub_name.strip_prefix(parent_name)?.strip_prefix('_')?;
    let mut parts = suffix.split('_');
    let unit = parts.next()?.parse::<u8>().ok()?;
    let body_style = parts.next()?.parse::<u8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((unit, body_style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexpr;

    const DEVICE_R: &str = r#"(kicad_symbol_lib
  (version 20251024)
  (symbol "R"
    (pin_numbers (hide yes))
    (pin_names (offset 0))
    (in_bom yes)
    (on_board yes)
    (property "Reference" "R" (at 2.032 0 90))
    (property "Value" "R" (at 0 0 90))
    (property "Description" "Resistor" (at 0 0 0))
    (property "ki_keywords" "R res resistor" (at 0 0 0))
    (symbol "R_0_1"
      (rectangle (start -1.016 -2.54) (end 1.016 2.54)
        (stroke (width 0.254) (type default))
        (fill (type none))))
    (symbol "R_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27)
        (name "" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 0 -3.81 90) (length 1.27)
        (name "" (effects (font (size 1.27 1.27))))
        (number "2" (effects (font (size 1.27 1.27))))))))"#;

    #[test]
    fn parses_device_r() {
        let tree = sexpr::parse(DEVICE_R).unwrap();
        let syms = extract_lib(&tree).unwrap();
        assert_eq!(syms.len(), 1);
        let s = &syms[0];
        assert_eq!(s.name, "R");
        assert!(s.extends.is_none());
        assert_eq!(s.properties.get("Reference").unwrap().value, "R");
        assert_eq!(s.pins.len(), 2);
        assert_eq!(s.pins[0].number, "1");
        assert_eq!(s.pins[0].electrical, "passive");
        assert_eq!(s.pins[0].length, 127); // 1.27 mm -> 127 (1/100 mm)
        assert_eq!(s.graphics.len(), 1);
        match &s.graphics[0].kind {
            ParsedGraphicKind::Rectangle { sx, sy, ex, ey } => {
                assert_eq!((*sx, *sy, *ex, *ey), (-102, -254, 102, 254));
            }
            _ => panic!("expected rectangle"),
        }
        assert!(s.units.contains(&(0, 1)));
        assert!(s.units.contains(&(1, 1)));
        assert!(s.flags.in_bom);
        assert!(s.flags.on_board);
        assert!(s.flags.hide_pin_numbers);
    }

    const EXTENDS_CHILD: &str = r#"(kicad_symbol_lib
  (version 20251024)
  (symbol "ATmega328P-A"
    (extends "ATmega48PV-10A")
    (property "Value" "ATmega328P-A" (at 0 0 0))
    (property "Description" "20MHz MCU" (at 0 0 0))))"#;

    const TOKITO_FORMAT_R: &str = r#"(tokito_symbol_lib
  (version 20251024)
  (generator "tokito_symbol_gen")
  (generator_version "2.0")
  (symbol "R"
    (in_bom yes)
    (on_board yes)
    (property "Reference" "R" (at 2.032 0 90))
    (property "Value" "R" (at 0 0 90))
    (property "Description" "Resistor" (at 0 0 0))
    (property "keywords" "R res resistor" (at 0 0 0))
    (property "fp_filters" "R_*" (at 0 0 0))
    (symbol "R_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27)
        (name "" (effects (font (size 1.27 1.27))))
        (number "1" (effects (font (size 1.27 1.27))))))))"#;

    #[test]
    fn parses_tokito_format_root() {
        let tree = sexpr::parse(TOKITO_FORMAT_R).unwrap();
        let syms = extract_lib(&tree).unwrap();
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "R");
        // Tokito format uses bare `keywords` / `fp_filters`.
        assert_eq!(syms[0].properties.get("keywords").unwrap().value, "R res resistor");
        assert_eq!(syms[0].properties.get("fp_filters").unwrap().value, "R_*");
    }

    #[test]
    fn parses_extends_child() {
        let tree = sexpr::parse(EXTENDS_CHILD).unwrap();
        let syms = extract_lib(&tree).unwrap();
        assert_eq!(syms.len(), 1);
        let s = &syms[0];
        assert_eq!(s.name, "ATmega328P-A");
        assert_eq!(s.extends.as_deref(), Some("ATmega48PV-10A"));
        assert_eq!(s.pins.len(), 0);
        assert_eq!(s.graphics.len(), 0);
    }

    #[test]
    fn unit_suffix_parsing() {
        assert_eq!(parse_unit_suffix("R_0_1", "R"), Some((0, 1)));
        assert_eq!(parse_unit_suffix("LM358_2_1", "LM358"), Some((2, 1)));
        assert_eq!(parse_unit_suffix("LM358_0_1", "LM358"), Some((0, 1)));
        // Parent name with its own underscores
        assert_eq!(
            parse_unit_suffix("ATmega328P-A_0_1", "ATmega328P-A"),
            Some((0, 1))
        );
        // Garbage suffix
        assert_eq!(parse_unit_suffix("R_foo", "R"), None);
    }
}
