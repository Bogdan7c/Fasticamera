//! Отказы тёплых камер: ячейка в `Failed`, основная не затронута, повторов до нового спроса нет.

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::zoom::StandbyDemand;

use super::standby_support::*;
use super::*;
use crate::backend::{BackendFailure, ConfigureFailure, DeviceErrorKind, OpenFailure, RequestFailure};
use crate::lens_status::LensSwitchMethod;
use crate::standby_status::{StandbyFailureReason, StandbySlotState};

fn backend_failure(operation: &'static str) -> BackendFailure {
    BackendFailure {
        operation,
        code: -7,
        detail: "тест".to_owned(),
    }
}

/// Название случая, событие тёплой и ожидаемая причина отказа ячейки.
type DeviceEventCase = (&'static str, fn(&StandbyRig) -> BackendEvent, StandbyFailureReason);

fn failed_reason(rig: &StandbyRig) -> StandbyFailureReason {
    match rig.slot_state(0) {
        StandbySlotState::Failed { reason } => reason,
        other => unreachable!("ячейка должна быть в Failed, а она {other:?}"),
    }
}

/// Первичная основная после любого отказа тёплой: стримит, камера и состояние те же, не закрывалась.
fn assert_primary_untouched(rig: &StandbyRig, calls: &[&'static str]) {
    assert!(
        !calls.contains(&"close_device") && !calls.contains(&"close_session"),
        "{calls:?}"
    );
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(wide_id()));
    assert_eq!(rig.lens.rig.state_name(), "streaming");
    assert_eq!(rig.lens.rig.engine.status().counters.recovery_attempts, 0);
}

#[test]
fn open_configure_and_request_failures_each_fail_only_the_slot_with_their_reason() {
    let open = OpenFailure::Backend(backend_failure("open"));
    let configure = ConfigureFailure::RejectedByHal {
        detail: "лимит потоков".to_owned(),
    };
    let request = RequestFailure::Backend(backend_failure("request"));
    for step in 0..3 {
        let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
        let fake = &rig.lens.rig.fake;
        match step {
            0 => fake.push_standby_open_result(Err(open.clone())),
            1 => fake.push_standby_configure_result(Err(configure.clone())),
            _ => fake.push_standby_request_result(Err(request.clone())),
        }
        rig.wheel_at(1.0);
        let expected = match step {
            0 => StandbyFailureReason::OpenFailed { failure: open.clone() },
            1 => StandbyFailureReason::ConfigureFailed {
                failure: configure.clone(),
            },
            _ => StandbyFailureReason::RequestFailed {
                failure: request.clone(),
            },
        };
        assert_eq!(failed_reason(&rig), expected, "шаг {step}");
        let calls = rig.calls();
        assert_primary_untouched(&rig, &calls);
        assert_eq!(rig.standby().counters.failures, 1);
        // Ресурсы тёплой освобождены (для шага 0 их и не было).
        assert_eq!(rig.lens.rig.fake.standby_camera(slot(0)), None, "шаг {step}");
        rig.lens.frames(2);
        assert_eq!(rig.lens.rig.state_name(), "streaming", "кадры основной идут");
    }
}

#[test]
fn failed_slot_is_not_retried_until_the_demand_toggles_off_and_on() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.lens
        .rig
        .fake
        .push_standby_open_result(Err(OpenFailure::Unavailable {
            reason: CameraUnavailableReason::TooManyCamerasOpen,
        }));
    rig.wheel_at(1.0);
    assert!(matches!(rig.slot_state(0), StandbySlotState::Failed { .. }));
    rig.calls();

    // Колесо продолжает крутиться: повторов нет.
    for ratio in [1.0, 1.05, 1.1] {
        rig.wheel_at(ratio);
    }
    assert!(rig.calls().is_empty(), "без нового спроса тёплая не открывается заново");

    // Спрос закончился и пауза прошла — отказ забыт; новый спрос пробует снова.
    rig.wheel_closed_at(1.0);
    rig.lens.rig.advance(Duration::from_millis(
        CameraEngineConfig::default().standby.release_delay_ms,
    ));
    assert_eq!(rig.slot_state(0), StandbySlotState::Idle);
    rig.wheel_at(1.0);
    assert_eq!(rig.slot_state(0), StandbySlotState::Warming);
    assert_eq!(rig.standby().counters.failures, 1);
}

#[test]
fn device_events_of_the_standby_fail_the_slot_and_never_reach_primary_recovery() {
    let cases: [DeviceEventCase; 3] = [
        (
            "отключение",
            |rig| BackendEvent::DeviceDisconnected {
                device: rig.lens.rig.fake.standby_device_generation(slot(0)).expect("поколение"),
            },
            StandbyFailureReason::DeviceDisconnected,
        ),
        (
            "ошибка устройства",
            |rig| BackendEvent::DeviceError {
                device: rig.lens.rig.fake.standby_device_generation(slot(0)).expect("поколение"),
                error: DeviceErrorKind::CameraService,
            },
            StandbyFailureReason::DeviceError {
                error: DeviceErrorKind::CameraService,
            },
        ),
        (
            "закрытие сессии платформой",
            |rig| BackendEvent::SessionClosed {
                session: rig
                    .lens
                    .rig
                    .fake
                    .standby_session_generation(slot(0))
                    .expect("поколение"),
            },
            StandbyFailureReason::SessionClosedByPlatform,
        ),
    ];
    for (name, event, reason) in cases {
        let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
        rig.wheel_at(1.0);
        rig.make_standby_ready(0);
        rig.calls();
        let stale_before = rig.lens.rig.engine.status().counters.stale_backend_events;
        let event = event(&rig);
        rig.lens.rig.feed(event);
        assert_eq!(failed_reason(&rig), reason, "{name}");
        let calls = rig.calls();
        assert_eq!(calls, vec!["close_standby"], "{name}");
        assert_eq!(rig.lens.rig.fake.open_camera(), Some(wide_id()), "{name}");
        assert_eq!(rig.lens.rig.state_name(), "streaming", "{name}");
        assert_eq!(rig.lens.rig.engine.status().counters.recoveries, 0);
        assert_eq!(
            rig.lens.rig.engine.status().counters.stale_backend_events,
            stale_before,
            "{name}"
        );
    }
}

#[test]
fn after_a_standby_failure_the_switch_uses_the_normal_path() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.lens
        .rig
        .fake
        .push_standby_request_result(Err(RequestFailure::NoSession));
    rig.wheel_at(1.0);
    rig.calls();
    rig.tap_while_warm(0.5);
    let calls = rig.calls();
    assert!(!calls.contains(&"swap_standby_with_primary"), "{calls:?}");
    assert!(calls.contains(&"open_device"), "{calls:?}");
    rig.lens.frames(1);
    let methods: Vec<LensSwitchMethod> = rig
        .lens
        .rig
        .fresh_events()
        .iter()
        .filter_map(|event| match event {
            CameraEvent::LensSwitched { method, .. } => Some(*method),
            _ => None,
        })
        .collect();
    assert_eq!(methods, vec![LensSwitchMethod::Reopened]);
}

#[test]
fn standby_failure_of_a_stream_the_camera_cannot_give_is_reported_not_skipped() {
    // Тёплая просит тот же режим, что основная; если бэкенд его не даёт — это отказ ячейки.
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.lens
        .rig
        .fake
        .push_standby_configure_result(Err(ConfigureFailure::DeviceDisconnected));
    rig.intent(
        1.0,
        camera_contract::zoom::ZoomGesture::Continuous,
        StandbyDemand::KeepNeighbourWarm,
    );
    assert_eq!(
        failed_reason(&rig),
        StandbyFailureReason::ConfigureFailed {
            failure: ConfigureFailure::DeviceDisconnected
        }
    );
}
