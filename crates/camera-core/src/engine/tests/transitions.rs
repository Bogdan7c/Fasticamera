//! Переходы машины состояний: последовательность `StateChanged` и журнал вызовов бэкенда.

use camera_contract::control::{CaptureIntent, ExposureControl, FocusControl, ManualControl};
use camera_contract::fixtures::camera_id;
use camera_contract::ids::LensRole;
use std::time::Duration;

use super::*;
use crate::backend::{DeviceGeneration, SessionGeneration};

#[test]
fn open_walks_closed_opening_opened_and_calls_open_device_with_generation_one() {
    let mut rig = Rig::new();
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.fresh_state_names(), vec!["opening", "opened"]);
    assert_eq!(
        rig.fake.take_calls(),
        vec![BackendCall::OpenDevice {
            camera: camera_id("2"),
            generation: DeviceGeneration(1)
        }]
    );
    assert!(rig.fake.is_device_open());
    assert_eq!(rig.engine.status().counters.devices_opened, 1);
}

#[test]
fn full_path_to_streaming_calls_backend_in_order_and_resolves_the_stream() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(
        rig.fresh_state_names(),
        vec!["opening", "opened", "configuring", "opened", "streaming"],
        "после configure без потока устройство «открыто», потом поток запущен"
    );
    let calls = rig.fake.take_calls();
    assert_eq!(
        call_names(&calls),
        vec!["open_device", "configure_session", "set_repeating_request"]
    );
    let BackendCall::ConfigureSession { stream, generation } = &calls[1] else {
        panic!("ожидался configure_session");
    };
    assert_eq!(*generation, SessionGeneration(1));
    assert_eq!(stream.camera, camera_id("2"));
    // Размер буфера движок берёт из карты потоков камеры, а не из режима.
    assert_eq!((stream.buffer_size.width, stream.buffer_size.height), (4096, 3072));

    let status = rig.engine.status();
    let configured = status.configured.expect("сессия сконфигурирована");
    assert_eq!(configured.stream, *stream);
    assert_eq!(status.stream.session, Some(SessionGeneration(1)));
    assert_eq!(
        status.stream.target_frame_duration.map(|d| d.as_nanos()),
        Some(16_666_667),
        "темп цели берётся из режима потока"
    );
    assert!(rig.fake.is_repeating());
}

#[test]
fn configure_alone_configures_the_session_and_leaves_state_opened_without_repeating_request() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.fresh_events();
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    let events = rig.fresh_events();
    assert_eq!(state_names(&events), vec!["configuring", "opened"]);
    assert!(events.iter().any(|e| matches!(e, CameraEvent::StreamConfigured { .. })));
    assert!(rig.fake.is_session_configured());
    assert!(!rig.fake.is_repeating());
}

#[test]
fn stop_streaming_returns_to_opened_and_keeps_the_session() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    assert_eq!(rig.send(CameraCommand::StopStreaming), CommandOutcome::Accepted);
    assert_eq!(rig.fresh_state_names(), vec!["opened"]);
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["stop_repeating"]);
    assert!(rig.fake.is_session_configured(), "сессия остаётся");
    assert!(rig.engine.status().configured.is_some());
    // Повторный запуск не пересоздаёт сессию.
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["set_repeating_request"]);
    assert_eq!(rig.state_name(), "streaming");
}

#[test]
fn changing_mode_while_streaming_goes_through_reconfiguring_without_reopening() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    assert_eq!(
        rig.send(CameraCommand::Configure { mode: mode_raw16_30() }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.fresh_state_names(), vec!["reconfiguring", "streaming"]);
    assert_eq!(
        call_names(&rig.fake.take_calls()),
        vec!["close_session", "configure_session", "set_repeating_request"]
    );
    let configured = rig.engine.status().configured.expect("сессия");
    assert_eq!(configured.stream.raw_format, RawFormat::Raw16);
    assert_eq!(rig.engine.status().stream.session, Some(SessionGeneration(2)));
    assert_eq!(
        rig.engine.status().counters.devices_opened,
        1,
        "устройство не переоткрывалось"
    );
}

#[test]
fn switch_to_ultra_wide_while_streaming_moves_mode_and_stream_to_the_other_camera() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    assert_eq!(
        rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.fresh_state_names(), vec!["reconfiguring", "streaming"]);
    let calls = rig.fake.take_calls();
    assert_eq!(
        call_names(&calls),
        vec![
            "close_device",
            "open_device",
            "configure_session",
            "set_repeating_request"
        ]
    );
    let BackendCall::ConfigureSession { stream, .. } = &calls[2] else {
        panic!("ожидался configure_session");
    };
    assert_eq!(stream.camera, camera_id("3"));
    assert_eq!((stream.buffer_size.width, stream.buffer_size.height), (4000, 3000));
    assert_eq!(stream.raw_format, RawFormat::Raw10, "режим перенесён как есть");
    assert_eq!(rig.fake.open_camera(), Some(camera_id("3")));
}

#[test]
fn close_releases_the_device_forgets_desired_and_a_later_open_starts_fresh() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    assert_eq!(rig.send(CameraCommand::Close), CommandOutcome::Accepted);
    assert_eq!(
        rig.state(),
        CameraState::Closed {
            reason: ClosedReason::NotRequested
        }
    );
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["close_device"]);
    assert!(!rig.fake.is_device_open());
    assert!(rig.engine.status().configured.is_none());
    // Желаемое забыто: открытие той же камеры снова Accepted, а потока и режима нет.
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.state_name(), "opened");
    assert!(!rig.fake.is_session_configured());
    assert_eq!(rig.send(CameraCommand::Close), CommandOutcome::Accepted);
    assert_eq!(rig.send(CameraCommand::Close), CommandOutcome::AlreadyInState);
}

#[test]
fn apply_controls_while_streaming_replaces_the_request_without_recreating_the_session() {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    let session_before = rig.fake.current_session_generation();
    let controls = ManualControl {
        exposure: ExposureControl::Manual {
            exposure_time: Duration::from_millis(4),
            iso: 200,
        },
        focus: FocusControl::Manual { distance_diopters: 2.0 },
    };
    assert_eq!(
        rig.send(CameraCommand::ApplyControls { controls }),
        CommandOutcome::Accepted
    );
    assert_eq!(call_names(&rig.fake.take_calls()), vec!["set_repeating_request"]);
    assert_eq!(rig.fake.repeating_request(), Some((CaptureIntent::Preview, controls)));
    assert_eq!(rig.fake.current_session_generation(), session_before);
    assert!(rig.fresh_state_names().is_empty(), "состояние не менялось");
    // Те же параметры ещё раз — no-op без вызовов.
    assert_eq!(
        rig.send(CameraCommand::ApplyControls { controls }),
        CommandOutcome::AlreadyInState
    );
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn apply_controls_without_streaming_touches_no_backend_but_the_next_stream_uses_them() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.fake.take_calls();
    let controls = ManualControl {
        exposure: ExposureControl::Manual {
            exposure_time: Duration::from_millis(2),
            iso: 100,
        },
        focus: FocusControl::ContinuousAuto,
    };
    rig.send(CameraCommand::ApplyControls { controls });
    assert!(rig.fake.take_calls().is_empty());
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::VideoRecord,
    });
    assert_eq!(
        rig.fake.repeating_request(),
        Some((CaptureIntent::VideoRecord, controls))
    );
}
