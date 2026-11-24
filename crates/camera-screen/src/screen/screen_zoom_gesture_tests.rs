//! Щипок, перетаскивание ленты и переворот камеры на экране (сессия 13): арена жестов, а не редьюсер.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, ZoomGesture};
use shell_contract::haptics::HapticPattern;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_kit::tokens::TextRoleKind;

use super::test_rig::{FRAME_MS, Rig, near, text_count_of};
use crate::effect::{Effect, ScreenCameraIntent};

fn lens_intents(effects: &[Effect]) -> Vec<LensIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Lens(intent) => Some(*intent),
            _ => None,
        })
        .collect()
}

fn ratio_of(rig: &Rig) -> f32 {
    rig.screen.state().zoom().ratio().value()
}

/// Круг выбранной кнопки ленты Ø 37 pt нарисован (значит, лента на экране).
fn strip_visible(list: &DrawList) -> bool {
    list.leaf_items().any(|item| {
        matches!(item, DrawItem::Shape(shape)
            if matches!(shape.geometry, ui_draw::shape::ShapeGeometry::Circle { diameter }
                if near(diameter, 37.0 * super::test_rig::PX_PER_PT, 1.0)))
    })
}

/// Два пальца на `half_distance_pt` от центра кадра влево и вправо.
fn fingers(center: Point, half_distance_pt: f64) -> (Point, Point) {
    (
        Point::new(center.x - half_distance_pt, center.y),
        Point::new(center.x + half_distance_pt, center.y),
    )
}

/// Разводит два пальца из `from_half` в `to_half` за `duration_ms`, не отпуская.
fn spread(rig: &mut Rig, center: Point, from_half: f64, to_half: f64, duration_ms: i64) {
    let (left, right) = fingers(center, from_half);
    rig.touch_pointer(0, TouchPhase::Began, left, rig.now_ms);
    rig.touch_pointer(1, TouchPhase::Began, right, rig.now_ms);
    rig.run(FRAME_MS);
    let steps = (duration_ms / FRAME_MS).max(1);
    for step in 1..=steps {
        let half = from_half + (to_half - from_half) * step as f64 / steps as f64;
        let (left, right) = fingers(center, half);
        rig.touch_pointer(0, TouchPhase::Moved, left, rig.now_ms);
        rig.touch_pointer(1, TouchPhase::Moved, right, rig.now_ms);
        rig.run(FRAME_MS);
    }
}

fn lift(rig: &mut Rig, center: Point, half: f64) {
    let (left, right) = fingers(center, half);
    rig.touch_pointer(0, TouchPhase::Ended, left, rig.now_ms);
    rig.touch_pointer(1, TouchPhase::Ended, right, rig.now_ms);
    rig.run(FRAME_MS);
}

// ---- щипок ----

#[test]
fn pinch_out_doubles_the_ratio_and_publishes_the_intent_without_touching_focus() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    let center = rig.frame_center();
    spread(&mut rig, center, 50.0, 100.0, 320);
    let mut effects = rig.effects();
    lift(&mut rig, center, 100.0);
    effects.extend(rig.effects());

    assert!(near(ratio_of(&rig), 2.0, 0.05), "{}", ratio_of(&rig));
    let intents = lens_intents(&effects);
    assert!(near(intents.last().expect("намерения щипка").ratio.value(), 2.0, 0.05));
    assert!(intents.iter().all(|intent| intent.facing == LensFacing::Back));
    // Щипок не ставит фокус и не блокирует AE/AF: тап и долгий тап видоискателя провалены двумя пальцами.
    assert!(
        effects.iter().all(|effect| !matches!(effect, Effect::Camera(_))),
        "команд камере нет: {effects:?}"
    );
    assert!(!rig.screen.state().zoom().is_pinching());
    assert_eq!(
        rig.screen.state().focus().indicator,
        crate::focus::FocusIndicator::Hidden
    );
    assert_eq!(rig.screen.diagnostics().gestures.pinches, 1);
}

#[test]
fn pinch_in_zooms_out_to_the_ultra_wide_limit_and_not_further() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    let center = rig.frame_center();
    spread(&mut rig, center, 100.0, 12.0, 400);
    assert!(near(ratio_of(&rig), 0.593, 1.0e-3), "{}", ratio_of(&rig));
}

#[test]
fn pinch_clicks_on_each_catalog_stop_it_crosses_and_not_between() {
    let mut rig = Rig::with_zoom(LensRole::UltraWide, MotionPreference::Full);
    let center = rig.frame_center();
    // От 0,593× до ≈ 2,6×: пересечены упоры 1× и 2×.
    spread(&mut rig, center, 30.0, 130.0, 1200);
    let played: Vec<HapticPattern> = rig
        .effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Haptic(pattern) => Some(pattern),
            _ => None,
        })
        .collect();
    assert!(near(ratio_of(&rig), 2.0, 0.9), "{}", ratio_of(&rig));
    assert_eq!(
        played,
        vec![HapticPattern::RigidImpact; 2],
        "щелчок на 1× и на 2×, тиков нет: {played:?}"
    );
}

#[test]
fn a_second_finger_cancels_a_started_pan_and_the_pinch_wins() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    let center = rig.frame_center();
    // Один палец уже ведёт горизонтальный пан видоискателя (смена режима)…
    rig.touch_pointer(0, TouchPhase::Began, center, rig.now_ms);
    for step in 1..=3 {
        rig.touch_pointer(
            0,
            TouchPhase::Moved,
            Point::new(center.x + 8.0 * f64::from(step), center.y),
            rig.now_ms,
        );
        rig.run(FRAME_MS);
    }
    assert_eq!(rig.screen.diagnostics().gestures.pans, 1, "пан начался");
    assert!(near(ratio_of(&rig), 1.0, 1.0e-6), "один палец зум не трогает");
    // …второй палец ложится и разводит: пан отменён, щипок побеждает.
    let first = Point::new(center.x + 24.0, center.y);
    let second = Point::new(center.x - 60.0, center.y);
    rig.touch_pointer(1, TouchPhase::Began, second, rig.now_ms);
    rig.run(FRAME_MS);
    for step in 1..=12 {
        rig.touch_pointer(
            1,
            TouchPhase::Moved,
            Point::new(second.x - 8.0 * f64::from(step), second.y),
            rig.now_ms,
        );
        rig.run(FRAME_MS);
    }
    assert!(ratio_of(&rig) > 1.3, "щипок победил пан: {}", ratio_of(&rig));
    assert!(rig.screen.diagnostics().gestures.cancelled >= 1, "пан отменён явно");
    let _ = first;
}

#[test]
fn one_finger_pan_across_the_viewfinder_never_zooms() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    let center = rig.frame_center();
    rig.drag(center, Point::new(center.x - 120.0, center.y), 320);
    assert!(near(ratio_of(&rig), 1.0, 1.0e-6));
    assert!(lens_intents(&rig.effects()).is_empty());
}

// ---- перетаскивание ленты без удержания ----

#[test]
fn dragging_the_strip_opens_the_wheel_and_the_same_finger_drives_it() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let from = rig.center_with(|geometry| geometry.zoom_button(1, 1));
    // Быстрое перетаскивание (> 10 pt, без паузы 0,5 с) — путь «пан».
    rig.drag(from, Point::new(from.x - 57.0, from.y), 480);
    assert!(rig.screen.state().zoom().is_wheel_open(), "пан по ленте открыл колесо");
    assert!(ratio_of(&rig) > 1.5, "тот же палец вёл колесо: {}", ratio_of(&rig));
    let intents = lens_intents(&rig.effects());
    assert!(!intents.is_empty());
}

// ---- переворот ----

fn flip_center(rig: &Rig) -> Point {
    rig.center_with(|geometry| geometry.flip_button())
}

#[test]
fn the_flip_button_toggles_the_direction_hides_the_strip_on_front_and_restores_the_back_ratio() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    // Перед переворотом на задней камере стоит 1,4×.
    let center = rig.frame_center();
    spread(&mut rig, center, 60.0, 84.0, 240);
    lift(&mut rig, center, 84.0);
    let before_flip = ratio_of(&rig);
    assert!(near(before_flip, 1.4, 0.06), "{before_flip}");
    rig.effects();
    assert!(strip_visible(rig.run(32)));

    rig.tap(flip_center(&rig));
    let to_front = rig.effects();
    assert_eq!(
        lens_intents(&to_front),
        [LensIntent {
            facing: LensFacing::Front,
            ratio: crate::test_support::ratio(1.0),
            gesture: ZoomGesture::Discrete,
            standby: camera_contract::zoom::StandbyDemand::NotNeeded,
        }]
    );
    assert_eq!(rig.screen.state().zoom().facing(), LensFacing::Front);
    let on_front = rig.run(500).clone();
    assert!(!strip_visible(&on_front), "на фронтальной ленты зума нет (решение 115)");
    assert_eq!(text_count_of(&on_front, ".6", TextRoleKind::ZoomLabel), 0);

    rig.tap(flip_center(&rig));
    let back = lens_intents(&rig.effects());
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].facing, LensFacing::Back);
    assert!(near(back[0].ratio.value(), before_flip, 1.0e-4), "{back:?}");
    assert!(strip_visible(rig.run(500)), "лента вернулась на задней камере");
}

#[test]
fn flip_tap_does_not_leak_into_focus_or_the_shutter() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.tap(flip_center(&rig));
    let effects = rig.effects();
    assert!(effects.iter().all(|effect| !matches!(
        effect,
        Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(_)) | Effect::Capture(_)
    )));
    assert_eq!(rig.screen.diagnostics().gestures.button_presses, 1);
}

// ---- пересоздание экрана ----

#[test]
fn a_reinstalled_screen_keeps_the_users_lens_and_sends_no_intent_at_creation() {
    use crate::config::ScreenConfig;
    use crate::screen::CameraScreen;
    use ui_kit::tokens::UiKitTokens;

    let mut screen =
        CameraScreen::new(ScreenConfig::default(), UiKitTokens::default(), MotionPreference::Full).expect("экран");
    screen.handle_action(crate::ScreenAction::CameraFactsChanged(
        crate::test_support::zoom_facts(LensRole::UltraWide),
    ));
    let request = super::test_rig::request(0, (super::test_rig::WIDTH_PX, super::test_rig::HEIGHT_PX));
    let list = screen.produce_frame(&request).expect("кадр");
    let effects = screen.drain_effects();

    assert!(near(screen.state().zoom().ratio().value(), 0.593, 1.0e-3));
    assert!(
        lens_intents(&effects).is_empty(),
        "при создании намерений нет: {effects:?}"
    );
    assert!(
        effects.iter().any(|effect| matches!(effect, Effect::Viewfinder(_))),
        "рендер получил кадрирование с усыновлённым зумом"
    );
    // Лента сразу показывает выбранный ширик.
    assert!(list.leaf_items().any(|item| matches!(item, DrawItem::Text(_))));
}
