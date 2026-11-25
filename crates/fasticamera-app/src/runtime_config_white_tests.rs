//! Секция `camera.white_memory` конфига нативной части: умолчание — файл; JSON, который собирает Kotlin
//! (`NativeConfig.whiteMemory`, intent-параметр `white_memory`), включает режим `none`; опечатка отвергает запуск.

use super::{ConfigError, RuntimeConfig};
use camera_core::white::WhiteFollowSpeed;

use crate::white_memory::WhiteMemoryMode;

fn with_camera(section: &str) -> String {
    format!(r#"{{"build_type":"debug","log_filter":"info","camera":{section}}}"#)
}

#[test]
fn memory_is_a_file_by_default() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(config.camera().white_memory.mode, WhiteMemoryMode::File);
    assert_eq!(config.camera().white_memory.file_name, "white.json");
}

#[test]
fn the_json_kotlin_sends_for_the_smoke_extra_turns_the_memory_off() {
    // Ровно та форма, которую строит `NativeConfig.toJson()` при `-e white_memory none`.
    let config = RuntimeConfig::from_json(&with_camera(r#"{"white_memory":{"mode":"none"}}"#)).expect("конфиг");
    assert_eq!(config.camera().white_memory.mode, WhiteMemoryMode::None);
}

#[test]
fn an_unknown_mode_and_a_bad_file_name_reject_the_whole_config() {
    let error = RuntimeConfig::from_json(&with_camera(r#"{"white_memory":{"mode":"cloud"}}"#)).expect_err("режим");
    assert!(matches!(error, ConfigError::Malformed(_)), "{error}");

    let error = RuntimeConfig::from_json(&with_camera(r#"{"white_memory":{"file_name":"../x"}}"#)).expect_err("имя");
    assert!(matches!(error, ConfigError::Camera(_)), "{error}");
    assert!(error.to_string().contains("camera.white_memory.file_name"), "{error}");
}

// ---- кнопки света панели белого против пределов движка ----

#[test]
fn the_default_light_buttons_fit_the_default_engine_limits() {
    RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("умолчания сходятся");
}

#[test]
fn a_light_button_above_the_engine_limit_rejects_the_whole_config_and_names_the_button() {
    let config = r#"{"build_type":"debug","log_filter":"info",
        "camera_screen":{"screen":{"white":{"lights":[
            {"label":"Sun","icon":"sun","kelvin":5600.0,"duv":0.0},
            {"label":"Arctic","icon":"cloud","kelvin":15000.0,"duv":0.0}
        ]}}}}"#;
    let error = RuntimeConfig::from_json(config).expect_err("15000 К выше предела движка 12000 К");
    assert!(matches!(error, ConfigError::WhitePanelOutsideEngineRange(_)), "{error}");
    assert!(error.to_string().contains("Arctic"), "{error}");
}

#[test]
fn the_limits_checked_are_the_engines_so_widening_them_admits_the_same_button() {
    let config = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"engine":{"three_a":{"white":{"max_kelvin":16000.0}}}},
        "camera_screen":{"screen":{"white":{"lights":[
            {"label":"Arctic","icon":"cloud","kelvin":15000.0,"duv":0.0}
        ]}}}}"#;
    RuntimeConfig::from_json(config).expect("предел движка расширен: кнопка допустима");
}

#[test]
fn a_light_button_tint_beyond_the_engine_tint_limit_is_rejected_too() {
    let config = r#"{"build_type":"debug","log_filter":"info",
        "camera_screen":{"screen":{"white":{"lights":[
            {"label":"Green","icon":"tree","kelvin":5000.0,"duv":0.2}
        ]}}}}"#;
    let error = RuntimeConfig::from_json(config).expect_err("оттенок 0,2 Duv вне ±0,05");
    assert!(matches!(error, ConfigError::WhitePanelOutsideEngineRange(_)), "{error}");
    assert!(error.to_string().contains("Green"), "{error}");
}

#[test]
fn the_screen_scenario_must_tap_a_light_button_that_exists() {
    let config = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"screen":{"touch":{"white_light_index":9}}}}}"#;
    let error = RuntimeConfig::from_json(config).expect_err("кнопки с номером 9 нет");
    assert!(
        matches!(error, ConfigError::ScreenScenarioWhiteLightMissing { index: 9, .. }),
        "{error}"
    );
}

// ---- правило возобновления белого (WB36) ----

use camera_core::white::WhiteResumeRule;

#[test]
fn the_resume_rule_defaults_to_take_from_source_and_nobody_has_chosen_anything() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(
        config.camera().engine.three_a.white.resume_rule,
        WhiteResumeRule::TakeFromSource
    );
    assert_eq!(config.camera().white_resume_choice, None);
}

#[test]
fn the_json_kotlin_sends_for_the_launch_extra_carries_the_choice() {
    // Ровно та форма, которую строит `NativeConfig.toJson()` при `-e white_resume_rule keep_last`.
    let config = RuntimeConfig::from_json(&with_camera(r#"{"white_resume_choice":"keep_last"}"#)).expect("конфиг");
    assert_eq!(config.camera().white_resume_choice, Some(WhiteResumeRule::KeepLast));
}

#[test]
fn the_engine_default_rule_is_read_from_the_config_section() {
    let config = RuntimeConfig::from_json(&with_camera(
        r#"{"engine":{"three_a":{"white":{"resume_rule":"keep_last"}}}}"#,
    ))
    .expect("конфиг");
    assert_eq!(
        config.camera().engine.three_a.white.resume_rule,
        WhiteResumeRule::KeepLast
    );
}

#[test]
fn a_bad_rule_rejects_the_whole_config_and_the_error_names_the_field() {
    let engine_default = RuntimeConfig::from_json(&with_camera(
        r#"{"engine":{"three_a":{"white":{"resume_rule":"sometimes"}}}}"#,
    ))
    .expect_err("значение в конфиге движка");
    assert!(matches!(engine_default, ConfigError::Malformed(_)), "{engine_default}");
    assert!(
        engine_default.to_string().contains("three_a.white.resume_rule"),
        "{engine_default}"
    );

    let launch_choice = RuntimeConfig::from_json(&with_camera(r#"{"white_resume_choice":"sometimes"}"#))
        .expect_err("значение параметра запуска");
    assert!(matches!(launch_choice, ConfigError::Malformed(_)), "{launch_choice}");
    assert!(
        launch_choice.to_string().contains("camera.white_resume_choice"),
        "{launch_choice}"
    );
    assert!(launch_choice.to_string().contains("sometimes"), "{launch_choice}");
}

// ---- скорость слежения за белым: параметр запуска `white_follow_ms` ----

#[test]
fn no_follow_speed_choice_by_default() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(config.camera().white_follow_speed_choice, None);
}

#[test]
fn the_json_kotlin_sends_for_the_follow_extra_carries_the_speed_as_a_number() {
    // Ровно та форма, которую строит `NativeConfig.toJson()` при `-e white_follow_ms 450`: число, не строка.
    let config = RuntimeConfig::from_json(&with_camera(r#"{"white_follow_speed_choice":450}"#)).expect("конфиг");
    assert_eq!(
        config.camera().white_follow_speed_choice,
        Some(WhiteFollowSpeed::from_millis(450).expect("скорость"))
    );
    let instant = RuntimeConfig::from_json(&with_camera(r#"{"white_follow_speed_choice":0}"#)).expect("конфиг");
    assert_eq!(
        instant.camera().white_follow_speed_choice.map(WhiteFollowSpeed::millis),
        Some(0),
        "ноль — «сразу», а не «не задано»"
    );
}

#[test]
fn a_bad_follow_speed_rejects_the_whole_config_and_the_error_names_the_field() {
    // Вне предела; дробь и отрицательное (Kotlin шлёт их строкой как есть); не число вовсе.
    for bad in ["10001", "-3", r#""1.5""#, r#""-3""#, r#""fast""#] {
        let error = RuntimeConfig::from_json(&with_camera(&format!(r#"{{"white_follow_speed_choice":{bad}}}"#)))
            .expect_err("значение параметра запуска");
        assert!(matches!(error, ConfigError::Malformed(_)), "{bad}: {error}");
        assert!(
            error.to_string().contains("camera.white_follow_speed_choice"),
            "{bad}: {error}"
        );
    }
    let out_of_range =
        RuntimeConfig::from_json(&with_camera(r#"{"white_follow_speed_choice":10001}"#)).expect_err("вне предела");
    assert!(out_of_range.to_string().contains("10001"), "{out_of_range}");
}
