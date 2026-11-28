//! Вид картинки для замеров цены (сессия 14o, решения 312–316): какой рецепт показывать в окне «вкл» и «выкл»
//! строки A/B и в работах стенда насыщения.
//!
//! Это ЕДИНСТВЕННОЕ место, где решается «как выглядит вид без колёс / с колёсами / на другой основе» ради замера.
//! Им пользуются и сценарий `viewfinder-stage-budget` (строки вида), и стенд `gpu-saturation-bench` (работы цены
//! вида), поэтому цифры двух приборов говорят об одном и том же рецепте.
//!
//! Что модуль НЕ делает: не ставит вид в рендер и не трогает библиотеку пресетов. Он только строит рецепт из
//! рецепта продукта; кто и как его показывает (заявка сценария на рендер, параметры работы стенда) — решает
//! вызывающий. Файл пресетов владельца замер не меняет никогда.
//!
//! Почему колёсам, тону и полоскам нужен «образец» из конфига: «Как глаз» колёс не трогает, а тон у него — только контраст.
//! Шейдер пропускает нейтральные колёса, тон и полоски по флагу (`look_stage.glsl`, `look_tone.glsl`,
//! `look_primaries.glsl`), поэтому строка «колёса вкл/выкл» на виде без колёс мерила бы ноль. Образец задаёт
//! ненулевые колёса, ручки тона и 12 полосок;
//! цена ветки от величины толчка почти не зависит (та же математика на каждом пикселе).

use look_contract::{
    ChannelPrimaries, ColorWheels, LookKnob, LookRecipe, LookSettings, PrimaryChannel, ToneBaseModel, WheelZone,
};
use serde::{Deserialize, Serialize};

/// Ручки тона 14l (стадия тона вида): контраст, средние, тени, света.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToneKnobsSample {
    /// Контраст.
    pub contrast: LookKnob,
    /// Средние тона.
    pub midtones: LookKnob,
    /// Тени.
    pub shadows: LookKnob,
    /// Света.
    pub highlights: LookKnob,
}

impl ToneKnobsSample {
    /// Все четыре ручки тона в нуле: стадия тона выключена (флаг шейдера 0).
    pub const NEUTRAL: ToneKnobsSample = ToneKnobsSample {
        contrast: LookKnob::NEUTRAL,
        midtones: LookKnob::NEUTRAL,
        shadows: LookKnob::NEUTRAL,
        highlights: LookKnob::NEUTRAL,
    };

    /// Ручки тона из настроек вида.
    fn of(settings: &LookSettings) -> Self {
        Self {
            contrast: settings.contrast,
            midtones: settings.midtones,
            shadows: settings.shadows,
            highlights: settings.highlights,
        }
    }

    /// Все ручки в нуле.
    pub fn is_neutral(&self) -> bool {
        *self == Self::NEUTRAL
    }

    /// Настройки `settings` с этими ручками тона; остальное (цвет, полосы, колёса) не меняется.
    fn applied_to(self, settings: LookSettings) -> LookSettings {
        LookSettings {
            contrast: self.contrast,
            midtones: self.midtones,
            shadows: self.shadows,
            highlights: self.highlights,
            ..settings
        }
    }
}

/// Конфиг образцов вида для замеров (`camera.scenarios.measurement_look`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MeasurementLookConfig {
    /// Колёса окна «колёса вкл». Обязаны быть ненулевыми: иначе строка мерит ноль.
    pub wheels_sample: ColorWheels,
    /// Ручки тона окна «тон вкл». Обязаны быть ненулевыми по той же причине.
    pub tone_sample: ToneKnobsSample,
    /// Полоски Lift / Gamma / Gain / Offset × R, G, B окна «полоски вкл» (14t). Обязаны быть ненулевыми: иначе строка
    /// мерила бы ноль. В образце тронуты ВСЕ 12 ручек, чтобы шейдер полосок исполнил все ветки формулы.
    pub primaries_sample: ChannelPrimaries,
}

/// Числа образца — не подбор вида (решение 304: вид не подбираем), а «включить ветку шейдера заметной силой».
/// `[measurement]` Умеренные значения внутри круга и диапазона ручек, чтобы ни одна ветка не ушла в упор.
impl Default for MeasurementLookConfig {
    fn default() -> Self {
        Self {
            wheels_sample: default_wheels_sample(),
            tone_sample: ToneKnobsSample {
                contrast: knob(0.20),
                midtones: knob(0.10),
                shadows: knob(0.15),
                highlights: knob(-0.15),
            },
            primaries_sample: default_primaries_sample(),
        }
    }
}

/// Значение ручки из литерала умолчания (литералы выше лежат в [−1, 1]). Без паники в продукте: если литерал когда-либо
/// выйдет из диапазона, ручка станет нейтральной, и [`MeasurementLookConfig::validate`] (он идёт при загрузке конфига,
/// а тест умолчаний его проверяет) громко откажет «образец нейтрален», а не «молча измерит ноль».
fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap_or(LookKnob::NEUTRAL)
}

/// Четыре колеса образца: каждое толкнуто в свою сторону, кольца яркости сдвинуты. Строится через
/// `serde`, чтобы не дублировать проверки `WheelPush`/`LookKnob`: литерал — тот же формат, что в конфиге.
fn default_wheels_sample() -> ColorWheels {
    let sample = serde_json::json!({
        "lift": { "push": [0.10, -0.20], "ring": 0.05 },
        "gamma": { "push": [-0.15, 0.10], "ring": -0.05 },
        "gain": { "push": [0.20, 0.15], "ring": 0.05 },
        "offset": { "push": [-0.05, -0.10], "ring": 0.0 }
    });
    // Тот же принцип, что у `knob`: сбой литерала даёт нейтральные колёса, а `validate` отказывает громко.
    serde_json::from_value(sample).unwrap_or(ColorWheels::NEUTRAL)
}

/// Значения образца полосок: по зонам Lift, Gamma, Gain, Offset; в каждой зоне порядок каналов R, G, B.
/// Знаки чередуются и ни одна из 12 ручек не нулевая: шейдер полосок работает на каждом канале и в обе стороны.
/// `[measurement]` Умеренные величины, как у остальных образцов: ветка включена, но не упёрта в край диапазона.
const PRIMARIES_SAMPLE_VALUES: [(WheelZone, [f32; 3]); 4] = [
    (WheelZone::Lift, [0.10, -0.05, 0.08]),
    (WheelZone::Gamma, [-0.10, 0.08, -0.06]),
    (WheelZone::Gain, [0.06, -0.08, 0.10]),
    (WheelZone::Offset, [-0.05, 0.04, -0.06]),
];

/// Образец полосок из [`PRIMARIES_SAMPLE_VALUES`]. Как у `knob`: литерал вне диапазона даёт нулевую ручку, а
/// [`MeasurementLookConfig::validate`] громко откажет, если нулевыми окажутся все 12 (иначе тест значений поймает).
fn default_primaries_sample() -> ChannelPrimaries {
    let mut primaries = ChannelPrimaries::NEUTRAL;
    for (zone, values) in PRIMARIES_SAMPLE_VALUES {
        for (channel, value) in PrimaryChannel::ALL.into_iter().zip(values) {
            primaries = primaries.with_value(zone, channel, knob(value));
        }
    }
    primaries
}

/// Неверный конфиг образцов.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeasurementLookConfigError {
    /// Нейтральные колёса образца: строка «колёса вкл/выкл» мерила бы ноль.
    #[error("measurement_look.wheels_sample: все колёса нейтральны — строка колёс мерила бы ноль")]
    NeutralWheelsSample,
    /// Нейтральные ручки тона образца: строка «тон вкл/выкл» мерила бы ноль.
    #[error("measurement_look.tone_sample: все ручки тона в нуле — строка тона мерила бы ноль")]
    NeutralToneSample,
    /// Все 12 полосок образца в нуле: строка «полоски вкл/выкл» мерила бы ноль.
    #[error("measurement_look.primaries_sample: все полоски в нуле — строка полосок мерила бы ноль")]
    NeutralPrimariesSample,
}

impl MeasurementLookConfig {
    /// Проверяет, что каждый образец действительно включает свою ветку шейдера.
    pub fn validate(&self) -> Result<(), MeasurementLookConfigError> {
        if self.wheels_sample.is_neutral() {
            return Err(MeasurementLookConfigError::NeutralWheelsSample);
        }
        if self.tone_sample.is_neutral() {
            return Err(MeasurementLookConfigError::NeutralToneSample);
        }
        if self.primaries_sample.is_neutral() {
            return Err(MeasurementLookConfigError::NeutralPrimariesSample);
        }
        Ok(())
    }
}

/// Какой вид показать в окне замера. Каждый вариант строится из рецепта продукта (активный пресет владельца
/// с его правками), меняя ровно одну вещь, — так разница двух окон и есть цена этой вещи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeasurementLook {
    /// Рецепт продукта как есть.
    Product,
    /// Основа продукта, слой вида целиком в нуле (ни тона, ни цвета, ни колёс): «нейтраль».
    NeutralOnProductBase,
    /// Ручки продукта на другой модели основы тона.
    ToneBase(ToneBaseModel),
    /// Продукт с колёсами образца вместо своих.
    WheelsSample,
    /// Продукт без колёс.
    WithoutWheels,
    /// Продукт с ручками тона образца вместо своих.
    ToneSample,
    /// Продукт без ручек тона (стадия тона 14l выключена).
    WithoutTone,
    /// Продукт с полосками образца вместо своих (14t).
    PrimariesSample,
    /// Продукт без полосок (стадия полосок выключена флагом шейдера).
    WithoutPrimaries,
}

impl MeasurementLook {
    /// Рецепт для окна замера из рецепта продукта `product` и образцов `config`.
    pub fn recipe_from(self, product: &LookRecipe, config: &MeasurementLookConfig) -> LookRecipe {
        let settings = product.settings.clone();
        match self {
            Self::Product => product.clone(),
            Self::NeutralOnProductBase => LookRecipe {
                base: product.base,
                settings: LookSettings::NEUTRAL,
            },
            Self::ToneBase(model) => LookRecipe { base: model, settings },
            Self::WheelsSample => LookRecipe {
                base: product.base,
                settings: LookSettings {
                    wheels: config.wheels_sample,
                    ..settings
                },
            },
            Self::WithoutWheels => LookRecipe {
                base: product.base,
                settings: LookSettings {
                    wheels: ColorWheels::NEUTRAL,
                    ..settings
                },
            },
            Self::ToneSample => LookRecipe {
                base: product.base,
                settings: config.tone_sample.applied_to(settings),
            },
            Self::WithoutTone => LookRecipe {
                base: product.base,
                settings: ToneKnobsSample::NEUTRAL.applied_to(settings),
            },
            Self::PrimariesSample => LookRecipe {
                base: product.base,
                settings: LookSettings {
                    primaries: config.primaries_sample,
                    ..settings
                },
            },
            Self::WithoutPrimaries => LookRecipe {
                base: product.base,
                settings: LookSettings {
                    primaries: ChannelPrimaries::NEUTRAL,
                    ..settings
                },
            },
        }
    }

    /// Короткое имя для отчёта и лога (`snake_case`, стабильно: его читает разбор в `scripts/`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::NeutralOnProductBase => "neutral",
            Self::ToneBase(ToneBaseModel::Agx) => "tone_base_agx",
            Self::ToneBase(ToneBaseModel::Aces) => "tone_base_aces",
            Self::ToneBase(ToneBaseModel::Filmic) => "tone_base_filmic",
            Self::ToneBase(ToneBaseModel::Legacy) => "tone_base_legacy",
            Self::WheelsSample => "wheels_sample",
            Self::WithoutWheels => "without_wheels",
            Self::ToneSample => "tone_sample",
            Self::WithoutTone => "without_tone",
            Self::PrimariesSample => "primaries_sample",
            Self::WithoutPrimaries => "without_primaries",
        }
    }
}

/// Вспомогательное для отчётов: включена ли в рецепте стадия тона 14l (хоть одна ручка тона не в нуле).
pub fn tone_knobs_active(settings: &LookSettings) -> bool {
    !ToneKnobsSample::of(settings).is_neutral()
}

#[cfg(test)]
#[path = "measurement_look_tests.rs"]
mod tests;
