//! Переезд кнопки меню при смене PHOTO ↔ CALIBRATE (решения 282, 295): пока режим меняется, на экране ровно одна
//! кнопка, она едет по прямой от места в прежнем режиме к месту в новом, материал — режима назначения с первого
//! кадра переезда; Reduce Motion — прыжок; пока кнопка едет, касаний она не ловит.

use look_contract::LookFacts;
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawList;
use ui_draw::icons::IconId;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::CalibrationMenu;
use crate::config::ScreenConfig;
use crate::look_test_support::look_facts;
use crate::modes::CaptureMode;

const FRAME: i64 = 16;

/// Центр значка кнопки меню (px экрана) и число таких значков в кадре.
fn menu_icon(list: &DrawList) -> (usize, (f32, f32)) {
    let icons = icons_of(list, IconId::ControlsMenu);
    let centre = icons
        .first()
        .map_or((f32::NAN, f32::NAN), |icon| (icon.transform.tx, icon.transform.ty));
    (icons.len(), centre)
}

/// Материал круга под значком: `true` — поверхность раздела (без стекла), `false` — стекло PHOTO.
fn button_is_section_surface(list: &DrawList, centre: (f32, f32)) -> bool {
    let near = |member: &(ui_draw::shape::ShapeGeometry, f32, f32)| {
        matches!(member.0, ui_draw::shape::ShapeGeometry::Circle { .. })
            && (member.1 - centre.0).abs() < 1.0
            && (member.2 - centre.1).abs() < 1.0
    };
    let glass = glass_members(list).iter().any(near);
    // Поверхность — члены `panel_members`, которых нет среди стекла.
    let surface = panel_members(list).len() > glass_members(list).len()
        && panel_members(list).iter().filter(|member| near(member)).count()
            > glass_members(list).iter().filter(|member| near(member)).count();
    assert!(
        glass != surface,
        "круг кнопки нарисован ровно одним материалом (стекло {glass}, поверхность {surface})"
    );
    surface
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Экран в PHOTO с поставленной ориентацией и фактами вида; кнопка стоит на своём месте.
fn photo_rig(orientation: DeviceOrientation, motion: MotionPreference) -> Rig {
    let mut rig = Rig::with(ScreenConfig::default(), motion);
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts() as LookFacts)));
    rig.screen
        .handle_action(ScreenAction::DeviceOrientationChanged(orientation));
    rig.run(600);
    rig.effects();
    rig
}

/// Кадры по 16 мс после смены режима; для каждого — центр значка, число значков и материал.
struct GlideTrace {
    frames: Vec<(usize, (f32, f32), bool)>,
}

fn trace_switch(rig: &mut Rig, mode: CaptureMode, frames: usize) -> GlideTrace {
    rig.screen.handle_action(ScreenAction::ModeSelected(mode));
    let mut trace = Vec::new();
    for _ in 0..frames {
        rig.now_ms += FRAME;
        let list = rig.frame().clone();
        let (count, centre) = menu_icon(&list);
        let surface = count > 0 && button_is_section_surface(&list, centre);
        trace.push((count, centre, surface));
    }
    GlideTrace { frames: trace }
}

/// Где якорь кнопки меню раздела на экране (px), по раскладке.
fn section_anchor_px(rig: &Rig) -> (f32, f32) {
    let layout = rig.screen.state().calibration().layout().expect("раскладка раздела");
    let anchor = layout.eye_point_on_surface(layout.menu_button_center());
    (anchor.x as f32 * PX_PER_PT, anchor.y as f32 * PX_PER_PT)
}

fn check_glide(orientation: DeviceOrientation) {
    let mut rig = photo_rig(orientation, MotionPreference::Full);
    let (count, photo_place) = menu_icon(&rig.frame().clone());
    assert_eq!(count, 1);
    let trace = trace_switch(&mut rig, CaptureMode::Calibrate, 60);
    let anchor = section_anchor_px(&rig);

    // Ровно одна кнопка в каждом кадре переезда и после него.
    assert!(
        trace.frames.iter().all(|(count, ..)| *count == 1),
        "двойной кнопки нет: {:?}",
        trace.frames
    );
    // Материал раздела с первого кадра переезда.
    assert!(trace.frames[0].2, "материал меняется одним кадром, в начале");
    assert!(trace.frames.iter().all(|(_, _, surface)| *surface));
    // Первый кадр — у места PHOTO, последний — ровно в якоре раскладки.
    let first = trace.frames[0].1;
    assert!(
        distance(first, photo_place) < 0.25 * distance(photo_place, anchor),
        "переезд начинается у места PHOTO: {first:?} против {photo_place:?}"
    );
    let last = trace.frames.last().unwrap().1;
    assert!(
        distance(last, anchor) < 1.0,
        "после переезда кнопка в якоре раскладки: {last:?} против {anchor:?}"
    );
    // Монотонно к цели и по прямой на экране.
    let total = distance(photo_place, anchor);
    assert!(total > 50.0, "места PHOTO и раздела различаются: {total}");
    let mut previous_remaining = f32::MAX;
    for (_, centre, _) in &trace.frames {
        let remaining = distance(*centre, anchor);
        assert!(
            remaining <= previous_remaining + 0.05,
            "кнопка не возвращается назад: {remaining} после {previous_remaining}"
        );
        previous_remaining = remaining;
        let cross = (anchor.0 - photo_place.0) * (centre.1 - photo_place.1)
            - (anchor.1 - photo_place.1) * (centre.0 - photo_place.0);
        assert!(
            cross.abs() / total < 1.0,
            "кнопка едет по прямой: отклонение {}",
            cross.abs() / total
        );
    }
    // Промежуточные кадры действительно есть (переезд, а не прыжок).
    assert!(
        trace
            .frames
            .iter()
            .any(|(_, c, _)| distance(*c, photo_place) > 10.0 && distance(*c, anchor) > 10.0),
        "кнопка побывала между местами"
    );
}

#[test]
fn the_button_glides_in_a_straight_line_from_the_photo_place_to_the_section_anchor_in_portrait() {
    check_glide(DeviceOrientation::Portrait);
}

#[test]
fn the_same_glide_holds_when_the_section_is_turned_in_landscape() {
    check_glide(DeviceOrientation::LandscapeLeft);
}

#[test]
fn the_way_back_glides_to_the_photo_place_and_keeps_one_button_with_the_photo_material() {
    let mut rig = photo_rig(DeviceOrientation::Portrait, MotionPreference::Full);
    let (_, photo_place) = menu_icon(&rig.frame().clone());
    trace_switch(&mut rig, CaptureMode::Calibrate, 60);
    let section_place = menu_icon(&rig.frame().clone()).1;
    let trace = trace_switch(&mut rig, CaptureMode::Photo, 60);
    assert!(
        trace.frames.iter().all(|(count, ..)| *count == 1),
        "двойной кнопки нет: {:?}",
        trace.frames
    );
    assert!(
        !trace.frames[0].2,
        "с первого кадра обратного пути материал PHOTO (стекло)"
    );
    assert!(
        distance(trace.frames[0].1, section_place) < 0.25 * distance(section_place, photo_place),
        "обратный путь начинается у места раздела"
    );
    let last = trace.frames.last().unwrap().1;
    assert!(
        distance(last, photo_place) < 1.0,
        "в конце кнопка на месте PHOTO: {last:?} против {photo_place:?}"
    );
}

#[test]
fn reduce_motion_makes_the_button_jump_in_one_frame() {
    let mut rig = photo_rig(DeviceOrientation::Portrait, MotionPreference::Reduced);
    let (_, photo_place) = menu_icon(&rig.frame().clone());
    let trace = trace_switch(&mut rig, CaptureMode::Calibrate, 30);
    let anchor = section_anchor_px(&rig);
    let jumped = trace.frames[0].1;
    assert!(
        distance(jumped, anchor) < 1.0,
        "уже в первом кадре кнопка в якоре: {jumped:?}"
    );
    assert!(distance(jumped, photo_place) > 50.0);
    assert!(
        trace
            .frames
            .iter()
            .all(|(count, centre, surface)| *count == 1 && distance(*centre, anchor) < 1.0 && *surface),
        "промежуточных мест нет"
    );
}

#[test]
fn a_tap_at_the_section_anchor_during_the_glide_does_nothing_but_works_after_it() {
    let mut rig = photo_rig(DeviceOrientation::Portrait, MotionPreference::Full);
    rig.screen
        .handle_action(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.now_ms += FRAME;
    rig.frame();
    let layout = *rig.screen.state().calibration().layout().expect("раскладка раздела");
    let anchor_pt: Point = layout.eye_point_on_surface(layout.menu_button_center());
    rig.tap(anchor_pt);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Closed,
        "пока кнопка едет, касание якоря меню не открывает"
    );
    rig.run(800);
    rig.effects();
    rig.tap(anchor_pt);
    rig.run(300);
    assert_ne!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Closed,
        "после переезда кнопка принимает касания"
    );
}
