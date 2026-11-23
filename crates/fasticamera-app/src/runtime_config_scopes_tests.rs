//! Секция `render.viewfinder.scopes` конфига нативной части (14h): умолчания, переопределение, отказ при опечатке и
//! при недопустимых значениях (запуск отвергается сразу, приборы не ломаются молча).

use render_loop::config::ScopesConfig;

use super::{ConfigError, RuntimeConfig};

fn with_scopes(section: &str) -> String {
    format!(r#"{{"build_type":"debug","log_filter":"info","render":{{"viewfinder":{{"scopes":{section}}}}}}}"#)
}

#[test]
fn a_config_without_the_section_uses_the_documented_scope_defaults() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(config.render().viewfinder.scopes, ScopesConfig::default());
}

#[test]
fn a_custom_section_overrides_only_the_named_fields_and_keeps_the_rest() {
    let config =
        RuntimeConfig::from_json(&with_scopes(r#"{"every_nth_frame":3,"max_picture_side_px":1024}"#)).expect("конфиг");
    let scopes = &config.render().viewfinder.scopes;
    assert_eq!((scopes.every_nth_frame, scopes.max_picture_side_px), (3, 1024));
    assert_eq!(scopes.model, ScopesConfig::default().model, "остальное по умолчанию");
}

#[test]
fn a_typo_or_an_invalid_value_rejects_the_whole_config() {
    let error = RuntimeConfig::from_json(&with_scopes(r#"{"every_nth_fram":3}"#)).expect_err("опечатка");
    assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
    assert!(error.to_string().contains("every_nth_fram"), "{error}");

    let error = RuntimeConfig::from_json(&with_scopes(r#"{"every_nth_frame":0}"#)).expect_err("нулевой период");
    assert!(matches!(error, ConfigError::Render(_)), "{error}");
}
