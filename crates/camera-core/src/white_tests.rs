//! Тесты границы белой: конфиг и запасной свет.

use super::*;
use crate::config::{CameraEngineConfig, CameraEngineConfigError};

fn field_of(error: WhiteConfigError) -> &'static str {
    let WhiteConfigError::Invalid { field, .. } = error;
    field
}

#[test]
fn default_config_is_valid_and_matches_owner_decisions() {
    let config = WhiteConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(
        (
            config.min_kelvin,
            config.max_kelvin,
            config.max_abs_duv,
            config.fallback_kelvin,
            config.fallback_duv,
            config.answer_timeout_ms,
            config.late_transition_ms
        ),
        (2_000.0, 12_000.0, 0.05, 5_600.0, 0.0, 3_000, 300)
    );
    let fallback = config
        .fallback_setting()
        .expect("запасной свет по умолчанию собирается");
    assert_eq!(fallback.temperature.kelvin(), 5_600.0);
    assert_eq!(fallback.tint.duv(), 0.0);
}

#[test]
fn every_bad_field_is_named_in_the_error() {
    let cases: [(&str, WhiteConfig); 9] = [
        (
            "min_kelvin",
            WhiteConfig {
                min_kelvin: 1_000.0,
                ..WhiteConfig::default()
            },
        ),
        (
            "max_kelvin",
            WhiteConfig {
                max_kelvin: 1_000.0,
                ..WhiteConfig::default()
            },
        ),
        (
            "max_abs_duv",
            WhiteConfig {
                max_abs_duv: f64::NAN,
                ..WhiteConfig::default()
            },
        ),
        (
            "fallback_kelvin",
            WhiteConfig {
                fallback_kelvin: 1_500.0,
                ..WhiteConfig::default()
            },
        ),
        (
            "fallback_kelvin",
            WhiteConfig {
                fallback_kelvin: 13_000.0,
                ..WhiteConfig::default()
            },
        ),
        (
            "fallback_duv",
            WhiteConfig {
                fallback_duv: 0.06,
                ..WhiteConfig::default()
            },
        ),
        (
            "answer_timeout_ms",
            WhiteConfig {
                answer_timeout_ms: 0,
                ..WhiteConfig::default()
            },
        ),
        (
            "answer_timeout_ms",
            WhiteConfig {
                answer_timeout_ms: WhiteConfig::MAX_WAIT_MS + 1,
                ..WhiteConfig::default()
            },
        ),
        (
            "late_transition_ms",
            WhiteConfig {
                late_transition_ms: WhiteConfig::MAX_TRANSITION_MS + 1,
                ..WhiteConfig::default()
            },
        ),
    ];
    for (expected_field, config) in cases {
        let error = config.validate().expect_err("конфиг с ошибкой отвергается");
        assert_eq!(field_of(error.clone()), expected_field, "{config:?}");
        assert!(error.to_string().contains(expected_field), "текст ошибки называет поле");
    }
}

#[test]
fn engine_config_rejects_bad_white_section_with_field_name() {
    let mut config = CameraEngineConfig::default();
    config.three_a.white.fallback_kelvin = 20_000.0;
    let error = config.validate().expect_err("запасной свет вне пределов");
    assert!(matches!(error, CameraEngineConfigError::InvalidWhite(_)));
    assert!(error.to_string().contains("three_a.white.fallback_kelvin"), "{error}");
}

#[test]
fn white_config_comes_from_json_and_rejects_unknown_fields() {
    let parsed: WhiteConfig = serde_json::from_str(r#"{"answer_timeout_ms": 1500}"#).expect("частичный JSON");
    assert_eq!(parsed.answer_timeout_ms, 1_500);
    assert_eq!(parsed.max_kelvin, 12_000.0, "остальное по умолчанию");
    assert!(serde_json::from_str::<WhiteConfig>(r#"{"answer_timeout":1}"#).is_err());
}

#[test]
fn unvalidated_config_still_yields_a_daylight_fallback() {
    let broken = WhiteConfig {
        fallback_kelvin: f64::NAN,
        ..WhiteConfig::default()
    };
    let setting = broken.fallback_setting_or_daylight();
    assert_eq!(setting.temperature.kelvin(), 5_600.0);
    assert_eq!(setting.tint.duv(), 0.0);
}

#[test]
fn range_follows_the_config_and_not_the_built_in_defaults() {
    let config = WhiteConfig {
        min_kelvin: 2_500.0,
        max_kelvin: 9_500.0,
        max_abs_duv: 0.03,
        ..WhiteConfig::default()
    };
    assert_eq!(config.validate(), Ok(()));
    let range = config.range();
    assert_eq!(range.min_temperature.kelvin(), 2_500.0);
    assert_eq!(range.max_temperature.kelvin(), 9_500.0);
    assert_eq!(range.max_abs_tint.duv(), 0.03);
    assert_eq!(WhiteStatus::initial().range, WhiteConfig::default().range());
}

// ---- правило возобновления (WB36) ----

#[test]
fn the_resume_rule_defaults_to_take_from_source_and_reads_from_a_plain_string() {
    assert_eq!(WhiteConfig::default().resume_rule, WhiteResumeRule::TakeFromSource);
    let parsed: WhiteConfig = serde_json::from_str(r#"{"resume_rule":"keep_last"}"#).expect("keep_last разбирается");
    assert_eq!(parsed.resume_rule, WhiteResumeRule::KeepLast);
    assert_eq!(parsed.answer_timeout_ms, 3_000, "остальное по умолчанию");
    let written = serde_json::to_string(&WhiteResumeRule::TakeFromSource).expect("запись");
    assert_eq!(written, r#""take_from_source""#);
}

#[test]
fn a_bad_resume_rule_in_the_config_is_rejected_with_the_field_name() {
    let error = serde_json::from_str::<WhiteConfig>(r#"{"resume_rule":"keep_everything"}"#).expect_err("значение");
    let text = error.to_string();
    assert!(text.contains("three_a.white.resume_rule"), "{text}");
    assert!(text.contains("keep_everything"), "{text}");

    // Тот же отказ внутри конфига движка целиком.
    let error = serde_json::from_str::<CameraEngineConfig>(r#"{"three_a":{"white":{"resume_rule":"never"}}}"#)
        .expect_err("вложенное значение");
    assert!(error.to_string().contains("three_a.white.resume_rule"), "{error}");
}

#[test]
fn resume_decision_table_rule_by_origin() {
    use camera_contract::white::{WhiteOrigin, WhiteSourceId};
    let phone = WhiteOrigin::Source(WhiteSourceId::PHONE);
    // (правило, происхождение белой, спрашивать ли источник при возобновлении)
    let table = [
        (WhiteResumeRule::TakeFromSource, phone, true),
        (WhiteResumeRule::TakeFromSource, WhiteOrigin::Fallback, true),
        (WhiteResumeRule::TakeFromSource, WhiteOrigin::Manual, false),
        (WhiteResumeRule::KeepLast, phone, false),
        (WhiteResumeRule::KeepLast, WhiteOrigin::Fallback, true),
        (WhiteResumeRule::KeepLast, WhiteOrigin::Manual, false),
    ];
    for (rule, origin, expected) in table {
        assert_eq!(rule.asks_source_on_resume(origin), expected, "{rule:?} при {origin:?}");
    }
}
