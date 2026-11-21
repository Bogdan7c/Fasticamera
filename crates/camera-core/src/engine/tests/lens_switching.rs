//! Смена объектива в движке: завершение по первому кадру, возврат при провале новой камеры,
//! восстановление после вытеснения без возврата, пересмотр цели посреди смены.

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::fixtures::camera_id;
use camera_contract::ids::{LensFacing, LensRole};

use super::lens_support::{LensRig, ultra_wide_id};
use super::*;
use crate::backend::{BackendFailure, CameraAvailability, ConfigureFailure, DeviceErrorKind, OpenFailure};
use crate::lens_status::{LensSwitchFailure, LensSwitchState};

fn switched_events(events: &[CameraEvent]) -> Vec<&CameraEvent> {
    events
        .iter()
        .filter(|event| matches!(event, CameraEvent::LensSwitched { .. }))
        .collect()
}

fn count_calls(calls: &[BackendCall], wanted: &str) -> usize {
    call_names(calls).into_iter().filter(|name| *name == wanted).count()
}

#[test]
fn intent_switches_wide_to_ultra_wide_and_completes_on_the_first_frame_of_the_new_stream() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);

    // Бэкенд: прежняя камера закрыта, открыта «3», сессия и запрос подняты.
    let calls = lens.rig.fake.take_calls();
    assert_eq!(
        call_names(&calls),
        vec![
            "close_device",
            "open_device",
            "configure_session",
            "set_repeating_request"
        ]
    );
    assert!(calls.iter().any(|call| matches!(call,
        BackendCall::OpenDevice { camera, .. } if *camera == ultra_wide_id())));
    assert_eq!(lens.rig.state_name(), "streaming");
    // Поток уже идёт, но кадра новой камеры ещё не было: смена не завершена.
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        }
    );
    assert!(switched_events(&lens.rig.fresh_events()).is_empty());

    lens.rig.now += Duration::from_millis(480);
    lens.frames(1);
    let events = lens.rig.fresh_events();
    assert_eq!(
        switched_events(&events),
        vec![&CameraEvent::LensSwitched {
            from: LensRole::Wide,
            to: LensRole::UltraWide,
            elapsed: Duration::from_millis(480)
        }]
    );
    let status = lens.rig.engine.status().lens;
    assert_eq!(
        status.switch,
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
    let active = status.active.expect("сессия ширика");
    assert_eq!(
        (active.role, active.camera, active.facing),
        (LensRole::UltraWide, ultra_wide_id(), LensFacing::Back)
    );
    assert_eq!(status.counters.switches_completed, 1);
    assert_eq!(status.counters.last_switch_elapsed, Some(Duration::from_millis(480)));
}

#[test]
fn failed_open_of_the_new_camera_reverts_blocks_the_role_and_does_not_retry() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::DeviceFailure,
    }));
    lens.intent(LensFacing::Back, 0.6);

    let events = lens.rig.fresh_events();
    assert!(events.contains(&CameraEvent::LensSwitchFailed {
        from: LensRole::Wide,
        to: LensRole::UltraWide,
        reason: LensSwitchFailure::Unavailable {
            reason: CameraUnavailableReason::DeviceFailure
        },
    }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::Unavailable { .. })),
        "это возврат, а не недоступность камеры"
    );
    // Открывали «3» (отказ), затем вернулись на «2»; поток снова идёт.
    assert_eq!(lens.open_cameras(), vec!["3", "2"]);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));
    assert_eq!(lens.rig.state_name(), "streaming");
    let status = lens.rig.engine.status().lens;
    assert_eq!(
        status.blocked.as_ref().map(|block| block.role),
        Some(LensRole::UltraWide)
    );
    assert_eq!(status.counters.switches_failed, 1);

    // Возврат тоже завершается первым кадром — экран проявляет прежнюю камеру.
    lens.frames(1);
    assert!(lens.rig.fresh_events().contains(&CameraEvent::LensSwitched {
        from: LensRole::Wide,
        to: LensRole::Wide,
        elapsed: Duration::ZERO
    }));
    assert_eq!(lens.switch_state(), LensSwitchState::Steady { role: LensRole::Wide });

    // Намерения в диапазоне ширика больше не пробуют его открывать.
    lens.rig.fake.take_calls();
    lens.intent(LensFacing::Back, 0.6);
    lens.intent(LensFacing::Back, 0.55);
    assert!(lens.rig.fake.take_calls().is_empty(), "блокировка держит");
    assert!(lens.rig.engine.status().lens.blocked.is_some());
}

#[test]
fn intent_selecting_another_role_clears_the_block_and_ultra_wide_works_again() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::ServiceFailure,
    }));
    lens.intent(LensFacing::Back, 0.6);
    lens.frames(1);
    lens.rig.fake.take_calls();

    lens.intent(LensFacing::Back, 1.2);
    assert!(
        lens.rig.engine.status().lens.blocked.is_none(),
        "намерение выбрало Wide — блок снят"
    );
    assert!(
        lens.rig.fake.take_calls().is_empty(),
        "Wide уже стоит: бэкенд не тронут"
    );

    lens.intent(LensFacing::Back, 0.6);
    assert_eq!(
        lens.rig.fake.open_camera(),
        Some(ultra_wide_id()),
        "ширик снова пробуется и открывается"
    );
}

#[test]
fn explicit_command_to_the_blocked_role_clears_the_block() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::DeviceFailure,
    }));
    lens.intent(LensFacing::Back, 0.6);
    assert!(lens.rig.engine.status().lens.blocked.is_some());

    let outcome = lens.rig.send(CameraCommand::SwitchTo {
        role: LensRole::UltraWide,
    });
    assert_eq!(outcome, CommandOutcome::Accepted);
    assert!(lens.rig.engine.status().lens.blocked.is_none());
    assert_eq!(lens.rig.fake.open_camera(), Some(ultra_wide_id()));
}

#[test]
fn device_error_before_the_first_frame_reverts_to_the_previous_lens() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.rig.fake.fail_current_device(DeviceErrorKind::CameraDevice);
    lens.rig.pump();

    assert!(lens.rig.fresh_events().iter().any(|event| matches!(
        event,
        CameraEvent::LensSwitchFailed {
            from: LensRole::Wide,
            to: LensRole::UltraWide,
            ..
        }
    )));
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));
    assert_eq!(lens.rig.state_name(), "streaming");
}

#[test]
fn rejected_stream_on_the_new_camera_reverts_and_keeps_the_desired_stream_for_the_old_one() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_configure_failure(ConfigureFailure::RejectedByHal {
        detail: "STREAM_CONFIGURE_FAIL".to_owned(),
    });
    lens.intent(LensFacing::Back, 0.6);

    let events = lens.rig.fresh_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamConfigurationFailed { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        CameraEvent::LensSwitchFailed {
            reason: LensSwitchFailure::StreamRejected { .. },
            ..
        }
    )));
    // Желаемый режим и поток не стёрты: прежняя камера снова стримит.
    assert_eq!(lens.rig.state_name(), "streaming");
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));
    assert!(lens.rig.fake.is_repeating());
}

#[test]
fn eviction_during_a_switch_waits_for_recovery_without_revert() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.rig.fake.disconnect_current_device();
    lens.rig.pump();

    assert_eq!(lens.rig.state_name(), "error", "ждём «камера свободна»");
    let events = lens.rig.fresh_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::LensSwitchFailed { .. }))
    );
    let status = lens.rig.engine.status().lens;
    assert!(status.blocked.is_none());
    assert_eq!(
        status.switch,
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        }
    );

    lens.rig.feed(BackendEvent::CameraAvailability {
        camera: ultra_wide_id(),
        availability: CameraAvailability::Available,
    });
    assert_eq!(
        lens.rig.fake.open_camera(),
        Some(ultra_wide_id()),
        "восстановились на новой камере"
    );
    lens.frames(1);
    assert!(lens.rig.fresh_events().iter().any(|event| matches!(
        event,
        CameraEvent::LensSwitched {
            from: LensRole::Wide,
            to: LensRole::UltraWide,
            ..
        }
    )));
}

#[test]
fn in_use_and_permission_failures_of_the_new_camera_are_not_reverted() {
    for reason in [
        CameraUnavailableReason::InUseByAnotherClient,
        CameraUnavailableReason::PermissionDenied,
        CameraUnavailableReason::DisabledByPolicy,
    ] {
        let mut lens = LensRig::streaming_on_wide();
        lens.rig.fake.push_open_result(Err(OpenFailure::Unavailable { reason }));
        lens.intent(LensFacing::Back, 0.6);
        let events = lens.rig.fresh_events();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CameraEvent::LensSwitchFailed { .. })),
            "{reason:?}: обычное восстановление"
        );
        assert!(events.contains(&CameraEvent::Unavailable { reason }));
        assert_eq!(lens.rig.state_name(), "error");
        assert!(lens.rig.engine.status().lens.blocked.is_none());
    }
}

#[test]
fn retargeting_mid_switch_cancels_the_intermediate_explicitly_and_leaks_no_device() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.intent(LensFacing::Front, 1.0);
    // 1.2× выше порога гистерезиса от ширика (1.05): третья цель — основная, то есть возврат.
    lens.intent(LensFacing::Back, 1.2);

    let events = lens.rig.fresh_events();
    assert!(events.contains(&CameraEvent::LensSwitchRetargeted {
        from: LensRole::Wide,
        abandoned: LensRole::UltraWide,
        to: LensRole::Front
    }));
    assert!(events.contains(&CameraEvent::LensSwitchRetargeted {
        from: LensRole::Wide,
        abandoned: LensRole::Front,
        to: LensRole::Wide
    }));
    let calls = lens.rig.fake.take_calls();
    let opened: Vec<String> = calls
        .iter()
        .filter_map(|call| match call {
            BackendCall::OpenDevice { camera, .. } => Some(camera.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(opened, vec!["3", "1", "2"]);
    assert_eq!(
        count_calls(&calls, "close_device"),
        3,
        "каждое открытие промежуточной камеры закрыто до следующего"
    );
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));

    // Вернулись на исходную: смена завершается её первым кадром, счётчик отмен — 2.
    lens.frames(1);
    assert_eq!(lens.switch_state(), LensSwitchState::Steady { role: LensRole::Wide });
    assert_eq!(lens.rig.engine.status().lens.counters.switches_retargeted, 2);
}

#[test]
fn chain_wide_to_ultra_wide_to_front_failing_on_front_reverts_to_wide() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.rig.fake.push_open_result(Err(OpenFailure::Backend(BackendFailure {
        operation: "open",
        code: 7,
        detail: "x".to_owned(),
    })));
    lens.intent(LensFacing::Front, 1.0);

    let events = lens.rig.fresh_events();
    assert!(events.iter().any(|event| matches!(
        event,
        CameraEvent::LensSwitchFailed {
            from: LensRole::Wide,
            to: LensRole::Front,
            ..
        }
    )));
    assert_eq!(
        lens.rig.fake.open_camera(),
        Some(camera_id("2")),
        "откат на A, а не на промежуточную B"
    );
    assert_eq!(
        lens.rig.engine.status().lens.blocked.map(|block| block.role),
        Some(LensRole::Front)
    );
}

#[test]
fn close_and_stop_streaming_forget_the_switch() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.rig.send(CameraCommand::StopStreaming);
    assert_eq!(lens.switch_state(), LensSwitchState::Idle);

    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.rig.send(CameraCommand::Close);
    assert_eq!(lens.switch_state(), LensSwitchState::Idle);
    assert!(lens.rig.engine.status().lens.active.is_none());
}

#[test]
fn tap_right_after_streaming_but_before_the_first_frame_is_still_a_switch() {
    // Находка 13a: тап `.6` пришёл, когда движок уже `Streaming`, но кадров ещё не обработал.
    let mut lens = LensRig::streaming_on_wide_before_first_frame();
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Steady { role: LensRole::Wide },
        "старт потока уже закрепил показ основного объектива"
    );

    lens.intent_discrete(LensFacing::Back, 0.6);
    assert_eq!(lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        },
        "кадр основной камеры не нужен: смена началась сразу"
    );
    assert!(switched_events(&lens.rig.fresh_events()).is_empty());

    lens.rig.now += Duration::from_millis(300);
    lens.frames(2);
    let events = lens.rig.fresh_events();
    assert_eq!(
        switched_events(&events),
        vec![&CameraEvent::LensSwitched {
            from: LensRole::Wide,
            to: LensRole::UltraWide,
            elapsed: Duration::from_millis(300)
        }],
        "ровно одно LensSwitched"
    );
    let status = lens.rig.engine.status().lens;
    assert_eq!(status.counters.switches_completed, 1);
    assert_eq!(
        status.switch,
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
}

#[test]
fn recovery_after_eviction_restarts_the_same_lens_without_a_fake_switch() {
    let mut lens = LensRig::streaming_on_wide_before_first_frame();
    lens.rig.fake.disconnect_current_device();
    lens.rig.pump();
    assert_eq!(lens.rig.state_name(), "error", "ждём «камера свободна»");
    lens.rig.fresh_events();

    lens.rig.feed(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    assert_eq!(lens.rig.state_name(), "streaming", "восстановились");
    lens.frames(3);

    assert!(
        switched_events(&lens.rig.fresh_events()).is_empty(),
        "восстановление той же камеры — не смена объектива"
    );
    let status = lens.rig.engine.status().lens;
    assert_eq!(status.switch, LensSwitchState::Steady { role: LensRole::Wide });
    assert_eq!(status.counters.switches_completed, 0);
}

#[test]
fn close_forgets_the_shown_lens_and_the_next_stream_start_is_a_first_show_again() {
    let mut lens = LensRig::streaming_on_wide_before_first_frame();
    lens.rig.send(CameraCommand::Close);
    assert_eq!(lens.switch_state(), LensSwitchState::Idle);

    // Новая камера — ширик: цель выбрана до старта потока, это первый показ, а не смена.
    lens.rig.send(CameraCommand::Open {
        target: CameraTarget::Role(LensRole::UltraWide),
    });
    lens.rig.send(CameraCommand::Configure {
        mode: mode_raw10_4x3_at(60),
    });
    assert_eq!(lens.switch_state(), LensSwitchState::Idle, "поток ещё не запущен");
    lens.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
    lens.frames(3);
    assert!(switched_events(&lens.rig.fresh_events()).is_empty());
    assert_eq!(lens.rig.engine.status().lens.counters.switches_completed, 0);
}
