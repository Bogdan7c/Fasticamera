//! Настройки основы ACES (конфиг), их проверка и ручки тона. Математика — в [`super`] и [`super::tonescale`].
//!
//! Числа ACES, которые определяют сам стандарт (коэффициенты сплайнов, ширина красного, опорная точка
//! свечения), живут константами в своих модулях; здесь — только то, что владелец вправе менять: куда
//! класть серый, форма колена, сила «подслащивания» и поправки выходного преобразования. Все стартовые числа
//! помечены `[estimate]`: их выбирает владелец в 14b на своих сценах.

use crate::exposure::ExposureGain;
use crate::gamut::GamutCompressionSettings;
use crate::tone::knobs::{ToneKnob, ToneKnobUnsupported, ToneModelKind, ToneModelSettings};
use crate::tone::working_io::{InvalidWorkingIo, WorkingInputGamut, WorkingIoSettings};

/// Наибольшая крутизна функции `t²/(t² + w²)` равна `3√3/(8w) ≈ 0,6495/w`: при ней сдвиг формы тени или света
/// ещё не разворачивает ось экспозиции (монотонность).
pub const SHAPE_SLOPE_FACTOR: f64 = 0.649_519_052_838_329;

/// Настройки основы тона ACES.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AcesSettings {
    /// Средний серый сцены в нормированном свете (до подтяжки AE): какой свет считать 18 %-серым.
    /// `[estimate]` 0,075 — как у прежней основы и AgX, чтобы экспозиция кадров совпадала.
    pub scene_middle_gray: f32,
    /// Во что превращается средний серый на экране (линейный свет 0…1). `[estimate]` 0,20, как у AgX.
    /// Чистый ACES даёт серому 0,18 около 0,10; недостающее поднимает сдвиг экспозиции модели (`shift_ev`).
    pub display_middle_gray: f32,
    /// Контраст: множитель наклона оси экспозиции вокруг серого (в логарифме, как степень ASC CDL). 1 — как в ACES.
    /// `[estimate]` 1,0.
    pub contrast: f32,
    /// Тени: сдвиг глубоких теней в ступенях (`+` — светлее), гладко сходит на нет у серого. `[estimate]` 0.
    pub shadows_ev: f32,
    /// Света: сдвиг света выше серого в ступенях (`+` — светлее), гладко сходит на нет у серого. `[estimate]` 0.
    pub highlights_ev: f32,
    /// Ширина перехода форм теней и света: ступеней от серого, где сдвиг достигает половины. `[estimate]` 2.
    pub shape_width_ev: f32,
    /// Сколько ступеней сдвига даёт единица ручки теней и света слоя вида. `[estimate]` 2.
    pub knob_shape_ev_per_unit: f32,
    /// Колено белой точки: ступеней выше серого (после формы теней и света), где кривая ACES сменяется плавным
    /// подходом к белому точки (`L` → ровно белый экрана). Серый и всё ниже колена от `L` не зависят. `[estimate]` 1,5.
    pub knee_ev: f32,
    /// Сила свечения RRT (`RRT_GLOW_GAIN`; в ACES 0,05; 0 — без свечения). `[estimate]`.
    pub glow_gain: f32,
    /// Масштаб красного модификатора RRT (`RRT_RED_SCALE`; в ACES 0,82; 1 — без модификатора). `[estimate]`.
    pub red_modifier_scale: f32,
    /// Глобальная десатурация RRT (`RRT_SAT_FACTOR`; в ACES 0,96; 1 — без неё). `[estimate]`.
    pub rrt_saturation: f32,
    /// Десатурация выходного преобразования (`ODT_SAT_FACTOR`; в ACES 0,93 для 100 нит; 1 — без неё). `[estimate]`.
    pub odt_saturation: f32,
    /// Поправка тусклого окружения (`DIM_SURROUND_GAMMA`; в ACES 0,9811; 1 — без поправки). `[estimate]`.
    pub surround_gamma: f32,
    /// Вход в рабочее пространство: что делать с цветом за охватом Rec. 2020 (общий шаг с AgX).
    pub input_gamut: WorkingInputGamut,
    /// Мягкое сжатие охвата выхода (общий шаг с AgX). `[estimate]`.
    pub output_gamut_compression: GamutCompressionSettings,
}

impl AcesSettings {
    /// Настройки общего шага рабочего пространства ([`WorkingSpaceIo`](crate::tone::working_io::WorkingSpaceIo)).
    pub fn working_io(&self) -> WorkingIoSettings {
        WorkingIoSettings {
            input_gamut: self.input_gamut,
            output_gamut_compression: self.output_gamut_compression,
        }
    }
}

impl Default for AcesSettings {
    fn default() -> Self {
        Self {
            scene_middle_gray: 0.075,
            display_middle_gray: 0.20,
            contrast: 1.0,
            shadows_ev: 0.0,
            highlights_ev: 0.0,
            shape_width_ev: 2.0,
            knob_shape_ev_per_unit: 2.0,
            knee_ev: 1.5,
            glow_gain: 0.05,
            red_modifier_scale: 0.82,
            rrt_saturation: 0.96,
            odt_saturation: 0.93,
            surround_gamma: 0.9811,
            input_gamut: WorkingInputGamut::ClipToZero,
            output_gamut_compression: GamutCompressionSettings::default(),
        }
    }
}

impl AcesSettings {
    /// Остаточная подтяжка AE `gain` (решение 72): средний серый сцены делится на `k`; белый точки прежний.
    /// Деление на `UNITY` точное — без подтяжки настройки не меняются на бит.
    pub fn with_exposure_gain(&self, gain: ExposureGain) -> Self {
        Self {
            scene_middle_gray: self.scene_middle_gray / gain.multiplier(),
            ..*self
        }
    }
}

/// Основа ACES не строится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidAces {
    #[error("{field} = {value}: нужно конечное число в (0, 1) (для серого сцены — после деления на подтяжку AE)")]
    OutOfUnitInterval { field: &'static str, value: f32 },
    #[error("{field} = {value}: нужно конечное число > 0")]
    NotPositive { field: &'static str, value: f32 },
    #[error("{field} = {value}: нужно конечное число в [{minimum}, {maximum}]")]
    OutOfRange {
        field: &'static str,
        value: f32,
        minimum: f32,
        maximum: f32,
    },
    #[error("{field} = {value}: нужно конечное число")]
    NotFinite { field: &'static str, value: f32 },
    #[error(
        "форма теней и света разворачивает ось экспозиции: contrast·shape_width_ev = {slope_budget} должно быть \
         больше {needed} (0,65·наибольший сдвиг)"
    )]
    ShapeNotMonotone { slope_budget: f32, needed: f32 },
    #[error(
        "колено {knee_ev} ступеней лежит за шкалой ACES: с поправкой серого {shift_ev} оно выше {limit_ev} ступеней \
         над серым (белый ACES)"
    )]
    KneeBeyondToneScale { knee_ev: f32, shift_ev: f32, limit_ev: f32 },
    #[error("кривая в колене {knee_ev} не растёт: наклон {slope}")]
    KneeSlopeNotPositive { knee_ev: f32, slope: f32 },
    #[error(
        "белый точки (L = 1) лежит на {white_ev} ступенях над серым, а колено на {knee_ev}: нужен запас не меньше \
         {minimum_headroom_ev} ступени"
    )]
    WhiteTooCloseToKnee {
        white_ev: f32,
        knee_ev: f32,
        minimum_headroom_ev: f32,
    },
    #[error("общий шаг входа и выхода: {0}")]
    Shared(#[from] InvalidWorkingIo),
    #[error("матрицы рабочего пространства или ACES вырождены")]
    SingularMatrix(#[from] crate::color::matrix::SingularMatrix),
}

impl AcesSettings {
    /// Проверка диапазонов отдельных чисел (связи между ними — при разрешении кривой).
    pub(super) fn validate_ranges(&self) -> Result<(), InvalidAces> {
        let unit = |field: &'static str, value: f32| {
            if value.is_finite() && value > 0.0 && value < 1.0 {
                Ok(())
            } else {
                Err(InvalidAces::OutOfUnitInterval { field, value })
            }
        };
        let positive = |field: &'static str, value: f32| {
            if value.is_finite() && value > 0.0 {
                Ok(())
            } else {
                Err(InvalidAces::NotPositive { field, value })
            }
        };
        let finite = |field: &'static str, value: f32| {
            if value.is_finite() {
                Ok(())
            } else {
                Err(InvalidAces::NotFinite { field, value })
            }
        };
        let within = |field: &'static str, value: f32, minimum: f32, maximum: f32| {
            if value.is_finite() && (minimum..=maximum).contains(&value) {
                Ok(())
            } else {
                Err(InvalidAces::OutOfRange {
                    field,
                    value,
                    minimum,
                    maximum,
                })
            }
        };
        unit("scene_middle_gray", self.scene_middle_gray)?;
        unit("display_middle_gray", self.display_middle_gray)?;
        positive("contrast", self.contrast)?;
        finite("shadows_ev", self.shadows_ev)?;
        finite("highlights_ev", self.highlights_ev)?;
        positive("shape_width_ev", self.shape_width_ev)?;
        positive("knob_shape_ev_per_unit", self.knob_shape_ev_per_unit)?;
        positive("knee_ev", self.knee_ev)?;
        within("glow_gain", self.glow_gain, 0.0, 1.0)?;
        within("red_modifier_scale", self.red_modifier_scale, 0.0, 1.0)?;
        within("rrt_saturation", self.rrt_saturation, 0.0, 1.5)?;
        within("odt_saturation", self.odt_saturation, 0.0, 1.5)?;
        within("surround_gamma", self.surround_gamma, 0.5, 1.5)?;
        let needed = SHAPE_SLOPE_FACTOR as f32 * self.shadows_ev.abs().max(self.highlights_ev.abs());
        let slope_budget = self.contrast * self.shape_width_ev;
        if slope_budget <= needed {
            return Err(InvalidAces::ShapeNotMonotone { slope_budget, needed });
        }
        Ok(())
    }
}

impl ToneModelSettings for AcesSettings {
    const KIND: ToneModelKind = ToneModelKind::Aces;

    /// ACES: контраст — множитель наклона оси экспозиции вокруг серого (`× 2^amount`, как CDL); средние тона —
    /// серый экрана (`× 2^amount`, как у AgX: серый сцены попадает ровно в новый серый); тени и света — сдвиг
    /// глубоких теней и света выше серого в ступенях (`amount × knob_shape_ev_per_unit`). Серый сцены и наклон на
    /// нём формой теней и света не меняются (C1 в нуле); у контраста и средних тонов серый — опора.
    /// Неподдержанных ручек у ACES нет.
    fn with_knob(&self, knob: ToneKnob, amount: f32) -> Result<Self, ToneKnobUnsupported> {
        let scale = amount.exp2();
        let shift = amount * self.knob_shape_ev_per_unit;
        Ok(match knob {
            ToneKnob::Contrast => Self {
                contrast: self.contrast * scale,
                ..*self
            },
            ToneKnob::Midtones => Self {
                display_middle_gray: self.display_middle_gray * scale,
                ..*self
            },
            ToneKnob::Shadows => Self {
                shadows_ev: self.shadows_ev + shift,
                ..*self
            },
            ToneKnob::Highlights => Self {
                highlights_ev: self.highlights_ev + shift,
                ..*self
            },
        })
    }
}
