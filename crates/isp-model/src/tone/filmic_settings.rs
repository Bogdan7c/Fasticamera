//! Настройки основы filmic (конфиг) и их проверка. Математика — в [`crate::tone::filmic`].
//!
//! Все числа — стартовые оценки `[estimate]`: их подбирает владелец в 14b на своих сценах; начальные
//! значения выбраны замером на синтетике (тесты `filmic_color_tests`, отчёт пакета).

use crate::color::matrix::SingularMatrix;
use crate::exposure::ExposureGain;
use crate::gamut::GamutCompressionSettings;
use crate::tone::knobs::{ToneKnob, ToneKnobUnsupported, ToneModelKind, ToneModelSettings};
use crate::tone::working_io::{InvalidWorkingIo, WorkingInputGamut, WorkingIoSettings};

/// Норма пикселя: по какому числу считается кривая, а цвет сохраняется масштабированием RGB (описание идеи
/// filmic: «кривая применяется к яркости-норме, каналы делятся на норму до кривой и умножаются после»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmicNorm {
    /// Без сохранения цветности: кривая по каждому каналу отдельно (насыщает тени, обесцвечивает света).
    PerChannel,
    /// Наибольший из каналов: отношения каналов сохраняются точно, но насыщенные цвета в белое сами не уходят
    /// (его ведёт шаг «обесцвечивание в светах», см. [`FilmicSettings::desaturation_onset`]).
    MaxRgb,
    /// Яркость `Y` рабочего пространства (строка `Y` матрицы Rec. 2020 → XYZ, нормированная на сумму 1).
    Luminance,
    /// «Степенная норма» `(R³ + G³ + B³)/(R² + G² + B²)`: компромисс между максимумом и яркостью.
    PowerNorm,
}

/// Настройки основы тона filmic (см. [`crate::tone::filmic`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FilmicSettings {
    /// Средний серый сцены в нормированном свете (до подтяжки AE). `[estimate]` 0,075 — как у AgX и прежней
    /// основы, чтобы экспозиция кадров совпадала.
    pub scene_middle_gray: f32,
    /// Во что превращается средний серый на экране (линейный свет 0…1). `[estimate]` 0,20.
    pub display_middle_gray: f32,
    /// Показатель `γ`: выход кривой `y` превращается в линейный свет как `y^γ`. `[estimate]` 2,4.
    pub display_gamma: f32,
    /// Контраст: наклон линейного участка (латитюды) в единицах `y` на ступень. `[estimate]`.
    pub slope: f32,
    /// Глубина чёрного `Δ_black`: ступеней ниже серого, где кривая даёт ровно 0. `[estimate]`.
    pub black_depth_ev: f32,
    /// Доля пути от серого к чёрному (по выходу кривой `y`), которую проходит линейный участок вниз: он
    /// кончается там, где `y = (1 − доля)·y_p`. Диапазон (0, 1). Длина участка в ступенях выводится:
    /// `доля·y_p/slope`. `[estimate]`.
    pub toe_linear_share: f32,
    /// То же вверх: доля пути от серого к белому (по `y`), которую проходит линейный участок. Диапазон (0, 1).
    /// `[estimate]`.
    pub shoulder_linear_share: f32,
    /// Степень носка `p0` (> 0; больше — резче излом у чёрного; при `p0 > 1` стык с прямой гладкий C2, при
    /// `p0 ≤ 1` — C1). `[estimate]`.
    pub toe_power: f32,
    /// Степень плеча `p1` (> 0; те же свойства). `[estimate]`.
    pub shoulder_power: f32,
    /// Норма пикселя (сохранение цветности). `[estimate]`.
    pub norm: FilmicNorm,
    /// Уровень выхода кривой `y`, с которого цвет гладко ведётся в белое (обесцвечивание в светах): вес
    /// `smootherstep((y − порог)/(1 − порог))`, на белом точки он равен 1. Диапазон [0, 1). Для нормы
    /// [`FilmicNorm::PerChannel`] не используется. `[estimate]`.
    pub desaturation_onset: f32,
    /// Вход в рабочее пространство: что делать с цветом за охватом Rec. 2020 (тот же тип и шаг, что у AgX).
    pub input_gamut: WorkingInputGamut,
    /// Мягкое сжатие охвата выхода (общий шаг «W → цвета выхода», как у AgX). `[estimate]`.
    pub output_gamut_compression: GamutCompressionSettings,
}

impl FilmicSettings {
    /// Сжатие охвата выхода по умолчанию: пороги 0,95, предел 1,25, степень 1,2. Пороги ACES RGC (0,80–0,88)
    /// сжимали бы уже внутренние насыщенные цвета (замер пакета: закат уходил на 6,5° против 2,5° с этими
    /// порогами); сжимаем только цвета у границы охвата и за ней. `[estimate]`.
    pub const DEFAULT_OUTPUT_COMPRESSION: GamutCompressionSettings = GamutCompressionSettings {
        threshold: [0.95; 3],
        distance_limit: [1.25; 3],
        power: 1.2,
    };

    /// Остаточная подтяжка AE `gain` (решение 72): серый сцены делится на `k`. Деление на `UNITY` точное.
    pub fn with_exposure_gain(&self, gain: ExposureGain) -> Self {
        Self {
            scene_middle_gray: self.scene_middle_gray / gain.multiplier(),
            ..*self
        }
    }
}

impl FilmicSettings {
    /// Настройки общего шага рабочего пространства ([`WorkingSpaceIo`](crate::tone::working_io::WorkingSpaceIo)).
    pub fn working_io(&self) -> WorkingIoSettings {
        WorkingIoSettings {
            input_gamut: self.input_gamut,
            output_gamut_compression: self.output_gamut_compression,
        }
    }
}

impl Default for FilmicSettings {
    fn default() -> Self {
        Self {
            scene_middle_gray: 0.075,
            display_middle_gray: 0.20,
            display_gamma: 2.4,
            slope: 0.18,
            black_depth_ev: 8.0,
            toe_linear_share: 0.5,
            shoulder_linear_share: 0.5,
            toe_power: 2.0,
            shoulder_power: 2.8,
            norm: FilmicNorm::PowerNorm,
            desaturation_onset: 0.7,
            input_gamut: WorkingInputGamut::ClipToZero,
            output_gamut_compression: Self::DEFAULT_OUTPUT_COMPRESSION,
        }
    }
}

/// Основа filmic не строится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidFilmic {
    #[error("{field} = {value}: нужно конечное число в (0, 1) (для серого сцены — после деления на подтяжку AE)")]
    OutOfUnitInterval { field: &'static str, value: f32 },
    #[error("{field} = {value}: нужно конечное число в [0, 1)")]
    OutOfHalfOpenUnitInterval { field: &'static str, value: f32 },
    #[error("{field} = {value}: нужно конечное число > 0")]
    NotPositive { field: &'static str, value: f32 },
    #[error(
        "плечо не помещается: наклон {slope} за запас {headroom_ev} ступеней до белого даёт {rise} < {required} \
         (нужен наклон не меньше {minimum_slope})"
    )]
    ShoulderTooShallow {
        slope: f32,
        headroom_ev: f32,
        rise: f32,
        required: f32,
        minimum_slope: f32,
    },
    #[error(
        "носок не помещается: наклон {slope} за глубину {black_depth_ev} ступеней даёт {rise} < {required} \
         (нужен наклон не меньше {minimum_slope} или глубина не меньше {minimum_depth_ev})"
    )]
    ToeTooShallow {
        slope: f32,
        black_depth_ev: f32,
        rise: f32,
        required: f32,
        minimum_slope: f32,
        minimum_depth_ev: f32,
    },
    #[error("матрицы рабочего пространства вырождены")]
    SingularMatrix(#[from] SingularMatrix),
    #[error("общий шаг входа/выхода (охват): {0}")]
    SharedStage(#[from] InvalidWorkingIo),
}

impl FilmicSettings {
    /// Проверка диапазонов отдельных чисел (связи между ними — при разрешении кривой).
    pub(super) fn validate_ranges(&self) -> Result<(), InvalidFilmic> {
        let open_unit = |field: &'static str, value: f32| {
            if value.is_finite() && value > 0.0 && value < 1.0 {
                Ok(())
            } else {
                Err(InvalidFilmic::OutOfUnitInterval { field, value })
            }
        };
        let positive = |field: &'static str, value: f32| {
            if value.is_finite() && value > 0.0 {
                Ok(())
            } else {
                Err(InvalidFilmic::NotPositive { field, value })
            }
        };
        open_unit("scene_middle_gray", self.scene_middle_gray)?;
        open_unit("display_middle_gray", self.display_middle_gray)?;
        open_unit("toe_linear_share", self.toe_linear_share)?;
        open_unit("shoulder_linear_share", self.shoulder_linear_share)?;
        positive("display_gamma", self.display_gamma)?;
        positive("slope", self.slope)?;
        positive("black_depth_ev", self.black_depth_ev)?;
        positive("toe_power", self.toe_power)?;
        positive("shoulder_power", self.shoulder_power)?;
        let onset = self.desaturation_onset;
        if !(onset.is_finite() && (0.0..1.0).contains(&onset)) {
            return Err(InvalidFilmic::OutOfHalfOpenUnitInterval {
                field: "desaturation_onset",
                value: onset,
            });
        }
        Ok(())
    }
}

impl ToneModelSettings for FilmicSettings {
    const KIND: ToneModelKind = ToneModelKind::Filmic;

    /// filmic поддерживает все четыре ручки: контраст — наклон латитюды `s`, средние тона — серый экрана,
    /// тени — степень носка `p0`, света — степень плеча `p1`. Направления проверены замером (тест
    /// `shadows_and_highlights_knobs_...`): меньшая `p0` — тени светлее (степень делится на `2^amount`),
    /// большая `p1` — света светлее (умножается). Форма носка/плеча не затрагивает линейный участок: ручки
    /// теней и светов не меняют ни серый, ни наклон, ни другую сторону кривой. Строимость кривой (`s·Δ_b > y_p`,
    /// `s·Δ_w > 1 − y_p`) от степеней не зависит, поэтому на краях решаются лишь контраст и средние тона.
    fn with_knob(&self, knob: ToneKnob, amount: f32) -> Result<Self, ToneKnobUnsupported> {
        let scale = amount.exp2();
        Ok(match knob {
            ToneKnob::Contrast => Self {
                slope: self.slope * scale,
                ..*self
            },
            ToneKnob::Midtones => Self {
                display_middle_gray: self.display_middle_gray * scale,
                ..*self
            },
            ToneKnob::Shadows => Self {
                toe_power: self.toe_power / scale,
                ..*self
            },
            ToneKnob::Highlights => Self {
                shoulder_power: self.shoulder_power * scale,
                ..*self
            },
        })
    }
}
