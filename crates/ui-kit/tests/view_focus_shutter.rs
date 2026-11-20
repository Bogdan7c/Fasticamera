#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K9/K10, вид: направляющая линия рамки фокуса видна только пока палец на «солнце» и
//! разорвана вокруг него; кольцо затвора — серая заливка со светлым ободком в 1 pt.

use ui_core::geometry::{Point, Rect, Size, Vector};
use ui_core::gesture::PanTranslation;
use ui_core::time::MotionPreference;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::focus_frame::{FocusFrame, FocusFrameSpec, SunInteraction};
use ui_kit::pan::PanUpdate;
use ui_kit::shutter::{ShutterButton, ShutterLook};
use ui_kit::tokens::UiKitTokens;
use ui_kit::value::QuantizedLinear;
use ui_kit::view::{PxPerPt, focus_frame_items, shutter_items};

mod common;
use common::*;

const SCALE: f32 = 3.0;

fn px() -> PxPerPt {
    PxPerPt::new(SCALE).unwrap()
}

fn node(size_pt: f64, center: (f32, f32)) -> NodePlacementPx {
    let side = size_pt as f32 * SCALE;
    NodePlacementPx {
        size: PxSize::new(side, side),
        transform: Affine2::translate(center.0, center.1),
        opacity: 1.0,
    }
}

fn close(actual: f32, expected: f32, what: &str) {
    assert!((actual - expected).abs() < 0.02, "{what}: {actual} != {expected}");
}

fn placed(tokens: &UiKitTokens) -> FocusFrame<QuantizedLinear> {
    let mut frame = FocusFrame::new(
        exposure_model(),
        0.0,
        FocusFrameSpec::default(),
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    frame
}

fn vertical(dy: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(0.0, dy),
        from_recognition: Vector::new(0.0, dy),
    }
}

/// Тонкие вертикальные куски направляющей: ширина 1 pt и высота больше длины тика.
fn guide_pieces(items: &[DrawItem]) -> Vec<(PxPoint, PxSize)> {
    items
        .iter()
        .filter_map(|item| {
            let DrawItem::Shape(shape) = item else { return None };
            let ShapeGeometry::RoundedRect { size, .. } = shape.geometry else {
                return None;
            };
            let is_line = (size.width - SCALE).abs() < 0.01 && size.height > 20.0 * SCALE;
            // Контур рамки — квадрат с обводкой, он сюда не попадает по ширине.
            is_line.then(|| (shape.transform.apply_point(PxPoint::ORIGIN), size))
        })
        .collect()
}

#[test]
fn guide_line_is_drawn_only_while_the_finger_is_on_the_sun() {
    let tokens = UiKitTokens::default();
    let mut frame = placed(&tokens);
    let frame_node = node(74.7, (600.0, 1200.0));
    let idle = frame.snapshot(ms(500));
    let idle_items = focus_frame_items(&frame_node, &idle, px(), &tokens.focus, &tokens.colors).unwrap();
    assert!(guide_pieces(&idle_items).is_empty(), "без пальца линии нет");
    assert_eq!(idle_items.len(), 14);

    frame
        .apply_pan(
            ms(600),
            PanUpdate::Began {
                translation: vertical(0.0),
            },
        )
        .unwrap();
    frame
        .apply_pan(
            ms(610),
            PanUpdate::Changed {
                translation: vertical(-10.0),
            },
        )
        .unwrap();
    let held = frame.snapshot(ms(610));
    assert_eq!(held.sun_interaction, SunInteraction::Dragged);
    let held_items = focus_frame_items(&frame_node, &held, px(), &tokens.focus, &tokens.colors).unwrap();
    // Диск и лучи на месте, плюс линия: до 14 + 2 куска.
    assert!(!guide_pieces(&held_items).is_empty());
    assert!(held_items.len() > idle_items.len());

    frame
        .apply_pan(ms(620), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
    let released = frame.snapshot(ms(2000));
    let released_items = focus_frame_items(&frame_node, &released, px(), &tokens.focus, &tokens.colors).unwrap();
    assert!(guide_pieces(&released_items).is_empty(), "палец отпущен: линии нет");
}

#[test]
fn guide_line_is_143_pt_long_centred_on_the_sun_start_with_a_gap_around_the_sun() {
    let tokens = UiKitTokens::default();
    let frame = placed(&tokens);
    let frame_node = node(74.7, (600.0, 1200.0));
    let mut snapshot = frame.snapshot(ms(2000));
    snapshot.sun_interaction = SunInteraction::Dragged;

    // «Солнце» на старте: разрыв посередине линии.
    let at_start = focus_frame_items(&frame_node, &snapshot, px(), &tokens.focus, &tokens.colors).unwrap();
    let pieces = guide_pieces(&at_start);
    assert_eq!(pieces.len(), 2);
    let sun_x = 600.0 + (37.35 + 17.8) * SCALE;
    for (centre, _) in &pieces {
        close(centre.x, sun_x, "линия на оси «солнца»");
    }
    let (upper, lower) = (pieces[0], pieces[1]);
    // Вместе с разрывом 2 × 15,5 pt куски дают ровно 143 pt.
    close(
        (upper.1.height + lower.1.height) / SCALE + 2.0 * 15.5,
        143.0,
        "полная длина линии",
    );
    let (upper_end, lower_start) = (upper.0.y + upper.1.height * 0.5, lower.0.y - lower.1.height * 0.5);
    close(lower_start - upper_end, 2.0 * 15.5 * SCALE, "разрыв вокруг «солнца»");
    close((upper_end + lower_start) * 0.5, 1200.0, "разрыв вокруг «солнца»: центр");

    // «Солнце» поднято на 30 pt: линия остаётся на месте, разрыв уехал вместе с ним.
    snapshot.sun_travel_up_pt = 30.0;
    let raised = focus_frame_items(&frame_node, &snapshot, px(), &tokens.focus, &tokens.colors).unwrap();
    let pieces = guide_pieces(&raised);
    let top = pieces
        .iter()
        .map(|(c, s)| c.y - s.height * 0.5)
        .fold(f32::MAX, f32::min);
    let bottom = pieces
        .iter()
        .map(|(c, s)| c.y + s.height * 0.5)
        .fold(f32::MIN, f32::max);
    close(top, 1200.0 - 71.5 * SCALE, "верх линии не сдвинулся");
    close(bottom, 1200.0 + 71.5 * SCALE, "низ линии не сдвинулся");
    let gap_centre = 1200.0 - 30.0 * SCALE;
    for (centre, size) in &pieces {
        let (from, to) = (centre.y - size.height * 0.5, centre.y + size.height * 0.5);
        assert!(
            to <= gap_centre - 15.5 * SCALE + 0.02 || from >= gap_centre + 15.5 * SCALE - 0.02,
            "кусок {from}..{to} заходит в разрыв вокруг {gap_centre}"
        );
    }
}

#[test]
fn shutter_ring_is_a_grey_translucent_fill_with_a_one_point_rim_inside_the_outer_edge() {
    let tokens = UiKitTokens::default();
    let placement = node(80.1, (660.0, 2405.7));
    let shutter = ShutterButton::new(
        Rect::centered_at(Point::new(220.0, 801.9), Size::new(80.1, 80.1)),
        ShutterLook::Ready,
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    let items = shutter_items(
        &placement,
        &shutter.snapshot(ms(0)),
        &tokens.glass.button,
        px(),
        &tokens.shutter,
        &tokens.colors,
    )
    .unwrap();
    let [DrawItem::Glass(_), DrawItem::Shape(ring), DrawItem::Shape(disc)] = items.as_slice() else {
        panic!("ожидались стекло, кольцо и диск: {items:?}");
    };
    // Заливка — серая полупрозрачная (белый с малой альфой), не белая и не прозрачная.
    assert_eq!(ring.fill, tokens.shutter.ring_fill);
    assert!(ring.fill.a > 0.05 && ring.fill.a < 0.5);
    // Ободок 1 pt светлее заливки и целиком внутри внешнего диаметра Ø 80,1.
    let rim = ring.stroke.expect("у кольца есть ободок");
    close(rim.width_px, SCALE, "толщина ободка");
    assert_eq!(rim.color, tokens.shutter.ring_rim_color);
    assert!(rim.color.a > ring.fill.a, "ободок светлее заливки");
    let ShapeGeometry::Circle { diameter } = ring.geometry else {
        panic!()
    };
    close(
        diameter + rim.width_px,
        80.1 * SCALE,
        "внешний край обводки = внешний диаметр",
    );
    // Диск Ø 67,9 меньше кольца и белый: между ними остаётся серое кольцо.
    let ShapeGeometry::Circle {
        diameter: disc_diameter,
    } = disc.geometry
    else {
        panic!()
    };
    close(disc_diameter, 67.9 * SCALE, "диск");
    assert_eq!(disc.fill, DisplayColor::WHITE);
}
