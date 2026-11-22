//! Тесты затвора: таймер, отмена, отказ во время съёмки, итог снимка, клавиша громкости, миниатюра.

use shell_contract::haptics::HapticPattern;

use crate::action::{KeyPhase, ScreenAction};
use crate::effect::Effect;
use crate::facts::{CameraFacts, CameraPresence};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::{AspectRatio, FlashMode, RawCapture, TimerSetting};
use crate::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId, ShutterState, ThumbnailState};
use crate::state::PanelState;
use crate::test_support::{Screen, at_ms, live_facts};
use crate::tools::{ToolId, ToolOption};
use shell_contract::orientation::DeviceOrientation;
use still_contract::plan::PrimaryFormat;

fn select_timer(screen: &mut Screen, timer: TimerSetting) {
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(timer)));
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

fn first_request() -> ShotRequestId {
    ShotRequestId(1)
}

#[test]
fn shutter_without_timer_starts_capturing_and_requests_a_shot_with_current_settings() {
    let mut screen = Screen::ready();
    // Цикл вспышки Off → Auto → On: два тапа.
    screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    screen.step(ScreenAction::StripToolTapped(ToolId::Raw));

    let transition = screen.step(ScreenAction::ShutterTapped { at: at_ms(1_000) });

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Capturing {
            request: first_request()
        }
    );
    let [Effect::Capture(shot), Effect::PlayShutterSound] = transition.effects.as_slice() else {
        panic!("нужны запрос снимка и звук затвора: {:?}", transition.effects)
    };
    assert_eq!(shot.request, first_request());
    assert_eq!(shot.pressed_at, at_ms(1_000), "момент нажатия = метка тапа");
    assert_eq!(shot.flash, FlashMode::On);
    assert_eq!(shot.raw, RawCapture::On);
    assert_eq!(shot.aspect, AspectRatio::FourByThree);
    assert_eq!(shot.primary_format, PrimaryFormat::Heif, "формат из конфига экрана");
    assert_eq!(
        shot.orientation,
        DeviceOrientation::Portrait,
        "датчик молчал: телефон вертикально"
    );
    assert_eq!(
        Some(shot.framing.clone()),
        screen.state.framing_intent().map(|intent| intent.zoom),
        "кадрирование снимка = кадрирование видоискателя"
    );
}

#[test]
fn timer_starts_a_countdown_without_a_shot_and_fires_exactly_at_the_end() {
    let mut screen = Screen::ready();
    select_timer(&mut screen, TimerSetting::Seconds3);

    let press = screen.step(ScreenAction::ShutterTapped { at: at_ms(10_000) });
    assert!(press.effects.is_empty(), "во время отсчёта снимка нет");
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Countdown { ends_at: at_ms(13_000) }
    );

    let early = screen.step(ScreenAction::Tick(at_ms(12_999)));
    assert!(early.effects.is_empty());
    assert!(screen.state.shutter().is_counting_down());

    let due = screen.step(ScreenAction::Tick(at_ms(13_000)));
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Capturing {
            request: first_request()
        }
    );
    assert!(
        matches!(due.effects.as_slice(), [Effect::Capture(shot), Effect::PlayShutterSound] if shot.request == first_request())
    );
}

#[test]
fn every_timer_slot_counts_its_own_configured_seconds() {
    for (setting, seconds) in [
        (TimerSetting::Seconds3, 3),
        (TimerSetting::Seconds5, 5),
        (TimerSetting::Seconds10, 10),
    ] {
        let mut screen = Screen::ready();
        select_timer(&mut screen, setting);
        screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
        assert_eq!(
            screen.state.shutter(),
            ShutterState::Countdown {
                ends_at: at_ms(seconds * 1_000)
            }
        );
    }
}

#[test]
fn shutter_during_countdown_cancels_it_and_no_shot_follows() {
    let mut screen = Screen::ready();
    select_timer(&mut screen, TimerSetting::Seconds3);
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });

    let cancel = screen.step(ScreenAction::ShutterTapped { at: at_ms(1_000) });
    assert_eq!(cancel.outcome, ActionOutcome::Applied);
    assert!(cancel.effects.is_empty());
    assert_eq!(screen.state.shutter(), ShutterState::Idle);

    let later = screen.step(ScreenAction::Tick(at_ms(60_000)));
    assert!(later.effects.is_empty(), "после отмены тик не должен снимать");
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
}

#[test]
fn settings_chosen_during_countdown_apply_to_the_shot() {
    let mut screen = Screen::ready();
    select_timer(&mut screen, TimerSetting::Seconds3);
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    screen.step(ScreenAction::StripToolTapped(ToolId::Raw));

    let due = screen.step(ScreenAction::Tick(at_ms(3_000)));

    assert!(
        matches!(due.effects.as_slice(), [Effect::Capture(shot), Effect::PlayShutterSound] if shot.raw == RawCapture::On)
    );
}

#[test]
fn shutter_during_capture_is_an_explicit_counted_refusal_without_a_second_request() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    let before = screen.state.clone();

    let second = screen.step(ScreenAction::ShutterTapped { at: at_ms(5) });

    assert_eq!(second.outcome, ActionOutcome::Rejected(Rejection::ShutterBusy));
    assert!(second.effects.is_empty());
    assert_eq!(screen.state.shutter(), before.shutter());
    assert_eq!(screen.state.rejections().get(Rejection::ShutterBusy), 1);
    // Отказ не тронул ничего, кроме счётчика и метки времени (время идёт и при отказе).
    let mut expected = before;
    expected.rejections = screen.state.rejections().to_owned();
    expected.last_time = at_ms(5);
    assert_eq!(screen.state, expected);
}

#[test]
fn future_shutter_states_refuse_new_shots_too() {
    for busy in [
        ShutterState::CapturingLong {
            request: first_request(),
            progress_percent: 40,
            cancel: crate::shutter::LongCaptureCancel::Allowed,
        },
        ShutterState::Burst {
            request: first_request(),
            shots_taken: 3,
        },
        ShutterState::Recording { since: at_ms(0) },
        ShutterState::RecordingLocked { since: at_ms(0) },
    ] {
        let mut screen = Screen::ready();
        screen.state.shutter = busy;
        let transition = screen.step(ScreenAction::ShutterTapped { at: at_ms(1) });
        assert_eq!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::ShutterBusy),
            "{busy:?}"
        );
        assert_eq!(screen.state.shutter(), busy);
    }
}

#[test]
fn saved_shot_returns_to_idle_and_fills_the_thumbnail() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });

    let done = screen.step(ScreenAction::CaptureFinished {
        request: first_request(),
        outcome: CaptureOutcome::Saved,
    });

    assert_eq!(done.outcome, ActionOutcome::Applied);
    assert!(done.effects.is_empty());
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: first_request(),
            picture: None
        }
    );
    // Следующий снимок получает новый номер и снова доходит до запроса.
    let next = screen.step(ScreenAction::ShutterTapped { at: at_ms(100) });
    assert!(
        matches!(next.effects.as_slice(), [Effect::Capture(shot), Effect::PlayShutterSound] if shot.request == ShotRequestId(2))
    );
}

#[test]
fn failed_shot_returns_to_idle_with_error_haptic_and_keeps_the_old_thumbnail() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    screen.step(ScreenAction::CaptureFinished {
        request: first_request(),
        outcome: CaptureOutcome::Saved,
    });
    screen.step(ScreenAction::ShutterTapped { at: at_ms(10) });

    let failed = screen.step(ScreenAction::CaptureFinished {
        request: ShotRequestId(2),
        outcome: CaptureOutcome::Failed(CaptureFailureReason::Storage),
    });

    assert_eq!(failed.effects, [Effect::Haptic(HapticPattern::Error)]);
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: first_request(),
            picture: None
        }
    );
}

#[test]
fn result_for_an_unknown_request_is_refused_and_does_not_end_the_current_shot() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });

    let stale = screen.step(ScreenAction::CaptureFinished {
        request: ShotRequestId(99),
        outcome: CaptureOutcome::Saved,
    });

    assert_eq!(stale.outcome, ActionOutcome::Rejected(Rejection::StaleCaptureResult));
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Capturing {
            request: first_request()
        }
    );
    assert_eq!(screen.state.thumbnail(), ThumbnailState::Empty);
    // Итог без съёмки тоже отвергается.
    let mut idle = Screen::ready();
    let none = idle.step(ScreenAction::CaptureFinished {
        request: first_request(),
        outcome: CaptureOutcome::Saved,
    });
    assert_eq!(none.outcome, ActionOutcome::Rejected(Rejection::StaleCaptureResult));
}

#[test]
fn on_screen_shutter_is_refused_while_the_menu_is_open_but_volume_key_shoots() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);

    let button = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert_eq!(button.outcome, ActionOutcome::Rejected(Rejection::ShutterHiddenByMenu));
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(
        screen.state.shutter_visibility(),
        crate::ShutterVisibility::HiddenUnderDrawer
    );

    let key = screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(1),
    });
    assert_eq!(key.outcome, ActionOutcome::Applied);
    assert!(matches!(
        key.effects.as_slice(),
        [Effect::Capture(_), Effect::PlayShutterSound]
    ));
    assert_eq!(
        screen.state.panel(),
        PanelState::ControlsMenu,
        "меню клавиша не закрывает"
    );
}

#[test]
fn volume_key_release_and_autorepeat_do_nothing() {
    let mut screen = Screen::ready();
    for phase in [KeyPhase::Released, KeyPhase::Repeated] {
        let before = screen.state.clone();
        let transition = screen.step(ScreenAction::VolumeKey { phase, at: at_ms(0) });
        assert_eq!(
            transition.outcome,
            ActionOutcome::NoChange(NoChangeReason::KeyHoldNotSupported)
        );
        assert!(transition.effects.is_empty());
        assert_eq!(screen.state.shutter(), before.shutter());
    }
}

#[test]
fn volume_key_cancels_a_countdown_like_the_button() {
    let mut screen = Screen::ready();
    select_timer(&mut screen, TimerSetting::Seconds5);
    screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(0),
    });
    assert!(screen.state.shutter().is_counting_down());
    screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(100),
    });
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
}

#[test]
fn shutter_without_a_live_camera_is_refused() {
    let mut screen = Screen::unprepared(crate::ScreenConfig::default());
    let starting = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert_eq!(starting.outcome, ActionOutcome::Rejected(Rejection::CameraNotLive));
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
}

#[test]
fn camera_loss_during_countdown_cancels_the_shot() {
    let mut screen = Screen::ready();
    select_timer(&mut screen, TimerSetting::Seconds3);
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });

    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: crate::UnavailableReason::InUse,
        },
        ..live_facts()
    }));
    let tick = screen.step(ScreenAction::Tick(at_ms(10_000)));

    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert!(tick.effects.is_empty(), "снимок у пропавшей камеры не запрашивается");
}

#[test]
fn thumbnail_tap_opens_the_system_gallery() {
    let mut screen = Screen::ready();
    let transition = screen.step(ScreenAction::ThumbnailTapped);
    assert_eq!(transition.effects, [Effect::OpenSystemGallery]);
    assert_eq!(screen.state.thumbnail(), ThumbnailState::Empty);
}

#[test]
fn full_flow_tap_shutter_to_saved_thumbnail_reaches_every_stage() {
    // Функциональный путь: кадр сфокусирован тапом, снимок запрошен, сервис ответил, миниатюра есть.
    let mut screen = Screen::ready();
    let focus = screen.tap_center(at_ms(0));
    assert!(matches!(focus.effects.as_slice(), [Effect::Camera(_)]));
    let shot = screen.step(ScreenAction::ShutterTapped { at: at_ms(500) });
    let Effect::Capture(request) = shot.effects[0].clone() else {
        panic!("нужен запрос снимка")
    };
    screen.step(ScreenAction::CaptureFinished {
        request: request.request,
        outcome: CaptureOutcome::Saved,
    });
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: request.request,
            picture: None
        }
    );
    assert_eq!(screen.state.rejections().total(), 0);
}

fn unavailable_camera() -> CameraFacts {
    CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: crate::facts::UnavailableReason::Failure,
        },
        ..live_facts()
    }
}

#[test]
fn camera_loss_during_capture_frees_the_shutter_counts_the_loss_and_signals_the_error() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert!(matches!(screen.state.shutter(), ShutterState::Capturing { .. }));

    let lost = screen.step(ScreenAction::CameraFactsChanged(unavailable_camera()));

    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.lost_captures(), 1);
    assert_eq!(lost.effects, [Effect::Haptic(HapticPattern::Error)]);
    // Поздний итог уже не воскрешает снимок.
    let late = screen.step(ScreenAction::CaptureFinished {
        request: first_request(),
        outcome: CaptureOutcome::Saved,
    });
    assert_eq!(late.outcome, ActionOutcome::Rejected(Rejection::StaleCaptureResult));
    assert_eq!(screen.state.thumbnail(), ThumbnailState::Empty);
    // Камера вернулась — затвор снова снимает.
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    let again = screen.step(ScreenAction::ShutterTapped { at: at_ms(5_000) });
    assert!(matches!(
        again.effects.as_slice(),
        [Effect::Capture(_), Effect::PlayShutterSound]
    ));
}

#[test]
fn camera_loss_without_a_capture_in_flight_is_not_counted_as_a_lost_shot() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(unavailable_camera()));
    assert_eq!(screen.state.lost_captures(), 0);
}

#[test]
fn missing_capture_result_frees_the_shutter_exactly_at_the_configured_timeout() {
    let mut screen = Screen::ready();
    let timeout_ms = i64::from(screen.config.capture.capture_timeout_ms);
    screen.step(ScreenAction::ShutterTapped { at: at_ms(1_000) });

    screen.step(ScreenAction::Tick(at_ms(1_000 + timeout_ms - 1)));
    assert!(matches!(screen.state.shutter(), ShutterState::Capturing { .. }));
    assert_eq!(screen.state.lost_captures(), 0);

    let overdue = screen.step(ScreenAction::Tick(at_ms(1_000 + timeout_ms)));
    assert_eq!(overdue.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.lost_captures(), 1);
    assert_eq!(overdue.effects, [Effect::Haptic(HapticPattern::Error)]);
}

#[test]
fn a_capture_finished_in_time_is_not_counted_as_lost_by_later_ticks() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    screen.step(ScreenAction::CaptureFinished {
        request: first_request(),
        outcome: CaptureOutcome::Saved,
    });
    screen.step(ScreenAction::Tick(at_ms(10 * 60_000)));
    assert_eq!(screen.state.lost_captures(), 0);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: first_request(),
            picture: None
        }
    );
}

#[test]
fn the_timeout_of_a_timer_shot_counts_from_the_moment_the_shot_was_requested() {
    let mut screen = Screen::ready();
    select_timer(&mut screen, TimerSetting::Seconds3);
    let timeout_ms = i64::from(screen.config.capture.capture_timeout_ms);
    screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    screen.step(ScreenAction::Tick(at_ms(3_000)));
    assert!(matches!(screen.state.shutter(), ShutterState::Capturing { .. }));
    // Отсчёт самого ожидания идёт от 3 с, а не от нажатия.
    screen.step(ScreenAction::Tick(at_ms(timeout_ms)));
    assert!(matches!(screen.state.shutter(), ShutterState::Capturing { .. }));
    screen.step(ScreenAction::Tick(at_ms(3_000 + timeout_ms)));
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.lost_captures(), 1);
}
