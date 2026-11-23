//! Настройки основы AgX (конфиг) и их проверка. Математика — в [`super::agx`].
//!
//! Все числа — стартовые оценки `[estimate]`: их подбирает владелец в 14b на своих сценах; начальные
//! значения выбраны замером на синтетике (см. отчёт пакета P1 и тесты `agx_color_tests`).

use super::working_io::{InvalidWorkingIo, WorkingInputGamut, WorkingIoSettings};
use crate::color::working_space::{InvalidPrimaryAdjustment, PrimaryAdjustment};
use crate::exposure::ExposureGain;
use crate::gamut::GamutCompressionSettings;

/// Настройки основы тона AgX (см. [`super::agx`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgxSettings {
    /// Средний серый сцены в нормированном свете (до подтяжки AE): какой свет считать 18 %-серым.
    /// `[estimate]` 0,075 — как у прежней основы, чтобы экспозиция кадров совпадала.
    pub scene_middle_gray: f32,
    /// Во что превращается средний серый на экране (линейный свет 0…1). `[estimate]` 0,20.
    pub display_middle_gray: f32,
    /// Показатель `γ`: выход кривой `y` превращается в линейный свет как `y^γ`. `[estimate]` 2,4.
    pub display_gamma: f32,
    /// Наклон кривой на сером, единиц `y` на ступень (контраст). `[estimate]`.
    pub slope: f32,
    /// Степень носка `p0` (> 0; больше — резче переход к прямой). `[estimate]`.
    pub toe_power: f32,
    /// Степень плеча `p1` (> 0). `[estimate]`.
    pub shoulder_power: f32,
    /// Глубина чёрного: ступеней ниже серого, где кривая даёт ровно 0 (`Δ_toe`). `[estimate]`.
    pub toe_depth_ev: f32,
    /// Матрица «внутрь»: сведение и поворот основных цветов R, G, B. `[estimate]`.
    pub inset: [PrimaryAdjustment; 3],
    /// Матрица «наружу». `[estimate]`; при равенстве с `inset` — точная обратная (цвета не меняются).
    pub outset: [PrimaryAdjustment; 3],
    /// Вход в рабочее пространство: что делать с цветом за охватом Rec. 2020 (выбрано замером).
    pub input_gamut: WorkingInputGamut,
    /// Мягкое сжатие охвата выхода (после матрицы в цвета экрана, перед обрезкой 0…1). `[estimate]`.
    pub output_gamut_compression: GamutCompressionSettings,
    /// Степень возврата оттенка после S-кривой, 0…1 (решение 180): 0 — «чистый» AgX, шаг пропускается целиком
    /// (бит в бит прежний выход); 1 — оттенок OKLCh полностью берётся с цвета до кривой. Умолчание 1: шаг есть
    /// и на GPU (видоискатель, качественный путь, снимок; паритет степеней 0, 0,5, 1 проверен). `[estimate]`.
    pub hue_restoration: f32,
    /// Гладкий порог серого для возврата оттенка: цветность (OKLab, приведённая к яркости выхода кривой), при
    /// которой вес возврата равен 1/2 (`w = C²/(C² + порог²)`). Ниже оттенок цвета до кривой — шум, и вес
    /// плавно сходит на нет. Остаток цветности у серого ≈ 5·10⁻⁴, порог — на порядки выше. `[estimate]`.
    pub hue_restoration_chroma_floor: f32,
}

impl AgxSettings {
    /// Остаточная подтяжка AE `gain` (решение 72): множитель света до логарифма, то есть средний серый
    /// сцены делится на `k`. Белый точки остаётся прежним. Деление на `UNITY` точное — без подтяжки
    /// настройки не меняются на бит.
    pub fn with_exposure_gain(&self, gain: ExposureGain) -> Self {
        Self {
            scene_middle_gray: self.scene_middle_gray / gain.multiplier(),
            ..*self
        }
    }
}

impl AgxSettings {
    /// Настройки общего шага рабочего пространства ([`WorkingSpaceIo`](super::working_io::WorkingSpaceIo)).
    pub fn working_io(&self) -> WorkingIoSettings {
        WorkingIoSettings {
            input_gamut: self.input_gamut,
            output_gamut_compression: self.output_gamut_compression,
        }
    }
}

impl Default for AgxSettings {
    fn default() -> Self {
        Self {
            scene_middle_gray: 0.075,
            display_middle_gray: 0.20,
            display_gamma: 2.4,
            slope: 0.18,
            toe_power: 2.0,
            shoulder_power: 2.8,
            toe_depth_ev: 7.0,
            inset: [
                PrimaryAdjustment {
                    amount: 0.51,
                    rotation_degrees: -10.4,
                },
                PrimaryAdjustment {
                    amount: 0.06,
                    rotation_degrees: 23.3,
                },
                PrimaryAdjustment {
                    amount: 0.08,
                    rotation_degrees: 3.4,
                },
            ],
            outset: [
                PrimaryAdjustment {
                    amount: 0.42,
                    rotation_degrees: -10.8,
                },
                PrimaryAdjustment {
                    amount: 0.24,
                    rotation_degrees: 20.9,
                },
                PrimaryAdjustment {
                    amount: 0.14,
                    rotation_degrees: 3.8,
                },
            ],
            input_gamut: WorkingInputGamut::ClipToZero,
            output_gamut_compression: GamutCompressionSettings::default(),
            hue_restoration: 1.0,
            hue_restoration_chroma_floor: 0.01,
        }
    }
}

/// Основа AgX не строится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidAgx {
    #[error("{field} = {value}: нужно конечное число в (0, 1) (для серого сцены — после деления на подтяжку AE)")]
    OutOfUnitInterval { field: &'static str, value: f32 },
    #[error("{field} = {value}: нужно конечное число > 0")]
    NotPositive { field: &'static str, value: f32 },
    #[error(
        "плечо вогнуто недостаточно: r1 = {ratio} ≤ 1 при белом 1 (наклон {slope} при запасе {headroom_ev} \
         ступеней); нужен наклон не меньше {minimum_slope}"
    )]
    ShoulderTooShallow {
        ratio: f32,
        slope: f32,
        headroom_ev: f32,
        minimum_slope: f32,
    },
    #[error(
        "носок вогнут недостаточно: r0 = {ratio} ≤ 1 (наклон {slope}, глубина {toe_depth_ev} ступеней); \
         нужна глубина не меньше {minimum_depth_ev}"
    )]
    ToeTooShallow {
        ratio: f32,
        slope: f32,
        toe_depth_ev: f32,
        minimum_depth_ev: f32,
    },
    #[error("матрица «внутрь», основной цвет {channel}: {source}")]
    Inset {
        channel: usize,
        #[source]
        source: InvalidPrimaryAdjustment,
    },
    #[error("матрица «наружу», основной цвет {channel}: {source}")]
    Outset {
        channel: usize,
        #[source]
        source: InvalidPrimaryAdjustment,
    },
    #[error("матрицы рабочего пространства вырождены")]
    SingularMatrix(#[from] crate::color::matrix::SingularMatrix),
    #[error("hue_restoration = {value}: нужно конечное число в [0, 1]")]
    HueRestorationOutOfRange { value: f32 },
    /// Общий шаг входа и выхода рабочего пространства не строится (сжатие охвата, матрицы `W`).
    #[error(transparent)]
    WorkingIo(#[from] InvalidWorkingIo),
}

impl AgxSettings {
    /// Проверка диапазонов отдельных чисел (связи между ними — при разрешении кривой).
    pub(super) fn validate_ranges(&self) -> Result<(), InvalidAgx> {
        let unit = |field: &'static str, value: f32| {
            if value.is_finite() && value > 0.0 && value < 1.0 {
                Ok(())
            } else {
                Err(InvalidAgx::OutOfUnitInterval { field, value })
            }
        };
        let positive = |field: &'static str, value: f32| {
            if value.is_finite() && value > 0.0 {
                Ok(())
            } else {
                Err(InvalidAgx::NotPositive { field, value })
            }
        };
        unit("scene_middle_gray", self.scene_middle_gray)?;
        unit("display_middle_gray", self.display_middle_gray)?;
        positive("display_gamma", self.display_gamma)?;
        positive("slope", self.slope)?;
        positive("toe_power", self.toe_power)?;
        positive("shoulder_power", self.shoulder_power)?;
        positive("toe_depth_ev", self.toe_depth_ev)?;
        positive("hue_restoration_chroma_floor", self.hue_restoration_chroma_floor)?;
        if !(self.hue_restoration.is_finite() && (0.0..=1.0).contains(&self.hue_restoration)) {
            return Err(InvalidAgx::HueRestorationOutOfRange {
                value: self.hue_restoration,
            });
        }
        for (channel, adjustment) in self.inset.iter().enumerate() {
            adjustment
                .validate()
                .map_err(|source| InvalidAgx::Inset { channel, source })?;
        }
        for (channel, adjustment) in self.outset.iter().enumerate() {
            adjustment
                .validate()
                .map_err(|source| InvalidAgx::Outset { channel, source })?;
        }
        Ok(())
    }
}
