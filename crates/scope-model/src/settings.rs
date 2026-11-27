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
    vectorscope_ycbcr::{MAX_YCBCR_GAIN, MIN_YCBCR_GAIN},
};

/// Наименьшая цветность края Y'CbCr: при зуме 2× (решение 325) правилу уходит усиление `0,5 / (край / 2)`, оно не
/// выше `MAX_YCBCR_GAIN`.
pub const MIN_YCBCR_EDGE_CHROMA: f32 = 2.0 * 0.5 / MAX_YCBCR_GAIN;
/// Наибольшая цветность края Y'CbCr: усиление `0,5 / край` не ниже `MIN_YCBCR_GAIN`.
pub const MAX_YCBCR_EDGE_CHROMA: f32 = 0.5 / MIN_YCBCR_GAIN;

/// Наименьший размах OKLab в настройках: при зуме 2× правилу уходит половина, и она не ниже `MIN_OKLAB_RANGE`.
pub const MIN_SETTINGS_OKLAB_RANGE: f32 = 2.0 * MIN_OKLAB_RANGE;

/// Усиление правила Y'CbCr для цветности края `edge_chroma` (1 — ±0,5 на краю) — единственный пересчёт настройки.
pub fn ycbcr_gain_for_edge_chroma(edge_chroma: f32) -> f32 {
    0.5 / edge_chroma
}

/// Ошибка настроек модели приборов: у каждой причины свой вариант.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScopeSettingsError {
    /// Потолок столбцов waveform/parade вне `1..=MAX_GRID_SIDE`.
    #[error("потолок столбцов waveform/parade {columns} вне 1..={MAX_GRID_SIDE}")]
    TraceMaxColumnsOutOfRange {
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
    #[error("цветность края вектороскопа Y'CbCr {chroma} вне {MIN_YCBCR_EDGE_CHROMA}..={MAX_YCBCR_EDGE_CHROMA}")]
    YCbCrEdgeChromaOutOfRange {
        /// Недопустимое значение.
        chroma: f32,
    },
    /// Размах OKLab вне допустимого.
    #[error("размах OKLab {range} вне {MIN_SETTINGS_OKLAB_RANGE}..={MAX_OKLAB_RANGE}")]
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
    /// Потолок столбцов сетки waveform и parade: ёмкость буфера счётчиков. Столбцов в кадре — столько, сколько
    /// отсчётов вдоль взгляда (решение 322; заменяет фиксированные 512 из решения 268), окно шире потолка берётся
    /// с бо́льшим шагом ([`crate::SampleCount::planned`]). `[estimate]` 2048: длинная сторона показанной картинки на
    /// всех потоках телефона ≤ 2048, шаг 1 остаётся всегда.
    pub trace_max_columns: u32,
    /// Уровней сетки waveform и parade. `[estimate]` 512: по уровню на 2 кода (было 256 — ступеньки по 3 px на
    /// картинке 827 px высотой; на листах 14r 512 и 1024 глазом не различались, 512 вдвое дешевле очистки).
    pub trace_levels: u32,
    /// Сторона квадратной сетки вектороскопа (одна для обеих систем осей). `[estimate]` 512 (было 256 — клетки по
    /// 3,2 px на картинке 827 px).
    pub vectorscope_size: u32,
    /// Цветность Y'CbCr (длина вектора Cb/Cr) на краю круга вектороскопа при зуме 1×. `[estimate]` 0,64 (решение 324):
    /// самая длинная цветность полос 100 % BT.709 — 0,596 у пурпурного и зелёного — встаёт на 0,93 радиуса, все
    /// цели 100 % внутри круга, как у Resolve и Final Cut. До 14r было 0,5 (цели 100 % выходили за круг).
    pub ycbcr_edge_chroma: f32,
    /// OKLab-цветность, уходящая на край сетки. `[estimate]` 0,4: цветность sRGB-красного 0,26, P3 — до 0,3.
    pub oklab_range: f32,
    /// Прореживание входа waveform, parade и Luma ВДОЛЬ взгляда (из этих отсчётов — столбцы сетки). 1 — каждый пиксель
    /// (решение 322: «песок» шага 2 на листах 14r), рычаг бюджета остаётся. Правило без своего шага берёт его по
    /// обеим осям.
    pub sample_stride: SampleStride,
    /// Прореживание входа waveform, parade и Luma ПОПЕРЁК взгляда. 2 — каждая вторая строка (решение 329: цена
    /// прибора в разделе): столбец получает вдвое меньше попаданий, но поровну, яркость следа — от плотности, вид тот же.
    pub trace_across_eye_stride: SampleStride,
    /// Прореживание входа вектороскопа. 2 — каждый второй пиксель по обеим осям (решение 328): вектороскоп собирает
    /// весь кадр в один круг, четверти отсчётов ему хватает (так было до 14r), а накопление вчетверо дешевле.
    pub vectorscope_sample_stride: SampleStride,
    /// Правило показа.
    pub render: RenderSettings,
}

impl Default for ScopeModelSettings {
    fn default() -> Self {
        Self {
            trace_max_columns: 2048,
            trace_levels: 512,
            vectorscope_size: 512,
            ycbcr_edge_chroma: 0.64,
            oklab_range: 0.4,
            sample_stride: SampleStride::EVERY_PIXEL,
            trace_across_eye_stride: SampleStride::EVERY_SECOND_PIXEL,
            vectorscope_sample_stride: SampleStride::EVERY_SECOND_PIXEL,
            render: RenderSettings::default(),
        }
    }
}

impl ScopeModelSettings {
    /// Проверка всех значений, включая бюджет памяти сеток.
    pub fn validate(&self) -> Result<(), ScopeSettingsError> {
        if !(1..=MAX_GRID_SIDE).contains(&self.trace_max_columns) {
            return Err(ScopeSettingsError::TraceMaxColumnsOutOfRange {
                columns: self.trace_max_columns,
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
        if !(MIN_YCBCR_EDGE_CHROMA..=MAX_YCBCR_EDGE_CHROMA).contains(&self.ycbcr_edge_chroma) {
            return Err(ScopeSettingsError::YCbCrEdgeChromaOutOfRange {
                chroma: self.ycbcr_edge_chroma,
            });
        }
        if !(MIN_SETTINGS_OKLAB_RANGE..=MAX_OKLAB_RANGE).contains(&self.oklab_range) {
            return Err(ScopeSettingsError::OklabRangeOutOfRange {
                range: self.oklab_range,
            });
        }
        // Бюджет ячеек: проверяем тем же конструктором формы, что и правила.
        GridShape::try_new(self.trace_max_columns, self.trace_levels, 3)?;
        GridShape::try_new(self.vectorscope_size, self.vectorscope_size, 1)?;
        self.render.validate()?;
        Ok(())
    }
}
