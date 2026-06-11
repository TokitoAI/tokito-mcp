//! Build `symbols.sqlite` from a `Vec<Ingested>`.
//!
//! Strategy: two passes inside one transaction.
//!   1. INSERT every symbol with `parent_id = NULL`. For non-extending
//!      symbols, fill the `body` BLOB with a postcard-encoded `SymbolBody`.
//!      For extending children, leave `body = NULL`.
//!   2. UPDATE `parent_id` on each extending child by looking up the
//!      `(lib_id, parent_name)` pair we already inserted.
//!   3. Walk the chain to backfill `pin_count` on children from the root.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use tokito_symbols::model::{
    Fill, Graphic, GraphicKind, Justify, Pin, PinElectrical, PinStyle, Point, PropKey,
    PropPlacement, StrokeKind, SymbolBody, SymbolFlags, Unit,
};

use crate::{ingest::Ingested, kicad::*};

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // DanglingExtends is reported via Stats today; reserved as an error variant.
pub enum EmitError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("postcard encode: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("dangling extends: {child_lib}:{child_name} -> {parent_name} (parent not found)")]
    DanglingExtends {
        child_lib: String,
        child_name: String,
        parent_name: String,
    },
    #[error("symbols/io")]
    Symbols(#[from] tokito_symbols::Error),
}

pub struct Stats {
    pub lib_count: usize,
    pub symbol_count: usize,
    pub root_count: usize,
    pub extends_count: usize,
    pub dangling_extends: Vec<(String, String, String)>,
}

pub fn build(conn: &mut Connection, items: Vec<Ingested>) -> Result<Stats, EmitError> {
    let mut items = items;
    // Deterministic insertion order: lib then name.
    items.sort_by(|a, b| {
        (a.lib.as_str(), a.symbol.name.as_str()).cmp(&(b.lib.as_str(), b.symbol.name.as_str()))
    });

    let tx = conn.transaction()?;

    // --- libs ---
    let mut lib_ids: HashMap<String, i64> = HashMap::new();
    for it in &items {
        if !lib_ids.contains_key(&it.lib) {
            tx.execute(
                "INSERT OR IGNORE INTO lib(name) VALUES(?1)",
                params![&it.lib],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM lib WHERE name = ?1",
                params![&it.lib],
                |r| r.get(0),
            )?;
            lib_ids.insert(it.lib.clone(), id);
        }
    }

    // --- pass 1: insert symbols ---
    let mut sym_ids: HashMap<(i64, String), i64> = HashMap::new();
    let mut extends_count = 0usize;
    let mut root_count = 0usize;
    {
        let mut insert = tx.prepare(
            "INSERT INTO symbol(
                lib_id, name, ref_des, description, keywords, fp_filters,
                datasheet, footprint, parent_id, pin_count, flags,
                body, body_format
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12)",
        )?;
        for it in &items {
            let lib_id = lib_ids[&it.lib];
            let ps = &it.symbol;
            let props = &ps.properties;

            let body_blob: Option<Vec<u8>> = if ps.extends.is_some() {
                extends_count += 1;
                None
            } else {
                root_count += 1;
                Some(postcard::to_stdvec(&build_body(ps))?)
            };
            let body_format = body_blob
                .as_ref()
                .map(|_| tokito_symbols::BODY_FORMAT_POSTCARD_V1);
            let pin_count = ps.pins.len() as i64;
            let flags_bits = pack_flags(&ps.flags);

            insert.execute(params![
                lib_id,
                &ps.name,
                prop(props, "Reference"),
                prop(props, "Description"),
                prop_either(props, "ki_keywords", "keywords"),
                prop_either(props, "ki_fp_filters", "fp_filters"),
                prop(props, "Datasheet"),
                prop(props, "Footprint"),
                pin_count,
                flags_bits,
                body_blob,
                body_format,
            ])?;
            let id = tx.last_insert_rowid();
            sym_ids.insert((lib_id, ps.name.clone()), id);
        }
    }

    // --- pass 2: link parent_id ---
    let mut dangling: Vec<(String, String, String)> = Vec::new();
    {
        let mut update = tx.prepare("UPDATE symbol SET parent_id = ?1 WHERE id = ?2")?;
        for it in &items {
            let Some(parent_name) = &it.symbol.extends else {
                continue;
            };
            let lib_id = lib_ids[&it.lib];
            let child_id = sym_ids[&(lib_id, it.symbol.name.clone())];
            match sym_ids.get(&(lib_id, parent_name.clone())) {
                Some(&parent_id) => {
                    update.execute(params![parent_id, child_id])?;
                }
                None => {
                    dangling.push((it.lib.clone(), it.symbol.name.clone(), parent_name.clone()));
                }
            }
        }
    }

    // --- pass 3: backfill pin_count from root via iterative update ---
    // Max depth in CERN data is 4; this loop terminates in <= max-depth passes.
    loop {
        let n = tx.execute(
            "UPDATE symbol SET pin_count = (
                SELECT p.pin_count FROM symbol p WHERE p.id = symbol.parent_id
             ) WHERE parent_id IS NOT NULL AND pin_count = 0
               AND (SELECT pin_count FROM symbol p WHERE p.id = symbol.parent_id) > 0",
            [],
        )?;
        if n == 0 {
            break;
        }
    }

    tx.commit()?;

    Ok(Stats {
        lib_count: lib_ids.len(),
        symbol_count: items.len(),
        root_count,
        extends_count,
        dangling_extends: dangling,
    })
}

fn prop(props: &std::collections::BTreeMap<String, ParsedProperty>, key: &str) -> String {
    props.get(key).map(|p| p.value.clone()).unwrap_or_default()
}

/// Looks up a property under either of two keys — used to accept both the
/// KiCad-style `ki_keywords` / `ki_fp_filters` and the Tokito-style
/// `keywords` / `fp_filters` without forcing the caller to know which file
/// format produced the input.
fn prop_either(
    props: &std::collections::BTreeMap<String, ParsedProperty>,
    primary: &str,
    fallback: &str,
) -> String {
    props
        .get(primary)
        .or_else(|| props.get(fallback))
        .map(|p| p.value.clone())
        .unwrap_or_default()
}

fn pack_flags(f: &ParsedFlags) -> i64 {
    (f.in_bom as i64)
        | ((f.on_board as i64) << 1)
        | ((f.in_pos_files as i64) << 2)
        | ((f.exclude_from_sim as i64) << 3)
        | ((f.hide_pin_numbers as i64) << 4)
        | ((f.duplicate_pin_numbers_are_jumpers as i64) << 5)
}

fn build_body(ps: &ParsedSymbol) -> SymbolBody {
    SymbolBody {
        pins: ps.pins.iter().map(convert_pin).collect(),
        graphics: ps.graphics.iter().map(convert_graphic).collect(),
        units: ps
            .units
            .iter()
            .map(|&(u, b)| Unit {
                unit: u,
                body_style: b,
            })
            .collect(),
        props_layout: ps
            .properties
            .iter()
            .filter_map(|(k, v)| convert_prop_layout(k, v))
            .collect(),
        flags: SymbolFlags {
            in_bom: ps.flags.in_bom,
            on_board: ps.flags.on_board,
            in_pos_files: ps.flags.in_pos_files,
            exclude_from_sim: ps.flags.exclude_from_sim,
            hide_pin_numbers: ps.flags.hide_pin_numbers,
            duplicate_pin_numbers_are_jumpers: ps.flags.duplicate_pin_numbers_are_jumpers,
        },
    }
}

fn convert_pin(p: &ParsedPin) -> Pin {
    Pin {
        number: p.number.clone(),
        name: p.name.clone(),
        electrical: pin_electrical(&p.electrical),
        style: pin_style(&p.style),
        x: p.x,
        y: p.y,
        rotation: p.rotation,
        length: p.length,
        unit: p.unit,
        body_style: p.body_style,
    }
}

fn pin_electrical(s: &str) -> PinElectrical {
    match s {
        "input" => PinElectrical::Input,
        "output" => PinElectrical::Output,
        "bidirectional" => PinElectrical::Bidirectional,
        "tri_state" => PinElectrical::TriState,
        "passive" => PinElectrical::Passive,
        "free" => PinElectrical::Free,
        "power_in" => PinElectrical::PowerIn,
        "power_out" => PinElectrical::PowerOut,
        "open_collector" => PinElectrical::OpenCollector,
        "open_emitter" => PinElectrical::OpenEmitter,
        "no_connect" => PinElectrical::NoConnect,
        _ => PinElectrical::Unspecified,
    }
}

fn pin_style(s: &str) -> PinStyle {
    match s {
        "inverted" => PinStyle::Inverted,
        "clock" => PinStyle::Clock,
        "inverted_clock" => PinStyle::InvertedClock,
        "input_low" => PinStyle::InputLow,
        "clock_low" => PinStyle::ClockLow,
        "output_low" => PinStyle::OutputLow,
        "edge_clock_high" => PinStyle::EdgeClockHigh,
        "non_logic" => PinStyle::NonLogic,
        _ => PinStyle::Line,
    }
}

fn convert_graphic(g: &ParsedGraphic) -> Graphic {
    Graphic {
        unit: g.unit,
        body_style: g.body_style,
        kind: match &g.kind {
            ParsedGraphicKind::Rectangle { sx, sy, ex, ey } => GraphicKind::Rectangle {
                start: Point { x: *sx, y: *sy },
                end: Point { x: *ex, y: *ey },
            },
            ParsedGraphicKind::Circle { cx, cy, radius } => GraphicKind::Circle {
                center: Point { x: *cx, y: *cy },
                radius: *radius,
            },
            ParsedGraphicKind::Arc {
                sx,
                sy,
                mx,
                my,
                ex,
                ey,
            } => GraphicKind::Arc {
                start: Point { x: *sx, y: *sy },
                mid: Point { x: *mx, y: *my },
                end: Point { x: *ex, y: *ey },
            },
            ParsedGraphicKind::Polyline { points } => GraphicKind::Polyline {
                points: points.iter().map(|&(x, y)| Point { x, y }).collect(),
            },
            ParsedGraphicKind::Bezier { points } => GraphicKind::Bezier {
                points: points.iter().map(|&(x, y)| Point { x, y }).collect(),
            },
            ParsedGraphicKind::Text {
                x,
                y,
                rotation,
                content,
                italic,
                bold,
            } => GraphicKind::Text {
                at: Point { x: *x, y: *y },
                rotation: *rotation,
                content: content.clone(),
                italic: *italic,
                bold: *bold,
            },
        },
        stroke: tokito_symbols::model::Stroke {
            width: g.stroke_width,
            kind: stroke_kind(&g.stroke_kind),
        },
        fill: fill_kind(&g.fill),
    }
}

fn stroke_kind(s: &str) -> StrokeKind {
    match s {
        "solid" => StrokeKind::Solid,
        "dash" => StrokeKind::Dash,
        "dash_dot" => StrokeKind::DashDot,
        "dot" => StrokeKind::Dot,
        _ => StrokeKind::Default,
    }
}

fn fill_kind(s: &str) -> Fill {
    match s {
        "outline" => Fill::Outline,
        "background" => Fill::Background,
        _ => Fill::None,
    }
}

fn convert_prop_layout(key: &str, p: &ParsedProperty) -> Option<PropPlacement> {
    let key = match key {
        "Reference" => PropKey::Reference,
        "Value" => PropKey::Value,
        "Footprint" => PropKey::Footprint,
        "Datasheet" => PropKey::Datasheet,
        "Description" => PropKey::Description,
        "ki_keywords" => PropKey::KiKeywords,
        "ki_fp_filters" => PropKey::KiFpFilters,
        _ => return None,
    };
    Some(PropPlacement {
        key,
        at: Point { x: p.x, y: p.y },
        rotation: p.rotation,
        hide: p.hide,
        show_name: p.show_name,
        justify: justify_kind(&p.justify),
        italic: p.italic,
        bold: p.bold,
        font_size: p.font_size,
    })
}

fn justify_kind(s: &str) -> Justify {
    match s {
        "left" => Justify::Left,
        "right" => Justify::Right,
        "top" => Justify::Top,
        "bottom" => Justify::Bottom,
        "left_bottom" => Justify::LeftBottom,
        "left_top" => Justify::LeftTop,
        "right_bottom" => Justify::RightBottom,
        "right_top" => Justify::RightTop,
        _ => Justify::Center,
    }
}
