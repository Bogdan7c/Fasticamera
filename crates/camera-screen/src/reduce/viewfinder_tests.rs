//! Тесты видоискателя: тап → индикатор + намерение камере, долгий тап → блокировка, закрытие панелей.

use camera_contract::three_a::{AeAfLock, SensorPoint};
use shell_contract::haptics::HapticPattern;
use ui_core::geometry::Point;

use crate::action::ScreenAction;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::CameraFacts;
use crate::focus::{FocusIndicator, FocusOrigin, LockState};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::AspectRatio;
use crate::state::PanelState;
use crate::test_support::{OUR_SURFACE, Screen, at_ms, facts_with_point, live_facts};
use crate::{ScreenConfig, ScreenState};

fn center_sensor_point() -> SensorPoint {
    SensorPoint::new(0.5, 0.5).expect("центр")
}

#[test]
fn tap_puts_indicator_at_the_tap_and_sends_focus_to_the_translated_point() {
    let mut screen = Screen::ready();
    let tap = screen.current_frame_center();

    let transition = screen.step(ScreenAction::ViewfinderTapped {
        point: tap,
        at: at_ms(100),
    });

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(
        transition.effects,
        [Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(
            center_sensor_point()
        ))]
    );
    assert_eq!(
        screen.state.focus().indicator(),
        FocusIndicator::Shown {
            at: tap,
            origin: FocusOrigin::Tap,
            since: at_ms(100)
        }
    );
}

#[test]
fn tap_translates_a_corner_of_the_frame_through_the_sensor_rotation() {
    // Датчик 4096×3072, ориентация 90°: левый верхний угол кадра на экране — левый нижний угол сенсора.
    let mut screen = Screen::ready();
    let frame = screen
        .state
        .layout()
        .expect("раскладка")
        .frame_rect(AspectRatio::FourByThree);
    let near_corner = Point::new(frame.min_x() + 0.001, frame.min_y() + 0.001);

    let transition = screen.step(ScreenAction::ViewfinderTapped {
        point: near_corner,
        at: at_ms(0),
    });

    let Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(sensor)) = transition.effects[0] else {
        panic!("ожидалось FocusAndExposeAt, получено {:?}", transition.effects);
    };
    assert!(sensor.x() < 0.001 && sensor.y() > 0.999, "{sensor:?}");
}

#[test]
fn tap_with_menu_open_only_closes_it_and_does_not_touch_focus() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);

    let transition = screen.tap_center(at_ms(10));

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.panel(), PanelState::Closed);
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn tap_with_exposure_panel_or_submenu_open_also_only_closes_it() {
    for open in [
        ScreenAction::StripToolTapped(crate::tools::ToolId::Exposure),
        ScreenAction::ControlsMenuOpenRequested,
    ] {
        let mut screen = Screen::ready();
        screen.step(open);
        if screen.state.panel() == PanelState::ControlsMenu {
            screen.step(ScreenAction::ControlsItemSelected(crate::tools::ToolId::Aspect));
            assert!(matches!(screen.state.panel(), PanelState::Submenu(_)));
        }
        let transition = screen.tap_center(at_ms(1));
        assert!(transition.effects.is_empty());
        assert_eq!(screen.state.panel(), PanelState::Closed);
        assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
    }
}

#[test]
fn tap_outside_the_frame_does_nothing() {
    let mut screen = Screen::ready();
    let before = screen.state.clone();

    // Верхняя чёрная полоса над кадром (y = 20 pt).
    let transition = screen.step(ScreenAction::ViewfinderTapped {
        point: Point::new(220.0, 20.0),
        at: at_ms(5),
    });

    assert_eq!(
        transition.outcome,
        ActionOutcome::NoChange(NoChangeReason::TapOutsideFrame)
    );
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.focus(), before.focus());
    assert_eq!(screen.state.rejections().total(), 0);
}

#[test]
fn tap_without_a_live_camera_is_rejected_and_counted() {
    let mut screen = Screen::unprepared(ScreenConfig::default());
    screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
    // Камера ещё стартует.
    let transition = screen.tap_center(at_ms(0));

    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::CameraNotLive));
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
    assert_eq!(screen.state.rejections().get(Rejection::CameraNotLive), 1);
}

#[test]
fn tap_without_stream_geometry_or_surface_is_rejected_with_the_reason() {
    let mut screen = Screen::unprepared(ScreenConfig::default());
    screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        stream: None,
        ..live_facts()
    }));
    let transition = screen.tap_center(at_ms(0));
    assert_eq!(
        transition.outcome,
        ActionOutcome::Rejected(Rejection::StreamGeometryUnknown)
    );

    let mut no_surface = Screen::unprepared(ScreenConfig::default());
    no_surface.step(ScreenAction::CameraFactsChanged(live_facts()));
    let transition = no_surface.step(ScreenAction::ViewfinderTapped {
        point: Point::new(220.0, 400.0),
        at: at_ms(0),
    });
    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::InvalidSurface));
}

#[test]
fn long_press_sends_lock_with_haptic_and_waits_for_camera_before_showing_the_plaque() {
    let mut screen = Screen::ready();
    let point = screen.current_frame_center();

    let transition = screen.step(ScreenAction::ViewfinderLongPressed { point, at: at_ms(500) });

    assert_eq!(
        transition.effects,
        [
            Effect::Camera(ScreenCameraIntent::LockFocusAndExposureAt(center_sensor_point())),
            Effect::Haptic(HapticPattern::MediumImpact),
        ]
    );
    assert!(matches!(screen.state.focus().lock(), LockState::Pending { .. }));
    assert!(
        !screen.state.focus().is_lock_plaque_visible(),
        "плашка появляется по факту камеры, не раньше"
    );

    // Камера подтверждает блокировку вместе с точкой новой блокировки.
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..facts_with_point(center_sensor_point(), 1)
    }));
    assert!(screen.state.focus().is_lock_plaque_visible());

    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Unlocked,
        ..live_facts()
    }));
    assert!(!screen.state.focus().is_lock_plaque_visible());
}

#[test]
fn long_press_with_a_panel_open_changes_nothing() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let point = screen.current_frame_center();
    let transition = screen.step(ScreenAction::ViewfinderLongPressed { point, at: at_ms(1) });
    assert_eq!(transition.outcome, ActionOutcome::NoChange(NoChangeReason::PanelOpen));
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.focus().lock(), LockState::Unlocked);
}

#[test]
fn tap_while_locked_sends_only_focus_and_leaves_unlocking_to_the_camera() {
    // `CameraCommand::FocusAndExposeAt` сам снимает блокировку, отдельный Reset не нужен;
    // плашка исчезает, когда камера подтвердит Unlocked.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..live_facts()
    }));
    assert!(screen.state.focus().is_lock_plaque_visible());

    let transition = screen.tap_center(at_ms(10));

    assert_eq!(transition.effects.len(), 1);
    assert!(matches!(
        transition.effects[0],
        Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(_))
    ));
    assert!(
        screen.state.focus().is_lock_plaque_visible(),
        "до подтверждения камеры плашка остаётся"
    );

    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert!(!screen.state.focus().is_lock_plaque_visible());
}

#[test]
fn tap_does_not_touch_capture_settings_exposure_or_shutter() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(crate::tools::ToolId::Flash));
    screen.step(ScreenAction::StripToolTapped(crate::tools::ToolId::Raw));
    let before: ScreenState = screen.state.clone();

    screen.tap_center(at_ms(7));

    assert_eq!(screen.state.settings(), before.settings());
    assert_eq!(screen.state.exposure(), before.exposure());
    assert_eq!(screen.state.shutter(), before.shutter());
    assert_eq!(screen.state.panel(), before.panel());
    assert_eq!(screen.state.thumbnail(), before.thumbnail());
}

#[test]
fn camera_point_moves_the_indicator_to_where_the_camera_actually_looks() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(100));
    let mapping = screen.state.viewfinder_mapping().expect("перевод");

    // Камера подтверждает тап, но чуть сдвинула точку: квадрат ставится по точке камеры.
    let camera_point = SensorPoint::new(0.5, 0.75).expect("точка");
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(camera_point, 1)));

    let expected = mapping.sensor_to_screen(camera_point).expect("экран");
    let FocusIndicator::Shown { at, origin, since } = screen.state.focus().indicator() else {
        panic!("индикатор должен остаться");
    };
    assert!(at.distance_to(expected) < 1e-9);
    assert_eq!(origin, FocusOrigin::Tap, "происхождение не меняется");
    assert_eq!(since, at_ms(100), "время показа не сбрасывается");
}

#[test]
fn camera_initiated_point_shows_an_indicator_without_any_tap() {
    // Задел под фокус-трекинг (решение 69): рамка движется без касания.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::Tick(at_ms(2_000)));
    let camera_point = SensorPoint::new(0.5, 0.5).expect("точка");

    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(camera_point, 9)));

    let FocusIndicator::Shown { at, origin, since } = screen.state.focus().indicator() else {
        panic!("индикатор должен появиться");
    };
    assert_eq!(origin, FocusOrigin::Camera);
    assert_eq!(since, at_ms(2_000));
    assert!(at.distance_to(screen.current_frame_center()) < 1e-6);

    // Та же точка (тот же номер запроса) индикатор не двигает и не воскрешает после затухания.
    screen.step(ScreenAction::Tick(at_ms(10_000)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(camera_point, 9)));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn stale_whole_frame_fact_does_not_erase_a_fresh_tap_but_a_later_one_does() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));

    // Камера ещё не успела обработать тап и присылает старое «весь кадр».
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert!(matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }));

    // Камера подтвердила тап точкой — ожидание закончилось.
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(
        center_sensor_point(),
        1,
    )));
    // Теперь «весь кадр» — это сброс на стороне камеры (например, смена камеры): квадрат убирается.
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}

#[test]
fn a_tap_made_while_the_camera_releases_the_old_point_is_not_erased_by_the_release() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(
        center_sensor_point(),
        1,
    )));
    // Движок отпустил старую точку (смена сцены) — но человек успел тапнуть в другое место: тап ещё ждёт камеру.
    let other_tap = Point::new(screen.current_frame_center().x + 20.0, screen.current_frame_center().y);
    screen.step(ScreenAction::ViewfinderTapped {
        point: other_tap,
        at: at_ms(500),
    });

    screen.step(ScreenAction::CameraFactsChanged(live_facts()));

    assert!(
        matches!(screen.state.focus().indicator(), FocusIndicator::Shown { at, .. } if at == other_tap),
        "свежий тап погашен устаревшим «весь кадр»: {:?}",
        screen.state.focus().indicator()
    );
    assert!(screen.state.focus().is_awaiting_camera_point());
}

#[test]
fn the_indicator_hides_when_the_camera_releases_a_confirmed_point() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(
        center_sensor_point(),
        1,
    )));
    assert!(matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }));

    // Движок отпустил точку: факт «весь кадр» без нового тапа гасит рамку сразу, не дожидаясь 3 с.
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));

    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
}
