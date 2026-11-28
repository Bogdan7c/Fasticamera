//! Модель вида (конфиг): во что переводятся ручки `[−1, 1]` (диапазоны) и какие полосы оттенка существуют.
//!
//! `LookSettings` (словарь, `look-contract`) — то, что крутит человек. [`LookModelSettings`] — «насколько
//! сильна ручка на краю» ([`KnobRange`]) и набор полос. Края выбраны владельцем в 14l по снимкам: край —
//! заведомо перебор, как в редакторах (решения 242–246); прочие числа — оценки `[estimate]`. Модель — часть конфига (`deny_unknown_fields`, `default`, [`LookModelSettings::validate`]).

use look_contract::HueBandId;
use serde::{Deserialize, Serialize};

use super::bands::MAX_LOOK_BANDS;
use super::primaries::{InvalidPrimariesModel, PrimariesModelSettings};
use super::range::KnobRange;
use super::tone_shape::ToneShapeSettings;
use super::wheels::{InvalidWheelModel, WheelModelSettings};

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
    /// Контраст, средние тона, тени, света: стадия тона вида после основы ([`super::tone_shape`], решение 236).
    pub tone: ToneShapeSettings,
    /// Насыщенность: множитель цветности `1 + величина` (минус физически не дальше 1 — это серый).
    pub saturation: KnobRange,
    /// Сочность: добавка к множителю цветности для малоцветного, до подавления кожей.
    pub vibrance: KnobRange,
    /// Теплота, майреды (`+` — теплее). На холодном или тёплом свете, где до края таблицы меньше, край ручки — край
    /// таблицы ([`super::white`]).
    pub warmth_mired: KnobRange,
    /// Оттенок (зелёный ↔ пурпурный), Duv (`+` — пурпурнее).
    pub tint_duv: KnobRange,
    /// Цветность OKLab, при которой вес сочности падает вдвое (уже насыщенное подавляется). `[estimate]`.
    pub vibrance_chroma_reference: f32,
    /// Окно кожи для сочности.
    pub skin: SkinWindow,
    /// Гладкий порог серого: цветность OKLab, при которой вес эффектов полос равен ½ (`w = C²/(C² + порог²)`).
    /// Выше остаточной цветности серого (≈ 5·10⁻⁴) на порядок и более. `[estimate]`.
    pub neutral_chroma_floor: f32,
    /// Полосы оттенка: число и центры — конфиг, не код. Стартовые 8, как у Lightroom (центры в OKLCh, `[estimate]`).
    pub bands: Vec<HueBandDefinition>,
    /// Сдвиг оттенка полосы, градусы OKLCh. ±60 — выбор владельца (решение 245): сильнее ≈ ±40 % хода соседние
    /// цвета на плавном переходе меняются местами («складка»), как это позволяют и редакторы.
    pub band_hue_degrees: KnobRange,
    /// Насыщенность полосы: множитель `1 + величина` (минус не дальше 1 — серый).
    pub band_saturation: KnobRange,
    /// Яркость полосы: множитель светлоты `1 + величина` (минус строго меньше 1 — не чёрное).
    pub band_luminance: KnobRange,
    /// Предел размера LUT: узлов на ось (`LookLut::new`). `[estimate]`.
    pub max_lut_size: u32,
    /// Колёса цвета по зонам: сила на краю и граница зон (решение 191).
    pub wheels: WheelModelSettings,
    /// Полоски Lift/Gamma/Gain/Offset по R, G, B: основа каналов и сила на краю (решения 339–342).
    pub primaries: PrimariesModelSettings,
}

impl Default for LookModelSettings {
    fn default() -> Self {
        Self {
            tone: ToneShapeSettings::default(),
            saturation: KnobRange { minus: 1.0, plus: 2.0 },
            vibrance: KnobRange::symmetric(1.0),
            warmth_mired: KnobRange::symmetric(150.0),
            tint_duv: KnobRange::symmetric(0.033),
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
            band_hue_degrees: KnobRange::symmetric(60.0),
            band_saturation: KnobRange { minus: 1.0, plus: 1.5 },
            band_luminance: KnobRange::symmetric(0.7),
            max_lut_size: 65,
            wheels: WheelModelSettings::default(),
            primaries: PrimariesModelSettings::default(),
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
    #[error("колёса цвета: {0}")]
    Wheels(#[from] InvalidWheelModel),
    #[error("полоски: {0}")]
    Primaries(#[from] InvalidPrimariesModel),
}

/// Наименьший зазор между центрами полос, градусы (между ближе окна вырождаются).
pub const MIN_BAND_GAP_DEGREES: f32 = 1.0;
/// Наибольший размер LUT, который допускает конфиг (память буфера GPU: `N³·12` байт).
pub const MAX_LUT_SIZE_LIMIT: u32 = 129;

/// Наибольший минус множителя `1 + величина`: ровно до нуля.
const MULTIPLIER_MINUS_LIMIT: f32 = 1.0;
/// Наибольший плюс множителей цветности: дальше любой цвет уходит за охват экрана.
const MAX_SATURATION_PLUS: f32 = 4.0;

impl InvalidLookModel {
    /// Число конечно и лежит в `min…max` (включительно).
    pub(super) fn check_number(field: &'static str, value: f32, min: f32, max: f32) -> Result<(), Self> {
        if value.is_finite() && value >= min && value <= max {
            Ok(())
        } else {
            Err(Self::OutOfRange { field, value, min, max })
        }
    }

    /// Оба края ручки — модули в `0…max`.
    pub(super) fn check_knob_range(field: &'static str, range: KnobRange, max: f32) -> Result<(), Self> {
        Self::check_knob_edges(field, range, max, max)
    }

    /// Края ручки — модули, у каждой стороны свой предел.
    pub(super) fn check_knob_edges(
        field: &'static str,
        range: KnobRange,
        max_minus: f32,
        max_plus: f32,
    ) -> Result<(), Self> {
        Self::check_number(field, range.minus, 0.0, max_minus)?;
        Self::check_number(field, range.plus, 0.0, max_plus)
    }
}

impl LookModelSettings {
    /// Проверка диапазонов и набора полос.
    pub fn validate(&self) -> Result<(), InvalidLookModel> {
        self.tone.validate()?;
        // Минус множителя `1 + величина` не дальше 1 (серый): больше — цветность сменила бы знак.
        InvalidLookModel::check_knob_edges(
            "saturation",
            self.saturation,
            MULTIPLIER_MINUS_LIMIT,
            MAX_SATURATION_PLUS,
        )?;
        InvalidLookModel::check_knob_edges("vibrance", self.vibrance, MULTIPLIER_MINUS_LIMIT, MAX_SATURATION_PLUS)?;
        InvalidLookModel::check_knob_range("warmth_mired", self.warmth_mired, 300.0)?;
        InvalidLookModel::check_knob_range("tint_duv", self.tint_duv, 0.05)?;
        let check = InvalidLookModel::check_number;
        check("vibrance_chroma_reference", self.vibrance_chroma_reference, 1e-3, 1.0)?;
        check("skin.hue_center_degrees", self.skin.hue_center_degrees, 0.0, 360.0)?;
        check("skin.half_width_degrees", self.skin.half_width_degrees, 1.0, 179.0)?;
        check("skin.protection", self.skin.protection, 0.0, 1.0)?;
        check("neutral_chroma_floor", self.neutral_chroma_floor, 1e-4, 1.0)?;
        InvalidLookModel::check_knob_range("band_hue_degrees", self.band_hue_degrees, 90.0)?;
        InvalidLookModel::check_knob_edges(
            "band_saturation",
            self.band_saturation,
            MULTIPLIER_MINUS_LIMIT,
            MAX_SATURATION_PLUS,
        )?;
        InvalidLookModel::check_knob_edges("band_luminance", self.band_luminance, 0.95, 2.0)?;
        if self.max_lut_size < 2 || self.max_lut_size > MAX_LUT_SIZE_LIMIT {
            return Err(InvalidLookModel::LutLimit {
                value: self.max_lut_size,
                max: MAX_LUT_SIZE_LIMIT,
            });
        }
        self.validate_bands()?;
        self.wheels.validate()?;
        self.primaries.validate()?;
        Ok(())
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
