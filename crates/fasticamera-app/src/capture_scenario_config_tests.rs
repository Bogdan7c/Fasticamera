//! Секция `camera.scenarios.capture`: умолчания верны, каждое неверное значение отвергается с названием поля.

use super::*;

#[test]
fn defaults_are_valid_and_describe_the_planned_burst_of_ten_at_a_hundred_milliseconds() {
    let config = CaptureScenarioConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!((config.burst.presses, config.burst.interval_ms), (10, 100));
    assert_eq!(
        (config.full_frame.width_px, config.full_frame.height_px),
        (4_096, 3_072)
    );
}

/// Как «сломать» конфиг одним полем.
type Breaker = fn(&mut CaptureScenarioConfig);

#[test]
fn every_zero_names_its_field() {
    let cases: [(&str, Breaker); 5] = [
        ("ready_timeout_ms", |config| config.ready_timeout_ms = 0),
        ("saved_timeout_ms", |config| config.saved_timeout_ms = 0),
        ("burst.presses", |config| config.burst.presses = 0),
        ("burst.interval_ms", |config| config.burst.interval_ms = 0),
        ("full_frame.height_px", |config| config.full_frame.height_px = 0),
    ];
    for (field, break_it) in cases {
        let mut config = CaptureScenarioConfig::default();
        break_it(&mut config);
        assert_eq!(
            config.validate(),
            Err(CaptureScenarioConfigError::Zero { field }),
            "{field}"
        );
    }
}

#[test]
fn the_capture_wait_cannot_be_longer_than_the_saving_wait() {
    let config = CaptureScenarioConfig {
        secured_timeout_ms: 30_000,
        saved_timeout_ms: 20_000,
        ..CaptureScenarioConfig::default()
    };
    assert_eq!(
        config.validate(),
        Err(CaptureScenarioConfigError::SecuredAfterSaved {
            secured: 30_000,
            saved: 20_000
        })
    );
}

#[test]
fn unknown_fields_are_rejected_when_parsing_json() {
    let error = serde_json::from_str::<CaptureScenarioConfig>(r#"{"ready_timeout":1}"#).unwrap_err();
    assert!(error.to_string().contains("ready_timeout"), "{error}");
    let parsed: CaptureScenarioConfig = serde_json::from_str(r#"{"burst":{"presses":3}}"#).unwrap();
    assert_eq!(parsed.burst.presses, 3);
    assert_eq!(parsed.burst.interval_ms, 100, "незаданное берётся из умолчаний");
}
