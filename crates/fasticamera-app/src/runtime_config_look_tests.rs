//! Секция `look_presets` конфига нативной части: умолчания, частичное переопределение, отказ при опечатке и при
//! неверном имени встроенного пресета (опечатка в конфиге отвергает запуск сразу, а не ломает пресеты молча).

use look_presets::PresetStoreConfig;

use super::{ConfigError, RuntimeConfig};

fn with_section(section: &str) -> String {
    format!(r#"{{"build_type":"debug","log_filter":"info","look_presets":{section}}}"#)
}

#[test]
fn the_section_is_optional_and_defaults_to_the_documented_values() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(config.look_presets(), &PresetStoreConfig::default());
}

#[test]
fn a_partial_override_changes_only_the_named_field() {
    let config = RuntimeConfig::from_json(&with_section(r#"{"file_name":"my-looks.json"}"#)).expect("конфиг");
    assert_eq!(config.look_presets().file_name, "my-looks.json");
    assert_eq!(
        config.look_presets().outcome_queue_capacity,
        PresetStoreConfig::default().outcome_queue_capacity
    );
}

#[test]
fn a_typo_in_the_section_rejects_the_whole_config() {
    let error = RuntimeConfig::from_json(&with_section(r#"{"file_nme":"x.json"}"#)).expect_err("опечатка");
    assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
    assert!(error.to_string().contains("file_nme"), "{error}");
}

#[test]
fn invalid_values_are_named_with_their_field() {
    let error = RuntimeConfig::from_json(&with_section(r#"{"file_name":"../escape.json"}"#)).expect_err("путь в имени");
    assert!(matches!(error, ConfigError::LookPresets(_)), "{error}");
    assert!(error.to_string().contains("escape.json"), "{error}");

    let error = RuntimeConfig::from_json(&with_section(r#"{"builtin_names":{"honest":"","as_eye":"As eye"}}"#))
        .expect_err("пустое имя встроенного");
    assert!(matches!(error, ConfigError::LookPresets(_)), "{error}");
}
