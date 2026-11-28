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
//! 2. **Контраст, средние тона, тени, света** → [`tone_shape::LookTone`]: отдельная стадия сразу после основы,
//!    одинаковая для всех моделей (решения 236, 244); нейтраль — пропуск флагом (`None`).
//! 3. **Полоски Lift / Gamma / Gain / Offset по каналам R, G, B** → [`primaries::LookPrimaries`]: RGB между тоном
//!    и цветом (решение 341), каналы — основные цвета Display P3 (решение 342); нейтраль — пропуск (`None`).
//! 4. **Колёса цвета по зонам, насыщенность, сочность, полосы** → [`color::LookColorParams`]: стадия в OKLab
//!    рабочего пространства `W` после тона; колёса ([`wheels::LookWheels`]) — её первый шаг; нейтраль — пропуск
//!    стадии флагом (`None`), нейтральные колёса — пропуск своего шага (`wheels: None`).
//! 5. **LUT** → [`lut::LookLut`]: после цвета вида, до перевода в цвета выхода.
//!
//! Стадии 2–5 и сдвиг белой не знают, какая модель основы выбрана: от неё берётся только рабочее
//! пространство `W` ([`crate::tone::base::ToneBase::working_primaries`]).

pub mod bands;
pub mod color;
pub mod lut;
pub mod model;
pub mod primaries;
pub mod range;
pub mod stage;
pub mod tone_shape;
pub mod validation;
pub mod wheels;
pub mod white;
pub mod white_stops;

use std::sync::Arc;

use look_contract::{HueBandId, LookSettings};

use crate::color::chromaticity::Primaries;
use color::LookColorParams;
use lut::LookLut;
use model::{InvalidLookModel, LookModelSettings};
use primaries::LookPrimaries;
use tone_shape::LookTone;
use white::{WhiteShift, WhiteShiftLimit};

/// Версия набора разрешённых чисел вида (растёт, когда меняется смысл или состав полей [`LookParams`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LookParamsVersion(pub u32);

/// Текущая версия [`LookParams`]: 1 — первая модель (14a), 2 — колёса цвета по зонам в стадии цвета (14i),
/// 3 — тон вида отдельной стадией после основы (14l), 4 — полоски Lift/Gamma/Gain/Offset по R, G, B между тоном и
/// цветом (14t).
pub const CURRENT_LOOK_PARAMS_VERSION: LookParamsVersion = LookParamsVersion(4);

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
    /// Сжат ли ход теплоты краем таблицы Робертсона (заполняет `IspParams::resolve`).
    pub white_shift_limit: WhiteShiftLimit,
}

/// Вид, разрешённый на кадр.
#[derive(Debug, Clone, PartialEq)]
pub struct LookParams {
    pub version: LookParamsVersion,
    /// Стадия тона вида; `None` — все четыре ручки тона в нуле, пропускается целиком.
    pub tone: Option<LookTone>,
    /// Полоски по каналам (решение 341: после тона, до колёс); `None` — все 12 ручек в нуле, шаг пропускается.
    pub primaries: Option<LookPrimaries>,
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
        let primaries = LookPrimaries::resolve(&controls.settings.primaries, &model.primaries, working)?;
        Ok(Self {
            version: CURRENT_LOOK_PARAMS_VERSION,
            tone: LookTone::resolve(&controls.settings, &model.tone),
            primaries,
            white_shift: white_shift(model, &controls.settings),
            color: resolved_color.params,
            lut: controls.lut.clone(),
            report: LookReport {
                unknown_band_ids: resolved_color.unknown_band_ids,
                white_shift_limit: WhiteShiftLimit::Unlimited,
            },
        })
    }

    /// Ничего не меняет в картинке (кроме, возможно, пустой диагностики): тон, цвет, белая, LUT — нейтральны.
    pub fn is_neutral(&self) -> bool {
        self.tone.is_none()
            && self.primaries.is_none()
            && self.white_shift.is_none()
            && self.color.is_none()
            && self.lut.is_none()
    }

    /// Картинку после основы ничего не меняет: стадий вида над светом экрана нет (сдвиг белой — раньше основы).
    pub fn has_no_display_stages(&self) -> bool {
        self.tone.is_none() && self.primaries.is_none() && self.color.is_none() && self.lut.is_none()
    }

    /// Применяет стадии вида к свету `W` одного пикселя (порядок: тон, полоски, цвет, таблица).
    pub fn apply_to_pixel(&self, working_rgb: [f32; 3]) -> [f32; 3] {
        let toned = match &self.tone {
            Some(tone) => tone.apply(working_rgb),
            None => working_rgb,
        };
        let barred = match &self.primaries {
            Some(primaries) => primaries.apply(toned),
            None => toned,
        };
        let colored = match &self.color {
            Some(color) => color.apply(barred),
            None => barred,
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

/// Теплота и оттенок → сдвиг белой точки. «+ теплота» СНИЖАЕТ майреды предполагаемого света (картинка
/// теплее), «+ оттенок» поднимает Duv предполагаемого света (картинка пурпурнее): см. [`white`].
pub fn white_shift(model: &LookModelSettings, settings: &LookSettings) -> WhiteShift {
    let warmth = model.warmth_mired.amount(settings.warmth);
    WhiteShift {
        // `0.0 − 0.0`, а не `−0.0`: нейтраль обязана быть ровно нулём ([`WhiteShift::is_none`]).
        mired: 0.0 - warmth,
        mired_edge: if warmth < 0.0 {
            model.warmth_mired.minus
        } else {
            model.warmth_mired.plus
        },
        duv: model.tint_duv.amount(settings.tint),
    }
}

#[cfg(test)]
mod tests;
