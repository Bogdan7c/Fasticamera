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
    /// Сколько кадров держит временный RAW-приёмник (`AImageReader`, `maxImages`).
    ///
    /// Кадры отпускаются сразу в слушателе, поэтому запас нужен только на гонку «HAL выдал
    /// следующий кадр, пока слушатель ещё отпускает предыдущий» при 60 к/с. Меньше 2 HAL не стартует
    /// конвейер (один буфер у приложения, один у HAL); 4 — тот же запас, что в зонде сессии 00.
    pub raw_reader_max_images: u32,
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
}

impl Default for AndroidCameraConfig {
    fn default() -> Self {
        Self {
            vendor_frame_duration_override_cameras: vec!["2".to_owned()],
            raw_reader_max_images: 4,
            max_public_range_frame_rate: 60,
            frame_number_window: 16,
            failure_history_capacity: 32,
        }
    }
}

/// Что не так с конфигом.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("в vendor_frame_duration_override_cameras пустой ID камеры")]
    EmptyOverrideCameraId(#[from] EmptyCameraId),
    #[error("raw_reader_max_images должно быть от {min} до {max}, а не {actual}")]
    ReaderMaxImagesOutOfRange { actual: u32, min: u32, max: u32 },
    #[error("max_public_range_frame_rate должно быть от 1 до {max}, а не {actual}")]
    PublicRangeFrameRateOutOfRange { actual: u32, max: u32 },
    #[error("frame_number_window должно быть от 1 до {max}, а не {actual}")]
    FrameNumberWindowOutOfRange { actual: usize, max: usize },
    #[error("failure_history_capacity должно быть от 1 до {max}, а не {actual}")]
    FailureHistoryOutOfRange { actual: usize, max: usize },
}

impl AndroidCameraConfig {
    /// Нижняя граница `maxImages`: один буфер у приложения, один у HAL.
    pub const MIN_READER_IMAGES: u32 = 2;
    /// Верхняя граница `maxImages`: каждый RAW-буфер 4096×3072 — десятки МиБ, больше 16 — ошибка конфига.
    pub const MAX_READER_IMAGES: u32 = 16;
    /// Верхняя граница окна номеров кадров и истории отказов.
    pub const MAX_WINDOW: usize = 1024;

    /// Проверяет значения и возвращает конфиг как есть; первая найденная ошибка — в результате.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for id in &self.vendor_frame_duration_override_cameras {
            CameraId::new(id.as_str())?;
        }
        if !(Self::MIN_READER_IMAGES..=Self::MAX_READER_IMAGES).contains(&self.raw_reader_max_images) {
            return Err(ConfigError::ReaderMaxImagesOutOfRange {
                actual: self.raw_reader_max_images,
                min: Self::MIN_READER_IMAGES,
                max: Self::MAX_READER_IMAGES,
            });
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
        Ok(())
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
