//! Описание сенсора для ISP: всё, что постоянно на сессию потока, из словаря камеры.
//!
//! Источник истины — паспорт потока ([`StreamPassport`]: мозаика, статические уровни, флаги
//! вендорской обработки) и калибровка цвета из характеристик камеры
//! ([`ColorCalibrationSet`], DNG-матрицы HAL). Покадровое (динамические уровни, нейтраль)
//! приходит отдельно, через [`crate::params::FrameInputs`].

use camera_contract::capabilities::{ColorCalibrationSet, LensShading};
use camera_contract::frame_passport::{CaptureColorHints, StreamPassport};
use camera_contract::raw_format::{MosaicKind, MosaicPattern};

use crate::cfa::{BayerPattern, UnsupportedCfa};
use crate::color::dng::{ColorProfileError, DngColorProfile};
use crate::levels::{InvalidLevels, NormalizationLevels};

/// Сенсор нельзя описать для эталона.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum SensorDescriptionError {
    #[error("мозаика {kind:?} не Bayer: эталон поддерживает только Bayer 2×2 (Quad Bayer и RGBW — отдельный план)")]
    UnsupportedMosaicKind { kind: MosaicKind },
    #[error(transparent)]
    UnsupportedCfa(#[from] UnsupportedCfa),
    #[error("статические уровни сенсора: {0}")]
    Levels(#[from] InvalidLevels),
    #[error("цветовой профиль: {0}")]
    ColorProfile(#[from] ColorProfileError),
}

/// Постоянное на сессию описание сенсора.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorDescription {
    pattern: BayerPattern,
    static_levels: NormalizationLevels,
    color_profile: DngColorProfile,
    vendor_lens_shading: LensShading,
}

impl SensorDescription {
    pub fn new(
        pattern: BayerPattern,
        static_levels: NormalizationLevels,
        color_profile: DngColorProfile,
        vendor_lens_shading: LensShading,
    ) -> Self {
        Self {
            pattern,
            static_levels,
            color_profile,
            vendor_lens_shading,
        }
    }

    /// Описание из паспорта потока и калибровки цвета камеры.
    pub fn from_stream_passport(
        passport: &StreamPassport,
        color_calibration: &[ColorCalibrationSet],
    ) -> Result<Self, SensorDescriptionError> {
        Ok(Self {
            pattern: bayer_pattern(passport.mosaic)?,
            static_levels: NormalizationLevels::from_sensor_levels(&passport.static_levels)?,
            color_profile: DngColorProfile::from_calibration_sets(color_calibration)?,
            vendor_lens_shading: passport.vendor_processing.lens_shading,
        })
    }

    pub fn pattern(&self) -> BayerPattern {
        self.pattern
    }

    pub fn static_levels(&self) -> NormalizationLevels {
        self.static_levels
    }

    pub fn color_profile(&self) -> &DngColorProfile {
        &self.color_profile
    }

    /// Применил ли вендор виньетирование к RAW (решает стадия 2).
    pub fn vendor_lens_shading(&self) -> LensShading {
        self.vendor_lens_shading
    }

    /// Уровни конкретного кадра: динамические из результата запроса важнее статических
    /// (Camera2: `SENSOR_DYNAMIC_BLACK/WHITE_LEVEL` точнее для этого кадра). Чего HAL не дал —
    /// берётся из статических уровней (на OnePlus 11 динамических нет, 00 C3).
    pub fn frame_levels(&self, hints: &CaptureColorHints) -> Result<NormalizationLevels, InvalidLevels> {
        let black = hints.dynamic_black_level.unwrap_or(self.static_levels.black_pattern());
        // u32 → f32 без потерь: уровни RAW ≤ 2^16.
        let white = hints
            .dynamic_white_level
            .map_or(self.static_levels.white(), |level| level as f32);
        NormalizationLevels::new(black, white)
    }
}

/// Раскладка Bayer из мозаики паспорта; другие виды мозаики — честная ошибка.
fn bayer_pattern(mosaic: MosaicPattern) -> Result<BayerPattern, SensorDescriptionError> {
    match mosaic.kind {
        MosaicKind::Bayer => Ok(BayerPattern::from_layout(mosaic.layout)?),
        kind @ (MosaicKind::QuadBayer | MosaicKind::Rgbw) => {
            Err(SensorDescriptionError::UnsupportedMosaicKind { kind })
        }
    }
}

#[cfg(test)]
#[path = "sensor_tests.rs"]
mod tests;
