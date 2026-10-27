//! Секция `camera.scenarios.power`: умолчания решения 45, JSON, отказ неверных значений.

use super::*;
use crate::runtime_config::{ConfigError, RuntimeConfig};

fn parse(section: &str) -> Result<RuntimeConfig, ConfigError> {
    RuntimeConfig::from_json(&format!(
        r#"{{"build_type":"debug","log_filter":"info","camera":{{"scenarios":{{"power":{section}}}}}}}"#
    ))
}

#[test]
fn defaults_follow_decision_45_and_resolve_to_typed_settings() {
    let settings = PowerMeasurementConfig::default().settings().expect("умолчания верны");
    assert_eq!(settings.ab.window, Duration::from_secs(60));
    assert_eq!(settings.ab.pairs, 3);
    assert_eq!(settings.ab.warmup, Duration::from_secs(10));
    assert_eq!(settings.sampling.period.duration(), Duration::from_millis(250));
    assert_eq!(settings.sampling.ring_capacity.get(), 512);
    assert_eq!(settings.endurance.duration, Duration::from_secs(600));
    assert_eq!(settings.endurance.snapshot_interval, Duration::from_secs(60));
    assert!(
        matches!(settings.sign, SignPolicy::Detect { rule } if rule.required.get() == 20 && rule.agreement.fraction() == 0.95)
    );
}

#[test]
fn json_overrides_partially_fixes_the_sign_and_rejects_typos() {
    let config = parse(r#"{"discharge_sign":"positive_when_discharging","ab":{"pairs":4}}"#).expect("секция");
    let power = &config.camera().scenarios.power;
    assert_eq!(power.ab.pairs, 4);
    assert_eq!(power.ab.window_ms, 60_000, "несказанное — по умолчанию");
    assert_eq!(
        power.settings().expect("верно").sign,
        SignPolicy::Fixed(DischargeSign::PositiveWhenDischarging)
    );
    for typo in [
        r#"{"window_ms":1}"#,
        r#"{"ab":{"pair":3}}"#,
        r#"{"discharge_sign":"guess"}"#,
    ] {
        assert!(matches!(parse(typo), Err(ConfigError::Malformed(_))), "{typo}");
    }
}

#[test]
fn invalid_values_are_named() {
    let with = |change: fn(&mut PowerMeasurementConfig)| {
        let mut config = PowerMeasurementConfig::default();
        change(&mut config);
        config.settings().expect_err("должно быть отвергнуто")
    };
    assert_eq!(
        with(|config| config.ab.pairs = 2),
        PowerMeasurementConfigError::TooFewPairs { pairs: 2, minimum: 3 }
    );
    assert_eq!(
        with(|config| config.ab.warmup_ms = 60_000),
        PowerMeasurementConfigError::WarmupNotShorterThanWindow {
            warmup_ms: 60_000,
            window_ms: 60_000
        }
    );
    assert_eq!(
        with(|config| config.sampling_period_ms = 0),
        PowerMeasurementConfigError::Zero {
            field: "sampling_period_ms"
        }
    );
    assert!(matches!(
        with(|config| config.max_missing_fraction = f64::NAN),
        PowerMeasurementConfigError::MissingFractionOutOfRange { .. }
    ));
    assert!(matches!(
        with(|config| config.endurance.snapshot_interval_ms = 700_000),
        PowerMeasurementConfigError::IntervalLongerThanRun { .. }
    ));
    let mut camera = crate::camera_scenario_config::CameraScenarioConfig::default();
    camera.power.ab.pairs = 1;
    assert!(
        camera
            .validate()
            .expect_err("секция камеры")
            .to_string()
            .contains("power.ab.pairs"),
        "ошибка всплывает в проверке секции камеры"
    );
}

#[test]
fn sign_agreement_and_plausible_current_range_are_validated() {
    let with = |change: fn(&mut PowerMeasurementConfig)| {
        let mut config = PowerMeasurementConfig::default();
        change(&mut config);
        config.settings().expect_err("должно быть отвергнуто")
    };
    assert_eq!(
        with(|config| config.sign_agreement_fraction = 0.5),
        PowerMeasurementConfigError::SignAgreementOutOfRange { value: 0.5 },
        "ничья не задаёт знак"
    );
    assert_eq!(
        with(|config| config.plausible_current_min_ma = 0),
        PowerMeasurementConfigError::PlausibleCurrentRangeInvalid {
            min_ma: 0,
            max_ma: 10_000
        }
    );
    assert_eq!(
        with(|config| config.plausible_current_max_ma = 10),
        PowerMeasurementConfigError::PlausibleCurrentRangeInvalid { min_ma: 10, max_ma: 10 }
    );
    let settings = PowerMeasurementConfig::default().settings().expect("умолчания");
    assert_eq!(settings.current_range.min_microamps(), 10_000);
    assert_eq!(settings.current_range.max_microamps(), 10_000_000);
}

#[test]
fn current_unit_defaults_to_milliamps_of_this_phone_and_reaches_the_sampling_request() {
    // OnePlus 11 отдаёт CURRENT_NOW в мА (факт 07): без этого умолчания таблица мА отвергалась бы целиком.
    let settings = PowerMeasurementConfig::default()
        .settings()
        .expect("умолчания корректны");
    assert_eq!(settings.sampling.current_now_unit, CurrentNowUnit::Milliamps);
    // Устройство «по документации» включается конфигом.
    let documented: PowerMeasurementConfig =
        serde_json::from_str(r#"{"current_now_unit":"microamps"}"#).expect("единица из конфига");
    assert_eq!(
        documented.settings().expect("корректно").sampling.current_now_unit,
        CurrentNowUnit::Microamps
    );
}
