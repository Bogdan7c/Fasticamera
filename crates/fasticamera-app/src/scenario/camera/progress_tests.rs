//! Дедлайны, имена рычагов и сжатая запись событий движка.

use camera_contract::capabilities::{FpsRange, FrameDuration};
use camera_contract::geometry::PixelSize;
use camera_contract::ids::CameraId;
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ConfiguredStream, ReadoutAspect, ResolvedStream, StreamMode, TargetFrameRate};
use camera_core::backend::ConfigureFailure;
use camera_core::command::{CommandOutcome, InvalidStateReason};
use camera_core::{CameraState, ClosedReason, CommandId};

use super::*;

fn configured(mechanism: FrameRateMechanism) -> ConfiguredStream {
    ConfiguredStream {
        stream: ResolvedStream {
            camera: CameraId::new("2").expect("id"),
            raw_format: RawFormat::Raw10,
            buffer_size: PixelSize::new(4096, 3072),
            readout: ReadoutAspect::SixteenByNine,
            frame_rate: TargetFrameRate::new(60).expect("60"),
        },
        frame_rate_mechanism: mechanism,
    }
}

#[test]
fn deadline_expires_exactly_at_its_moment() {
    let start = Instant::now();
    let deadline = Deadline::after_millis(start, 100);
    assert!(!deadline.is_expired(start));
    assert!(!deadline.is_expired(start + Duration::from_millis(99)));
    assert!(deadline.is_expired(start + Duration::from_millis(100)));
}

#[test]
fn lever_names_match_the_serde_tags_of_the_contract() {
    for mechanism in [
        FrameRateMechanism::HalDefault,
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 },
        },
        FrameRateMechanism::VendorSensorFrameDurationOverride {
            frame_duration: FrameDuration::from_nanos(16_666_667),
        },
    ] {
        assert_eq!(
            serde_json::to_value(mechanism).expect("json")["kind"],
            lever_name(&mechanism),
            "{mechanism:?}"
        );
    }
}

#[test]
fn event_notes_keep_the_facts_and_drop_the_bulky_state_payloads() {
    let streaming = CameraEvent::StateChanged {
        state: CameraState::Streaming {
            stream: configured(FrameRateMechanism::HalDefault),
        },
    };
    assert_eq!(EventNote::of(&streaming), EventNote::State { state: "streaming" });
    let closed = CameraEvent::StateChanged {
        state: CameraState::Closed {
            reason: ClosedReason::AppInBackground,
        },
    };
    assert_eq!(EventNote::of(&closed), EventNote::State { state: "closed" });

    let unavailable = CameraEvent::Unavailable {
        reason: CameraUnavailableReason::Evicted,
    };
    assert_eq!(
        EventNote::of(&unavailable),
        EventNote::Unavailable {
            reason: CameraUnavailableReason::Evicted
        }
    );
    let recovered = CameraEvent::Recovered {
        reason: CameraUnavailableReason::Evicted,
        attempts: 2,
    };
    assert_eq!(
        EventNote::of(&recovered),
        EventNote::Recovered {
            reason: CameraUnavailableReason::Evicted,
            attempts: 2
        }
    );
    let stream_configured = CameraEvent::StreamConfigured {
        stream: configured(FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 },
        }),
    };
    assert_eq!(
        EventNote::of(&stream_configured),
        EventNote::StreamConfigured {
            camera: "2".to_owned(),
            lever: "public_ae_target_range"
        }
    );
}

#[test]
fn command_and_failure_notes_carry_outcome_and_reason() {
    let handled = CameraEvent::CommandHandled {
        command: CommandId(7),
        name: "open",
        outcome: CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested),
    };
    match EventNote::of(&handled) {
        EventNote::Command { name, outcome } => {
            assert_eq!(name, "open");
            assert_eq!(outcome["kind"], "invalid_state");
        }
        other => panic!("ожидалась запись команды, получено {other:?}"),
    }
    let failed = CameraEvent::StreamConfigurationFailed {
        mode: StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::SixteenByNine,
            frame_rate: TargetFrameRate::new(60).expect("60"),
        },
        failure: ConfigureFailure::DeviceDisconnected,
    };
    match EventNote::of(&failed) {
        EventNote::StreamConfigurationFailed { failure } => assert!(!failure.is_empty()),
        other => panic!("ожидалась запись отказа, получено {other:?}"),
    }
}

#[test]
fn event_log_stamps_notes_with_time_since_its_start() {
    let start = Instant::now();
    let mut log = EventLog::started_at(start);
    assert_eq!(log.elapsed_ms(start), 0);
    log.absorb(
        start + Duration::from_millis(250),
        &[CameraEvent::Unavailable {
            reason: CameraUnavailableReason::Evicted,
        }],
    );
    log.absorb(start + Duration::from_millis(900), &[]);
    assert_eq!(log.notes().len(), 1, "пустая пачка ничего не записывает");
    assert_eq!(log.notes()[0].at_ms, 250);
    let json = serde_json::to_value(&log.notes()[0]).expect("json");
    assert_eq!(json["kind"], "unavailable");
    assert_eq!(json["reason"], "evicted");
    assert_eq!(json["at_ms"], 250);
}
