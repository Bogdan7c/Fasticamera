//! Тесты фактов камеры: недоступность, блокировка, ожидание подтверждения, прогресс фокуса.

use camera_contract::three_a::{AeAfLock, SensorPoint};

use crate::action::ScreenAction;
use crate::facts::{CameraFacts, CameraPresence, FocusPhase, UnavailableReason};
use crate::focus::LockWaitCheck;
use crate::focus::{FocusIndicator, LockState};
use crate::test_support::{Screen, at_ms, facts_with_point, live_facts};

fn unavailable() -> CameraFacts {
    CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: UnavailableReason::PermissionDenied,
        },
        ..live_facts()
    }
}

#[test]
fn unavailable_camera_hides_indicator_and_plaque_and_is_reported_to_the_screen() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..live_facts()
    }));
    assert!(screen.state.focus().is_lock_plaque_visible());

    screen.step(ScreenAction::CameraFactsChanged(unavailable()));

    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
    assert!(!screen.state.focus().is_lock_plaque_visible());
    assert!(screen.state.is_camera_unavailable());
    assert_eq!(
        screen.state.camera_presence(),
        CameraPresence::Unavailable {
            reason: UnavailableReason::PermissionDenied
        }
    );
}

#[test]
fn unavailable_camera_ignores_lock_and_point_facts() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..unavailable()
    }));
    assert!(!screen.state.focus().is_lock_plaque_visible());
}

#[test]
fn camera_coming_back_starts_with_a_clean_focus_state() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::CameraFactsChanged(unavailable()));
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
    assert!(!screen.state.is_camera_unavailable());
}

#[test]
fn stale_unlocked_fact_does_not_cancel_a_pending_lock_but_a_timeout_does() {
    let mut screen = Screen::ready();
    let point = screen.current_frame_center();
    screen.step(ScreenAction::ViewfinderLongPressed { point, at: at_ms(0) });

    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert!(
        matches!(screen.state.focus().lock(), LockState::Pending { .. }),
        "устаревшее Unlocked"
    );

    screen.step(ScreenAction::Tick(at_ms(1_499)));
    assert!(matches!(screen.state.focus().lock(), LockState::Pending { .. }));
    screen.step(ScreenAction::Tick(at_ms(1_500)));
    assert_eq!(
        screen.state.focus().lock(),
        LockState::Unlocked,
        "камера так и не подтвердила блокировку"
    );
    assert!(!screen.state.focus().is_lock_plaque_visible());
}

#[test]
fn camera_confirmation_before_the_timeout_shows_the_plaque_and_keeps_it_across_ticks() {
    let mut screen = Screen::ready();
    let point = screen.current_frame_center();
    screen.step(ScreenAction::ViewfinderLongPressed { point, at: at_ms(0) });
    // Камера подтверждает блокировку вместе с точкой новой блокировки (центр кадра — сенсор 0,5; 0,5).
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..facts_with_point(SensorPoint::new(0.5, 0.5).expect("центр"), 1)
    }));
    screen.step(ScreenAction::Tick(at_ms(60_000)));
    assert!(screen.state.focus().is_lock_plaque_visible());
    assert!(
        matches!(screen.state.focus().indicator(), FocusIndicator::Shown { .. }),
        "под блокировкой индикатор не гаснет"
    );
}

#[test]
fn focus_phase_follows_the_camera() {
    let mut screen = Screen::ready();
    for phase in [
        FocusPhase::Scanning,
        FocusPhase::Focused,
        FocusPhase::NotFocused,
        FocusPhase::NoResult,
    ] {
        screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
            focus: phase,
            ..live_facts()
        }));
        assert_eq!(screen.state.focus().phase(), phase);
    }
}

#[test]
fn identical_facts_change_nothing() {
    let mut screen = Screen::ready();
    let before = screen.state.clone();
    let repeat = screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert_eq!(screen.state, before);
    assert_eq!(
        repeat.outcome,
        crate::ActionOutcome::NoChange(crate::NoChangeReason::ValueUnchanged)
    );
}

/// Точка сенсора для точки экрана (через тот же перевод, что использует редьюсер).
fn sensor_of(screen: &Screen, point: ui_core::geometry::Point) -> SensorPoint {
    screen
        .state
        .viewfinder_mapping()
        .expect("перевод")
        .screen_to_sensor(point)
        .expect("точка в кадре")
}

#[test]
fn camera_answer_to_an_earlier_tap_does_not_pull_the_square_back() {
    let mut screen = Screen::ready();
    let center = screen.current_frame_center();
    let point_b = ui_core::geometry::Point::new(center.x, center.y + 40.0);
    let sensor_a = sensor_of(&screen, center);
    let sensor_b = sensor_of(&screen, point_b);
    screen.step(ScreenAction::ViewfinderTapped {
        point: center,
        at: at_ms(0),
    });
    screen.step(ScreenAction::ViewfinderTapped {
        point: point_b,
        at: at_ms(50),
    });

    // Камера успела ответить только на первый тап.
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(sensor_a, 1)));
    let FocusIndicator::Shown { at, .. } = screen.state.focus().indicator() else {
        panic!("квадрат должен остаться");
    };
    assert_eq!(at, point_b, "квадрат не прыгает на устаревшую точку A");

    // Ответ на второй тап: квадрат остаётся на B, ожидание снято.
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(sensor_b, 2)));
    let FocusIndicator::Shown { at, .. } = screen.state.focus().indicator() else {
        panic!("квадрат должен остаться");
    };
    assert!(at.distance_to(point_b) < 0.5, "квадрат на B: {at:?}");
    assert!(!screen.state.focus().is_awaiting_camera_point());
}

#[test]
fn a_point_set_by_the_camera_itself_is_accepted_when_the_screen_waits_for_nothing() {
    let mut screen = Screen::ready();
    let sensor = SensorPoint::new(0.5, 0.5).expect("центр");
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(sensor, 7)));
    assert!(matches!(
        screen.state.focus().indicator(),
        FocusIndicator::Shown {
            origin: crate::focus::FocusOrigin::Camera,
            ..
        }
    ));
}

#[test]
fn stale_locked_fact_does_not_confirm_a_repeated_long_press() {
    let mut screen = Screen::ready();
    let center = screen.current_frame_center();
    let point_b = ui_core::geometry::Point::new(center.x, center.y + 40.0);
    let sensor_a = sensor_of(&screen, center);
    let sensor_b = sensor_of(&screen, point_b);
    screen.step(ScreenAction::ViewfinderLongPressed {
        point: center,
        at: at_ms(0),
    });
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..facts_with_point(sensor_a, 1)
    }));
    assert!(screen.state.focus().is_lock_plaque_visible());

    // Повторный долгий тап в другом месте: ждём подтверждения новой блокировки.
    screen.step(ScreenAction::ViewfinderLongPressed {
        point: point_b,
        at: at_ms(100),
    });
    assert!(matches!(screen.state.focus().lock(), LockState::Pending { .. }));

    // Снимок фактов ещё несёт старую блокировку со старой точкой — это не подтверждение.
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..facts_with_point(sensor_a, 1)
    }));
    assert!(matches!(screen.state.focus().lock(), LockState::Pending { .. }));
    assert!(!screen.state.focus().is_lock_plaque_visible());

    // Подтверждение: новая точка вместе с блокировкой.
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        lock: AeAfLock::Locked,
        ..facts_with_point(sensor_b, 2)
    }));
    assert_eq!(screen.state.focus().lock(), LockState::Locked);
}

#[test]
fn lock_wait_check_reports_expiry_and_leaves_other_states_alone() {
    let mut screen = Screen::ready();
    let mut idle = screen.state.focus;
    assert_eq!(idle.expire_lock_wait(at_ms(10_000)), LockWaitCheck::NotExpired);
    assert_eq!(idle.lock(), LockState::Unlocked);

    let point = screen.current_frame_center();
    screen.step(ScreenAction::ViewfinderLongPressed { point, at: at_ms(0) });
    let mut waiting = screen.state.focus;
    assert_eq!(waiting.expire_lock_wait(at_ms(1_499)), LockWaitCheck::NotExpired);
    assert!(matches!(waiting.lock(), LockState::Pending { .. }));
    assert_eq!(waiting.expire_lock_wait(at_ms(1_500)), LockWaitCheck::Expired);
    assert_eq!(waiting.lock(), LockState::Unlocked);
}
