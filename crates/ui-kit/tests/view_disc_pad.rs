#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Вид площадки: перекрестие в центре узла, шайба в позиции точки (y вверх → вверх на экране).

use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::disc_pad::{DiscPad, DiscPadSpec, DiscPoint};
use ui_kit::tokens::UiKitTokens;
use ui_kit::view::{PxPerPt, ViewError, disc_pad_items};

const SCALE: f32 = 3.0;
const CENTER: (f32, f32) = (600.0, 800.0);

fn placement(opacity: f32) -> NodePlacementPx {
    NodePlacementPx {
        size: PxSize::new(300.0, 300.0),
        transform: Affine2::translate(CENTER.0, CENTER.1),
        opacity,
    }
}

fn pad_at(x: f64, y: f64) -> (DiscPad, UiKitTokens) {
    let tokens = UiKitTokens::default();
    let spec = DiscPadSpec::with_token_drive(100.0, &tokens.disc_pad);
    (DiscPad::new(spec, DiscPoint::clamped(x, y).unwrap()).unwrap(), tokens)
}

fn items_for(pad: &DiscPad, tokens: &UiKitTokens) -> Vec<DrawItem> {
    disc_pad_items(
        &placement(1.0),
        pad,
        PxPerPt::new(SCALE).unwrap(),
        &tokens.disc_pad,
        &tokens.colors,
    )
    .unwrap()
}

#[test]
fn puck_sits_at_point_with_up_on_screen_and_crosshair_at_center() {
    let (pad, tokens) = pad_at(0.5, 0.25);
    let items = items_for(&pad, &tokens);
    assert_eq!(items.len(), 3, "две линии перекрестия и шайба");

    let DrawItem::Shape(puck) = &items[2] else {
        panic!("{:?}", items[2])
    };
    let ShapeGeometry::Circle { diameter } = puck.geometry else {
        panic!()
    };
    assert_eq!(diameter, tokens.disc_pad.puck_diameter_pt as f32 * SCALE);
    assert!(puck.stroke.is_some(), "у шайбы есть обводка");
    let center = puck.transform.apply_point(PxPoint::ORIGIN);
    // x = 0,5 · 100 pt вправо; y = 0,25 · 100 pt ВВЕРХ = экранный y меньше.
    assert!((center.x - (CENTER.0 + 50.0 * SCALE)).abs() < 1e-3, "{center:?}");
    assert!((center.y - (CENTER.1 - 25.0 * SCALE)).abs() < 1e-3, "{center:?}");

    for cross in &items[..2] {
        let DrawItem::Shape(line) = cross else { panic!() };
        let at = line.transform.apply_point(PxPoint::ORIGIN);
        assert!((at.x - CENTER.0).abs() < 1e-3 && (at.y - CENTER.1).abs() < 1e-3);
    }
}

#[test]
fn puck_follows_the_pad_value() {
    let (mut pad, tokens) = pad_at(0.0, 0.0);
    let before = items_for(&pad, &tokens);
    pad.set_point(DiscPoint::clamped(-1.0, 0.0).unwrap());
    let after = items_for(&pad, &tokens);
    assert_ne!(before[2], after[2]);
    let DrawItem::Shape(puck) = &after[2] else { panic!() };
    let center = puck.transform.apply_point(PxPoint::ORIGIN);
    assert!((center.x - (CENTER.0 - 100.0 * SCALE)).abs() < 1e-3, "{center:?}");
}

#[test]
fn invalid_node_opacity_is_an_error_not_a_silent_clamp() {
    let (pad, tokens) = pad_at(0.0, 0.0);
    let result = disc_pad_items(
        &placement(f32::NAN),
        &pad,
        PxPerPt::new(SCALE).unwrap(),
        &tokens.disc_pad,
        &tokens.colors,
    );
    assert!(matches!(result, Err(ViewError::InvalidOpacity { .. })), "{result:?}");
}

#[test]
fn crosshair_and_puck_use_token_sizes_alphas_and_stroke() {
    let (pad, tokens) = pad_at(0.0, 0.0);
    let items = items_for(&pad, &tokens);
    let pad_tokens = tokens.disc_pad;
    let shape = |index: usize| match &items[index] {
        DrawItem::Shape(shape) => shape.clone(),
        other => panic!("{other:?}"),
    };
    // Горизонтальная линия: длина × толщина; вертикальная — наоборот.
    let ShapeGeometry::RoundedRect { size: horizontal, .. } = shape(0).geometry else {
        panic!()
    };
    assert_eq!(
        horizontal,
        PxSize::new(
            pad_tokens.crosshair_length_pt as f32 * SCALE,
            pad_tokens.crosshair_thickness_pt as f32 * SCALE
        )
    );
    let ShapeGeometry::RoundedRect { size: vertical, .. } = shape(1).geometry else {
        panic!()
    };
    assert_eq!(
        vertical,
        PxSize::new(
            pad_tokens.crosshair_thickness_pt as f32 * SCALE,
            pad_tokens.crosshair_length_pt as f32 * SCALE
        )
    );
    assert_eq!(shape(0).fill.a, pad_tokens.crosshair_alpha);
    assert_eq!(shape(1).fill.a, pad_tokens.crosshair_alpha);

    let puck = shape(2);
    assert_eq!(puck.fill.a, pad_tokens.puck_fill_alpha);
    let stroke = puck.stroke.expect("обводка");
    assert_eq!(stroke.width_px, pad_tokens.puck_stroke_pt as f32 * SCALE);
    assert_eq!(stroke.color.a, 1.0);
}

#[test]
fn node_opacity_reaches_every_item() {
    let (pad, tokens) = pad_at(0.2, 0.2);
    let items = disc_pad_items(
        &placement(0.4),
        &pad,
        PxPerPt::new(SCALE).unwrap(),
        &tokens.disc_pad,
        &tokens.colors,
    )
    .unwrap();
    assert_eq!(items.len(), 3);
    for item in &items {
        let DrawItem::Shape(shape) = item else { panic!() };
        assert_eq!(shape.opacity.get(), 0.4);
    }
}
