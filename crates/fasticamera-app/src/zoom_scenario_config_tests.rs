//! Секция `camera.scenarios.zoom`: умолчания верны, каждое неверное значение отвергается с названием поля.

use super::*;

#[test]
fn defaults_are_valid_and_follow_the_planned_stop_sequence() {
    let config = ZoomScenarioConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(config.stop_ratios, vec![0.6, 1.0, 2.0, 1.0, 0.6]);
    assert!(config.flip_round_trip);
}

#[test]
fn zero_values_name_their_field() {
    let config = ZoomScenarioConfig {
        max_switch_ms: 0,
        ..ZoomScenarioConfig::default()
    };
    assert_eq!(
        config.validate(),
        Err(ZoomScenarioConfigError::Zero { field: "max_switch_ms" })
    );
}

#[test]
fn empty_nan_and_non_positive_stops_are_rejected() {
    let empty = ZoomScenarioConfig {
        stop_ratios: Vec::new(),
        ..ZoomScenarioConfig::default()
    };
    assert_eq!(empty.validate(), Err(ZoomScenarioConfigError::NoStops));
    for bad in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
        let config = ZoomScenarioConfig {
            stop_ratios: vec![1.0, bad],
            ..ZoomScenarioConfig::default()
        };
        assert!(
            matches!(config.validate(), Err(ZoomScenarioConfigError::InvalidStopRatio { .. })),
            "{bad} должно быть отвергнуто"
        );
    }
}

#[test]
fn tolerance_must_be_a_fraction_strictly_between_zero_and_one() {
    for bad in [0.0, 1.0, -0.1, f64::NAN] {
        let config = ZoomScenarioConfig {
            frame_rate_tolerance: bad,
            ..ZoomScenarioConfig::default()
        };
        assert!(matches!(
            config.validate(),
            Err(ZoomScenarioConfigError::ToleranceOutOfRange { .. })
        ));
    }
}

#[test]
fn a_wheel_drag_that_leaves_the_screen_is_rejected() {
    let config = ZoomScenarioConfig {
        wheel_drag: WheelDragPlan {
            step_dx: -0.05,
            ..WheelDragPlan::default()
        },
        ..ZoomScenarioConfig::default()
    };
    assert_eq!(
        config.validate(),
        Err(ZoomScenarioConfigError::WheelDrag {
            reason: "протяжка выходит за край экрана"
        })
    );
}

#[test]
fn unknown_fields_are_rejected_when_parsing_json() {
    let parsed: Result<ZoomScenarioConfig, _> = serde_json::from_str(r#"{"stop_ratios":[1.0],"typo":1}"#);
    assert!(parsed.is_err(), "опечатка в ключе не должна молча игнорироваться");
    let ok: ZoomScenarioConfig = serde_json::from_str(r#"{"stop_ratios":[1.0]}"#).expect("частичная секция");
    assert_eq!(ok.stop_ratios, vec![1.0]);
    assert_eq!(
        ok.step_settle_ms,
        ZoomScenarioConfig::default().step_settle_ms,
        "остальное — умолчания"
    );
}

#[test]
fn seamless_plan_rejects_zero_timeouts_and_non_finite_steps() {
    let zero_timeout = ZoomScenarioConfig {
        seamless: SeamlessPlan {
            release_timeout_ms: 0,
            ..SeamlessPlan::default()
        },
        ..ZoomScenarioConfig::default()
    };
    assert_eq!(
        zero_timeout.validate(),
        Err(ZoomScenarioConfigError::Seamless {
            reason: "release_timeout_ms должно быть больше нуля"
        })
    );
    for bad in [f64::NAN, 0.0, -0.1, 0.6] {
        let config = ZoomScenarioConfig {
            seamless: SeamlessPlan {
                pinch_step: bad,
                ..SeamlessPlan::default()
            },
            ..ZoomScenarioConfig::default()
        };
        assert!(
            matches!(config.validate(), Err(ZoomScenarioConfigError::Seamless { .. })),
            "pinch_step {bad} должен быть отвергнут"
        );
    }
    let zero_tolerance = ZoomScenarioConfig {
        seamless: SeamlessPlan {
            switch_ratio_tolerance_milli: 0,
            ..SeamlessPlan::default()
        },
        ..ZoomScenarioConfig::default()
    };
    assert!(zero_tolerance.validate().is_err());
}
