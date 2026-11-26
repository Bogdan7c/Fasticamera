//! Виджеты раздела калибровки целиком: касания → действия → эффекты, список рисования, поворот раздела.
//!
//! Проверяется то, что видит и делает пользователь: палец тянет линейку ручки — уходит `SetLook` с точным
//! значением; тап по «↻» сбрасывает, только если значение отличается; «чистяк» держится и отпускается; прибор
//! стоит ровно на своей зоне и повёрнут вместе с разделом; PHOTO в режиме CALIBRATE не рисуется и не нажимается.

use look_contract::{EditState, GlobalKnob, LookControl, LookFacts, LookIntent, LookSettings, ToneBaseModel};
use scope_contract::{ScopeIntent, ScopeKind};
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
    calibrating_with(crate::config::ScreenConfig::default(), facts, orientation)
}

/// То же по своему конфигу экрана (сторона колонки ручек, числа раскладки).
pub(super) fn calibrating_with(
    config: crate::config::ScreenConfig,
    facts: LookFacts,
    orientation: DeviceOrientation,
) -> Rig {
    let mut rig = Rig::with(config, ui_core::time::MotionPreference::Full);
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
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Image(image) if image.source == ImageSource::Live(LiveImage::ColorScope) => Some(image),
            _ => None,
        })
        .collect()
}

// ----- прибор и чистая зона -----------------------------------------------------------------------------------------

#[test]
fn vectorscope_picture_is_a_square_centred_in_the_zone_and_matches_its_request() {
    let mut rig = portrait(look_facts());
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));
    rig.run(400);
    let requested = rig
        .effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Scopes(ScopeIntent::Show(request)) => Some(request),
            _ => None,
        })
        .expect("смена прибора просит у рендера новый прибор");
    let list = rig.frame().clone();
    let images = scope_images(&list);
    assert_eq!(images.len(), 1, "ровно одна картинка прибора");
    let image = images[0];
    let layout = *rig.screen.state().calibration().layout().unwrap();
    // Элемент рисования — квадрат со стороной меньшей стороны зоны и ровно размером запроса (1:1, без растяжения).
    assert_eq!(image.size_px.width, image.size_px.height, "квадрат, а не овал");
    assert_eq!(
        (image.size_px.width as u32, image.size_px.height as u32),
        (requested.image_size.width(), requested.image_size.height())
    );
    // Квадрат вписан во ВСЮ область картинки (без поля уровней, решение 274): сторона — меньшая сторона области.
    let area = layout.scope_picture_area();
    let area_px = (px(area.size.width).round() as u32, px(area.size.height).round() as u32);
    assert_eq!(image.size_px.width as u32, area_px.0.min(area_px.1));
    // Картинка стоит по центру области картинки, а не зоны: сверху полоска.
    assert!(
        near(image.transform.tx, px(area.center().x), 0.6) && near(image.transform.ty, px(area.center().y), 0.6),
        "центр квадрата на центре области картинки: ({}, {})",
        image.transform.tx,
        image.transform.ty
    );
}

#[test]
fn the_first_page_title_names_the_rgb_waveform_and_the_old_title_is_gone() {
    let mut rig = portrait(look_facts());
    let list = rig.frame().clone();
    assert_eq!(text_count_of(&list, "RGB WAVEFORM", TextRoleKind::Caption), 1);
    assert_eq!(
        text_count_of(&list, "WAVEFORM", TextRoleKind::Caption),
        0,
        "прежней подписи нет"
    );
}

#[test]
fn scope_image_lies_exactly_on_its_rect_below_the_strip_with_its_requested_size() {
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
        (
            layout.scope_image_size_for(ScopeKind::Waveform).width(),
            layout.scope_image_size_for(ScopeKind::Waveform).height()
        ),
        "размер картинки 1:1 с запросом прибора"
    );
    let image_rect = layout.scope_image_rect_for(ScopeKind::Waveform);
    let center_px = (px(image_rect.center().x), px(image_rect.center().y));
    assert!(
        near(image.transform.tx, center_px.0, 0.6) && near(image.transform.ty, center_px.1, 0.6),
        "центр картинки на центре её прямоугольника: ({}, {}) против {center_px:?}",
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
    // Путь на 60 единиц шкалы: линейка ровно на +60 (значение 0,30), без потерь на порог пана.
    drag_eye(&mut rig, ruler, Vector::new(-knob_finger_pt_from_zero(60.0), 0.0));
    let sent = set_looks(&rig.effects());
    let last = sent.last().expect("палец тянул ручку, SetLook ушёл");
    assert!((last.knob(GlobalKnob::Contrast).get() - 0.3).abs() < 1e-6, "{last:?}");
    assert_eq!(last.knob(GlobalKnob::Midtones).get(), 0.0, "соседние ручки не тронуты");
    assert!(sent.iter().all(|s| s.knob(GlobalKnob::Shadows).get() == 0.0));
}

/// Листает список ручек пальцем, пока линейка строки `slot` не окажется в видимой части окна (ниже шапки и выше
/// низа колонки), и возвращает её центр. Строка вне окна пальцем не берётся, поэтому её НЕ пропускаем, а доводим.
pub(super) fn scroll_row_into_view(rig: &mut Rig, slot: usize) -> Point {
    for _ in 0..8 {
        let ruler = center(rig, "calib_ruler", slot);
        let window = center(rig, "calib_knobs_window", 0);
        let knobs = rig
            .screen
            .state()
            .calibration()
            .layout()
            .expect("раскладка раздела")
            .zones()
            .knobs;
        // Верх окна — под шапкой и переключателем вида (≈ 112 pt от верха колонки).
        let visible = ruler.y >= knobs.min_y() + 140.0 && ruler.y <= knobs.max_y() - 60.0;
        if visible {
            return ruler;
        }
        // Палец вверх на расстояние до окна: вертикальная протяжка, начатая на линейке, листает список.
        let to_window = (ruler.y - window.y).clamp(-300.0, 300.0);
        let start = Point::new(window.x, window.y);
        drag_eye(rig, start, Vector::new(0.0, -to_window));
        rig.effects();
    }
    panic!("строка {slot} не доехала до видимой части окна за 8 протяжек");
}

#[test]
fn each_row_drives_its_own_knob() {
    let mut rig = portrait(look_facts());
    for (slot, knob_id) in GlobalKnob::ALL.into_iter().enumerate() {
        // Нижние строки лежат ниже окна списка: листаем до строки пальцем, а не пропускаем её.
        let ruler = scroll_row_into_view(&mut rig, slot);
        // Путь на +60 единиц (значение 0,30); начинаем на правой стороне линейки, чтобы палец не ушёл за левый край экрана.
        let finger_dx = knob_finger_pt_from_zero(60.0);
        drag_eye(
            &mut rig,
            Point::new(ruler.x + 60.0, ruler.y),
            Vector::new(-finger_dx, 0.0),
        );
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
    // Сброс к пресету со значением 0,153 (между единицами шкалы −200…+200): линейка стоит между ступенями.
    let mut facts = look_facts();
    facts.current.settings = facts.current.settings.with_knob(GlobalKnob::Contrast, knob(0.153));
    facts.preset = facts.current.clone();
    let mut rig = portrait(facts);
    rig.run(1500);
    assert!(
        rig.effects().iter().all(|effect| !matches!(effect, Effect::Look(_))),
        "программная синхронизация не рождает KnobDragged"
    );
    let position = rig.screen.calibration_row_position_steps(0).expect("строка Contrast");
    assert!(
        (position - 30.6).abs() < 0.02,
        "линейка стоит на 30,6 шага, а не на ступени: {position}"
    );
    let list = rig.frame().clone();
    assert_eq!(
        text_count_of(&list, "+31", TextRoleKind::ScaleValue),
        1,
        "подпись показывает значение из фактов округлённым до целого"
    );
    // Новые факты (например, пресет с другим значением) тоже едут без действий.
    let mut next = look_facts();
    next.current.settings = next.current.settings.with_knob(GlobalKnob::Contrast, knob(0.55));
    next.preset = next.current.clone();
    rig.screen.handle_action(ScreenAction::LookFactsChanged(Box::new(next)));
    rig.run(1500);
    assert!(rig.effects().iter().all(|effect| !matches!(effect, Effect::Look(_))));
    let moved = rig.screen.calibration_row_position_steps(0).unwrap();
    assert!((moved - 110.0).abs() < 0.02, "линейка доехала до +110: {moved}");
    assert_eq!(rig.screen.diagnostics().invalid_knob_values, 0);
}

#[test]
fn on_the_legacy_base_all_eight_knobs_take_the_finger_and_none_is_dimmed() {
    let mut facts = look_facts();
    facts.current.base = ToneBaseModel::Legacy;
    facts.preset = facts.current.clone();
    let mut rig = portrait(facts);
    let list = rig.frame().clone();
    // Строки тона, которые на legacy раньше были тусклыми, лежат в окне списка и обязаны быть яркими.
    for label in ["CONTRAST", "MIDTONES", "SHADOWS", "HIGHLIGHTS"] {
        for title in texts_of(&list, label, TextRoleKind::CardTitle) {
            assert!(near(title.opacity.get(), 1.0, 0.01), "{label} тусклая на legacy");
        }
    }
    for (slot, knob_id) in GlobalKnob::ALL.into_iter().enumerate() {
        let ruler = scroll_row_into_view(&mut rig, slot);
        drag_eye(&mut rig, Point::new(ruler.x + 60.0, ruler.y), Vector::new(-60.0, 0.0));
        let sent = set_looks(&rig.effects());
        let last = sent
            .last()
            .unwrap_or_else(|| panic!("legacy, ручка {knob_id:?}: палец ничего не отправил"));
        assert!(
            last.knob(knob_id).get() != 0.0,
            "legacy, ручка {knob_id:?}: значение не сдвинулось: {last:?}"
        );
    }
}

#[test]
fn without_look_facts_the_knobs_are_dim_and_inert() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.run(400);
    rig.effects();
    let ruler = center(&rig, "calib_ruler", 0);
    drag_eye(&mut rig, ruler, Vector::new(-60.0, 0.0));
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

/// Тап по карусели основы на `eye_dx` pt правее (глазами) её центра; что ушло библиотеке.
fn tap_base_at(rig: &mut Rig, eye_dx: f64) -> Vec<LookIntent> {
    let carousel = center(rig, "calib_base_carousel", 0);
    let offset = surface_vector(rig, Vector::new(eye_dx, 0.0));
    rig.tap(Point::new(carousel.x + offset.dx, carousel.y + offset.dy));
    rig.run(600);
    look_intents(&rig.effects()).into_iter().cloned().collect()
}

/// Сосед стоит на 69 pt от выбранной подписи (решение 229: тап по видимому соседу).
const NEIGHBOUR_DX: f64 = 69.0;

#[test]
fn tapping_the_visible_neighbour_selects_it_and_tapping_the_active_base_does_nothing() {
    for orientation in [DeviceOrientation::LandscapeLeft, DeviceOrientation::LandscapeRight] {
        let mut rig = calibrating(look_facts(), orientation);
        assert_eq!(
            tap_base_at(&mut rig, 0.0),
            Vec::new(),
            "{orientation:?}: AGX уже выбран"
        );
        assert_eq!(
            tap_base_at(&mut rig, NEIGHBOUR_DX),
            vec![LookIntent::SetToneBase(ToneBaseModel::Aces)],
            "{orientation:?}"
        );
        // Библиотека применила выбор и прислала факты; выбранная встала в центр, её сосед слева — снова AGX.
        let mut facts = look_facts();
        facts.current.base = ToneBaseModel::Aces;
        rig.act(ScreenAction::LookFactsChanged(Box::new(facts)));
        rig.run(600);
        assert_eq!(
            tap_base_at(&mut rig, -NEIGHBOUR_DX),
            vec![LookIntent::SetToneBase(ToneBaseModel::Agx)],
            "{orientation:?}"
        );
        rig.act(ScreenAction::LookFactsChanged(Box::new(look_facts())));
        rig.run(600);
        assert_eq!(
            tap_base_at(&mut rig, -NEIGHBOUR_DX),
            Vec::new(),
            "{orientation:?}: левее первой модели ничего нет"
        );
    }
}

#[test]
fn in_portrait_the_neighbour_base_is_visible_now_that_clean_left_the_header_and_a_tap_on_it_selects_it() {
    let mut rig = portrait(look_facts());
    // Решение 259: CLEAN ушёл вниз, карусели досталась вся ширина шапки, и справа от выбранной AGX видна ACES.
    assert_eq!(
        tap_base_at(&mut rig, 0.0),
        Vec::new(),
        "тап по выбранной основе ничего не меняет"
    );
    assert_eq!(
        tap_base_at(&mut rig, NEIGHBOUR_DX),
        vec![LookIntent::SetToneBase(ToneBaseModel::Aces)],
        "тап по видимому соседу в портрете выбирает его"
    );
    // Пустое место рядом с центром (между выбранной и соседом) и за левым краем ничего не выбирает.
    let mut rig = portrait(look_facts());
    for eye_dx in [-60.0, -20.0, 20.0] {
        assert_eq!(
            tap_base_at(&mut rig, eye_dx),
            Vec::new(),
            "тап на {eye_dx} pt от центра"
        );
    }
    assert_eq!(
        rig.screen.state().mode(),
        CaptureMode::Calibrate,
        "тап по шапке не проваливается на экран под ней"
    );
}

#[test]
fn without_facts_the_base_does_not_react_to_a_tap() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.run(400);
    rig.effects();
    assert_eq!(tap_base_at(&mut rig, NEIGHBOUR_DX), Vec::new());
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

// ----- полосы -------------------------------------------------------------------------------------------------------

// ----- прокрутка ----------------------------------------------------------------------------------------------------

// ----- приборы ------------------------------------------------------------------------------------------------------

#[test]
fn calibration_touch_targets_name_the_same_rulers_the_finger_really_catches() {
    let mut rig = portrait(look_facts());
    let targets = rig.screen.calibration_touch_targets().expect("раздел на экране");
    assert_eq!(targets.rulers, rig.screen.node_centers("calib_ruler"));
    assert_eq!(targets.resets.len(), targets.rulers.len(), "у каждой строки есть «↻»");
    assert_eq!(targets.band_dots.len(), 3, "точки трёх полос тестовых фактов");
    // Цель — не пустая бумага: палец на её линейке ведёт ручку.
    let ruler = targets.rulers[0];
    drag_eye(&mut rig, Point::new(ruler.x + 60.0, ruler.y), Vector::new(-60.0, 0.0));
    assert!(!set_looks(&rig.effects()).is_empty());
    // Вне режима калибровки целей нет.
    rig.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    rig.run(400);
    assert_eq!(rig.screen.calibration_touch_targets(), None);
}
