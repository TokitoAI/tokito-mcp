//! Wire + storage types for symbols.
//!
//! Coordinates are i32 1/100 mm throughout (1 unit = 10 µm). Pin-name and
//! number strings live owned per pin — they're short and there are few of
//! them per symbol, so interning isn't worth the complexity.

use serde::{Deserialize, Serialize};

/// What gets stored as `symbol.body` (postcard-encoded) for root symbols.
///
/// Extends children store NULL here — their inherited body comes from the
/// parent chain at resolve time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolBody {
    pub pins: Vec<Pin>,
    pub graphics: Vec<Graphic>,
    pub units: Vec<Unit>,
    pub props_layout: Vec<PropPlacement>,
    pub flags: SymbolFlags,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SymbolFlags {
    pub in_bom: bool,
    pub on_board: bool,
    pub in_pos_files: bool,
    pub exclude_from_sim: bool,
    pub hide_pin_numbers: bool,
    pub duplicate_pin_numbers_are_jumpers: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub number: String,
    pub name: String,
    pub electrical: PinElectrical,
    pub style: PinStyle,
    /// Position in 1/100 mm.
    pub x: i32,
    pub y: i32,
    /// Rotation in degrees, multiple of 90.
    pub rotation: i16,
    /// Length in 1/100 mm.
    pub length: i32,
    /// Which unit (1-based) this pin belongs to. 0 = common.
    pub unit: u8,
    /// Body style (1 or 2 in KiCad — usually 1).
    pub body_style: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinElectrical {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinStyle {
    #[default]
    Line,
    Inverted,
    Clock,
    InvertedClock,
    InputLow,
    ClockLow,
    OutputLow,
    EdgeClockHigh,
    NonLogic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graphic {
    pub unit: u8,
    pub body_style: u8,
    pub kind: GraphicKind,
    pub stroke: Stroke,
    pub fill: Fill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphicKind {
    Rectangle { start: Point, end: Point },
    Circle { center: Point, radius: i32 },
    Arc { start: Point, mid: Point, end: Point },
    Polyline { points: Vec<Point> },
    Bezier { points: Vec<Point> },
    Text { at: Point, rotation: i16, content: String, italic: bool, bold: bool },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Stroke {
    /// Width in 1/100 mm. 0 = default.
    pub width: i32,
    pub kind: StrokeKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeKind {
    #[default]
    Default,
    Solid,
    Dash,
    DashDot,
    Dot,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    #[default]
    None,
    Outline,
    Background,
}

/// One unit / body-style group from KiCad's `(symbol "X_n_m" ...)` decomposition.
/// Captured here so the renderer knows how to slot pins/graphics into a multi-unit
/// part like a quad opamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub unit: u8,
    pub body_style: u8,
}

/// Where a named property's text appears on the symbol. Properties themselves
/// (Reference, Value, Footprint, Datasheet, Description, ki_keywords, ki_fp_filters)
/// live as catalog columns — only their placement lives in the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropPlacement {
    pub key: PropKey,
    pub at: Point,
    pub rotation: i16,
    pub hide: bool,
    pub show_name: bool,
    pub justify: Justify,
    pub italic: bool,
    pub bold: bool,
    /// Font size in 1/100 mm.
    pub font_size: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKey {
    Reference,
    Value,
    Footprint,
    Datasheet,
    Description,
    KiKeywords,
    KiFpFilters,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Center,
    Left,
    Right,
    Top,
    Bottom,
    LeftBottom,
    LeftTop,
    RightBottom,
    RightTop,
}

/// Catalog row joined with its (resolved) body. What the resolver returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSymbol {
    pub lib: String,
    pub name: String,
    pub ref_des: String,
    pub description: String,
    pub keywords: String,
    pub fp_filters: String,
    pub datasheet: String,
    pub footprint: String,
    pub parent: Option<(String, String)>,
    pub body: SymbolBody,
}

/// A search-result row — catalog metadata only, no body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRef {
    pub lib: String,
    pub name: String,
    pub ref_des: String,
    pub description: String,
    pub keywords: String,
    pub pin_count: u16,
    /// BM25 score from FTS5, lower = better.
    pub score: f32,
}
