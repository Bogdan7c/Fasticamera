use super::*;

#[test]
fn defaults_are_valid_and_demand_55_fps_with_overshoot_above_the_selector_hysteresis() {
    let config = StandbyScenarioConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert!((config.min_back_fps - 55.0).abs() < f64::EPSILON);
    assert!(
        config.overshoot_ratio > 1.05,
        "выше гистерезиса селектора зума по умолчанию"
    );
}

#[test]
fn each_zero_timeout_is_rejected_by_name() {
    let broken = |change: fn(&mut StandbyScenarioConfig)| {
        let mut config = StandbyScenarioConfig::default();
        change(&mut config);
        config.validate().unwrap_err()
    };
    assert_eq!(
        broken(|c| c.home_timeout_ms = 0),
        StandbyScenarioConfigError::Zero {
            field: "home_timeout_ms"
        }
    );
    assert_eq!(
        broken(|c| c.alone_window_ms = 0),
        StandbyScenarioConfigError::Zero {
            field: "alone_window_ms"
        }
    );
    assert_eq!(
        broken(|c| c.warm_timeout_ms = 0),
        StandbyScenarioConfigError::Zero {
            field: "warm_timeout_ms"
        }
    );
    assert_eq!(
        broken(|c| c.both_window_ms = 0),
        StandbyScenarioConfigError::Zero {
            field: "both_window_ms"
        }
    );
    assert_eq!(
        broken(|c| c.swap_timeout_ms = 0),
        StandbyScenarioConfigError::Zero {
            field: "swap_timeout_ms"
        }
    );
    assert_eq!(
        broken(|c| c.release_timeout_ms = 0),
        StandbyScenarioConfigError::Zero {
            field: "release_timeout_ms"
        }
    );
}

#[test]
fn overshoot_and_fps_must_be_sane_numbers() {
    let mut config = StandbyScenarioConfig {
        overshoot_ratio: 1.0,
        ..StandbyScenarioConfig::default()
    };
    assert_eq!(
        config.validate(),
        Err(StandbyScenarioConfigError::InvalidOvershoot(1.0))
    );
    config.overshoot_ratio = f32::NAN;
    assert!(matches!(
        config.validate(),
        Err(StandbyScenarioConfigError::InvalidOvershoot(_))
    ));
    config = StandbyScenarioConfig {
        min_back_fps: 0.0,
        ..StandbyScenarioConfig::default()
    };
    assert_eq!(config.validate(), Err(StandbyScenarioConfigError::InvalidMinFps(0.0)));
}

#[test]
fn json_overrides_one_field_and_rejects_unknown_ones() {
    let parsed: StandbyScenarioConfig = serde_json::from_str(r#"{"both_window_ms": 3000}"#).expect("корректный JSON");
    assert_eq!(parsed.both_window_ms, 3000);
    assert_eq!(parsed.swap_timeout_ms, StandbyScenarioConfig::default().swap_timeout_ms);
    assert!(serde_json::from_str::<StandbyScenarioConfig>(r#"{"fps": 1}"#).is_err());
}
