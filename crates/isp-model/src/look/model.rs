//! Модель вида (конфиг): во что переводятся ручки `[−1, 1]` (диапазоны) и какие полосы оттенка существуют.
//!
//! `LookSettings` (словарь, `look-contract`) — то, что крутит человек. [`LookModelSettings`] — «насколько
//! сильна ручка на краю» и набор полос. Все числа — стартовые оценки `[estimate]`, их подбирает владелец в 14b
//! на своих сценах. Модель — часть конфига (`deny_unknown_fields`, `default`, [`LookModelSettings::validate`]).

use look_contract::HueBandId;
use serde::{Deserialize, Serialize};

use super::bands::MAX_LOOK_BANDS;

/// Полоса оттенка модели: имя и центр на круге OKLCh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HueBandDefinition {
    /// Идентификатор (его же используют правки пользователя в `LookSettings::bands`).
    pub id: HueBandId,
    /// Центр полосы, градусы OKLCh `[0, 360)`.
    pub center_degrees: f32,
}

/// Окно кожи для сочности: вокруг этого оттенка сочность ослабляется.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SkinWindow {
    /// Центр окна, градусы OKLCh. `[estimate]`: «Light skin» ColorChecker ≈ 50°, загар смещает к 60°.
    pub hue_center_degrees: f32,
    /// Полуширина окна, градусы: за ней защита кожи нулевая. `[estimate]`.
    pub half_width_degrees: f32,
    /// Доля сочности, подавляемая в центре окна, `[0, 1]`. `[estimate]`.
    pub protection: f32,
}

impl Default for SkinWindow {
    fn default() -> Self {
        Self {
            hue_center_degrees: 55.0,
            half_width_degrees: 35.0,
            protection: 0.8,
        }
    }
}

/// Настройки модели вида.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LookModelSettings {
    /// Контраст на краю ручки: `log2` множителя наклона кривой (`2^(k·range)`). Ограничен запасом плеча AgX:
    /// при меньшем наклоне плечо не успевает дойти до белого (см. [`super::validation`]). `[estimate]`.
    pub contrast_range_log2: f32,
    /// Средние тона на краю: сдвиг серого экрана в ступенях (`·2^(k·range)`). `[estimate]`.
    pub midtones_range_ev: f32,
    /// Тени на краю: `log2` изменения степени носка (`+` светлее). `[estimate]`.
    pub shadows_range_log2: f32,
    /// Света на краю: `log2` изменения степени плеча (`+` светлее). `[estimate]`.
    pub highlights_range_log2: f32,
    /// Насыщенность на краю: множитель цветности `1 + k·range` (`range ≤ 1`: на минусе серый). `[estimate]`.
    pub saturation_range: f32,
    /// Сочность на краю: добавка к множителю цветности для малоцветного, до подавления кожей. `[estimate]`.
    pub vibrance_range: f32,
    /// Теплота на краю, майреды (`+` — теплее). `[estimate]`.
    pub warmth_range_mired: f32,
    /// Оттенок (зелёный ↔ пурпурный) на краю, Duv (`+` — пурпурнее). `[estimate]`.
    pub tint_range_duv: f32,
    /// Цветность OKLab, при которой вес сочности падает вдвое (уже насыщенное подавляется). `[estimate]`.
    pub vibrance_chroma_reference: f32,
    /// Окно кожи для сочности.
    pub skin: SkinWindow,
    /// Гладкий порог серого: цветность OKLab, при которой вес эффектов полос равен ½ (`w = C²/(C² + порог²)`).
    /// Выше остаточной цветности серого (≈ 5·10⁻⁴) на порядок и более. `[estimate]`.
    pub neutral_chroma_floor: f32,
    /// Полосы оттенка: число и центры — конфиг, не код. Стартовые 8, как у Lightroom (центры в OKLCh, `[estimate]`).
    pub bands: Vec<HueBandDefinition>,
    /// Сдвиг оттенка полосы на краю ручки, градусы. `[estimate]`.
    pub band_hue_range_degrees: f32,
    /// Насыщенность полосы на краю: множитель `1 + k·range` (`range ≤ 1`). `[estimate]`.
    pub band_saturation_range: f32,
    /// Яркость полосы на краю: множитель светлоты `1 + k·range` (`range < 1`). `[estimate]`.
    pub band_luminance_range: f32,
    /// Предел размера LUT: узлов на ось (`LookLut::new`). `[estimate]`.
    pub max_lut_size: u32,
}

impl Default for LookModelSettings {
    fn default() -> Self {
        Self {
            contrast_range_log2: 0.1,
            midtones_range_ev: 0.3,
            shadows_range_log2: 0.6,
            highlights_range_log2: 0.6,
            saturation_range: 1.0,
            vibrance_range: 0.8,
            warmth_range_mired: 50.0,
            tint_range_duv: 0.012,
            vibrance_chroma_reference: 0.12,
            skin: SkinWindow::default(),
            neutral_chroma_floor: 0.01,
            bands: [
                ("red", 29.0),
                ("orange", 60.0),
                ("yellow", 105.0),
                ("green", 142.0),
                ("aqua", 195.0),
                ("blue", 264.0),
                ("purple", 305.0),
                ("magenta", 345.0),
            ]
            .into_iter()
            // Идентификаторы — литералы допустимого алфавита; тест `default_model_is_valid_and_has_eight_bands`
            // следит, что ни один не отброшен.
            .filter_map(|(id, center_degrees)| {
                HueBandId::new(id)
                    .ok()
                    .map(|id| HueBandDefinition { id, center_degrees })
            })
            .collect(),
            band_hue_range_degrees: 30.0,
            band_saturation_range: 0.8,
            band_luminance_range: 0.35,
            max_lut_size: 65,
        }
    }
}

/// Модель вида не годится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidLookModel {
    #[error("{field} = {value}: нужно конечное число в диапазоне {min}…{max}")]
    OutOfRange {
        field: &'static str,
        value: f32,
        min: f32,
        max: f32,
    },
    #[error("полос {count}: нужно от 1 до {max}")]
    BandCount { count: usize, max: usize },
    #[error("полоса №{band_index} (с нуля): центр {center_degrees}° вне [0, 360)")]
    BandCenterOutOfRange { band_index: usize, center_degrees: f32 },
    #[error(
        "полосы №{first_index} и №{second_index} (с нуля) имеют один идентификатор или центры ближе {min_gap_degrees}°"
    )]
    BandsTooClose {
        first_index: usize,
        second_index: usize,
        min_gap_degrees: f32,
    },
    #[error("предел размера LUT {value}: нужно от 2 до {max}")]
    LutLimit { value: u32, max: u32 },
    #[error("таблица {size} узлов на ось больше предела модели {max}")]
    LutLargerThanLimit { size: u32, max: u32 },
}

/// Наименьший зазор между центрами полос, градусы (между ближе окна вырождаются).
pub const MIN_BAND_GAP_DEGREES: f32 = 1.0;
/// Наибольший размер LUT, который допускает конфиг (память буфера GPU: `N³·12` байт).
pub const MAX_LUT_SIZE_LIMIT: u32 = 129;

impl LookModelSettings {
    /// Проверка диапазонов и набора полос (кривая на краях — [`super::validation`]).
    pub fn validate(&self) -> Result<(), InvalidLookModel> {
        let check = |field: &'static str, value: f32, min: f32, max: f32| {
            if value.is_finite() && value >= min && value <= max {
                Ok(())
            } else {
                Err(InvalidLookModel::OutOfRange { field, value, min, max })
            }
        };
        check("contrast_range_log2", self.contrast_range_log2, 0.0, 2.0)?;
        check("midtones_range_ev", self.midtones_range_ev, 0.0, 2.0)?;
        check("shadows_range_log2", self.shadows_range_log2, 0.0, 2.0)?;
        check("highlights_range_log2", self.highlights_range_log2, 0.0, 2.0)?;
        check("saturation_range", self.saturation_range, 0.0, 1.0)?;
        check("vibrance_range", self.vibrance_range, 0.0, 1.0)?;
        check("warmth_range_mired", self.warmth_range_mired, 0.0, 300.0)?;
        check("tint_range_duv", self.tint_range_duv, 0.0, 0.05)?;
        check("vibrance_chroma_reference", self.vibrance_chroma_reference, 1e-3, 1.0)?;
        check("skin.hue_center_degrees", self.skin.hue_center_degrees, 0.0, 360.0)?;
        check("skin.half_width_degrees", self.skin.half_width_degrees, 1.0, 179.0)?;
        check("skin.protection", self.skin.protection, 0.0, 1.0)?;
        check("neutral_chroma_floor", self.neutral_chroma_floor, 1e-4, 1.0)?;
        check("band_hue_range_degrees", self.band_hue_range_degrees, 0.0, 90.0)?;
        check("band_saturation_range", self.band_saturation_range, 0.0, 1.0)?;
        check("band_luminance_range", self.band_luminance_range, 0.0, 0.95)?;
        if self.max_lut_size < 2 || self.max_lut_size > MAX_LUT_SIZE_LIMIT {
            return Err(InvalidLookModel::LutLimit {
                value: self.max_lut_size,
                max: MAX_LUT_SIZE_LIMIT,
            });
        }
        self.validate_bands()
    }

    fn validate_bands(&self) -> Result<(), InvalidLookModel> {
        if self.bands.is_empty() || self.bands.len() > MAX_LOOK_BANDS {
            return Err(InvalidLookModel::BandCount {
                count: self.bands.len(),
                max: MAX_LOOK_BANDS,
            });
        }
        for (band_index, band) in self.bands.iter().enumerate() {
            if !(band.center_degrees.is_finite() && (0.0..360.0).contains(&band.center_degrees)) {
                return Err(InvalidLookModel::BandCenterOutOfRange {
                    band_index,
                    center_degrees: band.center_degrees,
                });
            }
        }
        for (first_index, first) in self.bands.iter().enumerate() {
            for (offset, second) in self.bands[first_index + 1..].iter().enumerate() {
                let difference = (first.center_degrees - second.center_degrees).abs();
                let circular_gap = difference.min(360.0 - difference);
                if first.id == second.id || circular_gap < MIN_BAND_GAP_DEGREES {
                    return Err(InvalidLookModel::BandsTooClose {
                        first_index,
                        second_index: first_index + 1 + offset,
                        min_gap_degrees: MIN_BAND_GAP_DEGREES,
                    });
                }
            }
        }
        Ok(())
    }
}
