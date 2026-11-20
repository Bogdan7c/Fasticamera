//! Сквозные тесты фасада: синтетические касания в пикселях → жесты → действия → эффекты намерений.

use camera_contract::control::ExposureBias;
use shell_contract::haptics::HapticPattern;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_kit::tokens::TextRoleKind;

use super::ScreenTouch;
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::settings::AspectRatio;
use crate::shutter::ShutterState;
use crate::state::{PanelState, ShutterVisibility};

fn focus_target(effects: &[Effect]) -> Option<camera_contract::three_a::SensorPoint> {
    effects.iter().find_map(|effect| match effect {
        Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(point)) => Some(*point),
        _ => None,
    })
}

#[test]
fn tap_in_the_frame_center_focuses_the_sensor_center() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    let target = focus_target(&rig.effects()).expect("тап по кадру даёт фокус");
    assert!(
        (target.x() - 0.5).abs() < 0.01 && (target.y() - 0.5).abs() < 0.01,
        "{target:?}"
    );
}

#[test]
fn tap_in_the_frame_corner_focuses_the_cropped_sensor_corner() {
    let mut rig = Rig::new();
    let frame = rig
        .screen
        .state()
        .layout()
        .expect("раскладка")
        .frame_rect(AspectRatio::FourByThree);
    rig.tap(Point::new(frame.min_x() + 0.5, frame.min_y() + 0.5));
    let target = focus_target(&rig.effects()).expect("фокус");
    // Датчик повёрнут на 90°: левый верхний угол экрана — левый нижний угол сенсора.
    assert!(target.x() < 0.02 && target.y() > 0.98, "{target:?}");
}

#[test]
fn long_press_locks_focus_with_a_medium_impact() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, center, start);
    // Удержание 0,5 с по меткам кадров: долгий тап срабатывает по таймеру, палец неподвижен.
    rig.now_ms = start + 520;
    rig.frame();
    let effects = rig.effects();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Camera(ScreenCameraIntent::LockFocusAndExposureAt(_)))),
        "{effects:?}"
    );
    assert!(
        effects.contains(&Effect::Haptic(HapticPattern::MediumImpact)),
        "{effects:?}"
    );
    assert!(rig.screen.diagnostics().gestures.long_presses == 1);
}

#[test]
fn tapping_the_shutter_requests_a_capture_and_blinks_the_frame() {
    let mut rig = Rig::new();
    let shutter = rig.shutter_center();
    rig.tap(shutter);
    let effects = rig.effects();
    assert!(effects.iter().any(|e| matches!(e, Effect::Capture(_))), "{effects:?}");
    assert!(matches!(rig.screen.state().shutter(), ShutterState::Capturing { .. }));
    // Затемнение кадра видно в списке: чёрный прямоугольник с ненулевой прозрачностью поверх кадра.
    assert!(rig.last.items.len() > 3);
}

#[test]
fn swipe_up_on_the_viewfinder_opens_the_menu_and_the_shutter_leaves_the_list() {
    let mut rig = Rig::new();
    let shutter_members_before = glass_members(rig.frame())
        .iter()
        .filter(|(geometry, ..)| matches!(geometry, ui_draw::shape::ShapeGeometry::Circle { diameter } if (*diameter - 80.1 * PX_PER_PT).abs() < 2.0))
        .count();
    assert_eq!(shutter_members_before, 1, "затвор есть в списке при закрытом меню");
    let center = rig.frame_center();
    rig.drag(center, Point::new(center.x, center.y - 120.0), 160);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    assert_eq!(
        rig.screen.state().shutter_visibility(),
        ShutterVisibility::HiddenUnderDrawer
    );
    // Шторка раскрылась, затвор растворился под ней и из списка ушёл.
    rig.run(1000);
    let shutter_members_after = glass_members(&rig.last)
        .iter()
        .filter(|(geometry, ..)| matches!(geometry, ui_draw::shape::ShapeGeometry::Circle { diameter } if (*diameter - 80.1 * PX_PER_PT).abs() < 2.0))
        .count();
    assert_eq!(shutter_members_after, 0, "затвор под шторкой не рисуется");
    // Нажатие по месту затвора не даёт снимка.
    let shutter = rig.shutter_center();
    rig.tap(shutter);
    assert!(!rig.effects().iter().any(|e| matches!(e, Effect::Capture(_))));
}

#[test]
fn swipe_down_on_the_open_menu_closes_it() {
    let mut rig = Rig::new();
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(600);
    let sheet_center = rig.center_with(|g| g.sheet());
    rig.drag(
        Point::new(sheet_center.x, sheet_center.y + 120.0),
        Point::new(sheet_center.x, sheet_center.y + 190.0),
        160,
    );
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
}

#[test]
fn thumbnail_tap_opens_the_system_gallery() {
    let mut rig = Rig::new();
    let thumbnail = rig.center_with(|g| g.thumbnail());
    rig.tap(thumbnail);
    assert!(rig.effects().contains(&Effect::OpenSystemGallery));
}

#[test]
fn vertical_pan_on_the_sun_sets_exposure_in_thirds_with_tick_haptics() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.effects();
    rig.run(300);
    // «Солнце» правее рамки фокуса и чуть выше её центра (токены K9).
    let tokens = ui_kit::tokens::UiKitTokens::default();
    let sun = Point::new(
        center.x + tokens.focus.frame_width_pt * 0.5 + tokens.focus.sun_offset_right_of_frame_pt,
        center.y - tokens.focus.sun_offset_above_center_pt,
    );
    rig.drag(sun, Point::new(sun.x, sun.y - 60.0), 320);
    let effects = rig.effects();
    let biases: Vec<f32> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Camera(ScreenCameraIntent::SetExposureBias(bias)) => Some(bias.ev()),
            _ => None,
        })
        .collect();
    assert!(!biases.is_empty(), "пан «солнца» двигает EV: {effects:?}");
    let steps = rig.screen.state().exposure().steps();
    assert!(steps > 0, "палец вверх увеличивает EV, шагов {steps}");
    for ev in &biases {
        let thirds = f64::from(*ev) * 3.0;
        assert!((thirds - thirds.round()).abs() < 1e-3, "EV {ev} не кратен 1/3");
    }
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Haptic(_))),
        "тики шкалы играют хаптику: {effects:?}"
    );
    // Фокус при этом не переставлялся.
    assert!(focus_target(&effects).is_none());
    let _ = ExposureBias::NONE;
}

#[test]
fn horizontal_swipe_with_one_mode_changes_nothing() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.drag(center, Point::new(center.x - 150.0, center.y), 160);
    rig.run(1200);
    assert_eq!(rig.screen.state().mode(), crate::modes::CaptureMode::Photo);
    let effects = rig.effects();
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Camera(_) | Effect::Capture(_))),
        "{effects:?}"
    );
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
}

#[test]
fn touch_gap_cancels_a_running_gesture_without_actions() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, center, start);
    rig.touch(TouchPhase::Moved, Point::new(center.x, center.y - 40.0), start + 16);
    rig.screen.handle_touch(ScreenTouch::Gap);
    rig.now_ms = start + 40;
    rig.frame();
    assert_eq!(
        rig.screen.state().panel(),
        PanelState::Closed,
        "отменённый жест меню не открывает"
    );
    assert!(rig.effects().is_empty());
    assert_eq!(rig.screen.diagnostics().touch_gaps, 1);
    assert!(rig.screen.diagnostics().gestures.cancelled >= 1);
}

#[test]
fn touches_before_the_first_frame_are_counted_not_lost_silently() {
    let mut screen = super::CameraScreen::new(
        crate::ScreenConfig::default(),
        ui_kit::tokens::UiKitTokens::default(),
        ui_core::time::MotionPreference::Full,
    )
    .expect("экран");
    screen.handle_touch(ScreenTouch::Gap);
    assert_eq!(screen.diagnostics().touches_dropped_before_first_frame, 1);
}

#[test]
fn strip_flash_icon_tap_toggles_flash_and_changes_the_icon() {
    use ui_draw::icons::IconId;
    let mut rig = Rig::new();
    assert_eq!(icon_transforms(rig.frame(), IconId::FlashOff).len(), 1);
    let flash = rig.center_with(|g| g.strip_icon(0, 3));
    rig.tap(flash);
    assert_eq!(rig.screen.state().settings().flash, crate::settings::FlashMode::On);
    rig.run(100);
    assert_eq!(icon_transforms(&rig.last, IconId::FlashOn).len(), 1);
    assert_eq!(icon_transforms(&rig.last, IconId::FlashOff).len(), 0);
}

#[test]
fn focus_indicator_disappears_when_the_state_hides_it_and_lock_plaque_follows_the_camera() {
    use camera_contract::three_a::AeAfLock;
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(300);
    assert_eq!(sun_discs(&rig.last).len(), 1, "рамка и «солнце» показаны");
    // Срок индикатора — у `ScreenState` (`indicator_visible_ms`): после него рамка уходит.
    rig.run(4_000);
    assert_eq!(
        rig.screen.state().focus().indicator(),
        crate::focus::FocusIndicator::Hidden
    );
    rig.run(600);
    assert_eq!(sun_discs(&rig.last).len(), 0, "«солнце» ушло вместе с рамкой");
    assert_eq!(
        text_count_of(&rig.last, "0.0", TextRoleKind::StripValue),
        1,
        "остался только текст EV над кадром"
    );
    // Плашка LOCK: появляется по факту камеры и уходит, когда камера сняла блокировку.
    let plain = text_count(&rig.last);
    let mut locked = crate::test_support::live_facts();
    locked.lock = AeAfLock::Locked;
    rig.screen.handle_action(ScreenAction::CameraFactsChanged(locked));
    rig.screen.handle_action(ScreenAction::ViewfinderLongPressed {
        point: center,
        at: ms(rig.now_ms),
    });
    // Подтверждение блокировки приходит вместе с точкой новой блокировки (старое `Locked` не в счёт).
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(crate::CameraFacts {
            lock: AeAfLock::Locked,
            ..crate::test_support::facts_with_point(
                camera_contract::three_a::SensorPoint::new(0.5, 0.5).expect("центр"),
                1,
            )
        }));
    rig.run(800);
    assert!(
        rig.screen.state().focus().is_lock_plaque_visible(),
        "камера подтвердила блокировку"
    );
    assert!(text_count(&rig.last) > plain);
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(crate::test_support::live_facts()));
    rig.run(800);
    assert!(!rig.screen.state().focus().is_lock_plaque_visible());
    assert_eq!(text_count(&rig.last), plain, "плашка ушла вместе с блокировкой");
}
