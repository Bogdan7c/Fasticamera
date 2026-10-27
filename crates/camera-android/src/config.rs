//! Настройки бэкенда камеры на Camera2 NDK: ничего не зашито в код (правило проекта «не хардкодить»).
//!
//! Конфиг фиксируется на процесс. Значения по умолчанию подобраны по фактам сессии 00
//! (`mem:device/oneplus11-camera-facts`); каждое поле объясняет, откуда число.

use camera_contract::ids::{CameraId, EmptyCameraId};
use serde::Deserialize;

/// Настройки `camera-android`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AndroidCameraConfig {
    /// ID камер, где вендорские ключи CamX (`disableFPSLimits` + `overrideSensorFrameDuration`)
    /// проверены на телефоне и разрешены. Только основная камера «2»: на ширике «3» эти ключи
    /// ломают поток — 0 кадров и перезапуск HAL (00, попытка 17). Камеры вне списка вендорских
    /// ключей не получают никогда: чужой телефон или другая камера — это неизвестный HAL.
    pub vendor_frame_duration_override_cameras: Vec<String>,
    /// Максимальная частота (к/с), которую бэкенд пробует просить публичным диапазоном AE. Выше —
    /// `FrameRateNotAchievable`: рычагов за пределами 60 к/с на RAW у OnePlus 11 нет (00: high-speed
    /// сессия с RAW отвергнута).
    pub max_public_range_frame_rate: u32,
    /// Размер окна сопоставления «начало кадра → номер кадра» (колбэк результата NDK не отдаёт
    /// номер кадра, а колбэк начала — отдаёт). Должно покрывать число кадров в полёте
    /// между началом экспозиции и готовым результатом (при 60 к/с конвейер HAL — единицы кадров).
    pub frame_number_window: usize,
    /// Сколько последних отказов хранит диагностика (старые вытесняются со счётчиком).
    pub failure_history_capacity: usize,
    /// Сколько миллисекунд останавливающийся бэкенд ждёт возврата кадров от потребителей, прежде
    /// чем оставить reader закрытой сессии до конца процесса (`readers_leaked`). Reader нельзя
    /// удалять, пока GPU может читать его кадры; ожидание — единственное место, где поток камеры
    /// вообще ждёт поток рендера, поэтому оно ограничено. Кадр у GPU живёт несколько кадров
    /// (десятки миллисекунд), 500 мс — с большим запасом.
    pub retired_reader_drain_timeout_ms: u64,
    /// Шаг опроса (мс) при этом ожидании: как часто проверять, вернулись ли кадры.
    pub retired_reader_poll_interval_ms: u64,
}

impl Default for AndroidCameraConfig {
    fn default() -> Self {
        Self {
            vendor_frame_duration_override_cameras: vec!["2".to_owned()],
            max_public_range_frame_rate: 60,
            frame_number_window: 16,
            failure_history_capacity: 32,
            retired_reader_drain_timeout_ms: 500,
            retired_reader_poll_interval_ms: 5,
        }
    }
}

/// Что не так с конфигом.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("в vendor_frame_duration_override_cameras пустой ID камеры")]
    EmptyOverrideCameraId(#[from] EmptyCameraId),
    #[error("max_public_range_frame_rate должно быть от 1 до {max}, а не {actual}")]
    PublicRangeFrameRateOutOfRange { actual: u32, max: u32 },
    #[error("frame_number_window должно быть от 1 до {max}, а не {actual}")]
    FrameNumberWindowOutOfRange { actual: usize, max: usize },
    #[error("failure_history_capacity должно быть от 1 до {max}, а не {actual}")]
    FailureHistoryOutOfRange { actual: usize, max: usize },
    #[error("retired_reader_drain_timeout_ms должно быть от 1 до {max}, а не {actual}")]
    RetiredReaderTimeoutOutOfRange { actual: u64, max: u64 },
    #[error("retired_reader_poll_interval_ms должно быть от 1 до {max}, а не {actual}")]
    RetiredReaderPollOutOfRange { actual: u64, max: u64 },
}

impl AndroidCameraConfig {
    /// Верхняя граница окна номеров кадров и истории отказов.
    pub const MAX_WINDOW: usize = 1024;
    /// Верхняя граница ожидания возврата кадров при остановке: минута — уже зависание.
    pub const MAX_RETIRED_READER_TIMEOUT_MS: u64 = 60_000;
    /// Верхняя граница шага опроса при ожидании.
    pub const MAX_RETIRED_READER_POLL_MS: u64 = 1_000;

    /// Проверяет значения и возвращает конфиг как есть; первая найденная ошибка — в результате.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for id in &self.vendor_frame_duration_override_cameras {
            CameraId::new(id.as_str())?;
        }
        let max_rate = camera_contract::stream::TargetFrameRate::MAX;
        if !(1..=max_rate).contains(&self.max_public_range_frame_rate) {
            return Err(ConfigError::PublicRangeFrameRateOutOfRange {
                actual: self.max_public_range_frame_rate,
                max: max_rate,
            });
        }
        if !(1..=Self::MAX_WINDOW).contains(&self.frame_number_window) {
            return Err(ConfigError::FrameNumberWindowOutOfRange {
                actual: self.frame_number_window,
                max: Self::MAX_WINDOW,
            });
        }
        if !(1..=Self::MAX_WINDOW).contains(&self.failure_history_capacity) {
            return Err(ConfigError::FailureHistoryOutOfRange {
                actual: self.failure_history_capacity,
                max: Self::MAX_WINDOW,
            });
        }
        if !(1..=Self::MAX_RETIRED_READER_TIMEOUT_MS).contains(&self.retired_reader_drain_timeout_ms) {
            return Err(ConfigError::RetiredReaderTimeoutOutOfRange {
                actual: self.retired_reader_drain_timeout_ms,
                max: Self::MAX_RETIRED_READER_TIMEOUT_MS,
            });
        }
        if !(1..=Self::MAX_RETIRED_READER_POLL_MS).contains(&self.retired_reader_poll_interval_ms) {
            return Err(ConfigError::RetiredReaderPollOutOfRange {
                actual: self.retired_reader_poll_interval_ms,
                max: Self::MAX_RETIRED_READER_POLL_MS,
            });
        }
        Ok(())
    }

    /// Предел ожидания возврата кадров при остановке бэкенда.
    pub fn retired_reader_drain_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.retired_reader_drain_timeout_ms)
    }

    /// Шаг опроса при этом ожидании.
    pub fn retired_reader_poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.retired_reader_poll_interval_ms)
    }

    /// Разрешены ли вендорские ключи CamX на этой камере (она в списке проверенных).
    pub fn allows_vendor_frame_duration_override(&self, camera: &CameraId) -> bool {
        self.vendor_frame_duration_override_cameras
            .iter()
            .any(|allowed| allowed == camera.as_str())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
