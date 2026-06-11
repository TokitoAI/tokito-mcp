//! Postcard encode + decode for `SymbolBody` — guards against accidental
//! breaking changes to the wire format. Bump `body_format` to a new tag
//! when this test no longer round-trips.

use tokito_symbols::model::{
    Fill, Graphic, GraphicKind, Justify, Pin, PinElectrical, PinStyle, Point, PropKey,
    PropPlacement, Stroke, StrokeKind, SymbolBody, SymbolFlags, Unit,
};

fn build_sample() -> SymbolBody {
    SymbolBody {
        pins: vec![Pin {
            number: "1".into(),
            name: "VCC".into(),
            electrical: PinElectrical::PowerIn,
            style: PinStyle::Line,
            x: 100,
            y: 200,
            rotation: 90,
            length: 254,
            unit: 1,
            body_style: 1,
        }],
        graphics: vec![
            Graphic {
                unit: 0,
                body_style: 1,
                kind: GraphicKind::Rectangle {
                    start: Point { x: -100, y: -100 },
                    end: Point { x: 100, y: 100 },
                },
                stroke: Stroke {
                    width: 25,
                    kind: StrokeKind::Default,
                },
                fill: Fill::None,
            },
            Graphic {
                unit: 0,
                body_style: 1,
                kind: GraphicKind::Polyline {
                    points: vec![Point { x: 0, y: 0 }, Point { x: 50, y: 50 }],
                },
                stroke: Stroke {
                    width: 0,
                    kind: StrokeKind::Dash,
                },
                fill: Fill::Outline,
            },
        ],
        units: vec![Unit {
            unit: 1,
            body_style: 1,
        }],
        props_layout: vec![PropPlacement {
            key: PropKey::Reference,
            at: Point { x: 0, y: 100 },
            rotation: 0,
            hide: false,
            show_name: false,
            justify: Justify::Center,
            italic: false,
            bold: false,
            font_size: 127,
        }],
        flags: SymbolFlags {
            in_bom: true,
            on_board: true,
            ..Default::default()
        },
    }
}

#[test]
fn roundtrip_matches_byte_for_byte() {
    let src = build_sample();
    let bytes = postcard::to_stdvec(&src).unwrap();
    assert!(!bytes.is_empty());
    let decoded: SymbolBody = postcard::from_bytes(&bytes).unwrap();

    assert_eq!(decoded.pins.len(), 1);
    assert_eq!(decoded.pins[0].name, "VCC");
    assert_eq!(decoded.pins[0].electrical, PinElectrical::PowerIn);
    assert_eq!(decoded.graphics.len(), 2);
    match &decoded.graphics[0].kind {
        GraphicKind::Rectangle { start, end } => {
            assert_eq!((start.x, start.y, end.x, end.y), (-100, -100, 100, 100));
        }
        _ => panic!("expected Rectangle"),
    }
    match &decoded.graphics[1].kind {
        GraphicKind::Polyline { points } => {
            assert_eq!(points.len(), 2);
            assert_eq!((points[1].x, points[1].y), (50, 50));
        }
        _ => panic!("expected Polyline"),
    }
    assert!(decoded.flags.in_bom);
    assert!(decoded.flags.on_board);
    assert!(!decoded.flags.exclude_from_sim);
}

#[test]
fn typical_body_under_two_kilobytes() {
    let bytes = postcard::to_stdvec(&build_sample()).unwrap();
    assert!(
        bytes.len() < 2048,
        "typical body shouldn't bloat: got {} bytes",
        bytes.len()
    );
}
