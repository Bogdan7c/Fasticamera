//! Секция `camera`: умолчания, частичное переопределение, опечатки и неверные значения.

use super::*;
use crate::runtime_config::{ConfigError, RuntimeConfig};

fn config_with_camera(camera_section: &str) -> Result<RuntimeConfig, ConfigError> {
    RuntimeConfig::from_json(&format!(
        r#"{{"build_type":"debug","log_filter":"info","camera":{camera_section}}}"#
    ))
}

#[test]
fn absent_section_gives_the_documented_defaults_of_every_owner() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(*config.camera(), CameraConfig::default());
    assert_eq!(config.camera().engine, CameraEngineConfig::default());
    assert_eq!(config.camera().android, AndroidCameraConfig::default());
    assert_eq!(config.camera().scenarios.stream_steps.len(), 6);
    config.camera().validate().expect("умолчания согласованы");
}

#[test]
fn partial_section_overrides_only_named_fields() {
    let config = config_with_camera(
        r#"{"engine":{"recovery":{"max_consecutive_attempts":7}},"android":{"raw_reader_max_images":6},"scenarios":{"measure_ms":2000}}"#,
    )
    .expect("частичная секция");
    let camera = config.camera();
    assert_eq!(camera.engine.recovery.max_consecutive_attempts, 7);
    assert_eq!(
        camera.engine.recovery.retry_backoff_ms,
        CameraEngineConfig::default().recovery.retry_backoff_ms,
        "несказанное берёт значение по умолчанию"
    );
    assert_eq!(camera.android.raw_reader_max_images, 6);
    assert_eq!(camera.scenarios.measure_ms, 2000);
    assert_eq!(
        camera.scenarios.catalog_timeout_ms,
        CameraScenarioConfig::default().catalog_timeout_ms
    );
}

#[test]
fn typos_at_every_level_are_rejected() {
    for typo in [
        r#"{"engin":{}}"#,
        r#"{"engine":{"recovry":{}}}"#,
        r#"{"android":{"raw_reader_max_image":4}}"#,
        r#"{"scenarios":{"measure_sec":5}}"#,
        r#"{"scenarios":{"evict":{"rol":"wide"}}}"#,
    ] {
        let error = config_with_camera(typo).expect_err(typo);
        assert!(matches!(error, ConfigError::Malformed(_)), "{typo}: {error}");
    }
}

#[test]
fn bad_values_are_reported_by_the_part_that_owns_them() {
    let engine = config_with_camera(r#"{"engine":{"recovery":{"max_consecutive_attempts":0}}}"#).expect_err("engine");
    assert!(
        matches!(engine, ConfigError::Camera(CameraConfigError::Engine(_))),
        "{engine}"
    );
    let android = config_with_camera(r#"{"android":{"raw_reader_max_images":1}}"#).expect_err("android");
    assert!(
        matches!(android, ConfigError::Camera(CameraConfigError::Android(_))),
        "{android}"
    );
    let scenarios = config_with_camera(r#"{"scenarios":{"stream_steps":[]}}"#).expect_err("scenarios");
    assert!(
        matches!(
            scenarios,
            ConfigError::Camera(CameraConfigError::Scenarios(CameraScenarioConfigError::NoStreamSteps))
        ),
        "{scenarios}"
    );
    assert!(scenarios.to_string().contains("camera.scenarios"), "{scenarios}");
}

#[test]
fn unknown_enum_value_in_a_step_is_rejected_at_parse_time() {
    let error = config_with_camera(
        r#"{"scenarios":{"stream_steps":[{"camera_id":"2","raw_format":"raw12","readout":"four_by_three","frame_rate":60,"expected_lever":"any","min_measured_fps":55.0}]}}"#,
    )
    .expect_err("raw12");
    assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
}
