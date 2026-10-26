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
    let parsed: AndroidCameraConfig =
        serde_json::from_str(r#"{"raw_reader_max_images": 6, "vendor_frame_duration_override_cameras": ["2", "7"]}"#)
            .expect("корректный JSON");
    assert_eq!(parsed.raw_reader_max_images, 6);
    assert_eq!(parsed.vendor_frame_duration_override_cameras, vec!["2", "7"]);
    assert_eq!(
        parsed.frame_number_window,
        AndroidCameraConfig::default().frame_number_window
    );
    assert!(parsed.allows_vendor_frame_duration_override(&camera_id("7")));

    let error = serde_json::from_str::<AndroidCameraConfig>(r#"{"raw_reader_max_image": 6}"#).unwrap_err();
    assert!(
        error.to_string().contains("raw_reader_max_image"),
        "опечатка в имени поля не молчит: {error}"
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
        broken(|c| c.raw_reader_max_images = 1),
        ConfigError::ReaderMaxImagesOutOfRange {
            actual: 1,
            min: 2,
            max: 16
        }
    );
    assert_eq!(
        broken(|c| c.raw_reader_max_images = 17),
        ConfigError::ReaderMaxImagesOutOfRange {
            actual: 17,
            min: 2,
            max: 16
        }
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
