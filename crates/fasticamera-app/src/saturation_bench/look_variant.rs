//! Виды картинки для работ стенда насыщения: что показывает работа цены вида (`look_cost`) и какой вид меряет линейка
//! паритета на устройстве (`look_parity`).
//!
//! Модуль знает только РЕЦЕПТЫ ([`LookRecipe`]): кто какой вид покажет, решает каталог, а как рецепт становится
//! параметрами ISP — `ProductInputs::params_with_look`. Образцы колёс и тона берутся из конфига
//! `camera.scenarios.measurement_look` (общие со сценарием `viewfinder-stage-budget`), поэтому цифры двух приборов
//! говорят об одном и том же виде.

use isp_model::look::model::LookModelSettings;
use look_contract::{BandAdjustment, BandAdjustments, LookKnob, LookRecipe, LookSettings, ToneBaseModel};
use serde::{Serialize, Serializer};

use super::inputs::ProductInputs;
use crate::measurement_look::MeasurementLook;

/// Вид окна замера цены: обёртка над [`MeasurementLook`] из конфига замеров, которая умеет сериализоваться в каталог
/// (имя вида в результате — стабильное, его читают разборщики в `scripts/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeasuredLook(pub MeasurementLook);

impl Serialize for MeasuredLook {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.name())
    }
}

impl MeasuredLook {
    /// Рецепт окна замера из рецепта продукта и образцов конфига.
    pub fn recipe(self, inputs: &ProductInputs) -> LookRecipe {
        self.0.recipe_from(inputs.product_look(), inputs.measurement_look())
    }
}

/// Какой вид гоняет линейка паритета на данной основе тона.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParityCase {
    /// Ручки активного пресета продукта на этой основе.
    ProductLook,
    /// Яркость всех полос оттенка на краю −1 (худший одиночный край полос 14a, см. `BAND_EXTREME_LUMINANCE`).
    BandExtremes,
    /// Образец колёс цвета из конфига замеров (худший случай колёс 14i).
    WheelsSample,
    /// Образец ручек тона из конфига замеров (стадия тона 14l).
    ToneSample,
    /// Образец 12 полосок из конфига замеров (стадия полосок 14t).
    PrimariesSample,
}

/// Вид линейки паритета: основа тона и случай.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ParityLook {
    /// Модель основы тона.
    pub base: ToneBaseModel,
    /// Какие ручки включены.
    pub case: ParityCase,
}

/// Края ручек полос у случая [`ParityCase::BandExtremes`]: яркость ВСЕХ полос на краю −1, оттенок и насыщенность в нуле.
///
/// Почему именно она: перебор одиночных краёв (оттенок ±1, насыщенность ±1, яркость ±1) на всех четырёх основах на ПК
/// (RADV) показал, что худший — яркость −1 (на AgX с нейтралью D65 1,35e-5, на остальных 6,7…7,6e-6; остальные края
/// ниже 2,4e-6). Полоса оттенка в сцене «развёртка по оттенку» проходит весь круг со стыком 360° → 0°, поэтому сдвиг
/// оттенка отдельным случаем не нужен. Сочетание всех краёв сразу (1, 1, −1) тестами 14a не покрывалось и на AgX даёт
/// 1,7e-5: это находка, а не случай по умолчанию.
const BAND_EXTREME_HUE: f32 = 0.0;
const BAND_EXTREME_SATURATION: f32 = 0.0;
const BAND_EXTREME_LUMINANCE: f32 = -1.0;

impl ParityLook {
    /// Рецепт вида для линейки. Кроме `ProductLook`, остальные случаи берут НЕЙТРАЛЬНЫЕ настройки плюс одну ветку
    /// (как тесты паритета на ПК): так ошибка относится к этой ветке, а не к пресету владельца.
    pub fn recipe(self, inputs: &ProductInputs) -> Result<LookRecipe, String> {
        let settings = match self.case {
            ParityCase::ProductLook => inputs.product_look().settings.clone(),
            ParityCase::BandExtremes => LookSettings {
                bands: band_extremes(inputs.look_model())?,
                ..LookSettings::NEUTRAL
            },
            ParityCase::WheelsSample => LookSettings {
                wheels: inputs.measurement_look().wheels_sample,
                ..LookSettings::NEUTRAL
            },
            ParityCase::ToneSample => {
                let tone = inputs.measurement_look().tone_sample;
                LookSettings {
                    contrast: tone.contrast,
                    midtones: tone.midtones,
                    shadows: tone.shadows,
                    highlights: tone.highlights,
                    ..LookSettings::NEUTRAL
                }
            }
            ParityCase::PrimariesSample => LookSettings {
                primaries: inputs.measurement_look().primaries_sample,
                ..LookSettings::NEUTRAL
            },
        };
        Ok(LookRecipe {
            base: self.base,
            settings,
        })
    }
}

/// Правки ВСЕХ полос модели вида на краях ручек.
fn band_extremes(model: &LookModelSettings) -> Result<BandAdjustments, String> {
    let knob = |value: f32| LookKnob::new(value).map_err(|error| format!("край ручки полосы {value}: {error}"));
    let adjustments = model
        .bands
        .iter()
        .map(|definition| {
            Ok(BandAdjustment {
                band: definition.id.clone(),
                hue: knob(BAND_EXTREME_HUE)?,
                saturation: knob(BAND_EXTREME_SATURATION)?,
                luminance: knob(BAND_EXTREME_LUMINANCE)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    BandAdjustments::new(adjustments).map_err(|error| format!("правки полос не собрались: {error}"))
}

#[cfg(test)]
#[path = "look_variant_tests.rs"]
mod tests;
