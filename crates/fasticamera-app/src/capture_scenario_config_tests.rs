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

#[test]
fn frame_thresholds_must_be_sane_and_the_refusal_cap_must_leave_room_for_a_saved_shot() {
    let cases: [(&str, Breaker); 4] = [
        ("share above one", |config| config.min_exact_frame_share = 1.5),
        ("share is nan", |config| config.min_exact_frame_share = f64::NAN),
        ("zero delay limit", |config| config.max_press_to_frame_ms = 0.0),
        ("infinite delay limit", |config| {
            config.max_press_to_frame_ms = f64::INFINITY
        }),
    ];
    for (name, break_it) in cases {
        let mut config = CaptureScenarioConfig::default();
        break_it(&mut config);
        assert!(
            matches!(config.validate(), Err(CaptureScenarioConfigError::OutOfRange { .. })),
            "{name}"
        );
    }
    let mut config = CaptureScenarioConfig::default();
    config.burst.max_refused_presses = config.burst.presses;
    assert_eq!(
        config.validate(),
        Err(CaptureScenarioConfigError::RefusalsCoverWholeBurst {
            refused: 10,
            presses: 10
        })
    );
}

#[test]
fn the_retired_min_saved_shots_field_is_rejected_instead_of_silently_ignored() {
    let error = serde_json::from_str::<CaptureScenarioConfig>(r#"{"burst":{"min_saved_shots":1}}"#).unwrap_err();
    assert!(error.to_string().contains("min_saved_shots"), "{error}");
}

#[test]
fn the_burst_drop_share_must_be_a_fraction() {
    // Решение 147: порог выброшенных кадров серии — доля 0..=1, а не счётчик.
    assert_eq!(CaptureScenarioConfig::default().burst.max_jitter_overflow_share, 0.15);
    for bad in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
        let mut config = CaptureScenarioConfig::default();
        config.burst.max_jitter_overflow_share = bad;
        assert!(
            matches!(
                config.validate(),
                Err(CaptureScenarioConfigError::OutOfRange {
                    field: "burst.max_jitter_overflow_share",
                    ..
                })
            ),
            "{bad}"
        );
    }
    let mut edge = CaptureScenarioConfig::default();
    edge.burst.max_jitter_overflow_share = 1.0;
    assert_eq!(edge.validate(), Ok(()));
}
