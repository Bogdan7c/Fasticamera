//! Слой вида (решения 98, 99, 176, 178): ручки тона и цвета, теплота/оттенок, LUT — разрешённые в числа кадра.
//!
//! # Границы
//! * Словарь значений — `look-contract` ([`LookSettings`]): что крутит человек, значения ручек `[−1, 1]`.
//! * [`model::LookModelSettings`] (конфиг) — «насколько сильна ручка на краю» и набор полос оттенка.
//! * [`LookParams`] — разрешённое на кадр, лежит ВНУТРИ [`crate::params::IspParams`] (с версией типа).
//!
//! # Где что применяется (порядок по цепочке кадра)
//! 1. **Теплота, оттенок** → [`white::WhiteShift`]: сдвиг белой точки «для рисования» ДО перевода белой точки в
//!    нейтраль камеры (делает [`crate::params::IspParams::resolve`]). AE, статистика AWB, `AsShotNeutral` DNG,
//!    сглаживание AWB и белая вспышки видят НЕсдвинутую точку.
//! 2. **Контраст, средние тона, тени, света** → [`ToneKnobAmounts`]: через единый шов модели основы
//!    ([`crate::tone::knobs`]); вторая кривая не добавляется.
//! 3. **Колёса цвета по зонам, насыщенность, сочность, полосы** → [`color::LookColorParams`]: стадия в OKLab
//!    рабочего пространства `W` после основы; колёса ([`wheels::LookWheels`]) — её первый шаг; нейтраль — пропуск
//!    стадии флагом (`None`), нейтральные колёса — пропуск своего шага (`wheels: None`).
//! 4. **LUT** → [`lut::LookLut`]: после цвета вида, до перевода в цвета выхода.
//!
//! Стадии 3 и 4 и сдвиг белой не знают, какая модель основы выбрана: от неё берётся только рабочее
//! пространство `W` ([`crate::tone::base::ToneBase::working_primaries`]).

pub mod bands;
pub mod color;
pub mod lut;
pub mod model;
pub mod stage;
pub mod validation;
pub mod wheels;
pub mod white;

use std::sync::Arc;

use look_contract::{HueBandId, LookKnob, LookSettings};

use crate::color::chromaticity::Primaries;
use crate::tone::knobs::{ToneKnobAmounts, ToneKnobUnsupported};
use color::LookColorParams;
use lut::LookLut;
use model::{InvalidLookModel, LookModelSettings};
use white::{WhiteShift, WhiteShiftLimit};

/// Версия набора разрешённых чисел вида (растёт, когда меняется смысл или состав полей [`LookParams`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LookParamsVersion(pub u32);

/// Текущая версия [`LookParams`]: 1 — первая модель (14a), 2 — колёса цвета по зонам в стадии цвета (14i).
pub const CURRENT_LOOK_PARAMS_VERSION: LookParamsVersion = LookParamsVersion(2);

/// Входы вида в обработку кадра: значения ручек, модель вида и (опционально) таблица.
///
/// Нейтральное умолчание ([`Default`]): все ручки в нуле, модель по умолчанию, таблицы нет — картинка равна
/// основе без вида бит в бит.
#[derive(Debug, Clone, PartialEq)]
pub struct LookControls {
    /// Значения ручек (то, что крутит человек; снимок берёт их на момент нажатия).
    pub settings: LookSettings,
    /// Модель вида из конфига (общая на все кадры, поэтому `Arc`).
    pub model: Arc<LookModelSettings>,
    /// Таблица вида (задел, импорт — бэклог).
    pub lut: Option<Arc<LookLut>>,
}

impl LookControls {
    /// Вид с этими значениями ручек и моделью; без таблицы.
    pub fn new(settings: LookSettings, model: Arc<LookModelSettings>) -> Self {
        Self {
            settings,
            model,
            lut: None,
        }
    }

    /// То же, с таблицей.
    #[must_use]
    pub fn with_lut(self, lut: Arc<LookLut>) -> Self {
        Self { lut: Some(lut), ..self }
    }
}

impl Default for LookControls {
    fn default() -> Self {
        Self::new(LookSettings::NEUTRAL, Arc::new(LookModelSettings::default()))
    }
}

/// Что при разрешении вида пришлось проигнорировать: не ошибка, а диагностика.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LookReport {
    /// Правки полос, которых нет в модели (id неизвестен): игнорируются.
    pub unknown_band_ids: Vec<HueBandId>,
    /// Ненулевые ручки тона, которых выбранная модель основы не поддерживает: не применены.
    pub unsupported_tone_knobs: Vec<ToneKnobUnsupported>,
    /// Упёрся ли сдвиг белой в край таблицы Робертсона (заполняет `IspParams::resolve`).
    pub white_shift_limit: WhiteShiftLimit,
}

/// Вид, разрешённый на кадр.
#[derive(Debug, Clone, PartialEq)]
pub struct LookParams {
    pub version: LookParamsVersion,
    /// Ручки тона в единицах, не зависящих от модели основы.
    pub tone: ToneKnobAmounts,
    /// Сдвиг белой точки для рисования; применяется в `IspParams::resolve` до перевода в нейтраль камеры.
    pub white_shift: WhiteShift,
    /// Стадия цвета вида; `None` — нейтрально, пропускается целиком.
    pub color: Option<LookColorParams>,
    /// Таблица после цвета вида.
    pub lut: Option<Arc<LookLut>>,
    /// Диагностика разрешения.
    pub report: LookReport,
}

impl LookParams {
    /// Разрешает вид. `working` — рабочее пространство основы (`W`).
    pub fn resolve(controls: &LookControls, working: &Primaries) -> Result<Self, ResolveLookError> {
        let model = &*controls.model;
        model.validate()?;
        if let Some(lut) = controls.lut.as_ref().filter(|lut| lut.size() > model.max_lut_size) {
            return Err(InvalidLookModel::LutLargerThanLimit {
                size: lut.size(),
                max: model.max_lut_size,
            }
            .into());
        }
        let resolved_color = LookColorParams::resolve(&controls.settings, model, working)?;
        Ok(Self {
            version: CURRENT_LOOK_PARAMS_VERSION,
            tone: tone_amounts(model, &controls.settings),
            white_shift: white_shift(model, &controls.settings),
            color: resolved_color.params,
            lut: controls.lut.clone(),
            report: LookReport {
                unknown_band_ids: resolved_color.unknown_band_ids,
                unsupported_tone_knobs: Vec::new(),
                white_shift_limit: WhiteShiftLimit::Unlimited,
            },
        })
    }

    /// Ничего не меняет в картинке (кроме, возможно, пустой диагностики): тон, цвет, белая, LUT — нейтральны.
    pub fn is_neutral(&self) -> bool {
        self.tone.is_neutral() && self.white_shift.is_none() && self.color.is_none() && self.lut.is_none()
    }

    /// Применяет цвет вида и таблицу к свету `W` одного пикселя (порядок: цвет, затем таблица).
    pub fn apply_to_pixel(&self, working_rgb: [f32; 3]) -> [f32; 3] {
        let colored = match &self.color {
            Some(color) => color.apply(working_rgb),
            None => working_rgb,
        };
        match &self.lut {
            Some(lut) => lut.apply(colored),
            None => colored,
        }
    }
}

/// Вид не разрешается.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ResolveLookError {
    #[error("модель вида: {0}")]
    Model(#[from] InvalidLookModel),
    #[error("матрицы OKLab рабочего пространства вырождены: {0}")]
    Singular(#[from] crate::color::matrix::SingularMatrix),
}

/// Ручки тона → единицы без привязки к модели основы (диапазоны — модель вида). Нуль ручки — ровно нуль.
pub fn tone_amounts(model: &LookModelSettings, settings: &LookSettings) -> ToneKnobAmounts {
    let scaled = |knob: LookKnob, range: f32| if knob.is_neutral() { 0.0 } else { knob.get() * range };
    ToneKnobAmounts {
        contrast_log2: scaled(settings.contrast, model.contrast_range_log2),
        midtones_ev: scaled(settings.midtones, model.midtones_range_ev),
        shadows: scaled(settings.shadows, model.shadows_range_log2),
        highlights: scaled(settings.highlights, model.highlights_range_log2),
    }
}

/// Теплота и оттенок → сдвиг белой точки. «+ теплота» СНИЖАЕТ майреды предполагаемого света (картинка
/// теплее), «+ оттенок» поднимает Duv предполагаемого света (картинка пурпурнее): см. [`white`].
pub fn white_shift(model: &LookModelSettings, settings: &LookSettings) -> WhiteShift {
    WhiteShift {
        mired: if settings.warmth.is_neutral() {
            0.0
        } else {
            -settings.warmth.get() * model.warmth_range_mired
        },
        duv: if settings.tint.is_neutral() {
            0.0
        } else {
            settings.tint.get() * model.tint_range_duv
        },
    }
}

#[cfg(test)]
mod tests;
