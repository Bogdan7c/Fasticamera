//! Тесты двух фаз снимка (решение 140): «кадр захвачен» освобождает затвор, сохранение идёт отдельно;
//! набор сохраняемых ограничен, сроки двух фаз раздельные, миниатюра — по порядку сохранения.

use shell_contract::haptics::HapticPattern;
use shell_contract::orientation::DeviceOrientation;

use crate::action::{KeyPhase, ScreenAction};
use crate::effect::Effect;
use crate::facts::CameraFacts;
use crate::outcome::{ActionOutcome, Rejection};
use crate::settings::{AspectRatio, TimerSetting};
use crate::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId, ShutterState, ThumbnailState};
use crate::test_support::{Screen, at_ms, live_facts};
use crate::tools::{ToolId, ToolOption};

fn id(number: u64) -> ShotRequestId {
    ShotRequestId(number)
}

/// Нажимает затвор и возвращает запрос снимка из эффектов.
fn press(screen: &mut Screen, at: i64) -> crate::shutter::ShotRequest {
    let transition = screen.step(ScreenAction::ShutterTapped { at: at_ms(at) });
    match transition.effects.first() {
        Some(Effect::Capture(shot)) => shot.clone(),
        other => panic!("нажатие должно дать снимок: {other:?} / {:?}", transition.outcome),
    }
}

fn secure(screen: &mut Screen, request: ShotRequestId) -> ActionOutcome {
    screen.step(ScreenAction::CaptureSecured { request }).outcome
}

fn finish(screen: &mut Screen, request: ShotRequestId, outcome: CaptureOutcome) -> crate::reduce::ScreenTransition {
    screen.step(ScreenAction::CaptureFinished { request, outcome })
}

#[test]
fn secured_frame_frees_the_shutter_and_the_next_shot_starts_while_the_first_is_saving() {
    let mut screen = Screen::ready();
    let first = press(&mut screen, 0);
    assert!(matches!(screen.state.shutter(), ShutterState::Capturing { .. }));

    assert_eq!(secure(&mut screen, first.request), ActionOutcome::Applied);
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.saving_shots(), 1);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Empty,
        "сохранения ещё не было"
    );

    // Второй снимок — пока первый не сохранён.
    let second = press(&mut screen, 100);
    assert_eq!(second.request, id(2));
    assert_eq!(second.pressed_at, at_ms(100));
    assert_eq!(screen.state.shutter(), ShutterState::Capturing { request: id(2) });

    // Итог первого приходит поздно, но принимается и не трогает затвор второго.
    let saved = finish(&mut screen, first.request, CaptureOutcome::Saved);
    assert_eq!(saved.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.shutter(), ShutterState::Capturing { request: id(2) });
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: id(1),
            picture: None
        }
    );
    assert_eq!(screen.state.saving_shots(), 0);
}

#[test]
fn ten_quick_shots_all_reach_the_capture_service_and_the_eleventh_press_is_refused_explicitly() {
    let mut screen = Screen::ready();
    let capacity = screen.config.capture.max_saving_shots;
    assert_eq!(capacity, 10);
    let mut captured = Vec::new();
    for number in 0..capacity {
        let shot = press(&mut screen, i64::try_from(number).expect("мало") * 50);
        secure(&mut screen, shot.request);
        captured.push(shot.request);
    }
    assert_eq!(captured.len(), 10, "десять нажатий — десять эффектов Capture");
    assert_eq!(screen.state.saving_shots(), 10);

    // Одиннадцатое: явный отказ «подожди», эффектов нет, затвор свободен, уже снятое не потеряно.
    let before = screen.state.clone();
    let eleventh = screen.step(ScreenAction::ShutterTapped { at: at_ms(600) });
    assert_eq!(eleventh.outcome, ActionOutcome::Rejected(Rejection::SavingQueueFull));
    assert!(eleventh.effects.is_empty());
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.saving_shots(), 10);
    assert_eq!(screen.state.rejections().get(Rejection::SavingQueueFull), 1);
    assert_eq!(screen.state.lost_captures(), 0);
    assert_eq!(screen.state.saving_overflows(), 0);
    assert_eq!(screen.state.thumbnail(), before.thumbnail());

    // Клавиша громкости получает тот же отказ, а таймер не запускает пустой отсчёт.
    let key = screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(610),
    });
    assert_eq!(key.outcome, ActionOutcome::Rejected(Rejection::SavingQueueFull));

    // Одно сохранение освободило место — снова можно снимать.
    finish(&mut screen, captured[0], CaptureOutcome::Saved);
    let again = press(&mut screen, 700);
    assert_eq!(again.request, id(11));
}

#[test]
fn full_saving_set_refuses_to_start_a_timer_countdown() {
    let mut screen = Screen::ready();
    screen.config.capture.max_saving_shots = 1;
    screen.state = crate::state::ScreenState::new(&screen.config).expect("экран");
    screen.step(ScreenAction::SurfaceResized(crate::test_support::OUR_SURFACE));
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    let shot = press(&mut screen, 0);
    secure(&mut screen, shot.request);
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);

    let press_timer = screen.step(ScreenAction::ShutterTapped { at: at_ms(10) });

    assert_eq!(press_timer.outcome, ActionOutcome::Rejected(Rejection::SavingQueueFull));
    assert_eq!(screen.state.shutter(), ShutterState::Idle, "отсчёт не запущен");
}

#[test]
fn thumbnail_shows_the_shot_saved_last_not_the_one_pressed_last() {
    let mut screen = Screen::ready();
    let first = press(&mut screen, 0);
    secure(&mut screen, first.request);
    let second = press(&mut screen, 100);
    secure(&mut screen, second.request);

    finish(&mut screen, second.request, CaptureOutcome::Saved);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: id(2),
            picture: None
        }
    );
    finish(&mut screen, first.request, CaptureOutcome::Saved);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: id(1),
            picture: None
        },
        "первым нажатый сохранился позже — он и в миниатюре"
    );
}

#[test]
fn capture_phase_timeout_frees_the_shutter_counts_the_loss_and_a_late_secured_is_stale() {
    let mut screen = Screen::ready();
    let timeout_ms = i64::from(screen.config.capture.capture_timeout_ms);
    let shot = press(&mut screen, 1_000);

    screen.step(ScreenAction::Tick(at_ms(1_000 + timeout_ms - 1)));
    assert!(matches!(screen.state.shutter(), ShutterState::Capturing { .. }));
    let overdue = screen.step(ScreenAction::Tick(at_ms(1_000 + timeout_ms)));

    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.lost_captures(), 1);
    assert_eq!(overdue.effects, [Effect::Haptic(HapticPattern::Error)]);
    let late = screen.step(ScreenAction::CaptureSecured { request: shot.request });
    assert_eq!(late.outcome, ActionOutcome::Rejected(Rejection::StaleCaptureResult));
    assert_eq!(screen.state.saving_shots(), 0, "опоздавший кадр на учёт не берётся");
    assert_eq!(screen.state.rejections().get(Rejection::StaleCaptureResult), 1);
}

#[test]
fn the_capture_phase_is_much_shorter_than_the_saving_phase() {
    // Затвор не должен ждать сохранения: срок захвата короче срока сохранения.
    let config = crate::config::ScreenConfig::default();
    assert!(config.capture.capture_timeout_ms < config.capture.saving_timeout_ms);
}

#[test]
fn saving_timeout_loses_only_that_shot_and_does_not_touch_the_shutter() {
    let mut screen = Screen::ready();
    let saving_ms = i64::from(screen.config.capture.saving_timeout_ms);
    let old = press(&mut screen, 0);
    screen.step(ScreenAction::Tick(at_ms(10)));
    secure(&mut screen, old.request);
    // Затвор занят следующим снимком в момент, когда у первого выйдет срок сохранения.
    let current = press(&mut screen, saving_ms - 500);

    let tick = screen.step(ScreenAction::Tick(at_ms(10 + saving_ms)));

    assert_eq!(screen.state.lost_captures(), 1);
    assert_eq!(screen.state.saving_shots(), 0);
    assert_eq!(tick.effects, [Effect::Haptic(HapticPattern::Error)]);
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Capturing {
            request: current.request
        },
        "затвор не тронут"
    );
    let late = finish(&mut screen, old.request, CaptureOutcome::Saved);
    assert_eq!(late.outcome, ActionOutcome::Rejected(Rejection::StaleCaptureResult));
    assert_eq!(screen.state.thumbnail(), ThumbnailState::Empty);
}

#[test]
fn saving_deadline_is_exact_to_the_millisecond() {
    let mut screen = Screen::ready();
    let saving_ms = i64::from(screen.config.capture.saving_timeout_ms);
    let shot = press(&mut screen, 0);
    screen.step(ScreenAction::Tick(at_ms(100)));
    secure(&mut screen, shot.request);

    screen.step(ScreenAction::Tick(at_ms(100 + saving_ms - 1)));
    assert_eq!(screen.state.lost_captures(), 0);
    assert_eq!(screen.state.saving_shots(), 1);
    screen.step(ScreenAction::Tick(at_ms(100 + saving_ms)));
    assert_eq!(screen.state.lost_captures(), 1);
}

#[test]
fn failure_before_capture_frees_the_shutter_and_vibrates() {
    let mut screen = Screen::ready();
    let shot = press(&mut screen, 0);

    let failed = finish(
        &mut screen,
        shot.request,
        CaptureOutcome::Failed(CaptureFailureReason::Camera),
    );

    assert_eq!(failed.effects, [Effect::Haptic(HapticPattern::Error)]);
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.saving_shots(), 0);
    assert_eq!(screen.state.thumbnail(), ThumbnailState::Empty);
}

#[test]
fn failure_after_capture_vibrates_and_leaves_the_shutter_alone() {
    let mut screen = Screen::ready();
    let first = press(&mut screen, 0);
    secure(&mut screen, first.request);
    let second = press(&mut screen, 100);

    let failed = finish(
        &mut screen,
        first.request,
        CaptureOutcome::Failed(CaptureFailureReason::Storage),
    );

    assert_eq!(failed.effects, [Effect::Haptic(HapticPattern::Error)]);
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Capturing {
            request: second.request
        }
    );
    assert_eq!(screen.state.saving_shots(), 0);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Empty,
        "ошибка миниатюру не меняет"
    );
}

#[test]
fn unknown_duplicate_and_idle_results_are_refused_without_changing_the_state() {
    let mut screen = Screen::ready();
    let shot = press(&mut screen, 0);
    assert_eq!(
        secure(&mut screen, id(99)),
        ActionOutcome::Rejected(Rejection::StaleCaptureResult)
    );
    assert_eq!(
        screen.state.shutter(),
        ShutterState::Capturing { request: shot.request }
    );
    secure(&mut screen, shot.request);
    assert_eq!(
        secure(&mut screen, shot.request),
        ActionOutcome::Rejected(Rejection::StaleCaptureResult),
        "повторный Secured того же снимка"
    );
    assert_eq!(screen.state.saving_shots(), 1, "дубль не добавил запись");
    finish(&mut screen, shot.request, CaptureOutcome::Saved);
    let duplicate = finish(&mut screen, shot.request, CaptureOutcome::Saved);
    assert_eq!(
        duplicate.outcome,
        ActionOutcome::Rejected(Rejection::StaleCaptureResult)
    );
    assert_eq!(screen.state.rejections().get(Rejection::StaleCaptureResult), 3);
}

#[test]
fn camera_loss_abandons_only_the_capturing_shot_not_the_saving_ones() {
    let mut screen = Screen::ready();
    let first = press(&mut screen, 0);
    secure(&mut screen, first.request);
    press(&mut screen, 100);

    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        presence: crate::facts::CameraPresence::Unavailable {
            reason: crate::facts::UnavailableReason::Failure,
        },
        ..live_facts()
    }));

    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.lost_captures(), 1, "потерян только снимок в захвате");
    assert_eq!(screen.state.saving_shots(), 1, "захваченный сохраняется и без камеры");
    finish(&mut screen, first.request, CaptureOutcome::Saved);
    assert_eq!(
        screen.state.thumbnail(),
        ThumbnailState::Last {
            shot: first.request,
            picture: None
        }
    );
}

#[test]
fn timer_shot_is_pressed_and_sounds_at_the_moment_the_countdown_fires() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);

    let start = screen.step(ScreenAction::ShutterTapped { at: at_ms(10_000) });
    assert!(
        start.effects.is_empty(),
        "при нажатии кнопки таймера ни снимка, ни звука"
    );
    let early = screen.step(ScreenAction::Tick(at_ms(12_999)));
    assert!(early.effects.is_empty());

    let due = screen.step(ScreenAction::Tick(at_ms(13_016)));
    let [Effect::Capture(shot), Effect::PlayShutterSound] = due.effects.as_slice() else {
        panic!("в момент срабатывания нужны снимок и звук: {:?}", due.effects)
    };
    assert_eq!(
        shot.pressed_at,
        at_ms(13_016),
        "момент снимка = тик, на котором сработал таймер"
    );
}

#[test]
fn shot_request_carries_orientation_aspect_and_volume_key_time_of_the_moment() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(
        AspectRatio::SixteenByNine,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);

    let key = screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(777),
    });

    let [Effect::Capture(shot), Effect::PlayShutterSound] = key.effects.as_slice() else {
        panic!("нужны снимок и звук: {:?}", key.effects)
    };
    assert_eq!(shot.pressed_at, at_ms(777), "время клавиши громкости");
    assert_eq!(shot.orientation, DeviceOrientation::LandscapeLeft);
    assert_eq!(shot.aspect, AspectRatio::SixteenByNine);
    assert_eq!(shot.framing.aspect, AspectRatio::SixteenByNine.crop_aspect());
}

#[test]
fn rejected_presses_play_no_sound() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let hidden = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert!(hidden.effects.is_empty());
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    press(&mut screen, 10);
    let busy = screen.step(ScreenAction::ShutterTapped { at: at_ms(20) });
    assert_eq!(busy.outcome, ActionOutcome::Rejected(Rejection::ShutterBusy));
    assert!(busy.effects.is_empty(), "занятый затвор не звучит");
}
