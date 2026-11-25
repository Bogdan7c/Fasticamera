#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K3, вид шкалы (`view::scale_items`): риски стоят там, где их положение по `marks()`/`snapshot()`,
//! крупные длиннее, отметка текущего значения на месте и жёлтая, вертикальная шкала повёрнута.

use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::light::LightDirection;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::drive::{DragOrigin, GrowthDirection, ScrollTransition};
use ui_kit::scale::{ScaleSpec, TickVisibility};
use ui_kit::tokens::UiKitTokens;
use ui_kit::value::Emphasis;
use ui_kit::view::{PxPerPt, ViewError, scale_items};

mod common;
use common::*;

const SCALE: f32 = 3.0;
/// Центр узла шкалы на экране, px.
const CENTER: (f32, f32) = (660.0, 900.0);

fn px() -> PxPerPt {
    PxPerPt::new(SCALE).unwrap()
}

/// Узел шкалы `extent_px` вдоль оси и 120 px поперёк (оба направления проверяются одним помощником).
fn node(width_px: f32, height_px: f32, opacity: f32) -> NodePlacementPx {
    NodePlacementPx {
        size: PxSize::new(width_px, height_px),
        transform: Affine2::translate(CENTER.0, CENTER.1),
        opacity,
    }
}

/// Одна нарисованная риска: центр на экране, размер и цвет.
#[derive(Debug, Clone, Copy)]
struct Tick {
    center: PxPoint,
    size: PxSize,
    fill: DisplayColor,
}

fn ticks(items: &[DrawItem]) -> Vec<Tick> {
    items
        .iter()
        .map(|item| {
            let DrawItem::Shape(shape) = item else {
                panic!("шкала должна рисовать только фигуры: {item:?}")
            };
            let ShapeGeometry::RoundedRect { size, .. } = shape.geometry else {
                panic!("риска — прямоугольник: {:?}", shape.geometry)
            };
            Tick {
                center: shape.transform.apply_point(PxPoint::ORIGIN),
                size,
                fill: shape.fill,
            }
        })
        .collect()
}

fn scale_with(
    direction: GrowthDirection,
    value: f64,
    tokens: &UiKitTokens,
) -> ui_kit::scale::SnapScale<ui_kit::value::QuantizedLinear> {
    exposure_scale(value, exposure_spec(direction, DragOrigin::FromRecognition), tokens)
}

fn close(actual: f32, expected: f32, what: &str) {
    assert!((actual - expected).abs() < 0.01, "{what}: {actual} != {expected}");
}

#[test]
fn horizontal_ticks_stand_at_their_mark_offsets_in_pixels() {
    let tokens = UiKitTokens::default();
    let scale = scale_with(GrowthDirection::FingerLeft, 0.0, &tokens);
    let placement = node(660.0, 120.0, 1.0);
    let items = scale_items(&placement, &scale, ms(0), px(), &tokens.scale_ticks, &tokens.colors).unwrap();
    let drawn = ticks(&items);

    // Ожидание строим из `marks()` независимо от вида: риски в пределах ±330 px от центра.
    let current = scale.snapshot(ms(0)).offset_points;
    let expected: Vec<(f32, Emphasis)> = scale
        .marks()
        .iter()
        .map(|mark| (((mark.offset_points - current) as f32) * SCALE, mark.emphasis))
        .filter(|(along, _)| along.abs() <= 330.0)
        .collect();
    assert!(
        expected.len() > 10,
        "в рамке должно быть много рисок: {}",
        expected.len()
    );
    // Последний элемент — отметка текущего значения, остальные — риски по порядку marks().
    assert_eq!(drawn.len(), expected.len() + 1);
    for (tick, (along, emphasis)) in drawn.iter().zip(&expected) {
        close(tick.center.x, CENTER.0 + along, "x риски");
        close(tick.center.y, CENTER.1, "y риски");
        // Вертикальная риска: тонкая (1,5 pt), высота по виду выделения.
        close(tick.size.width, 1.5 * SCALE, "толщина");
        let length_pt = match emphasis {
            Emphasis::Tick => tokens.scale_ticks.tick_length_pt,
            Emphasis::Reference => tokens.scale_ticks.reference_length_pt,
            Emphasis::Step => panic!("у шкалы EV риска на каждом шаге, шага без риски нет"),
        };
        close(tick.size.height, length_pt as f32 * SCALE, "длина");
    }
    // Крупная риска (ноль) длиннее обычной и ровно одна на этой шкале в рамке.
    let long = drawn[..expected.len()]
        .iter()
        .filter(|t| t.size.height > 10.0 * SCALE)
        .count();
    assert_eq!(long, 1);
}

#[test]
fn current_value_mark_is_yellow_longest_and_stays_at_the_centre_while_ticks_move() {
    let tokens = UiKitTokens::default();
    let mut scale = scale_with(GrowthDirection::FingerLeft, 0.0, &tokens);
    scale.set_value(1.0, ScrollTransition::Immediate, ms(0)).unwrap();
    let placement = node(660.0, 120.0, 1.0);
    let items = scale_items(&placement, &scale, ms(0), px(), &tokens.scale_ticks, &tokens.colors).unwrap();
    let drawn = ticks(&items);

    let mark = drawn.last().unwrap();
    assert_eq!(mark.fill, tokens.colors.focus_yellow);
    close(mark.center.x, CENTER.0, "отметка в центре");
    close(
        mark.size.height,
        tokens.scale_ticks.current_mark_length_pt as f32 * SCALE,
        "длина отметки",
    );
    assert!(drawn.iter().all(|tick| tick.size.height <= mark.size.height));

    // Значение 1,0 = 3 шага × 12 pt = 36 pt: «ноль» уехал на 36 pt влево от центра (контент едет за пальцем).
    let zero = drawn
        .iter()
        .find(|tick| (tick.size.height - tokens.scale_ticks.reference_length_pt as f32 * SCALE).abs() < 0.01)
        .unwrap();
    close(zero.center.x, CENTER.0 - 36.0 * SCALE, "ноль после сдвига значения");
    // Крупная риска — основным цветом, обычная — приглушённым.
    assert_eq!(zero.fill, tokens.colors.text_primary);
    assert!(drawn.iter().any(|tick| tick.fill == tokens.colors.text_muted));
}

#[test]
fn vertical_scale_is_rotated_ticks_are_horizontal_and_run_along_y() {
    let tokens = UiKitTokens::default();
    let scale = scale_with(GrowthDirection::FingerUp, 0.0, &tokens);
    let placement = node(120.0, 660.0, 1.0);
    let items = scale_items(&placement, &scale, ms(0), px(), &tokens.scale_ticks, &tokens.colors).unwrap();
    let drawn = ticks(&items);

    let current = scale.snapshot(ms(0)).offset_points;
    let expected: Vec<f32> = scale
        .marks()
        .iter()
        .map(|mark| ((mark.offset_points - current) as f32) * SCALE)
        .filter(|along| along.abs() <= 330.0)
        .collect();
    assert_eq!(drawn.len(), expected.len() + 1);
    for (tick, along) in drawn.iter().zip(&expected) {
        close(tick.center.x, CENTER.0, "x риски вертикальной шкалы");
        close(tick.center.y, CENTER.1 + along, "y риски");
        // Размеры поменялись местами: длина идёт по x, толщина по y.
        close(tick.size.height, 1.5 * SCALE, "толщина по y");
        assert!(tick.size.width >= 10.0 * SCALE - 0.01);
    }
}

#[test]
fn opposite_growth_direction_mirrors_the_marks() {
    let tokens = UiKitTokens::default();
    let placement = node(660.0, 120.0, 1.0);
    let left = scale_with(GrowthDirection::FingerLeft, 0.0, &tokens);
    let right = scale_with(GrowthDirection::FingerRight, 0.0, &tokens);
    let draw = |scale: &ui_kit::scale::SnapScale<ui_kit::value::QuantizedLinear>| {
        ticks(&scale_items(&placement, scale, ms(0), px(), &tokens.scale_ticks, &tokens.colors).unwrap())
    };
    let (from_left, from_right) = (draw(&left), draw(&right));
    // Значение 0 лежит посередине модели ±2, поэтому набор рисок зеркален относительно центра.
    let mut left_x: Vec<f32> = from_left.iter().map(|t| t.center.x - CENTER.0).collect();
    let mut right_x: Vec<f32> = from_right.iter().map(|t| CENTER.0 - t.center.x).collect();
    left_x.sort_by(f32::total_cmp);
    right_x.sort_by(f32::total_cmp);
    assert_eq!(left_x.len(), right_x.len());
    for (a, b) in left_x.iter().zip(&right_x) {
        close(*a, *b, "зеркальная позиция");
    }
    // Но не тождественны на несимметричном значении: при value > 0 «ноль» уезжает в разные стороны.
    let mut shifted_left = scale_with(GrowthDirection::FingerLeft, 0.0, &tokens);
    let mut shifted_right = scale_with(GrowthDirection::FingerRight, 0.0, &tokens);
    shifted_left.set_value(1.0, ScrollTransition::Immediate, ms(0)).unwrap();
    shifted_right
        .set_value(1.0, ScrollTransition::Immediate, ms(0))
        .unwrap();
    let zero_x = |drawn: &[Tick]| {
        drawn
            .iter()
            .find(|t| (t.size.height - tokens.scale_ticks.reference_length_pt as f32 * SCALE).abs() < 0.01)
            .unwrap()
            .center
            .x
    };
    assert!(zero_x(&draw(&shifted_left)) < CENTER.0);
    assert!(zero_x(&draw(&shifted_right)) > CENTER.0);
}

#[test]
fn hidden_ticks_draw_nothing_and_node_opacity_is_applied() {
    let tokens = UiKitTokens::default();
    let hidden = ScaleSpec {
        ticks: TickVisibility::Hidden,
        ..exposure_spec(GrowthDirection::FingerLeft, DragOrigin::FromRecognition)
    };
    let scale = exposure_scale(0.0, hidden, &tokens);
    let items = scale_items(
        &node(660.0, 120.0, 1.0),
        &scale,
        ms(0),
        px(),
        &tokens.scale_ticks,
        &tokens.colors,
    )
    .unwrap();
    assert!(items.is_empty(), "«солнце» без рисок не рисует ничего");

    let visible = scale_with(GrowthDirection::FingerLeft, 0.0, &tokens);
    let items = scale_items(
        &node(660.0, 120.0, 0.5),
        &visible,
        ms(0),
        px(),
        &tokens.scale_ticks,
        &tokens.colors,
    )
    .unwrap();
    assert!(!items.is_empty());
    for item in &items {
        let DrawItem::Shape(shape) = item else { panic!() };
        close(shape.opacity.get(), 0.5, "прозрачность узла");
    }
    // Список из таких элементов проходит проверку словаря рисования.
    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    list.items = items;
    list.validate().unwrap();
}

#[test]
fn invalid_node_opacity_is_an_error_not_a_silent_clamp() {
    let tokens = UiKitTokens::default();
    let scale = scale_with(GrowthDirection::FingerLeft, 0.0, &tokens);
    let result = scale_items(
        &node(660.0, 120.0, 1.5),
        &scale,
        ms(0),
        px(),
        &tokens.scale_ticks,
        &tokens.colors,
    );
    assert!(matches!(result, Err(ViewError::InvalidOpacity { what: "scale", .. })));
}
