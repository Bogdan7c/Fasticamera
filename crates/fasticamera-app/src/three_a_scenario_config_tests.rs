use super::*;
use crate::runtime_config::RuntimeConfig;

fn config_with(three_a: &str) -> Result<RuntimeConfig, crate::runtime_config::ConfigError> {
    RuntimeConfig::from_json(&format!(
        r#"{{"build_type":"debug","log_filter":"info","camera":{{"scenarios":{{"three_a":{three_a}}}}}}}"#
    ))
}

#[test]
fn defaults_are_valid_and_match_the_documented_plan() {
    let config = ThreeAScenarioConfig::default();
    config.validate().expect("умолчания верны");
    assert_eq!(config.tap.points.len(), 5, "центр и четыре угла");
    assert_eq!(config.manual.shutter_ns, 1_000_000, "1/1000 с");
    assert_eq!(config.manual.long_shutter_ns, 100_000_000, "1/10 с");
    assert_eq!(
        config.bracket.exposures_ns,
        [2_000_000, 4_000_000, 8_000_000, 16_000_000]
    );
    assert_eq!(config.ev.biases_ev, [0.0, 1.0, -1.0]);
}

#[test]
fn section_arrives_through_runtime_config_with_partial_override_and_defaults() {
    let runtime = config_with(r#"{"manual":{"iso":800},"lock":{"hold_ms":500}}"#).expect("частичная секция");
    let three_a = &runtime.camera().scenarios.three_a;
    assert_eq!(three_a.manual.iso, 800);
    assert_eq!(three_a.lock.hold_ms, 500);
    assert_eq!(three_a.manual.shutter_ns, 1_000_000, "несказанное — по умолчанию");
    assert_eq!(three_a.bracket, BracketStepConfig::default());
}

#[test]
fn unknown_key_at_any_level_is_rejected() {
    assert!(config_with(r#"{"manul":{}}"#).is_err());
    assert!(config_with(r#"{"manual":{"isso":1}}"#).is_err());
    assert!(config_with(r#"{"tap":{"points":[{"x":0.5,"y":0.5,"z":1}]}}"#).is_err());
}

#[test]
fn invalid_values_are_reported_by_name() {
    let reject = |edit: fn(&mut ThreeAScenarioConfig)| {
        let mut config = ThreeAScenarioConfig::default();
        edit(&mut config);
        config.validate().expect_err("значение должно отвергаться")
    };
    assert_eq!(
        reject(|c| c.manual.iso = 0),
        ThreeAScenarioConfigError::Zero { field: "manual.iso" }
    );
    assert!(matches!(
        reject(|c| c.manual.tolerance_fraction = f64::NAN),
        ThreeAScenarioConfigError::FractionOutOfRange { .. }
    ));
    assert!(matches!(
        reject(|c| c.ev.ratio_tolerance_fraction = 1.0),
        ThreeAScenarioConfigError::FractionOutOfRange { .. }
    ));
    assert_eq!(reject(|c| c.tap.points.clear()), ThreeAScenarioConfigError::NoTapPoints);
    assert!(matches!(
        reject(|c| c.tap.points[1].x = 1.5),
        ThreeAScenarioConfigError::TapPointOutOfFrame { index: 1, .. }
    ));
    assert_eq!(
        reject(|c| c.tap.points[0] = TapPointConfig { x: 0.2, y: 0.5 }),
        ThreeAScenarioConfigError::FirstTapPointNotCenter
    );
    assert_eq!(
        reject(|c| c.ev.biases_ev = vec![1.0, 0.0]),
        ThreeAScenarioConfigError::BadEvBiases
    );
    assert_eq!(
        reject(|c| c.ev.biases_ev = vec![0.0, 1.0, 1.0]),
        ThreeAScenarioConfigError::BadEvBiases
    );
    assert!(matches!(
        reject(|c| c.bracket.exposures_ns = vec![1; 5]),
        ThreeAScenarioConfigError::BadBracketLength { limit: 4 }
    ));
    // Отсчёт «приготовься»: по умолчанию 3 с шагом 1 с; шаг длиннее всего отсчёта отвергается.
    assert_eq!(
        reject(|c| {
            c.lock.prepare_countdown_ms = 500;
            c.lock.prepare_step_ms = 1_000;
        }),
        ThreeAScenarioConfigError::PrepareStepLongerThanCountdown {
            step_ms: 1_000,
            countdown_ms: 500
        }
    );
    assert_eq!(
        reject(|c| c.lock.prepare_step_ms = 0),
        ThreeAScenarioConfigError::Zero {
            field: "lock.prepare_step_ms"
        }
    );
    assert_eq!(
        reject(|c| c.manual.long_shutter_ns = c.manual.shutter_ns),
        ThreeAScenarioConfigError::LongShutterNotLonger
    );
    assert!(matches!(
        reject(|c| c.awb.warn_mired_vs_hal = -1.0),
        ThreeAScenarioConfigError::BadMiredThreshold { .. }
    ));
}

#[test]
fn invalid_section_is_rejected_by_the_whole_runtime_config() {
    let error = config_with(r#"{"lock":{"hold_ms":0}}"#).expect_err("нулевое удержание");
    assert!(error.to_string().contains("lock.hold_ms"), "{error}");
}

#[test]
fn lock_prepare_countdown_defaults_and_can_be_switched_off() {
    let lock = LockStepConfig::default();
    assert_eq!((lock.prepare_countdown_ms, lock.prepare_step_ms), (3_000, 1_000));
    let mut config = ThreeAScenarioConfig::default();
    config.lock.prepare_countdown_ms = 0;
    assert_eq!(
        config.validate(),
        Ok(()),
        "0 — без отсчёта, шаг при этом не проверяется"
    );
}
