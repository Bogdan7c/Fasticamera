//! Тесты секции `camera_screen`: умолчания, частичное переопределение, отказ на опечатке и неверных значениях —
//! через настоящий разбор конфига нативной части.

use super::*;
use crate::runtime_config::{ConfigError, RuntimeConfig};

fn parse(section: &str) -> Result<RuntimeConfig, ConfigError> {
    RuntimeConfig::from_json(&format!(
        r#"{{"build_type":"debug","log_filter":"info","camera_screen":{section}}}"#
    ))
}

#[test]
fn absent_section_takes_documented_defaults_and_validates() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(*config.camera_screen(), CameraScreenConfig::default());
    assert!(config.camera_screen().enabled, "экран включён по умолчанию");
    assert_eq!(config.camera_screen().validate(), Ok(()));
}

#[test]
fn partial_section_overrides_only_named_fields() {
    let config = parse(r#"{"enabled":false,"input_queue_capacity":8,"screen":{}}"#).expect("конфиг");
    assert!(!config.camera_screen().enabled);
    assert_eq!(config.camera_screen().input_queue_capacity, 8);
    assert_eq!(
        config.camera_screen().gravity_every_n_frames,
        CameraScreenConfig::default().gravity_every_n_frames
    );
}

#[test]
fn typos_are_rejected_instead_of_silently_ignored() {
    let error = parse(r#"{"enabld":false}"#).expect_err("опечатка в имени");
    assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
    let nested = parse(r#"{"screen":{"layuot":{}}}"#).expect_err("опечатка во вложенной секции");
    assert!(matches!(nested, ConfigError::Malformed(_)), "{nested}");
}

#[test]
fn invalid_values_reject_the_whole_config() {
    for (section, expected) in [
        (
            r#"{"input_queue_capacity":0}"#,
            CameraScreenConfigError::InputQueueCapacity { value: 0 },
        ),
        (
            r#"{"gravity_every_n_frames":0}"#,
            CameraScreenConfigError::GravityEveryNFrames,
        ),
        (
            r#"{"fake_capture_delay_ms":3600000}"#,
            CameraScreenConfigError::FakeCaptureDelay { value: 3_600_000 },
        ),
        (
            r#"{"fake_capture_journal_capacity":0}"#,
            CameraScreenConfigError::FakeCaptureJournalCapacity { value: 0 },
        ),
        (
            r#"{"reinstall":{"max_attempts":0}}"#,
            CameraScreenConfigError::ReinstallAttempts { value: 0 },
        ),
        (
            r#"{"reinstall":{"pause_ms":3600000}}"#,
            CameraScreenConfigError::ReinstallPause { value: 3_600_000 },
        ),
        (
            r#"{"reinstall":{"healthy_after_ms":0}}"#,
            CameraScreenConfigError::ReinstallHealthyAfterZero,
        ),
        (
            r#"{"report_min_interval_ms":3600000}"#,
            CameraScreenConfigError::ReportInterval { value: 3_600_000 },
        ),
    ] {
        let error = parse(section).expect_err(section);
        assert!(
            matches!(&error, ConfigError::CameraScreen(inner) if *inner == expected),
            "{section}: {error}"
        );
    }
}

#[test]
fn invalid_screen_numbers_are_reported_as_screen_errors() {
    let mut config = CameraScreenConfig::default();
    config.screen.exposure.step_ev = 0.0;
    assert!(
        matches!(config.validate(), Err(CameraScreenConfigError::Screen(_))),
        "{:?}",
        config.validate()
    );
}

#[test]
fn fake_capture_delay_converts_milliseconds() {
    let config = CameraScreenConfig {
        fake_capture_delay_ms: 250,
        ..CameraScreenConfig::default()
    };
    assert_eq!(config.fake_capture_delay(), Duration::from_millis(250));
}

#[test]
fn reinstall_section_is_partial_and_converts_to_durations() {
    let config = parse(r#"{"reinstall":{"pause_ms":120}}"#).expect("конфиг");
    let reinstall = config.camera_screen().reinstall;
    assert_eq!(reinstall.pause(), Duration::from_millis(120));
    assert_eq!(reinstall.max_attempts, ScreenReinstallConfig::default().max_attempts);
    assert_eq!(
        reinstall.healthy_after(),
        Duration::from_millis(ScreenReinstallConfig::default().healthy_after_ms)
    );
    let typo = parse(r#"{"reinstall":{"pausa_ms":1}}"#).expect_err("опечатка");
    assert!(matches!(typo, ConfigError::Malformed(_)), "{typo}");
}
