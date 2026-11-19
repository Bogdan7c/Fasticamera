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

#[test]
fn defaults_countdown_ends_before_the_next_cue_and_leaves_room_for_the_act_prompt() {
    let config = AeTraceScenarioConfig::default();
    assert_eq!(config.cue_countdown_steps, 3);
    assert_eq!(config.cue_countdown_step_ms, 1_000);
    assert_eq!(config.cue_display_ms, 2_000);
    assert_eq!(config.countdown_ms(), 3_000);
    assert!(config.countdown_ms() + config.cue_display_ms < config.cue_interval_ms);
}

#[test]
fn countdown_that_does_not_fit_between_cues_is_rejected() {
    // 5 с отсчёта + 2 с надписи не помещаются в период 6 с.
    let too_long = AeTraceScenarioConfig {
        cue_countdown_steps: 5,
        ..AeTraceScenarioConfig::default()
    };
    assert_eq!(
        too_long.validate(),
        Err(AeTraceScenarioConfigError::CountdownTooLong {
            countdown_ms: 5_000,
            display_ms: 2_000,
            interval_ms: 6_000,
        })
    );
    // Длинная надпись подсказки тоже упирается в период.
    let long_display = AeTraceScenarioConfig {
        cue_display_ms: 3_000,
        ..AeTraceScenarioConfig::default()
    };
    assert!(matches!(
        long_display.validate(),
        Err(AeTraceScenarioConfigError::CountdownTooLong { .. })
    ));
}

#[test]
fn countdown_can_be_switched_off_but_its_step_and_display_cannot_be_zero() {
    let without_countdown = AeTraceScenarioConfig {
        cue_countdown_steps: 0,
        ..AeTraceScenarioConfig::default()
    };
    assert_eq!(without_countdown.validate(), Ok(()));
    assert_eq!(without_countdown.countdown_ms(), 0);

    let zero_step = AeTraceScenarioConfig {
        cue_countdown_step_ms: 0,
        ..AeTraceScenarioConfig::default()
    };
    assert_eq!(
        zero_step.validate(),
        Err(AeTraceScenarioConfigError::Zero {
            field: "cue_countdown_step_ms"
        })
    );
    let zero_display = AeTraceScenarioConfig {
        cue_display_ms: 0,
        ..AeTraceScenarioConfig::default()
    };
    assert_eq!(
        zero_display.validate(),
        Err(AeTraceScenarioConfigError::Zero {
            field: "cue_display_ms"
        })
    );
}
