//! Сквозной путь: касание в пикселях → сцена `ui-core` → жест нажатия → пружина масштаба (токены `ui-kit`) →
//! порядок рисования дерева → мост в пиксели `ui-draw` → список со стеклом → пиксели GPU. Нажатая кнопка на картинке
//! больше ненажатой на ожидаемое число px и лежит там, где её видит хит-тест сцены.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use support::{Rig, assert_no_validation_errors, empty_list, gpu_context_or_skip, pixel};
use ui_core::animator::{AnimatedProperty, AnimationRequest, PropertyKey, StartVelocity};
use ui_core::geometry::{Insets, Point, Rect};
use ui_core::gesture::{GestureState, PressSpec, RecognizerSpec};
use ui_core::layout::{AnchorRule, AxisPlacement, Placement, ReferenceArea};
use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};
use ui_core::time::MotionPreference;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, GlassGroupItem, GlassMember, PressState};
use ui_draw::geometry::PxPoint;
use ui_draw::scene_bridge::node_placement_px;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::{MotionToken, UiKitTokens};
use ui_render_vulkan::BackdropInput;

const WIDTH_PX: u32 = 1080;
const HEIGHT_PX: u32 = 2412;
const PX_PER_PT: f64 = 1080.0 / 440.0;
/// Кнопка 120 × 56 pt в (160, 800) pt.
const BUTTON: Rect = Rect::from_xywh(160.0, 800.0, 120.0, 56.0);

fn ns(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000 + millis * 1_000_000)
}

fn touch(phase: TouchPhase, x_pt: f64, y_pt: f64, millis: i64) -> TouchSample {
    TouchSample {
        pointer: PointerId::new(0),
        phase,
        position: SurfacePoint {
            x_px: (x_pt * PX_PER_PT) as f32,
            y_px: (y_pt * PX_PER_PT) as f32,
        },
        time: ns(millis),
    }
}

/// Сцена с одной кнопкой; возвращает сцену и узел кнопки.
fn button_scene() -> (UiScene<&'static str>, ui_core::id::NodeId) {
    let mut scene: UiScene<&'static str> = UiScene::new(
        SurfaceGeometry {
            width_px: WIDTH_PX,
            height_px: HEIGHT_PX,
            safe_area: Insets::ZERO,
        },
        SceneSettings::default(),
    )
    .unwrap();
    let root = scene.tree().root();
    let node = scene.insert_child(root, "button").unwrap();
    scene
        .set_placement(
            node,
            Placement::Anchored(AnchorRule {
                area: ReferenceArea::Frame,
                horizontal: AxisPlacement::Start {
                    inset: BUTTON.origin.x,
                    length: BUTTON.size.width,
                },
                vertical: AxisPlacement::Start {
                    inset: BUTTON.origin.y,
                    length: BUTTON.size.height,
                },
            }),
        )
        .unwrap();
    scene.layout().unwrap();
    scene
        .add_recognizer(node, RecognizerSpec::Press(PressSpec::default()), "press")
        .unwrap();
    (scene, node)
}

/// Список рисования кадра из дерева сцены: каждая кнопка — стеклянная капсула из `NodeVisual`.
fn draw_list_from_scene(
    scene: &UiScene<&'static str>,
    tokens: &UiKitTokens,
    pressed: bool,
) -> ui_draw::draw_list::DrawList {
    let mut list = empty_list();
    for visual in scene.tree().draw_order() {
        if visual.label != "button" {
            continue;
        }
        let placement = node_placement_px(&visual, PX_PER_PT);
        let params = tokens.glass.button.resolve(placement.size, PX_PER_PT as f32);
        let member = GlassMember {
            geometry: ShapeGeometry::Capsule { size: placement.size },
            // Преобразование из дерева уже содержит масштаб нажатия от пружины: рендер его не добавляет.
            transform: placement.transform,
            press: PressState {
                amount: if pressed { 1.0 } else { 0.0 },
                touch_point_local: PxPoint::ORIGIN,
            },
        };
        list.items.push(DrawItem::Glass(GlassGroupItem {
            params,
            members: vec![member],
            merge_radius_px: 0.0,
            effect: 1.0,
            opacity: Opacity::new(placement.opacity).unwrap(),
        }));
    }
    list
}

/// Крайние x по строке `row`, где яркость выше порога (стекло над чёрным светлее чёрного за счёт плёнки тона).
fn row_extent(pixels: &[[f32; 4]], row: u32) -> (u32, u32) {
    let lit = |x: u32| {
        let p = pixel(pixels, WIDTH_PX, x, row);
        0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2] > 0.05
    };
    let left = (0..WIDTH_PX).find(|x| lit(*x)).unwrap();
    let right = (0..WIDTH_PX).rev().find(|x| lit(*x)).unwrap();
    (left, right)
}

#[test]
fn pressed_glass_button_is_larger_by_the_spring_scale_and_sits_where_the_scene_hit_test_sees_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let tokens = UiKitTokens::default();
    let (mut scene, node) = button_scene();
    let mut rig = Rig::new(
        &context,
        (WIDTH_PX, HEIGHT_PX),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let black = BackdropInput::Absent {
        fill: DisplayColor::BLACK,
    };
    let row = ((BUTTON.origin.y + BUTTON.size.height * 0.5) * PX_PER_PT) as u32;

    // Ненажатая кнопка.
    let rest_list = draw_list_from_scene(&scene, &tokens, false);
    let (_, rest_pixels) = rig.render_with_clear(&rest_list, black, [0.0, 0.0, 0.0, 1.0]);
    let rest_pixels_ok = rest_pixels;
    let (rest_left, rest_right) = row_extent(&rest_pixels_ok, row);

    // Касание в центре кнопки; реакция экрана: на Began жеста нажатия запустить пружину масштаба.
    let centre = Point::new(BUTTON.center().x, BUTTON.center().y);
    scene.handle_touch(&touch(TouchPhase::Began, centre.x, centre.y, 0));
    let press_scale = f64::from(tokens.glass.button.press.scale_gain) + 1.0;
    let spring = tokens
        .motion
        .spring(MotionToken::Snappy, MotionPreference::Full)
        .unwrap();
    let mut began = 0;
    for event in scene.drain_gesture_events() {
        if event.intent == "press" && event.state == GestureState::Began {
            began += 1;
            for property in [AnimatedProperty::ScaleX, AnimatedProperty::ScaleY] {
                scene
                    .animate(AnimationRequest {
                        key: PropertyKey { node, property },
                        target: press_scale,
                        spring,
                        velocity: StartVelocity::Zero,
                        now: event.time,
                    })
                    .unwrap();
            }
        }
    }
    assert_eq!(began, 1, "касание в центре кнопки запускает жест нажатия");
    // Пружина доезжает за ~1 с (60 кадров по 16 мс).
    for frame in 1..=60 {
        scene.advance_frame(ns(frame * 16));
    }
    let visual_bounds = scene.tree().visual_bounds(node).unwrap();
    assert!(
        (visual_bounds.size.width - BUTTON.size.width * press_scale).abs() < 0.2,
        "пружина дошла до масштаба нажатия: {visual_bounds:?}"
    );

    let pressed_list = draw_list_from_scene(&scene, &tokens, true);
    let (_, pressed_pixels) = rig.render_with_clear(&pressed_list, black, [0.0, 0.0, 0.0, 1.0]);
    let (pressed_left, pressed_right) = row_extent(&pressed_pixels, row);

    // Ширина выросла на (масштаб − 1) × ширина кнопки в px (допуск 3 px: край размыт сглаживанием и тёмным краем).
    let rest_width = (rest_right - rest_left) as f64;
    let pressed_width = (pressed_right - pressed_left) as f64;
    let expected_growth = BUTTON.size.width * PX_PER_PT * (press_scale - 1.0);
    assert!(
        ((pressed_width - rest_width) - expected_growth).abs() < 3.0,
        "рост ширины {} px, ожидалось {expected_growth} px (покой {rest_width}, нажата {pressed_width})",
        pressed_width - rest_width
    );
    assert!(
        pressed_left < rest_left && pressed_right > rest_right,
        "нажатая кнопка шире в обе стороны от центра"
    );
    // И её левый край — там, где дерево сцены считает видимый край (допуск 3 px).
    let expected_left = visual_bounds.min_x() * PX_PER_PT;
    assert!(
        (f64::from(pressed_left) - expected_left).abs() < 3.0,
        "левый край на картинке {pressed_left}, у сцены {expected_left}"
    );

    // Хит-тест: точка на 6 px внутри нарисованного края (но снаружи ненажатой кнопки) попадает в кнопку, точка на 6 px
    // снаружи — нет; а на картинке в этих местах стекло есть и нет соответственно.
    let hit = |x_px: f64| {
        scene
            .tree()
            .hit_path(Point::new(x_px / PX_PER_PT, centre.y))
            .contains(&node)
    };
    let inside_px = f64::from(pressed_left) + 6.0;
    let outside_px = f64::from(pressed_left) - 6.0;
    assert!(
        inside_px < f64::from(rest_left) - 2.0,
        "точка между краями нажатой и ненажатой кнопки"
    );
    assert!(
        hit(inside_px) && !hit(outside_px),
        "хит-тест видит кнопку там же, где её нарисовали"
    );
    let luma = |p: [f32; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    assert!(luma(pixel(&pressed_pixels, WIDTH_PX, inside_px as u32, row)) > 0.05);
    assert!(luma(pixel(&pressed_pixels, WIDTH_PX, outside_px as u32, row)) < 0.02);
    // Ненажатая кнопка в той же точке на картинке отсутствует: между краями её не было.
    assert!(luma(pixel(&rest_pixels_ok, WIDTH_PX, inside_px as u32, row)) < 0.02);
    drop((rig, rest_list, pressed_list));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
