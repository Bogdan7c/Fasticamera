//! Намерения управления камерой: шаблон запроса, экспозиция и фокус.
//!
//! В 04 — минимальный набор, достаточный для сессии и smoke. Полный домен ручных параметров
//! и 3A за контрактом — сессия 08.

use std::time::Duration;

use serde::Serialize;

/// Для чего поток: определяет шаблон запроса Camera2 (`ACameraDevice_request_template`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureIntent {
    /// Видоискатель (`TEMPLATE_PREVIEW`).
    Preview,
    /// Запись видео (`TEMPLATE_RECORD`): HAL настраивает 3A на плавность.
    VideoRecord,
}

/// Экспозиция: вендорский автомат или ручные выдержка и ISO (решение 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ExposureControl {
    Auto,
    Manual { exposure_time: Duration, iso: u32 },
}

/// Фокус: непрерывный вендорский автофокус или ручная дистанция.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum FocusControl {
    ContinuousAuto,
    /// Дистанция в диоптриях (0 — бесконечность).
    Manual {
        distance_diopters: f32,
    },
}

/// Набор ручных параметров, который применяется к повторяющемуся запросу целиком.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ManualControl {
    pub exposure: ExposureControl,
    pub focus: FocusControl,
}

impl ManualControl {
    /// Всё на автомате — значение по умолчанию для нового потока.
    pub const AUTOMATIC: ManualControl = ManualControl {
        exposure: ExposureControl::Auto,
        focus: FocusControl::ContinuousAuto,
    };
}

impl Default for ManualControl {
    fn default() -> Self {
        Self::AUTOMATIC
    }
}
