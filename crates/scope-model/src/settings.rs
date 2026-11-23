//! Настройки модели приборов: размеры сеток, усиление, размах, прореживание, правило показа.
//! Каждое значение по умолчанию — оценка `[estimate]`; замеры и глаза владельца уточняют их в 14b.

use serde::{Deserialize, Serialize};

use crate::grid::GridError;
use crate::render_settings::{RenderSettings, RenderSettingsError};
use crate::sampling::SampleStride;
use crate::trace_rules::MAX_LEVELS;
use crate::vectorscope_geometry::{MAX_VECTORSCOPE_SIZE, MIN_VECTORSCOPE_SIZE};
use crate::{
    grid::{GridShape, MAX_GRID_SIDE},
    oklab_rule::{MAX_OKLAB_RANGE, MIN_OKLAB_RANGE},
    vectorscope_ycbcr::MAX_YCBCR_GAIN,
};

/// Ошибка настроек модели приборов: у каждой причины свой вариант.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScopeSettingsError {
    /// Число столбцов waveform/parade вне `1..=MAX_GRID_SIDE`.
    #[error("столбцов waveform/parade {columns} вне 1..={MAX_GRID_SIDE}")]
    TraceColumnsOutOfRange {
        /// Недопустимое число.
        columns: u32,
    },
    /// Число уровней waveform/parade вне `1..=MAX_LEVELS`.
    #[error("уровней waveform/parade {levels} вне 1..={MAX_LEVELS}")]
    TraceLevelsOutOfRange {
        /// Недопустимое число.
        levels: u32,
    },
    /// Сторона вектороскопа вне допустимых.
    #[error("сторона вектороскопа {size} вне {MIN_VECTORSCOPE_SIZE}..={MAX_VECTORSCOPE_SIZE}")]
    VectorscopeSizeOutOfRange {
        /// Недопустимая сторона.
        size: u32,
    },
    /// Усиление Y'CbCr вне допустимого.
    #[error("усиление вектороскопа Y'CbCr {gain} вне 1..={MAX_YCBCR_GAIN}")]
    YCbCrGainOutOfRange {
        /// Недопустимое значение.
        gain: f32,
    },
    /// Размах OKLab вне допустимого.
    #[error("размах OKLab {range} вне {MIN_OKLAB_RANGE}..={MAX_OKLAB_RANGE}")]
    OklabRangeOutOfRange {
        /// Недопустимое значение.
        range: f32,
    },
    /// Форма сетки превышает бюджет (число ячеек).
    #[error(transparent)]
    GridBudget(#[from] GridError),
    /// Настройки показа недопустимы.
    #[error(transparent)]
    Render(#[from] RenderSettingsError),
}

/// Настройки модели приборов.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScopeModelSettings {
    /// Столбцов сетки waveform и parade. `[estimate]` 256: больше, чем пикселей прибора по ширине четверти
    /// экрана, не нужно.
    pub trace_columns: u32,
    /// Уровней сетки waveform и parade. `[estimate]` 256: по уровню на 4 кода.
    pub trace_levels: u32,
    /// Сторона квадратной сетки вектороскопа (одна для обеих систем осей). `[estimate]` 256.
    pub vectorscope_size: u32,
    /// Усиление вектороскопа Y'CbCr (1 — полный размах на край). `[estimate]` 1.
    pub ycbcr_gain: f32,
    /// OKLab-цветность, уходящая на край сетки. `[estimate]` 0,4: цветность sRGB-красного 0,26, P3 — до 0,3.
    pub oklab_range: f32,
    /// Прореживание входа. `[estimate]` 2: четверть отсчётов, рычаг бюджета (замер в 14b).
    pub sample_stride: SampleStride,
    /// Правило показа.
    pub render: RenderSettings,
}

impl Default for ScopeModelSettings {
    fn default() -> Self {
        Self {
            trace_columns: 256,
            trace_levels: 256,
            vectorscope_size: 256,
            ycbcr_gain: 1.0,
            oklab_range: 0.4,
            sample_stride: SampleStride::EVERY_SECOND_PIXEL,
            render: RenderSettings::default(),
        }
    }
}

impl ScopeModelSettings {
    /// Проверка всех значений, включая бюджет памяти сеток.
    pub fn validate(&self) -> Result<(), ScopeSettingsError> {
        if !(1..=MAX_GRID_SIDE).contains(&self.trace_columns) {
            return Err(ScopeSettingsError::TraceColumnsOutOfRange {
                columns: self.trace_columns,
            });
        }
        if !(1..=MAX_LEVELS).contains(&self.trace_levels) {
            return Err(ScopeSettingsError::TraceLevelsOutOfRange {
                levels: self.trace_levels,
            });
        }
        if !(MIN_VECTORSCOPE_SIZE..=MAX_VECTORSCOPE_SIZE).contains(&self.vectorscope_size) {
            return Err(ScopeSettingsError::VectorscopeSizeOutOfRange {
                size: self.vectorscope_size,
            });
        }
        if !(1.0..=MAX_YCBCR_GAIN).contains(&self.ycbcr_gain) {
            return Err(ScopeSettingsError::YCbCrGainOutOfRange { gain: self.ycbcr_gain });
        }
        if !(MIN_OKLAB_RANGE..=MAX_OKLAB_RANGE).contains(&self.oklab_range) {
            return Err(ScopeSettingsError::OklabRangeOutOfRange {
                range: self.oklab_range,
            });
        }
        // Бюджет ячеек: проверяем тем же конструктором формы, что и правила.
        GridShape::try_new(self.trace_columns, self.trace_levels, 3)?;
        GridShape::try_new(self.vectorscope_size, self.vectorscope_size, 1)?;
        self.render.validate()?;
        Ok(())
    }
}
