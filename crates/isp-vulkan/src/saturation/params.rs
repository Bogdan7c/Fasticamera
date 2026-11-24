//! Параметры обработки «как у продукта»: рецепт для `IspParams::resolve`.
//!
//! Продукт собирает `IspParams` в `render-loop` (`IspParamsCache::resolve_if_changed`): `ProcessingControls::with_defaults`
//! поверх описания сенсора из паспорта потока, затем тон, основа тона, подтяжка AE, виньетирование и вид.
//! `isp-vulkan` от `render-loop` не зависит, поэтому здесь тот же порядок на тех же типах `isp-model`.
//! Описание сенсора и нейтраль (калибровка цвета камеры) даёт вызывающий: выдумывать матрицы нельзя.

use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::exposure::ExposureGain;
use isp_model::image::DisplayGamut;
use isp_model::lens_shading::LensShadingControl;
use isp_model::look::LookControls;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::sensor::SensorDescription;
use isp_model::tone::ToneCurveSettings;
use isp_model::tone::base::ToneBase;

use super::error::SaturationWorkError;

/// Рецепт параметров: умолчания равны продуктовым (`ProcessingControls::with_defaults`).
#[derive(Debug, Clone)]
pub struct ProductParamsRecipe {
    /// Охват экрана (продукт на OnePlus 11 — Display P3; sRGB — запасной).
    pub gamut: DisplayGamut,
    /// Способ демозаика: `Preview` — видоискатель, `Quality` — снимок и стенд V4.
    pub demosaic: DemosaicMethod,
    /// Ручки тона экрана (умолчание `ViewfinderToneConfig::default` = умолчание `isp-model`).
    pub tone: ToneCurveSettings,
    /// Модель основы тона.
    pub tone_base: ToneBase,
    /// Подтяжка AE (единица — без подтяжки).
    pub exposure_gain: ExposureGain,
    /// Режим виньетирования (карту берёт сам `SensorDescription`).
    pub lens_shading: LensShadingControl,
    /// Вид картинки: ручки и таблица LUT.
    pub look: LookControls,
}

impl ProductParamsRecipe {
    /// Видоискатель: демозаик `Preview`, охват P3.
    pub fn viewfinder() -> Self {
        Self::with_demosaic(DemosaicMethod::Preview, DisplayGamut::DisplayP3)
    }

    /// Качественный конвейер (снимок, стенд V4): демозаик `Quality`.
    pub fn quality(gamut: DisplayGamut) -> Self {
        Self::with_demosaic(DemosaicMethod::Quality, gamut)
    }

    /// Значения те же, что даёт `ProcessingControls::with_defaults` (проверяет тест `recipe_matches_processing_defaults`).
    fn with_demosaic(demosaic: DemosaicMethod, gamut: DisplayGamut) -> Self {
        Self {
            gamut,
            demosaic,
            tone: ToneCurveSettings::default(),
            tone_base: ToneBase::default(),
            exposure_gain: ExposureGain::UNITY,
            lens_shading: LensShadingControl::default(),
            look: LookControls::default(),
        }
    }

    /// Разрешает `IspParams` для сенсора и нейтрали (как `resolve_if_changed` продукта: уровни — статические).
    pub fn resolve(
        &self,
        sensor: &SensorDescription,
        neutral: CameraNeutral,
    ) -> Result<IspParams, SaturationWorkError> {
        let controls = ProcessingControls {
            tone: self.tone,
            tone_base: self.tone_base,
            exposure_gain: self.exposure_gain,
            lens_shading: self.lens_shading,
            look: self.look.clone(),
            ..ProcessingControls::with_defaults(neutral, self.demosaic, self.gamut)
        };
        let frame = FrameInputs {
            levels: sensor.static_levels(),
        };
        IspParams::resolve(sensor, &frame, &controls).map_err(|error| SaturationWorkError::Params {
            what: error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_matches_processing_defaults() {
        let neutral = CameraNeutral::new([0.5, 1.0, 0.5]).unwrap();
        for recipe in [
            ProductParamsRecipe::viewfinder(),
            ProductParamsRecipe::quality(DisplayGamut::Srgb),
        ] {
            let defaults = ProcessingControls::with_defaults(neutral, recipe.demosaic, recipe.gamut);
            assert_eq!(recipe.tone, defaults.tone);
            assert_eq!(recipe.tone_base, defaults.tone_base);
            assert_eq!(recipe.exposure_gain, defaults.exposure_gain);
            assert_eq!(recipe.lens_shading, defaults.lens_shading);
        }
    }
}
