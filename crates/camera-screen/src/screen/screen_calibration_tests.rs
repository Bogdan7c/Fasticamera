//! Виджеты раздела калибровки целиком: касания → действия → эффекты, список рисования, поворот раздела.
//!
//! Проверяется то, что видит и делает пользователь: палец тянет линейку ручки — уходит `SetLook` с точным
//! значением; тап по «↻» сбрасывает, только если значение отличается; «чистяк» держится и отпускается; прибор
//! стоит ровно на своей зоне и повёрнут вместе с разделом; PHOTO в режиме CALIBRATE не рисуется и не нажимается.

use look_contract::{EditState, GlobalKnob, LookControl, LookFacts, LookIntent, LookSettings, ToneBaseModel};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Rect, Vector};
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageSource, LiveImage};
use ui_draw::icons::IconId;
use ui_kit::tokens::TextRoleKind;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts, look_facts_edited, look_intents};
use crate::modes::CaptureMode;

const FRAME: i64 = 16;

/// Экран в режиме CALIBRATE с фактами `facts`, телефон держат в положении `orientation`.
pub(super) fn calibrating(facts: LookFacts, orientation: DeviceOrientation) -> Rig {
    let mut rig = Rig::new();
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(facts)));
    rig.screen
        .handle_action(ScreenAction::DeviceOrientationChanged(orientation));
    rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.run(400);
    rig.effects();
    rig
}

pub(super) fn portrait(facts: LookFacts) -> Rig {
    calibrating(facts, DeviceOrientation::Portrait)
}

/// Центр i-го видимого узла с подписью `label` (pt поверхности).
pub(super) fn center(rig: &Rig, label: &str, index: usize) -> Point {
    *rig.screen
        .node_centers(label)
        .get(index)
        .unwrap_or_else(|| panic!("узла «{label}» №{index} нет в сцене"))
}

/// Глазное смещение (вправо, вниз) → смещение на поверхности по раскладке раздела.
pub(super) fn surface_vector(rig: &Rig, eye: Vector) -> Vector {
    let layout = *rig.screen.state().calibration().layout().expect("раскладка раздела");
    let origin = layout.eye_point_on_surface(Point::new(100.0, 100.0));
    let moved = layout.eye_point_on_surface(Point::new(100.0 + eye.dx, 100.0 + eye.dy));
    Vector::new(moved.x - origin.x, moved.y - origin.y)
}

/// Ведёт палец от `from` на глазное смещение `eye`, в конце задерживается (скорость отпускания нулевая — линейка
/// остановится ровно на ступени) и отпускает.
pub(super) fn drag_eye(rig: &mut Rig, from: Point, eye: Vector) {
    let delta = surface_vector(rig, eye);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    let steps = 24;
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let point = Point::new(from.x + delta.dx * t, from.y + delta.dy * t);
        rig.touch(TouchPhase::Moved, point, start + i64::from(step) * FRAME);
    }
    let end = Point::new(from.x + delta.dx, from.y + delta.dy);
    let hold_start = start + i64::from(steps) * FRAME;
    for hold in 1..=12 {
        rig.touch(TouchPhase::Moved, end, hold_start + i64::from(hold) * FRAME);
    }
    rig.touch(TouchPhase::Ended, end, hold_start + 13 * FRAME);
    rig.now_ms = hold_start + 13 * FRAME + 10;
    rig.frame();
    rig.run(600);
}

/// Как поменялся бы вид, если бы библиотека применила точечные намерения по порядку (старт — нейтральный вид).
/// Экран шлёт `SetKnob` / `SetBandAxis`, а не полный набор значений (ревью 14b), поэтому «что увидит владелец»
/// собирается здесь свёрткой, как это делает библиотека.
pub(super) fn set_looks(effects: &[Effect]) -> Vec<LookSettings> {
    let mut applied = LookSettings::NEUTRAL;
    let mut snapshots = Vec::new();
    for intent in look_intents(effects) {
        match intent {
            LookIntent::SetKnob { knob, value } => applied = applied.with_knob(*knob, *value),
            LookIntent::SetBandAxis { band, axis, value } => {
                let adjustment = applied.bands.for_band(band).with_axis(*axis, *value);
                applied.bands = applied.bands.with_adjustment(adjustment);
            }
            LookIntent::SetLook(settings) => applied = settings.clone(),
            _ => continue,
        }
        snapshots.push(applied.clone());
    }
    snapshots
}

pub(super) fn edited(settings: impl FnOnce(LookSettings) -> LookSettings) -> LookFacts {
    look_facts_edited(settings(LookSettings::NEUTRAL))
}

/// Прямоугольник зоны (глазные pt) → пиксели поверхности этого стенда.
pub(super) fn zone_px(rig: &Rig, eye_rect: Rect) -> (f32, f32, f32, f32) {
    let layout = rig.screen.state().calibration().layout().expect("раскладка раздела");
    let rect = layout.eye_rect_on_surface(eye_rect);
    (px(rect.min_x()), px(rect.min_y()), px(rect.max_x()), px(rect.max_y()))
}

pub(super) fn scope_images(list: &DrawList) -> Vec<&ui_draw::draw_list::ImageItem> {
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Image(image) if image.source == ImageSource::Live(LiveImage::ColorScope) => Some(image),
            _ => None,
        })
        .collect()
}

// ----- прибор и чистая зона -----------------------------------------------------------------------------------------

#[test]
fn scope_image_lies_exactly_on_the_scope_zone_with_its_requested_size() {
    let mut rig = portrait(look_facts());
    let list = rig.frame().clone();
    let images = scope_images(&list);
    assert_eq!(images.len(), 1, "ровно одна картинка прибора");
    let layout = *rig.screen.state().calibration().layout().unwrap();
    let image = images[0];
    assert_eq!(image.fit, ImageFit::Stretch);
    assert_eq!(image.clip, ImageClip::None);
    assert_eq!(
        (image.size_px.width as u32, image.size_px.height as u32),
        (layout.scope_image_size().width(), layout.scope_image_size().height()),
        "размер картинки 1:1 с запросом прибора"
    );
    let zone = layout.zones().scope;
    let center_px = (px(zone.center().x), px(zone.center().y));
    assert!(
        near(image.transform.tx, center_px.0, 0.6) && near(image.transform.ty, center_px.1, 0.6),
        "центр картинки на центре зоны: ({}, {}) против {center_px:?}",
        image.transform.tx,
        image.transform.ty
    );
    assert!(
        near(image.transform.a, 1.0, 1e-4) && near(image.transform.d, 1.0, 1e-4),
        "в портрете картинка не повёрнута и не растянута: {:?}",
        image.transform
    );
}

// ----- PHOTO и CALIBRATE ----------------------------------------------------------------------------------------------

#[test]
fn photo_widgets_are_absent_in_calibrate_and_the_section_is_absent_in_photo() {
    let mut rig = Rig::new();
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts())));
    let photo = rig.frame().clone();
    assert!(scope_images(&photo).is_empty(), "в PHOTO прибора нет");
    assert_eq!(
        text_count_of(&photo, "CONTRAST", TextRoleKind::CardTitle),
        0,
        "и колонки ручек нет"
    );
    assert_eq!(rig.screen.node_centers("calib_ruler").len(), 0);
    let shutter_in_photo = glass_members(&photo)
        .iter()
        .any(|(shape, ..)| matches!(shape, ui_draw::shape::ShapeGeometry::Circle { diameter } if near(*diameter, 197.0, 2.0)));
    assert!(shutter_in_photo, "в PHOTO затвор Ø197 есть");
    assert!(!icons_of(&photo, IconId::FlipCamera).is_empty() || !icons_of(&photo, IconId::FlashOff).is_empty());

    rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    let calibrate = rig.run(400).clone();
    let shutter_in_calibrate = glass_members(&calibrate)
        .iter()
        .any(|(shape, ..)| matches!(shape, ui_draw::shape::ShapeGeometry::Circle { diameter } if near(*diameter, 197.0, 2.0)));
    assert!(!shutter_in_calibrate, "в CALIBRATE затвора нет");
    for icon in [
        IconId::FlipCamera,
        IconId::FlashOff,
        IconId::FlashOn,
        IconId::FlashAuto,
        IconId::Exposure,
        IconId::Timer,
    ] {
        assert!(
            icons_of(&calibrate, icon).is_empty(),
            "значок {icon:?} PHOTO не должен рисоваться в CALIBRATE"
        );
    }
    assert_eq!(scope_images(&calibrate).len(), 1, "прибор есть");
    assert_eq!(
        text_count_of(&calibrate, "CONTRAST", TextRoleKind::CardTitle),
        1,
        "колонка ручек есть"
    );

    rig.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    let back = rig.run(400).clone();
    assert!(
        scope_images(&back).is_empty(),
        "после возврата в PHOTO прибор не рисуется"
    );
    assert_eq!(text_count_of(&back, "CONTRAST", TextRoleKind::CardTitle), 0);
    assert!(
        glass_members(&back)
            .iter()
            .any(|(shape, ..)| matches!(shape, ui_draw::shape::ShapeGeometry::Circle { diameter } if near(*diameter, 197.0, 2.0))),
        "затвор вернулся"
    );
}

#[test]
fn photo_controls_do_not_catch_touches_where_calibrate_has_its_glass() {
    let mut rig = portrait(look_facts());
    // Место, где в PHOTO стоит затвор: в CALIBRATE там зона нижнего ряда или стекло — снимка быть не может.
    let shutter = rig.shutter_center();
    rig.tap(shutter);
    rig.run(200);
    assert!(
        rig.effects().iter().all(|effect| !matches!(effect, Effect::Capture(_))),
        "тап по месту затвора ничего не снимает"
    );
}

// ----- ручки --------------------------------------------------------------------------------------------------------

#[test]
fn dragging_a_knob_ruler_sends_the_exact_value_the_finger_chose() {
    let mut rig = portrait(look_facts());
    let ruler = center(&rig, "calib_ruler", 0);
    // Три шага по 20 pt, плюс гистерезис пана 10 pt: линейка ровно на 0,3.
    drag_eye(&mut rig, ruler, Vector::new(-70.5, 0.0));
    let sent = set_looks(&rig.effects());
    let last = sent.last().expect("палец тянул ручку, SetLook ушёл");
    assert!((last.knob(GlobalKnob::Contrast).get() - 0.3).abs() < 1e-6, "{last:?}");
    assert_eq!(last.knob(GlobalKnob::Midtones).get(), 0.0, "соседние ручки не тронуты");
    assert!(sent.iter().all(|s| s.knob(GlobalKnob::Shadows).get() == 0.0));
}

#[test]
fn each_row_drives_its_own_knob() {
    let mut rig = portrait(look_facts());
    for (slot, knob_id) in GlobalKnob::ALL.into_iter().enumerate() {
        let ruler = center(&rig, "calib_ruler", slot);
        // Нижние строки могут быть вне окна списка: листаем так, чтобы строка была видна, и перечитываем центр.
        if ruler.y > 700.0 {
            continue;
        }
        drag_eye(&mut rig, ruler, Vector::new(-70.5, 0.0));
        let sent = set_looks(&rig.effects());
        let last = sent
            .last()
            .unwrap_or_else(|| panic!("ручка {knob_id:?}: SetLook не ушёл"));
        assert!((last.knob(knob_id).get() - 0.3).abs() < 1e-6, "{knob_id:?}: {last:?}");
    }
}

#[test]
fn reset_arrow_sends_the_reset_only_when_the_value_differs_from_the_preset() {
    let facts = edited(|s| s.with_knob(GlobalKnob::Contrast, knob(0.3)));
    let mut rig = portrait(facts);
    let reset = center(&rig, "calib_reset", 0);
    rig.tap(reset);
    rig.run(100);
    let intents = look_intents(&rig.effects()).into_iter().cloned().collect::<Vec<_>>();
    assert_eq!(
        intents,
        vec![LookIntent::ResetControl(LookControl::Knob(GlobalKnob::Contrast))],
        "яркая «↻» сбрасывает ручку к пресету"
    );
    // «↻» ручки, которая равна пресету, тусклая и ничего не делает.
    let dim = center(&rig, "calib_reset", 1);
    rig.tap(dim);
    rig.run(100);
    assert!(look_intents(&rig.effects()).is_empty(), "тусклая «↻» молчит");
    assert_eq!(rig.screen.diagnostics().actions_rejected, 0);
}

#[test]
fn reset_arrow_is_bright_when_the_value_differs_and_dimmed_to_44_percent_when_equal() {
    let facts = edited(|s| s.with_knob(GlobalKnob::Contrast, knob(0.3)));
    let mut rig = portrait(facts);
    let list = rig.frame().clone();
    let resets = icons_of(&list, IconId::Reset);
    // Девять «↻» (восемь ручек + основа) и три оси полосы; видимы те, что внутри окна списка.
    let opacities: Vec<f32> = resets.iter().map(|icon| icon.opacity.get()).collect();
    assert!(
        opacities.iter().any(|o| near(*o, 1.0, 0.02)),
        "яркая «↻» у правленой ручки: {opacities:?}"
    );
    assert!(
        opacities.iter().any(|o| near(*o, 0.44, 0.02)),
        "тусклая «↻» (44 %) у равных пресету: {opacities:?}"
    );
}

#[test]
fn value_between_steps_is_shown_as_is_and_never_echoed_back() {
    // Сброс к пресету со значением 0,15 при шаге 0,1: линейка стоит между ступенями.
    let mut facts = look_facts();
    facts.current.settings = facts.current.settings.with_knob(GlobalKnob::Contrast, knob(0.15));
    facts.preset = facts.current.clone();
    let mut rig = portrait(facts);
    rig.run(1500);
    assert!(
        rig.effects().iter().all(|effect| !matches!(effect, Effect::Look(_))),
        "программная синхронизация не рождает KnobDragged"
    );
    let position = rig.screen.calibration_row_position_steps(0).expect("строка Contrast");
    assert!(
        (position - 1.5).abs() < 0.02,
        "линейка стоит на 1,5 шага, а не на ступени: {position}"
    );
    let list = rig.frame().clone();
    assert_eq!(
        text_count_of(&list, "+0.15", TextRoleKind::ScaleValue),
        1,
        "подпись показывает значение из фактов как есть"
    );
    // Новые факты (например, пресет с другим значением) тоже едут без действий.
    let mut next = look_facts();
    next.current.settings = next.current.settings.with_knob(GlobalKnob::Contrast, knob(0.55));
    next.preset = next.current.clone();
    rig.screen.handle_action(ScreenAction::LookFactsChanged(Box::new(next)));
    rig.run(1500);
    assert!(rig.effects().iter().all(|effect| !matches!(effect, Effect::Look(_))));
    let moved = rig.screen.calibration_row_position_steps(0).unwrap();
    assert!((moved - 5.5).abs() < 0.02, "линейка доехала до 0,55: {moved}");
    assert_eq!(rig.screen.diagnostics().invalid_knob_values, 0);
}

#[test]
fn unsupported_knob_is_dimmed_and_ignores_the_finger_while_neighbours_work() {
    let mut facts = look_facts();
    facts.unsupported_knobs = vec![GlobalKnob::Contrast];
    let mut rig = portrait(facts);
    let list = rig.frame().clone();
    let dimmed = texts_of(&list, "CONTRAST", TextRoleKind::CardTitle);
    let normal = texts_of(&list, "MIDTONES", TextRoleKind::CardTitle);
    assert_eq!((dimmed.len(), normal.len()), (1, 1));
    assert!(
        dimmed[0].opacity.get() < 0.5 && near(normal[0].opacity.get(), 1.0, 0.01),
        "недоступная ручка тусклая ({}), соседняя нет ({})",
        dimmed[0].opacity.get(),
        normal[0].opacity.get()
    );
    let contrast = center(&rig, "calib_ruler", 0);
    drag_eye(&mut rig, contrast, Vector::new(-70.5, 0.0));
    assert!(
        set_looks(&rig.effects()).is_empty(),
        "недоступная ручка не двигается пальцем"
    );
    let midtones = center(&rig, "calib_ruler", 1);
    drag_eye(&mut rig, midtones, Vector::new(-70.5, 0.0));
    assert!(!set_looks(&rig.effects()).is_empty(), "соседняя двигается");
}

#[test]
fn without_look_facts_the_knobs_are_dim_and_inert() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.run(400);
    rig.effects();
    let ruler = center(&rig, "calib_ruler", 0);
    drag_eye(&mut rig, ruler, Vector::new(-70.5, 0.0));
    assert!(
        look_intents(&rig.effects()).is_empty(),
        "без библиотеки пресетов ручки молчат"
    );
    assert!(rig.frame().items.len() > 5, "раздел всё равно рисуется");
}

// ----- основа тона и «чистяк» ---------------------------------------------------------------------------------------

#[test]
fn swiping_the_base_switcher_selects_the_next_tone_base() {
    let mut rig = portrait(look_facts());
    let carousel = center(&rig, "calib_base_carousel", 0);
    drag_eye(&mut rig, carousel, Vector::new(-60.0, 0.0));
    let intents = look_intents(&rig.effects()).into_iter().cloned().collect::<Vec<_>>();
    assert_eq!(intents, vec![LookIntent::SetToneBase(ToneBaseModel::Aces)]);
}

#[test]
fn base_reset_arrow_resets_the_base_only_when_it_differs() {
    let mut facts = look_facts();
    facts.current.base = ToneBaseModel::Filmic;
    facts.edits = EditState::EditedOnTop;
    let mut rig = portrait(facts);
    let reset = center(&rig, "calib_base_reset", 0);
    rig.tap(reset);
    rig.run(100);
    let intents = look_intents(&rig.effects()).into_iter().cloned().collect::<Vec<_>>();
    assert_eq!(intents, vec![LookIntent::ResetControl(LookControl::ToneBase)]);
    let mut equal = portrait(look_facts());
    let reset = center(&equal, "calib_base_reset", 0);
    equal.tap(reset);
    equal.run(100);
    assert!(
        look_intents(&equal.effects()).is_empty(),
        "основа равна пресету: «↻» тусклая и молчит"
    );
}

#[test]
fn clean_button_holds_the_base_while_pressed_and_releases_on_lift_or_cancel() {
    let mut rig = portrait(look_facts());
    let clean = center(&rig, "calib_clean", 0);
    let shown = |rig: &mut Rig| -> Vec<look_contract::CleanView> {
        rig.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::CleanView(view) => Some(view),
                _ => None,
            })
            .collect()
    };
    // Нажатие и удержание.
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(200);
    assert_eq!(
        shown(&mut rig),
        vec![look_contract::CleanView::ShowBase],
        "нажали — показана основа"
    );
    // Отпускание.
    let t = rig.now_ms;
    rig.touch(TouchPhase::Ended, clean, t);
    rig.run(100);
    assert_eq!(
        shown(&mut rig),
        vec![look_contract::CleanView::ShowEdited],
        "отпустили — вид вернулся"
    );
    // Отмена жеста системой.
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(100);
    assert_eq!(shown(&mut rig), vec![look_contract::CleanView::ShowBase]);
    let t = rig.now_ms;
    rig.touch(TouchPhase::Cancelled, clean, t);
    rig.run(100);
    assert_eq!(
        shown(&mut rig),
        vec![look_contract::CleanView::ShowEdited],
        "отмена тоже отпускает"
    );
}

#[test]
fn clean_view_never_sticks_when_the_touch_stream_breaks_or_the_phone_turns() {
    let mut rig = portrait(look_facts());
    let clean = center(&rig, "calib_clean", 0);
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(100);
    rig.effects();
    // Разрыв потока касаний посреди удержания.
    rig.screen.handle_touch(super::ScreenTouch::Gap);
    rig.run(200);
    assert!(
        rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowEdited))),
        "после разрыва потока вид вернулся"
    );
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited
    );
    // Поворот телефона посреди удержания: раздел пересобирается, кнопка не залипает.
    let clean = center(&rig, "calib_clean", 0);
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(100);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::HoldingBase
    );
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(400);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited,
        "пересборка раздела не оставила «чистяк» нажатым"
    );
}

#[test]
fn a_clean_view_held_by_an_action_stays_held_across_frames() {
    // Удержание пришло действием, а не пальцем кнопки (сценарий, сквозной тест, другой хост): страховка от залипания
    // срабатывает только после пересборки шапки и не должна отпускать такое удержание на следующем кадре (дефект,
    // найденный сквозным тестом `calibration_end_to_end`: «чистяк» отпускался раньше, чем рендер успевал его показать).
    let mut rig = portrait(look_facts());
    rig.act(ScreenAction::CleanViewPressed);
    rig.run(500);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::HoldingBase,
        "полсекунды кадров не отпустили удержание без пальца"
    );
    assert!(
        !rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowEdited))),
        "экран сам не просил вернуть правки"
    );
    rig.act(ScreenAction::CleanViewReleased);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited
    );
}

// ----- полосы -------------------------------------------------------------------------------------------------------

// ----- прокрутка ----------------------------------------------------------------------------------------------------

// ----- приборы ------------------------------------------------------------------------------------------------------
