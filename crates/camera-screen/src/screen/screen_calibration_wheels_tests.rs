//! Вид колёс цвета целиком: переключатель, вкладки, круг толчка, кольцо и три «↻» (решения 204–207).
//!
//! Проверяется то, что делает пользователь: тап по WHEELS открывает вид колёс, палец ведёт точку круга, и наружу
//! уходит точечное намерение с нужным углом и силой в КАЖДОЙ ориентации телефона; цвет кольца под направлением
//! пальца совпадает с цветом, который рисует экран.

use look_contract::{ColorWheel, LookFacts, LookIntent, WheelPush, WheelZone};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_core::gesture::PanSpec;
use ui_draw::draw_list::{DrawItem, DrawList, ImageItem, ImageSource};
use ui_draw::icons::IconId;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, center, drag_eye, edited, surface_vector};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::{ControlViewKind, WheelPadPlacement};
use crate::effect::Effect;
use crate::look_test_support::{look_facts, look_intents};

pub(super) const FRAME: i64 = 16;
pub(super) const ALL_ORIENTATIONS: [DeviceOrientation; 4] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
    DeviceOrientation::PortraitUpsideDown,
];

/// Экран в разделе калибровки с включённым видом колёс (эффекты включения отброшены).
pub(super) fn wheels(facts: LookFacts, orientation: DeviceOrientation) -> Rig {
    let mut rig = calibrating(facts, orientation);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(300);
    rig.effects();
    rig
}

pub(super) fn pad(rig: &Rig) -> WheelPadPlacement {
    rig.screen.wheel_pad_placement().expect("круг колеса есть на экране")
}

pub(super) fn push_of(a: f32, b: f32) -> WheelPush {
    WheelPush::new(a, b).expect("точка теста внутри круга")
}

/// Намерения толчка из эффектов: `(колесо, точка)` по порядку.
pub(super) fn pushes(effects: &[Effect]) -> Vec<(WheelZone, WheelPush)> {
    look_intents(effects)
        .into_iter()
        .filter_map(|intent| match intent {
            LookIntent::SetWheelPush { zone, push } => Some((*zone, *push)),
            _ => None,
        })
        .collect()
}

/// Все намерения вида (для проверок «ничего лишнего не ушло»).
pub(super) fn intents(effects: &[Effect]) -> Vec<LookIntent> {
    look_intents(effects).into_iter().cloned().collect()
}

/// Факты, где у колеса `zone` стоит `wheel`, остальное — нейтраль и равно пресету не будет (правка поверх пресета).
pub(super) fn facts_with_wheel(zone: WheelZone, wheel: ColorWheel) -> LookFacts {
    edited(|mut settings| {
        settings.wheels = settings.wheels.with_wheel(zone, wheel);
        settings
    })
}

/// Глазной вектор длины `length_pt` под углом `degrees` против часовой (0° = вправо, 90° = вверх глазами).
pub(super) fn eye_direction(degrees: f64, length_pt: f64) -> Vector {
    let radians = degrees.to_radians();
    // Глазная ось y идёт ВНИЗ, поэтому «вверх» — это минус.
    Vector::new(length_pt * radians.cos(), -length_pt * radians.sin())
}

/// Шагов в пути пальца `drag_eye`.
pub(super) const DRAG_STEPS: f64 = 24.0;

/// Ожидаемая сила толчка при пути пальца `length_pt` (его ведёт `drag_eye` равными шагами).
///
/// Площадка считает смещение «от распознавания пана», а пан распознаётся на первом шаге, где путь строго больше
/// порога `hysteresis`: до этого места палец точку не двигал. Дальше — замедление `drive_ratio` и деление на радиус
/// области толчка. Длину пути подбирают так, чтобы порог не приходился ровно на шаг (там решает округление).
pub(super) fn expected_strength(length_pt: f64, pad: &WheelPadPlacement) -> f64 {
    let step = length_pt / DRAG_STEPS;
    let hysteresis = PanSpec::default().hysteresis_pt;
    let recognized_at = ((hysteresis / step).floor() + 1.0) * step;
    let drive = ui_kit::tokens::UiKitTokens::default().disc_pad.drive_ratio;
    (length_pt - recognized_at) * drive / pad.pad_radius_pt
}

/// Картинки кольца в списке рисования: растр из памяти размером с внешний диаметр кольца.
pub(super) fn ring_images(list: &DrawList) -> Vec<&ImageItem> {
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Image(image) if matches!(image.source, ImageSource::Picture(_)) => Some(image),
            _ => None,
        })
        .collect()
}

/// Непрозрачность значка «↻» с центром в `center_pt` (поверхность, pt); `None` — значка там нет.
pub(super) fn reset_opacity_at(list: &DrawList, center_pt: Point) -> Option<f32> {
    let (x, y) = (px(center_pt.x), px(center_pt.y));
    list.leaf_items().find_map(|item| match item {
        DrawItem::Icon(icon)
            if icon.icon == IconId::Reset
                && near(icon.transform.tx, x, 1.5)
                && near(icon.transform.ty, y, 1.5)
                && icon.opacity.get() > 0.01 =>
        {
            Some(icon.opacity.get())
        }
        _ => None,
    })
}

/// Ведёт палец из `from` на глазной вектор с кадром после КАЖДОГО касания (жест идёт «вживую»: действия и
/// пересборки между касаниями видны), вызывает `midway` на середине пути, в конце задерживается и отпускает.
pub(super) fn drag_eye_with(rig: &mut Rig, from: Point, eye: Vector, mut midway: impl FnMut(&mut Rig)) {
    let delta = surface_vector(rig, eye);
    let start = rig.now_ms;
    let touch_at = |rig: &mut Rig, phase: TouchPhase, point: Point, at_ms: i64| {
        rig.touch(phase, point, at_ms);
        rig.now_ms = at_ms;
        rig.frame();
    };
    touch_at(rig, TouchPhase::Began, from, start);
    let steps = 24;
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let point = Point::new(from.x + delta.dx * t, from.y + delta.dy * t);
        touch_at(rig, TouchPhase::Moved, point, start + i64::from(step) * FRAME);
        if step == steps / 2 {
            midway(rig);
        }
    }
    let end = Point::new(from.x + delta.dx, from.y + delta.dy);
    let hold_start = start + i64::from(steps) * FRAME;
    for hold in 1..=12 {
        touch_at(rig, TouchPhase::Moved, end, hold_start + i64::from(hold) * FRAME);
    }
    touch_at(rig, TouchPhase::Ended, end, hold_start + 13 * FRAME);
    rig.run(300);
}

// ----- переключатель вида -------------------------------------------------------------------------------------------

#[test]
fn tapping_wheels_replaces_the_scale_rows_with_the_wheel_view_and_scales_brings_them_back() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    assert!(
        rig.screen.node_centers("calib_wheel_pad").is_empty(),
        "по умолчанию вид шкал, круга нет"
    );
    let scale_rows = rig.screen.node_centers("calib_row").len();
    assert!(scale_rows >= 11, "в виде шкал все строки: {scale_rows}");

    let wheels_segment = center(&rig, "calib_view_segment", 1);
    rig.tap(wheels_segment);
    rig.run(300);

    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Wheels);
    assert_eq!(rig.screen.node_centers("calib_wheel_pad").len(), 1, "круг есть");
    assert_eq!(
        rig.screen.node_centers("calib_row").len(),
        1,
        "из шкал осталась только шкала кольца"
    );
    assert!(rig.screen.wheel_pad_placement().is_some());

    let scales_segment = center(&rig, "calib_view_segment", 0);
    rig.tap(scales_segment);
    rig.run(300);
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Scales);
    assert!(
        rig.screen.node_centers("calib_wheel_pad").is_empty(),
        "круга больше нет"
    );
    assert_eq!(
        rig.screen.node_centers("calib_row").len(),
        scale_rows,
        "шкалы вернулись"
    );
    assert!(
        rig.screen.wheel_pad_placement().is_none(),
        "в виде шкал запрос круга пуст"
    );
}

#[test]
fn tapping_the_active_segment_again_changes_nothing() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let before = rig.screen.diagnostics();
    let active = center(&rig, "calib_view_segment", 1);
    rig.tap(active);
    rig.run(200);
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Wheels);
    assert_eq!(rig.screen.node_centers("calib_wheel_pad").len(), 1);
    assert!(look_intents(&rig.effects()).is_empty());
    assert_eq!(
        rig.screen.diagnostics().actions_rejected,
        before.actions_rejected,
        "повторный тап — не отказ"
    );
}

#[test]
fn the_chosen_view_and_wheel_survive_a_rotation_of_the_phone() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let gain_tab = center(&rig, "calib_wheel_tab", 2);
    rig.tap(gain_tab);
    rig.run(200);
    assert_eq!(rig.screen.state().calibration().selected_wheel(), WheelZone::Gain);

    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(600);

    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Wheels);
    assert_eq!(rig.screen.state().calibration().selected_wheel(), WheelZone::Gain);
    assert_eq!(
        rig.screen.node_centers("calib_wheel_pad").len(),
        1,
        "круг пересобран в альбоме"
    );
    let list = rig.frame().clone();
    assert!(
        !texts_of(&list, "GAIN", TextRoleKind::CardTitle).is_empty(),
        "название выбранного колеса — GAIN"
    );
}

#[test]
fn the_wheel_view_fits_the_column_without_scrolling_in_every_orientation() {
    for orientation in ALL_ORIENTATIONS {
        let mut rig = wheels(look_facts(), orientation);
        let placement = pad(&rig);
        // Альбом ниже портрета: круг вписывается по месту, но остаётся крупным (ориентиры решения 205: 170 / 150 pt).
        let diameter = placement.circle.size.width;
        let portrait = matches!(
            orientation,
            DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown
        );
        let (min, max) = if portrait { (169.0, 171.0) } else { (100.0, 171.0) };
        assert!(
            diameter >= min && diameter <= max,
            "{orientation:?}: диаметр круга {diameter} pt вне {min}..{max}"
        );
        // Ряд шкалы кольца целиком в колонке: палец вниз по колонке ничего не прокручивает.
        let ruler = center(&rig, "calib_ruler", 0);
        drag_eye(&mut rig, ruler, Vector::new(0.0, -60.0));
        assert_eq!(
            rig.screen.calibration_scroll_offset(),
            Some(0.0),
            "{orientation:?}: вид колёс не прокручивается"
        );
    }
}

// ----- запрос геометрии круга ---------------------------------------------------------------------------------------

#[test]
fn the_pad_placement_query_points_at_the_circle_the_screen_draws() {
    for orientation in ALL_ORIENTATIONS {
        let rig = wheels(look_facts(), orientation);
        let placement = pad(&rig);
        let drawn = center(&rig, "calib_wheel_pad", 0);
        let queried = placement.circle.center();
        assert!(
            (drawn.x - queried.x).abs() < 0.5 && (drawn.y - queried.y).abs() < 0.5,
            "{orientation:?}: узел круга {drawn:?}, запрос {queried:?}"
        );
        assert!(placement.pad_radius_pt > 0.0 && placement.pad_radius_pt < placement.circle.size.width / 2.0);
    }
}

#[test]
fn the_pad_placement_query_is_empty_outside_the_wheel_view() {
    let rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    assert!(rig.screen.wheel_pad_placement().is_none());
    let photo = Rig::new();
    assert!(photo.screen.wheel_pad_placement().is_none());
}

#[test]
fn tab_and_segment_labels_fit_inside_their_capsules() {
    // Подпись шире капсулы вылезла бы за неё: проверяем по реальным прогонам текста на экране.
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let list = rig.frame().clone();
    for label in ["LIFT", "GAMMA", "GAIN", "OFFSET"] {
        let runs = texts_of(&list, label, TextRoleKind::SegmentLabel);
        assert!(!runs.is_empty(), "вкладка {label} нарисована");
        let width_pt = f64::from(runs[0].run.advance_px) / f64::from(PX_PER_PT);
        // Портретная колонка: внутренняя ширина 196 pt, четыре вкладки с зазором 4 pt.
        let capsule_pt = (196.0 - 3.0 * 4.0) / 4.0;
        assert!(
            width_pt < capsule_pt - 2.0,
            "{label}: {width_pt} pt не влезает в {capsule_pt} pt"
        );
    }
}

#[test]
fn the_active_view_segment_is_highlighted_and_follows_the_choice() {
    let yellow = ui_kit::tokens::UiKitTokens::default().colors.focus_yellow;
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let list = rig.frame().clone();
    let colour_of = |list: &DrawList, label: &str| texts_of(list, label, TextRoleKind::SegmentLabel)[0].color;
    assert_eq!(colour_of(&list, "SCALES"), yellow, "по умолчанию активен SCALES");
    assert_ne!(colour_of(&list, "WHEELS"), yellow);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    let list = rig.run(300).clone();
    assert_eq!(colour_of(&list, "WHEELS"), yellow, "после выбора подсвечен WHEELS");
    assert_ne!(colour_of(&list, "SCALES"), yellow);
}
