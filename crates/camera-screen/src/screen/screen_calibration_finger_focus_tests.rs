//! «Картинка под пальцем» (решение 273): пока палец ведёт линейку ручки или колесо, остаётся только ведомый орган
//! на своей подложке; на отпускании всё возвращается. Проверяется по списку рисования и по касаниям.

use look_contract::{GlobalKnob, LookIntent};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::screen_calibration_tests::{center, edited, portrait, surface_vector};
use super::screen_calibration_wheels_tests::{eye_direction, pad, wheels};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts, look_intents};

const FRAME: i64 = 16;

/// Высоты (px) видимых поверхностей раздела (заливка поверхности из токенов): панель колонки — самая высокая,
/// подложка под ведомым органом — низкая.
fn surface_heights_px(list: &DrawList) -> Vec<f32> {
    let surface = UiKitTokens::default().surface.fill_color;
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape)
                if (shape.fill.r, shape.fill.g, shape.fill.b) == (surface.r, surface.g, surface.b)
                    && shape.opacity.get() > 0.05 =>
            {
                Some(shape.geometry.extent().height)
            }
            _ => None,
        })
        .collect()
}

/// Есть ли панель колонки ручек: поверхность высотой ровно в зону колонки (прибор и подложки — другой высоты).
fn knobs_panel_is_drawn(rig: &Rig, list: &DrawList) -> bool {
    let zone = rig
        .screen
        .state()
        .calibration()
        .layout()
        .expect("раскладка раздела")
        .zones()
        .knobs;
    let panel_px = zone.size.height as f32 * PX_PER_PT;
    surface_heights_px(list)
        .iter()
        .any(|height| (height - panel_px).abs() < 1.5)
}

fn count(list: &DrawList, text: &str, role: TextRoleKind) -> usize {
    texts_of(list, text, role).len()
}

/// Палец берёт линейку/круг в `from`, ведёт на глазное смещение `eye` и ДЕРЖИТ там (не отпуская) `hold_ms`.
fn drag_and_hold(rig: &mut Rig, from: Point, eye: Vector, hold_ms: i64) {
    let delta = surface_vector(rig, eye);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    let steps = 12;
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let point = Point::new(from.x + delta.dx * t, from.y + delta.dy * t);
        rig.touch(TouchPhase::Moved, point, start + i64::from(step) * FRAME);
    }
    let end = Point::new(from.x + delta.dx, from.y + delta.dy);
    let hold_start = start + i64::from(steps) * FRAME;
    let frames = hold_ms / FRAME;
    for hold in 1..=frames {
        rig.touch(TouchPhase::Moved, end, hold_start + hold * FRAME);
        rig.now_ms = hold_start + hold * FRAME;
        rig.frame();
    }
}

/// Отпустить палец там, где он стоит (`at`), и дать всему вернуться.
fn release(rig: &mut Rig, at: Point) {
    let released_at = rig.now_ms + FRAME;
    rig.touch(TouchPhase::Ended, at, released_at);
    rig.now_ms = released_at + 10;
    rig.frame();
    rig.run(600);
}

#[test]
fn while_a_finger_drives_a_ruler_only_that_row_stays_and_everything_returns_on_release() {
    let mut rig = portrait(look_facts());
    let ruler = center(&rig, "calib_ruler", 0);
    let ruler_before = center(&rig, "calib_ruler", 0);
    let at_rest = rig.frame().clone();
    assert_eq!(count(&at_rest, "CONTRAST", TextRoleKind::CardTitle), 1);
    assert_eq!(count(&at_rest, "MIDTONES", TextRoleKind::CardTitle), 1);
    assert_eq!(
        count(&at_rest, "TONE", TextRoleKind::Caption),
        1,
        "заголовок группы виден"
    );
    assert_eq!(count(&at_rest, "SCALES", TextRoleKind::SegmentLabel), 1);
    assert!(knobs_panel_is_drawn(&rig, &at_rest), "в покое панель колонки на месте");

    drag_and_hold(&mut rig, ruler, Vector::new(-30.0, 0.0), 600);
    let held = rig.frame().clone();
    assert_eq!(
        count(&held, "CONTRAST", TextRoleKind::CardTitle),
        1,
        "ведомая строка осталась"
    );
    assert_eq!(
        count(&held, "MIDTONES", TextRoleKind::CardTitle),
        0,
        "другие строки растворились"
    );
    assert_eq!(
        count(&held, "TONE", TextRoleKind::Caption),
        0,
        "заголовок группы растворился"
    );
    assert_eq!(
        count(&held, "SCALES", TextRoleKind::SegmentLabel),
        0,
        "переключатель растворился"
    );
    assert_eq!(count(&held, "WHEELS", TextRoleKind::SegmentLabel), 0);
    assert!(!knobs_panel_is_drawn(&rig, &held), "панели колонки нет");
    let backing = surface_heights_px(&held);
    let row_with_backing_px = (rig_row_height_pt() + 2.0 * backing_padding_pt()) as f32 * PX_PER_PT;
    assert!(
        backing.iter().any(|height| (height - row_with_backing_px).abs() < 1.5),
        "под ведомой строкой своя подложка высотой {row_with_backing_px}: {backing:?}"
    );
    assert_eq!(
        center(&rig, "calib_ruler", 0),
        ruler_before,
        "зона касания ведомой линейки не сдвинулась"
    );

    release(&mut rig, ruler);
    let back = rig.frame().clone();
    assert_eq!(
        count(&back, "MIDTONES", TextRoleKind::CardTitle),
        1,
        "другие строки вернулись"
    );
    assert_eq!(count(&back, "TONE", TextRoleKind::Caption), 1);
    assert_eq!(count(&back, "SCALES", TextRoleKind::SegmentLabel), 1);
    assert!(knobs_panel_is_drawn(&rig, &back), "панель вернулась");
    assert!(
        surface_heights_px(&back)
            .iter()
            .all(|height| (height - row_with_backing_px).abs() >= 1.5),
        "подложка под строкой исчезла"
    );
}

fn rig_row_height_pt() -> f64 {
    crate::calibration::CalibrationConfig::default().widgets.row_height_pt
}

fn backing_padding_pt() -> f64 {
    crate::calibration::CalibrationConfig::default()
        .widgets
        .finger_focus_backing_padding_pt
}

/// Сбросы, которые ушли намерениями вида.
fn resets_in(effects: &[Effect]) -> usize {
    look_intents(effects)
        .into_iter()
        .filter(|intent| matches!(intent, LookIntent::ResetControl(_)))
        .count()
}

#[test]
fn another_row_does_not_take_a_touch_while_it_is_faded_but_does_in_rest() {
    let facts = edited(|settings| settings.with_knob(GlobalKnob::Midtones, knob(0.3)));
    let mut rig = portrait(facts);
    // Контроль: в покое «↻» правленой строки MIDTONES сбрасывает значение.
    let midtones_reset = center(&rig, "calib_reset", 1);
    rig.tap(midtones_reset);
    rig.run(100);
    assert_eq!(resets_in(&rig.effects()), 1, "в покое «↻» строки работает");

    let ruler = center(&rig, "calib_ruler", 0);
    drag_and_hold(&mut rig, ruler, Vector::new(-30.0, 0.0), 600);
    rig.effects();
    // Второй палец касается «↻» растворённой строки.
    let at = rig.now_ms + FRAME;
    rig.touch_pointer(1, TouchPhase::Began, midtones_reset, at);
    rig.touch_pointer(1, TouchPhase::Ended, midtones_reset, at + 2 * FRAME);
    rig.now_ms = at + 3 * FRAME;
    rig.frame();
    assert_eq!(resets_in(&rig.effects()), 0, "растворённая строка касаний не принимает");

    release(&mut rig, ruler);
    rig.effects();
    rig.tap(midtones_reset);
    rig.run(100);
    assert_eq!(
        resets_in(&rig.effects()),
        1,
        "после отпускания строка снова принимает касания"
    );
}

#[test]
fn while_a_finger_drives_the_wheel_pad_the_block_stays_and_the_tabs_fade() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let at_rest = rig.frame().clone();
    assert_eq!(count(&at_rest, "GAMMA", TextRoleKind::SegmentLabel), 1, "вкладки видны");
    assert_eq!(count(&at_rest, "RING", TextRoleKind::CardTitle), 1);
    let circle_center = pad(&rig).circle.center();

    drag_and_hold(&mut rig, circle_center, eye_direction(0.0, 20.0), 600);
    let held = rig.frame().clone();
    assert_eq!(
        count(&held, "GAMMA", TextRoleKind::SegmentLabel),
        0,
        "вкладки растворились"
    );
    assert_eq!(
        count(&held, "SCALES", TextRoleKind::SegmentLabel),
        0,
        "переключатель растворился"
    );
    assert_eq!(
        count(&held, "LIFT", TextRoleKind::CardTitle),
        1,
        "строка названия с числами осталась"
    );
    assert_eq!(
        count(&held, "RING", TextRoleKind::CardTitle),
        1,
        "шкала кольца осталась"
    );
    assert!(!knobs_panel_is_drawn(&rig, &held), "панель колонки растворилась");

    release(&mut rig, circle_center);
    let back = rig.frame().clone();
    assert_eq!(count(&back, "GAMMA", TextRoleKind::SegmentLabel), 1);
    assert_eq!(count(&back, "SCALES", TextRoleKind::SegmentLabel), 1);
    assert!(knobs_panel_is_drawn(&rig, &back));
}

#[test]
fn the_ring_ruler_of_a_wheel_keeps_the_whole_wheel_block_visible() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let ring_ruler = center(&rig, "calib_ruler", 0);
    drag_and_hold(&mut rig, ring_ruler, Vector::new(-30.0, 0.0), 600);
    let held = rig.frame().clone();
    assert_eq!(count(&held, "LIFT", TextRoleKind::CardTitle), 1);
    assert_eq!(count(&held, "RING", TextRoleKind::CardTitle), 1);
    assert_eq!(count(&held, "GAMMA", TextRoleKind::SegmentLabel), 0);
}

#[test]
fn a_rotation_during_the_drag_does_not_leave_the_column_faded() {
    let mut rig = portrait(look_facts());
    let ruler = center(&rig, "calib_ruler", 0);
    drag_and_hold(&mut rig, ruler, Vector::new(-30.0, 0.0), 300);
    assert_eq!(
        count(&rig.frame().clone(), "MIDTONES", TextRoleKind::CardTitle),
        0,
        "растворено"
    );
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(600);
    let after = rig.frame().clone();
    assert_eq!(
        count(&after, "MIDTONES", TextRoleKind::CardTitle),
        1,
        "после поворота раздел пересобран и не растворён"
    );
    assert!(knobs_panel_is_drawn(&rig, &after), "панель колонки на месте");
    release(&mut rig, ruler);
    assert_eq!(count(&rig.frame().clone(), "MIDTONES", TextRoleKind::CardTitle), 1);
}

#[test]
fn a_rotation_during_a_wheel_pad_drag_is_not_a_frame_error_and_the_wheel_block_is_not_faded() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let circle_center = pad(&rig).circle.center();
    drag_and_hold(&mut rig, circle_center, eye_direction(0.0, 20.0), 300);
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(600);
    let after = rig.frame().clone();
    assert_eq!(
        count(&after, "GAMMA", TextRoleKind::SegmentLabel),
        1,
        "вкладки на месте после поворота"
    );
    assert!(knobs_panel_is_drawn(&rig, &after));
}

/// Подписи узлов, которые ловят касание в точке `point` (pt поверхности), от глубокого к корню.
fn hit_labels(rig: &Rig, point: Point) -> Vec<&'static str> {
    let tree = rig.screen.scene_for_tests().expect("сцена").tree();
    tree.hit_path(point)
        .into_iter()
        .filter_map(|node| tree.label(node).ok())
        .collect()
}

#[test]
fn the_faded_rows_stop_catching_touches_at_once_not_when_they_are_already_invisible() {
    // Хит-тест сцены и так не ловит узлы почти нулевой прозрачности, поэтому проверяется самое начало растворения,
    // когда строки ещё почти непрозрачны: касание «↻» другой строки уже не должно её ловить.
    let facts = edited(|settings| settings.with_knob(GlobalKnob::Midtones, knob(0.3)));
    let mut rig = portrait(facts);
    let midtones_reset = center(&rig, "calib_reset", 1);
    assert!(
        hit_labels(&rig, midtones_reset).contains(&"calib_reset"),
        "в покое «↻» ловит касание"
    );
    let ruler = center(&rig, "calib_ruler", 0);
    drag_and_hold(&mut rig, ruler, Vector::new(-30.0, 0.0), FRAME);
    let opacity_of_title = texts_of(&rig.frame().clone(), "MIDTONES", TextRoleKind::CardTitle)
        .first()
        .map(|text| text.opacity.get());
    assert!(
        opacity_of_title.is_some_and(|opacity| opacity > 0.5),
        "строка ещё видна, растворение только началось: {opacity_of_title:?}"
    );
    assert!(
        !hit_labels(&rig, midtones_reset).contains(&"calib_reset"),
        "растворяющаяся строка уже не ловит касание"
    );
}
