//! Параметры модели зума (секция `zoom` конфига движка камеры).
//!
//! Значения с пометкой `[estimate]` не измерены на iPhone/телефоне и подлежат подбору в 13a
//! (правило 96: незамеренное — оценка владельца в коде, а не молчаливая константа).

use serde::{Deserialize, Serialize};

/// Параметры шкалы зума и выбора камеры.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomConfig {
    /// Наибольшее допустимое смещение оптической оси от центра сенсора, доля длинной стороны.
    /// Замер 13: у ширика OnePlus 11 ось смещена на ≈ 6 % — настоящая калибровка; у фронта HAL
    /// отдаёт заглушку `0, 0` (смещение ≈ 50 %). 15 % разделяет их с запасом. `[estimate]`
    pub calibration_max_principal_point_offset: f32,
    /// Цифровые упоры шкалы задних камер (кропы опорной), например `2.0` (решение 111).
    /// Упор не создаётся, если вне диапазона опорной камеры или рядом с настоящим объективом.
    pub digital_crop_stops: Vec<f32>,
    /// Предел коэффициента задних камер (решение 112: 10×; `availableMaxDigitalZoom` HAL).
    pub back_max_ratio: f32,
    /// Предел коэффициента фронтальной камеры (решение 115: щипок до 3×).
    pub front_max_ratio: f32,
    /// Запас при переходе на более длинный объектив: переключаемся, когда коэффициент не меньше
    /// `native × (1 + запас)`; пока короткий объектив продолжает кадрировать. `[estimate]`,
    /// подбирается на телефоне (13a).
    pub switch_up_hysteresis: f32,
    /// Допуск совпадения цифрового упора с настоящим объективом, относительный: упор `2.0` не
    /// создаётся, если объектив лежит в `2.0 ± 5 %`. Так будущий настоящий телевик ≈ 2× заменяет
    /// кроп, а не соседствует с ним. `[estimate]`
    pub stop_dedupe_tolerance: f32,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            calibration_max_principal_point_offset: 0.15,
            digital_crop_stops: vec![2.0],
            back_max_ratio: 10.0,
            front_max_ratio: 3.0,
            switch_up_hysteresis: 0.05,
            stop_dedupe_tolerance: 0.05,
        }
    }
}

/// Конфиг зума некорректен.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ZoomConfigError {
    #[error("zoom.calibration_max_principal_point_offset должен быть в (0, 0.5], а не {0}")]
    InvalidCalibrationOffset(f32),
    #[error("zoom.digital_crop_stops: упор {0} должен быть конечным и больше 1")]
    InvalidCropStop(f32),
    #[error("zoom.{field} должен быть конечным и не меньше 1, а не {value}")]
    InvalidMaxRatio { field: &'static str, value: f32 },
    #[error("zoom.switch_up_hysteresis должен быть конечным и в [0, 1], а не {0}")]
    InvalidHysteresis(f32),
    #[error("zoom.stop_dedupe_tolerance должен быть конечным и в [0, 0.5), а не {0}")]
    InvalidDedupeTolerance(f32),
}

impl ZoomConfig {
    /// Проверка значений; NaN и бесконечности отклоняются (сравнения с NaN ложны).
    pub fn validate(&self) -> Result<(), ZoomConfigError> {
        let offset = self.calibration_max_principal_point_offset;
        if !(offset.is_finite() && offset > 0.0 && offset <= 0.5) {
            return Err(ZoomConfigError::InvalidCalibrationOffset(offset));
        }
        if let Some(stop) = self
            .digital_crop_stops
            .iter()
            .copied()
            .find(|stop| !(stop.is_finite() && *stop > 1.0))
        {
            return Err(ZoomConfigError::InvalidCropStop(stop));
        }
        for (field, value) in [
            ("back_max_ratio", self.back_max_ratio),
            ("front_max_ratio", self.front_max_ratio),
        ] {
            if !(value.is_finite() && value >= 1.0) {
                return Err(ZoomConfigError::InvalidMaxRatio { field, value });
            }
        }
        let hysteresis = self.switch_up_hysteresis;
        if !(hysteresis.is_finite() && (0.0..=1.0).contains(&hysteresis)) {
            return Err(ZoomConfigError::InvalidHysteresis(hysteresis));
        }
        let tolerance = self.stop_dedupe_tolerance;
        if !(tolerance.is_finite() && (0.0..0.5).contains(&tolerance)) {
            return Err(ZoomConfigError::InvalidDedupeTolerance(tolerance));
        }
        Ok(())
    }
}
