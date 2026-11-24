//! Сквозные тесты зума экрана (сессия 13): лента кнопок, колесо, щипок, переворот, завеса перехода. Касания идут
//! через настоящую арену жестов, кадры — через `produce_frame`; проверяются эффекты и список рисования.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::LensIntent;
use shell_contract::haptics::HapticPattern;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::TextRoleKind;

use super::test_rig::{FRAME_MS, PX_PER_PT, Rig, ms, near, px, text_count_of, texts_of};
use crate::effect::Effect;

// ---- наблюдения за списком рисования и эффектами ----

/// Центры нарисованных подписей зума с этим текстом по x, pt.
fn label_centers_x_pt(list: &DrawList, text: &str) -> Vec<f64> {
    texts_of(list, text, TextRoleKind::ZoomLabel)
        .iter()
        .map(|item| f64::from(item.transform.tx + item.run.advance_px * 0.5) / f64::from(PX_PER_PT))
        .collect()
}

/// Центры по x (pt) подписей зума, нарисованных цветом выбранной кнопки (`zoom_selected`): сколько бы цифр ни
/// стояло в тексте, выбранная подпись одна.
fn yellow_label_centers_x_pt(list: &DrawList) -> Vec<f64> {
    let yellow = ui_kit::tokens::UiKitTokens::default().colors.zoom_selected;
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Text(text) if text.color == yellow && text.opacity.get() > 0.05 => {
                Some(f64::from(text.transform.tx + text.run.advance_px * 0.5) / f64::from(PX_PER_PT))
            }
            _ => None,
        })
        .collect()
}

/// Тёмный круг выбранной кнопки ленты Ø 37 pt: есть ли он и где (центр по x, pt).
fn selected_circle_x_pt(list: &DrawList) -> Vec<f64> {
    circles_of_diameter_pt(list, 37.0)
}

fn circles_of_diameter_pt(list: &DrawList, diameter_pt: f32) -> Vec<f64> {
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape)
                if matches!(shape.geometry, ShapeGeometry::Circle { diameter } if near(diameter, px(f64::from(diameter_pt)), 1.0)) =>
            {
                Some(f64::from(shape.transform.tx) / f64::from(PX_PER_PT))
            }
            _ => None,
        })
        .collect()
}

/// Есть ли диск колеса (круг Ø 486 pt).
fn wheel_disc_visible(list: &DrawList) -> bool {
    list.leaf_items().any(|item| match item {
        DrawItem::Shape(shape) => {
            matches!(shape.geometry, ShapeGeometry::Circle { diameter } if near(diameter, px(486.0), 1.0))
                && shape.opacity.get() > 0.05
        }
        _ => false,
    })
}

fn lens_intents(effects: &[Effect]) -> Vec<LensIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Lens(intent) => Some(*intent),
            _ => None,
        })
        .collect()
}

fn haptics(effects: &[Effect]) -> Vec<HapticPattern> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Haptic(pattern) => Some(*pattern),
            _ => None,
        })
        .collect()
}

fn ratio_of(rig: &Rig) -> f32 {
    rig.screen.state().zoom().ratio().value()
}

fn button_center(rig: &Rig, slot: usize, selected: usize) -> Point {
    rig.center_with(|geometry| geometry.zoom_button(slot, selected))
}

/// Держим палец на `from` до долгого тапа, затем ведём его на `dx_pt` вправо за `drag_ms`. Палец не отпущен.
fn hold_then_drag(rig: &mut Rig, from: Point, dx_pt: f64, drag_ms: i64) -> Point {
    rig.touch(TouchPhase::Began, from, rig.now_ms);
    rig.run(560);
    let steps = (drag_ms / FRAME_MS).max(1);
    let mut point = from;
    for step in 1..=steps {
        point = Point::new(from.x + dx_pt * step as f64 / steps as f64, from.y);
        rig.touch(TouchPhase::Moved, point, rig.now_ms);
        rig.run(FRAME_MS);
    }
    point
}

fn release(rig: &mut Rig, at: Point) {
    rig.touch(TouchPhase::Ended, at, rig.now_ms);
    rig.run(FRAME_MS);
}

// ---- лента кнопок ----

#[test]
fn the_strip_shows_catalog_labels_with_the_selected_one_yellow_at_x_220() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    let list = rig.run(32).clone();

    // Выбранная «1×» всегда на x = 220; соседи — через шаг 39,0 (`[measured: R00 t=88.5–100]`).
    let selected = label_centers_x_pt(&list, "1\u{d7}");
    assert_eq!(selected.len(), 1);
    assert!((selected[0] - 220.0).abs() < 0.6, "{selected:?}");
    let ultra_wide = label_centers_x_pt(&list, ".6");
    let crop = label_centers_x_pt(&list, "2");
    assert_eq!(ultra_wide.len(), 1, "ширик: «.6» без «×» и без нуля");
    assert!((ultra_wide[0] - 181.0).abs() < 0.8, "{ultra_wide:?}");
    assert!((crop[0] - 259.0).abs() < 0.8, "{crop:?}");
    // Тёмный круг Ø 37 стоит под выбранной.
    let circles = selected_circle_x_pt(&list);
    assert_eq!(circles.len(), 1);
    assert!((circles[0] - 220.0).abs() < 0.6);
    // Цвет выбранной — золотистый #FFCE00 из токенов, не белый.
    let yellow = texts_of(&list, "1\u{d7}", TextRoleKind::ZoomLabel)[0].color;
    assert_eq!(yellow, ui_kit::tokens::UiKitTokens::default().colors.zoom_selected);
    assert_ne!(yellow, DisplayColor::WHITE);
}

#[test]
fn after_adopting_the_ultra_wide_the_strip_selects_the_first_button_and_writes_0_6_with_a_zero() {
    let mut rig = Rig::with_zoom(LensRole::UltraWide, MotionPreference::Full);
    let list = rig.run(32).clone();
    let selected = label_centers_x_pt(&list, "0.6\u{d7}");
    assert_eq!(selected.len(), 1, "выбранная кнопка на iOS: «0,5×» → у нас «0.6×»");
    assert!((selected[0] - 220.0).abs() < 0.6);
    assert_eq!(label_centers_x_pt(&list, "1").len(), 1);
    assert!((label_centers_x_pt(&list, "1")[0] - 259.0).abs() < 0.8);
}

#[test]
fn tapping_a_strip_button_glides_the_zoom_publishes_one_intent_and_slides_the_strip() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    rig.tap(button_center(&rig, 0, 1));
    // Приложение забирает эффекты каждый кадр: промежуточные намерения видны, очередь их не копит.
    let mut effects = rig.effects();
    for _ in 0..25 {
        rig.run(FRAME_MS);
        effects.extend(rig.effects());
    }

    let intents = lens_intents(&effects);
    assert_eq!(intents.len(), 1, "один тап = одно намерение: {intents:?}");
    assert!(near(intents[0].ratio.value(), 0.593, 1.0e-3));
    assert!(intents.iter().all(|intent| intent.facing == LensFacing::Back));
    assert!(haptics(&effects).is_empty(), "кнопки без вибрации");
    assert!(near(ratio_of(&rig), 0.593, 1.0e-3));
    // Лента доехала: «0.6×» выбрана на x = 220, «1» стала соседом справа.
    let list = rig.run(200).clone();
    assert!((label_centers_x_pt(&list, "0.6\u{d7}")[0] - 220.0).abs() < 0.6);
    assert!((label_centers_x_pt(&list, "1")[0] - 259.0).abs() < 0.8);
    assert_eq!(selected_circle_x_pt(&list).len(), 1);
}

#[test]
fn the_strip_slides_during_the_move_instead_of_jumping() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    rig.tap(button_center(&rig, 2, 1));
    let early = rig.run(48).clone();
    // Через ≈ 0,1 с после тапа «2×» ещё не на месте: она между старой (259) и новой (220) позицией.
    let moving = yellow_label_centers_x_pt(&early);
    assert_eq!(moving.len(), 1);
    assert!(moving[0] > 221.0 && moving[0] < 258.5, "{moving:?}");
    let settled = rig.run(600).clone();
    assert!((label_centers_x_pt(&settled, "2\u{d7}")[0] - 220.0).abs() < 0.6);
}

#[test]
fn reduce_motion_puts_the_strip_on_the_new_place_at_once() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Reduced);
    rig.run(32);
    rig.tap(button_center(&rig, 2, 1));
    let early = rig.run(32).clone();
    let moving = yellow_label_centers_x_pt(&early);
    assert_eq!(moving.len(), 1);
    assert!(
        (moving[0] - 220.0).abs() < 0.6,
        "без перемещений лента встаёт сразу: {moving:?}"
    );
}

// ---- колесо ----

#[test]
fn long_press_on_the_strip_opens_the_wheel_and_replaces_only_the_strip() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let before = rig.run(16).clone();
    assert!(!wheel_disc_visible(&before));
    assert_eq!(selected_circle_x_pt(&before).len(), 1);

    let from = button_center(&rig, 1, 1);
    rig.touch(TouchPhase::Began, from, rig.now_ms);
    let list = rig.run(700).clone();

    assert!(rig.screen.state().zoom().is_wheel_open());
    assert!(wheel_disc_visible(&list), "диск колеса нарисован");
    assert!(
        selected_circle_x_pt(&list).is_empty(),
        "кнопок ленты нет: круг выбранной пропал"
    );
    // Режимы и затвор остались на месте (`[measured: R00 t=88.5–99.5]`).
    assert_eq!(text_count_of(&list, "PHOTO", TextRoleKind::ModeLabel), 1);
    let shutter = rig.shutter_center();
    assert!(
        super::test_rig::glass_members(&list)
            .iter()
            .any(|(_, x, y)| near(*x, px(shutter.x), 2.0) && near(*y, px(shutter.y), 2.0)),
        "затвор на месте"
    );
    // Показание на месте ленты: жёлтое «1×».
    assert_eq!(text_count_of(&list, "1\u{d7}", TextRoleKind::ZoomLabel), 1);
    release(&mut rig, from);
}

/// Маркер колеса — измеренный жёлтый треугольник 3,3 × 8,7 pt (правило 96), а не прямоугольник: иконка
/// `ZoomMarkerTriangle` цвета выбранного зума, ровно над центром диска на радиусе середины кольца.
#[test]
fn the_open_wheel_draws_the_measured_yellow_triangle_marker_above_the_disc_centre() {
    use ui_draw::icons::IconId;

    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = button_center(&rig, 1, 1);
    rig.touch(TouchPhase::Began, from, rig.now_ms);
    let list = rig.run(700).clone();
    assert!(rig.screen.state().zoom().is_wheel_open());

    let markers: Vec<_> = list
        .leaf_items()
        .filter_map(|item| match item {
            DrawItem::Icon(icon) if icon.icon == IconId::ZoomMarkerTriangle => Some(*icon),
            _ => None,
        })
        .collect();
    assert_eq!(markers.len(), 1, "ровно один маркер");
    let marker = markers[0];
    assert_eq!(
        marker.color,
        ui_kit::tokens::UiKitTokens::default().colors.zoom_selected
    );
    assert!(near(marker.size_px.width, px(3.3), 0.05), "{:?}", marker.size_px);
    assert!(near(marker.size_px.height, px(8.7), 0.05), "{:?}", marker.size_px);
    // Диск колеса (Ø 486 pt): маркер строго над его центром, на радиусе 230,5 pt.
    let disc = list
        .leaf_items()
        .find_map(|item| match item {
            DrawItem::Shape(shape)
                if matches!(shape.geometry, ShapeGeometry::Circle { diameter } if near(diameter, px(486.0), 1.0)) =>
            {
                Some(shape.transform)
            }
            _ => None,
        })
        .expect("диск колеса");
    assert!(near(marker.transform.tx, disc.tx, 0.5), "x маркера = x центра диска");
    assert!(
        near(disc.ty - marker.transform.ty, px(230.5), 0.5),
        "радиус маркера: {}",
        disc.ty - marker.transform.ty
    );
    release(&mut rig, from);
}

#[test]
fn dragging_the_wheel_changes_the_ratio_on_a_log_scale() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = button_center(&rig, 1, 1);
    // Одна октава дуги — 57 pt по токену `Dial`; палец влево — значение растёт.
    let end = hold_then_drag(&mut rig, from, -57.0, 640);
    let effects = rig.effects();
    let last = lens_intents(&effects).last().copied().expect("намерения колеса");
    assert!(
        near(last.ratio.value(), 2.0, 0.12),
        "57 pt влево = октава вверх: {}",
        last.ratio.value()
    );
    assert_eq!(last.facing, LensFacing::Back);
    // Вдвое меньший сдвиг — не 1,5 (линейно), а √2 (логарифмически) ≈ 1,41.
    release(&mut rig, end);
    rig.run(3000);
    assert!(!rig.screen.state().zoom().is_wheel_open());
}

#[test]
fn half_an_octave_of_drag_gives_the_square_root_of_two() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = button_center(&rig, 1, 1);
    hold_then_drag(&mut rig, from, -28.5, 480);
    let value = ratio_of(&rig);
    assert!(near(value, std::f32::consts::SQRT_2, 0.09), "{value}");
}

#[test]
fn the_wheel_leaves_after_the_idle_time_and_the_strip_comes_back() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = button_center(&rig, 1, 1);
    let end = hold_then_drag(&mut rig, from, -10.0, 160);
    release(&mut rig, end);
    rig.run(1500);
    assert!(
        rig.screen.state().zoom().is_wheel_open(),
        "через 1,5 с колесо ещё на месте"
    );
    // Магнит упора доезжает до ≈ 0,5 с после отпускания, затем ≈ 2,15 с покоя — и колесо уходит.
    rig.run(3000);
    assert!(!rig.screen.state().zoom().is_wheel_open());
    let list = rig.run(400).clone();
    assert!(!wheel_disc_visible(&list), "колесо растворилось");
    assert_eq!(selected_circle_x_pt(&list).len(), 1, "лента вернулась");
}

#[test]
fn wheel_gives_ticks_and_a_rigid_click_on_the_stop_without_overloading_the_motor() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = button_center(&rig, 1, 1);
    // Медленно от 1× к 2× (≈ 57 pt за 2,4 с): по пути ≈ 9 рисок и упор «2» в конце.
    hold_then_drag(&mut rig, from, -58.0, 2400);
    let played = haptics(&rig.effects());
    let ticks = played.iter().filter(|p| **p == HapticPattern::SelectionTick).count();
    let clicks = played.iter().filter(|p| **p == HapticPattern::RigidImpact).count();
    assert!(ticks >= 7, "тик на каждой риске: {played:?}");
    assert_eq!(clicks, 1, "один щелчок на упоре «2»: {played:?}");
}

#[test]
fn a_fast_fling_never_vibrates_more_often_than_every_30_ms() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = button_center(&rig, 1, 1);
    rig.touch(TouchPhase::Began, from, rig.now_ms);
    rig.run(560);
    // Быстрый бросок: 200 pt влево за 160 мс, дальше колесо доезжает по инерции. Время каждого кадра пишем.
    let mut moments = Vec::new();
    let mut point = from;
    for step in 1..=10 {
        point = Point::new(from.x - 20.0 * f64::from(step), from.y);
        rig.touch(TouchPhase::Moved, point, rig.now_ms);
        rig.run(FRAME_MS);
        moments.extend(haptics(&rig.effects()).into_iter().map(|_| rig.now_ms));
    }
    release(&mut rig, point);
    for _ in 0..60 {
        rig.run(FRAME_MS);
        moments.extend(haptics(&rig.effects()).into_iter().map(|_| rig.now_ms));
    }
    assert!(moments.len() >= 4, "бросок вибрирует: {moments:?}");
    assert!(
        moments.windows(2).all(|pair| pair[1] - pair[0] >= 30),
        "вибрации ближе 30 мс друг к другу: {moments:?}"
    );
}

// ---- состояния для сверки (колесо на 1,7×) ----

#[test]
fn the_open_wheel_at_1_7x_shows_the_yellow_readout_the_focal_length_and_the_ring() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    rig.screen
        .handle_action(crate::ScreenAction::ZoomWheelTouched { at: ms(rig.now_ms) });
    rig.screen.handle_action(crate::ScreenAction::ZoomWheelDragged {
        requested: crate::test_support::ratio(1.7),
        at: ms(rig.now_ms),
    });
    let list = rig.run(400).clone();

    // Показание: «1.7×» жёлтым и «41MM» (24 мм × 1,7).
    assert_eq!(yellow_label_centers_x_pt(&list).len(), 1);
    assert_eq!(text_count_of(&list, "1.7\u{d7}", TextRoleKind::ZoomLabel), 1);
    assert_eq!(text_count_of(&list, "41MM", TextRoleKind::ZoomFocalLabel), 1);
    // Фокусные объективов на кольце: 14MM у ширика и 24MM у основной.
    assert_eq!(text_count_of(&list, "14MM", TextRoleKind::ZoomFocalLabel), 1);
    assert_eq!(text_count_of(&list, "24MM", TextRoleKind::ZoomFocalLabel), 1);
    // Подписи-числа кольца (упоры и круглые значения): «.6», «1», «2», «3», «5», «10».
    for label in [".6", "1", "2", "3", "5", "10"] {
        assert_eq!(
            text_count_of(&list, label, TextRoleKind::ZoomLabel),
            1,
            "подпись «{label}» на кольце"
        );
    }
    assert!(wheel_disc_visible(&list));
    // Значение состояния, шкала колеса и показание согласованы, эхо-намерения не плодились.
    assert!(near(ratio_of(&rig), 1.7, 1.0e-4));
}

#[test]
fn a_pinch_moves_the_open_wheel_to_the_new_value_without_echoing_intents_back() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    rig.screen
        .handle_action(crate::ScreenAction::ZoomWheelTouched { at: ms(rig.now_ms) });
    rig.screen
        .handle_action(crate::ScreenAction::ZoomWheelReleased { at: ms(rig.now_ms) });
    rig.run(64);
    rig.effects();
    // Значение меняет щипок (программа для колеса): шкала встаёт на него молча, наружу уходит ровно одно намерение.
    rig.screen
        .handle_action(crate::ScreenAction::ZoomPinchBegan { at: ms(rig.now_ms) });
    rig.screen.handle_action(crate::ScreenAction::ZoomPinchChanged {
        scale: 2.5,
        at: ms(rig.now_ms),
    });
    rig.run(64);
    let intents = lens_intents(&rig.effects());
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert!(near(intents[0].ratio.value(), 2.5, 1.0e-4));
    assert!(
        rig.screen.diagnostics().echo_events_suppressed >= 1,
        "эхо шкалы погашено"
    );
}
