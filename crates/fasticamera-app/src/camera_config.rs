//! Секция `camera` конфига нативной части: движок камеры, бэкенд Camera2 и параметры сценариев.
//!
//! Секция необязательна: Kotlin её пока не присылает, действуют значения по умолчанию владельцев
//! (`CameraEngineConfig`, `AndroidCameraConfig`, `CameraScenarioConfig`). Прислать можно частично,
//! опечатка в имени поля — ошибка (`deny_unknown_fields` на каждом уровне).

use camera_android::AndroidCameraConfig;
use camera_core::CameraEngineConfig;
use camera_core::config::CameraEngineConfigError;
use serde::Deserialize;

use crate::camera_scenario_config::{CameraScenarioConfig, CameraScenarioConfigError};

/// Настройки камеры целиком.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraConfig {
    /// Движок камеры: восстановление, роли, учёт темпа, очереди (`camera-core`).
    pub engine: CameraEngineConfig,
    /// Бэкенд на Camera2 NDK (`camera-android`).
    pub android: AndroidCameraConfig,
    /// Параметры smoke-сценариев камеры.
    pub scenarios: CameraScenarioConfig,
}

/// Какая часть секции `camera` неверна.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CameraConfigError {
    #[error("camera.engine: {0}")]
    Engine(#[from] CameraEngineConfigError),
    #[error("camera.android: {0}")]
    Android(#[from] camera_android::ConfigError),
    #[error("camera.scenarios: {0}")]
    Scenarios(#[from] CameraScenarioConfigError),
}

impl CameraConfig {
    /// Проверяет все три части; первая найденная ошибка — в результате.
    pub fn validate(&self) -> Result<(), CameraConfigError> {
        self.engine.validate()?;
        self.android.validate()?;
        self.scenarios.validate()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "camera_config_tests.rs"]
mod tests;
