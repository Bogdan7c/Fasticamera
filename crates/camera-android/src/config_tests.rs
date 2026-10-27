use camera_contract::fixtures::camera_id;

use super::*;

#[test]
fn defaults_are_valid_and_allow_camx_only_on_the_main_camera() {
    let config = AndroidCameraConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert!(config.allows_vendor_frame_duration_override(&camera_id("2")));
    // Ширик (3), фронт (1) и логическая (0): ключи CamX запрещены (00, попытка 17).
    for forbidden in ["0", "1", "3"] {
        assert!(
            !config.allows_vendor_frame_duration_override(&camera_id(forbidden)),
            "камера {forbidden}"
        );
    }
}

#[test]
fn json_overrides_defaults_field_by_field_and_unknown_field_is_rejected() {
    let parsed: AndroidCameraConfig = serde_json::from_str(
        r#"{"retired_reader_drain_timeout_ms": 900, "vendor_frame_duration_override_cameras": ["2", "7"]}"#,
    )
    .expect("корректный JSON");
    assert_eq!(parsed.retired_reader_drain_timeout_ms, 900);
    assert_eq!(parsed.vendor_frame_duration_override_cameras, vec!["2", "7"]);
    assert_eq!(
        parsed.frame_number_window,
        AndroidCameraConfig::default().frame_number_window
    );
    assert!(parsed.allows_vendor_frame_duration_override(&camera_id("7")));

    let error = serde_json::from_str::<AndroidCameraConfig>(r#"{"raw_reader_max_images": 6}"#).unwrap_err();
    assert!(
        error.to_string().contains("raw_reader_max_images"),
        "снятое поле (его заменил план пула потока) не молчит: {error}"
    );
}

#[test]
fn validate_rejects_each_out_of_range_value() {
    let broken = |change: fn(&mut AndroidCameraConfig)| {
        let mut config = AndroidCameraConfig::default();
        change(&mut config);
        config.validate().unwrap_err()
    };
    assert_eq!(
        broken(|c| c.retired_reader_drain_timeout_ms = 0),
        ConfigError::RetiredReaderTimeoutOutOfRange { actual: 0, max: 60_000 }
    );
    assert_eq!(
        broken(|c| c.retired_reader_drain_timeout_ms = 60_001),
        ConfigError::RetiredReaderTimeoutOutOfRange {
            actual: 60_001,
            max: 60_000
        }
    );
    assert_eq!(
        broken(|c| c.retired_reader_poll_interval_ms = 0),
        ConfigError::RetiredReaderPollOutOfRange { actual: 0, max: 1_000 }
    );
    assert_eq!(
        broken(|c| c.max_public_range_frame_rate = 0),
        ConfigError::PublicRangeFrameRateOutOfRange { actual: 0, max: 480 }
    );
    assert_eq!(
        broken(|c| c.frame_number_window = 0),
        ConfigError::FrameNumberWindowOutOfRange { actual: 0, max: 1024 }
    );
    assert_eq!(
        broken(|c| c.failure_history_capacity = 2000),
        ConfigError::FailureHistoryOutOfRange {
            actual: 2000,
            max: 1024
        }
    );
    assert_eq!(
        broken(|c| c.vendor_frame_duration_override_cameras = vec![String::new()]),
        ConfigError::EmptyOverrideCameraId(EmptyCameraId)
    );
}

#[test]
fn retired_reader_durations_follow_the_millisecond_fields() {
    let config = AndroidCameraConfig::default();
    assert_eq!(
        config.retired_reader_drain_timeout(),
        std::time::Duration::from_millis(500)
    );
    assert_eq!(
        config.retired_reader_poll_interval(),
        std::time::Duration::from_millis(5)
    );
}
