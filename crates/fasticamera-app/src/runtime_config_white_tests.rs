//! Секция `camera.white_memory` конфига нативной части: умолчание — файл; JSON, который собирает Kotlin
//! (`NativeConfig.whiteMemory`, intent-параметр `white_memory`), включает режим `none`; опечатка отвергает запуск.

use super::{ConfigError, RuntimeConfig};
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
