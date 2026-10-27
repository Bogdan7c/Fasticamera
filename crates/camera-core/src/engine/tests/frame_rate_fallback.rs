//! Откат 4:3@60 -> 4:3@30 при недостижимой частоте (решение 23): честное событие, ровно одна
//! повторная настройка, поток продолжается; прочие сбои и выключенный откат ведут себя как раньше.

use camera_contract::control::CaptureIntent;
use camera_contract::stream::ReadoutAspect;

use super::*;
use crate::backend::{ConfigureFailure, UnsupportedStream};
use crate::config::FrameRateFallback;

fn not_achievable_60() -> ConfigureFailure {
    ConfigureFailure::Unsupported(UnsupportedStream::FrameRateNotAchievable {
        requested: TargetFrameRate::new(60).expect("60 к/с"),
    })
}

/// Стенд с открытой основной камерой; команда `Configure` и запуск потока — на вызывающем.
fn rig_with_fallback(policy: FrameRateFallback) -> Rig {
    let mut config = CameraEngineConfig::default();
    config.stream.frame_rate_fallback = policy;
    let mut rig = Rig::with_config(config, AppPresence::Foreground);
    assert_eq!(
        rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::Accepted
    );
    rig.fresh_events();
    rig.fake.take_calls();
    rig
}

fn configure_and_stream(rig: &mut Rig, mode: StreamMode) {
    rig.send(CameraCommand::Configure { mode });
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
}

fn configure_calls(rig: &Rig) -> Vec<TargetFrameRate> {
    rig.fake
        .take_calls()
        .into_iter()
        .filter_map(|call| match call {
            BackendCall::ConfigureSession { stream, .. } => Some(stream.frame_rate),
            _ => None,
        })
        .collect()
}

#[test]
fn unreachable_60_degrades_to_30_and_keeps_streaming() {
    let mut rig = rig_with_fallback(FrameRateFallback::default());
    rig.fake.push_configure_failure(not_achievable_60());
    configure_and_stream(&mut rig, mode_raw10_4x3_at(60));

    let events = rig.fresh_events();
    let degraded_at = events
        .iter()
        .position(|event| {
            matches!(event, CameraEvent::StreamDegraded { requested, fallback, failure }
                if requested.frame_rate.frames_per_second() == 60
                    && fallback.frame_rate.frames_per_second() == 30
                    && fallback.readout == ReadoutAspect::FourByThree
                    && *failure == not_achievable_60())
        })
        .expect("честное событие StreamDegraded");
    let configured_at = events
        .iter()
        .position(|event| {
            matches!(event, CameraEvent::StreamConfigured { stream }
                if stream.stream.frame_rate.frames_per_second() == 30)
        })
        .expect("сессия сконфигурирована уже на 30 к/с");
    assert!(
        degraded_at < configured_at,
        "сначала сообщаем об откате, потом настройка"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamConfigurationFailed { .. })),
        "откат — не отказ"
    );

    assert_eq!(rig.state_name(), "streaming");
    let status = rig.engine.status();
    let configured = status.configured.expect("сессия живая");
    assert_eq!(configured.stream.frame_rate.frames_per_second(), 30);
    assert_eq!(rig.engine.desired.mode, Some(mode_raw10_4x3_at(30)));
    assert_eq!(
        configure_calls(&rig)
            .iter()
            .map(|rate| rate.frames_per_second())
            .collect::<Vec<_>>(),
        vec![60, 30],
        "ровно две настройки: запрошенная и запасная"
    );
}

#[test]
fn cadence_target_follows_the_fallback_rate() {
    let mut rig = rig_with_fallback(FrameRateFallback::default());
    rig.fake.push_configure_failure(not_achievable_60());
    configure_and_stream(&mut rig, mode_raw10_4x3_at(60));
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(
        rig.engine.status().stream.target_frame_duration,
        Some(TargetFrameRate::new(30).expect("30 к/с").frame_duration()),
        "темп считается от 30 к/с, иначе каждый кадр был бы «пропуском»"
    );
}

#[test]
fn failing_fallback_is_a_normal_rejection_without_retry_loop() {
    let mut rig = rig_with_fallback(FrameRateFallback::default());
    rig.fake.push_configure_failure(not_achievable_60());
    rig.fake.push_configure_failure(ConfigureFailure::RejectedByHal {
        detail: "STREAM_CONFIGURE_FAIL".to_owned(),
    });
    configure_and_stream(&mut rig, mode_raw10_4x3_at(60));

    let events = rig.fresh_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamDegraded { .. }))
    );
    assert!(events.iter().any(|event| matches!(event,
        CameraEvent::StreamConfigurationFailed { mode, failure: ConfigureFailure::RejectedByHal { .. } }
            if mode.frame_rate.frames_per_second() == 30)));
    assert_eq!(rig.state_name(), "opened");
    assert_eq!(configure_calls(&rig).len(), 2, "один откат, без бесконечных повторов");
}

#[test]
fn other_failures_do_not_degrade() {
    let readout = ConfigureFailure::Unsupported(UnsupportedStream::ReadoutNotAvailable {
        readout: ReadoutAspect::FourByThree,
    });
    let rejected = ConfigureFailure::RejectedByHal { detail: "x".to_owned() };
    for failure in [readout, rejected] {
        let mut rig = rig_with_fallback(FrameRateFallback::default());
        rig.fake.push_configure_failure(failure.clone());
        configure_and_stream(&mut rig, mode_raw10_4x3_at(60));
        let events = rig.fresh_events();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CameraEvent::StreamDegraded { .. }))
        );
        assert!(events.iter().any(|event| matches!(event,
            CameraEvent::StreamConfigurationFailed { mode, failure: reported }
                if mode.frame_rate.frames_per_second() == 60 && *reported == failure)));
        assert_eq!(configure_calls(&rig).len(), 1);
    }
}

#[test]
fn requested_rate_not_above_fallback_is_rejected_as_before() {
    let mut rig = rig_with_fallback(FrameRateFallback::default());
    rig.fake.push_configure_failure(ConfigureFailure::Unsupported(
        UnsupportedStream::FrameRateNotAchievable {
            requested: TargetFrameRate::new(30).expect("30 к/с"),
        },
    ));
    configure_and_stream(&mut rig, mode_raw10_4x3_at(30));
    let events = rig.fresh_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamDegraded { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamConfigurationFailed { .. }))
    );
    assert_eq!(configure_calls(&rig).len(), 1);
}

#[test]
fn disabled_fallback_keeps_the_old_rejection() {
    let mut rig = rig_with_fallback(FrameRateFallback::Disabled);
    rig.fake.push_configure_failure(not_achievable_60());
    configure_and_stream(&mut rig, mode_raw10_4x3_at(60));
    let events = rig.fresh_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamDegraded { .. }))
    );
    assert!(events.iter().any(|event| matches!(event,
        CameraEvent::StreamConfigurationFailed { mode, .. } if mode.frame_rate.frames_per_second() == 60)));
    assert_eq!(rig.state_name(), "opened");
    assert_eq!(configure_calls(&rig).len(), 1);
}
