//! Секция `camera` конфига нативной части: движок камеры, бэкенд Camera2 и параметры сценариев.
//!
//! Секция необязательна: Kotlin её пока не присылает, действуют значения по умолчанию владельцев
//! (`CameraEngineConfig`, `AndroidCameraConfig`, `CameraScenarioConfig`). Прислать можно частично,
//! опечатка в имени поля — ошибка (`deny_unknown_fields` на каждом уровне).

use camera_android::AndroidCameraConfig;
use camera_core::CameraEngineConfig;
use camera_core::config::CameraEngineConfigError;
use raw_frame_stream::{RawFrameStreamConfig, RawFrameStreamConfigError};
use serde::Deserialize;

use crate::auto_exposure_link::ExposureControllerKind;
use crate::camera_scenario_config::{CameraScenarioConfig, CameraScenarioConfigError};
use crate::viewfinder_autostart::{ViewfinderAutostartConfig, ViewfinderAutostartConfigError};

/// Запас таблицы происхождений запросов сверх длины серии с брекетингом. Разовые записи серии
/// (кадры брекета, триггер фокуса) не должны вытеснять друг друга, пока их результаты не пришли:
/// вытесненная запись превращает результат в `Unknown` и ломает сбор серии (находка ревью 08).
/// 8 — серия плюс несколько тапов фокуса, которые могут лететь одновременно.
pub const REQUEST_ORIGIN_MARGIN: usize = 8;

/// Настройки камеры целиком.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraConfig {
    /// Движок камеры: восстановление, роли, учёт темпа, очереди (`camera-core`).
    pub engine: CameraEngineConfig,
    /// Бэкенд на Camera2 NDK (`camera-android`).
    pub android: AndroidCameraConfig,
    /// Поток RAW-кадров между камерой и рендером: спаривание кадра с метаданными, размер пула
    /// буферов, кольцо снимков (`raw-frame-stream`, решения 40–43 сессии 05).
    pub raw_stream: RawFrameStreamConfig,
    /// Параметры smoke-сценариев камеры.
    pub scenarios: CameraScenarioConfig,
    /// Временная (до сессии 12) политика: открыть живой видоискатель при старте приложения.
    pub viewfinder_autostart: ViewfinderAutostartConfig,
    /// Чья автоэкспозиция ведёт камеру: наша по статистике RAW (по умолчанию) или вендорская
    /// (`"own"` | `"vendor"`, сравнение A/B на телефоне, сессия 08a). Параметры нашего AE —
    /// `engine.three_a.exposure`.
    pub exposure_controller: ExposureControllerKind,
}

/// Какая часть секции `camera` неверна.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CameraConfigError {
    #[error("camera.engine: {0}")]
    Engine(#[from] CameraEngineConfigError),
    #[error("camera.android: {0}")]
    Android(#[from] camera_android::ConfigError),
    #[error("{0}")]
    RawStream(#[from] RawFrameStreamConfigError),
    #[error("camera.scenarios: {0}")]
    Scenarios(#[from] CameraScenarioConfigError),
    #[error("{0}")]
    ViewfinderAutostart(#[from] ViewfinderAutostartConfigError),
    #[error(
        "camera.android.request_origin_capacity ({capacity}) должно быть не меньше \
         camera.engine.three_a.max_bracket_frames ({bracket_limit}) + {margin}: записи серии вытеснят друг друга"
    )]
    RequestOriginTooSmallForBracket {
        capacity: usize,
        bracket_limit: usize,
        margin: usize,
    },
}

impl CameraConfig {
    /// Проверяет все части; первая найденная ошибка — в результате.
    pub fn validate(&self) -> Result<(), CameraConfigError> {
        self.engine.validate()?;
        self.android.validate()?;
        self.raw_stream.validate()?;
        self.scenarios.validate()?;
        self.viewfinder_autostart.validate()?;
        self.validate_request_origin_capacity()
    }

    /// Согласованность двух владельцев: таблица происхождений бэкенда вмещает целую серию движка.
    fn validate_request_origin_capacity(&self) -> Result<(), CameraConfigError> {
        let bracket_limit = self.engine.three_a.max_bracket_frames;
        let capacity = self.android.request_origin_capacity;
        if capacity < bracket_limit + REQUEST_ORIGIN_MARGIN {
            return Err(CameraConfigError::RequestOriginTooSmallForBracket {
                capacity,
                bracket_limit,
                margin: REQUEST_ORIGIN_MARGIN,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "camera_config_tests.rs"]
mod tests;
