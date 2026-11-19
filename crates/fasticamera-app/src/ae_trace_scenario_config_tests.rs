//! Проверки секции `camera.scenarios.ae_trace`: умолчания, разбор JSON, каждое правило `validate`.

use super::*;

fn parse(json: &str) -> Result<AeTraceScenarioConfig, serde_json::Error> {
    serde_json::from_str(json)
}

#[test]
fn defaults_are_valid_and_match_the_acceptance_plan() {
    let config = AeTraceScenarioConfig::default();
    assert_eq!(config.validate(), Ok(()));
    // Шаг 5 плана 08c: 36 + 36 + 20 секунд, подсказка каждые 6 с.
    assert_eq!(config.total_ms(), 92_000);
    assert_eq!(config.cue_interval_ms, 6_000);
}

#[test]
fn partial_json_keeps_the_other_defaults_and_unknown_fields_are_rejected() {
    let config = parse(r#"{"photo_phase_ms": 12000}"#).expect("разбор");
    assert_eq!(config.photo_phase_ms, 12_000);
    assert_eq!(config.video_phase_ms, AeTraceScenarioConfig::default().video_phase_ms);
    assert!(parse(r#"{"no_such_field": 1}"#).is_err());
}

#[test]
fn zero_values_name_the_offending_field() {
    let cases: [(&'static str, AeTraceScenarioConfig); 4] = [
        (
            "photo_phase_ms",
            AeTraceScenarioConfig {
                photo_phase_ms: 0,
                ..AeTraceScenarioConfig::default()
            },
        ),
        (
            "cue_interval_ms",
            AeTraceScenarioConfig {
                cue_interval_ms: 0,
                ..AeTraceScenarioConfig::default()
            },
        ),
        (
            "sample_interval_ms",
            AeTraceScenarioConfig {
                sample_interval_ms: 0,
                ..AeTraceScenarioConfig::default()
            },
        ),
        (
            "detail_decimation_ms",
            AeTraceScenarioConfig {
                detail_decimation_ms: 0,
                ..AeTraceScenarioConfig::default()
            },
        ),
    ];
    for (field, config) in cases {
        assert_eq!(config.validate(), Err(AeTraceScenarioConfigError::Zero { field }));
    }
}

#[test]
fn non_finite_and_non_positive_thresholds_are_rejected() {
    let not_a_number = AeTraceScenarioConfig {
        min_step_ev: f64::NAN,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        not_a_number.validate(),
        Err(AeTraceScenarioConfigError::NotPositive {
            field: "min_step_ev",
            ..
        })
    ));
    let negative = AeTraceScenarioConfig {
        flicker_tolerance_ms: -0.05,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        negative.validate(),
        Err(AeTraceScenarioConfigError::NotPositive {
            field: "flicker_tolerance_ms",
            ..
        })
    ));
}

#[test]
fn schedule_rules_are_checked() {
    let short_interval = AeTraceScenarioConfig {
        cue_interval_ms: 2_000,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        short_interval.validate(),
        Err(AeTraceScenarioConfigError::CueIntervalTooShort { .. })
    ));
    let short_phase = AeTraceScenarioConfig {
        video_phase_ms: 5_000,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        short_phase.validate(),
        Err(AeTraceScenarioConfigError::PhaseShorterThanCue {
            field: "video_phase_ms",
            ..
        })
    ));
    let many_cues = AeTraceScenarioConfig {
        photo_phase_ms: 100_000,
        series_capacity: 100_000,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        many_cues.validate(),
        Err(AeTraceScenarioConfigError::TooManyCues { .. })
    ));
    let swallowed = AeTraceScenarioConfig {
        static_warmup_ms: 20_000,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        swallowed.validate(),
        Err(AeTraceScenarioConfigError::WarmupSwallowsStatic { .. })
    ));
}

#[test]
fn series_must_fit_the_schedule_and_the_window_must_hold_enough_samples() {
    let small_capacity = AeTraceScenarioConfig {
        series_capacity: 100,
        ..AeTraceScenarioConfig::default()
    };
    assert_eq!(
        small_capacity.validate(),
        Err(AeTraceScenarioConfigError::CapacityTooSmall {
            capacity: 100,
            needed: 1_841
        })
    );
    let thin_window = AeTraceScenarioConfig {
        sample_interval_ms: 400,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        thin_window.validate(),
        Err(AeTraceScenarioConfigError::WindowTooThin { .. })
    ));
}
