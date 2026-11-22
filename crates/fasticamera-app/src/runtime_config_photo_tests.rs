//! Тесты секций `device` и `photo` конфига нативной части: разбор, проверка при `initialize`, стык с экраном.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use super::*;

fn config_with(extra_sections: &str) -> String {
    format!(r#"{{"build_type":"debug","log_filter":"info"{extra_sections}}}"#)
}

#[test]
fn minimal_json_has_no_device_identity_and_default_photo_section() {
    let config = RuntimeConfig::from_json(&config_with("")).unwrap();
    assert_eq!(
        config.device().identity(),
        Err(DeviceIdentityError::Absent { field: "make" })
    );
    assert_eq!(config.photo(), &PhotoConfig::default());
}

#[test]
fn kotlin_device_section_becomes_identity() {
    let config = RuntimeConfig::from_json(&config_with(r#","device":{"make":"OnePlus","model":"CPH2449"}"#)).unwrap();
    let identity = config.device().identity().unwrap();
    assert_eq!((identity.make(), identity.model()), ("OnePlus", "CPH2449"));
}

#[test]
fn blank_device_value_rejects_the_whole_config() {
    let error = RuntimeConfig::from_json(&config_with(r#","device":{"make":" ","model":"CPH2449"}"#)).unwrap_err();
    assert!(matches!(error, ConfigError::Device(_)), "{error}");
}

#[test]
fn photo_section_values_reach_the_config_and_typos_are_rejected() {
    let config = RuntimeConfig::from_json(&config_with(r#","photo":{"service":{"jpeg_quality":70}}"#)).unwrap();
    assert_eq!(config.photo().service.jpeg_quality, 70);
    let typo = RuntimeConfig::from_json(&config_with(r#","photo":{"servise":{}}"#)).unwrap_err();
    assert!(matches!(typo, ConfigError::Malformed(_)), "{typo}");
    let bad_value = RuntimeConfig::from_json(&config_with(r#","photo":{"service":{"heif_quality":200}}"#)).unwrap_err();
    assert!(matches!(bad_value, ConfigError::Photo(_)), "{bad_value}");
}

#[test]
fn service_limit_below_screen_saving_limit_is_rejected_at_the_junction() {
    // Экран по умолчанию пускает в сохранение до 10 снимков; служба с пределом 3 их бы не приняла.
    let error = RuntimeConfig::from_json(&config_with(r#","photo":{"service":{"max_pending_stills":3}}"#)).unwrap_err();
    assert!(
        matches!(
            error,
            ConfigError::Photo(PhotoConfigError::ServiceLimitBelowScreenLimit { .. })
        ),
        "{error}"
    );
}

#[test]
fn screen_timeouts_not_above_service_timeouts_are_rejected_at_the_junction() {
    // Служба ждёт кадр 3 с, а экран сдаётся через 2 с: экран объявил бы потерянным ещё живой снимок.
    let capture =
        RuntimeConfig::from_json(&config_with(r#","photo":{"service":{"capture_timeout_ms":3000}}"#)).unwrap_err();
    assert!(
        matches!(
            capture,
            ConfigError::Photo(PhotoConfigError::ScreenCaptureTimeoutNotAboveService { .. })
        ),
        "{capture}"
    );
    let saving =
        RuntimeConfig::from_json(&config_with(r#","photo":{"service":{"develop_timeout_ms":60000}}"#)).unwrap_err();
    assert!(
        matches!(
            saving,
            ConfigError::Photo(PhotoConfigError::ScreenSavingTimeoutNotAboveService { .. })
        ),
        "{saving}"
    );
}

#[test]
fn render_target_timeout_is_checked_against_the_service_queue_at_the_junction() {
    // Рендер ждёт цель 10 с, а десятый снимок очереди ждёт её ≈ 18 с: конфиг отвергается целиком.
    let error =
        RuntimeConfig::from_json(&config_with(r#","render":{"still":{"target_timeout_ms":10000}}"#)).unwrap_err();
    assert!(
        matches!(
            error,
            ConfigError::Photo(PhotoConfigError::RenderTargetTimeoutBelowQueueWait { .. })
        ),
        "{error}"
    );
}
