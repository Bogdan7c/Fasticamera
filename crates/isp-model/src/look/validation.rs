//! Проверка модели вида на краях: кривая основы обязана строиться при ЛЮБОЙ комбинации крайних ручек тона и
//! крайней остаточной подтяжке AE (`±max_residual_ev`, решение 72).
//!
//! Строимость кривой AgX не монотонна по четырём ручкам сразу (наклон и серый экрана тянут `r₁ > 1` и `r₀ > 1`
//! в разные стороны), поэтому перебираются все углы куба `{−1, 0, +1}⁴` для ручек и `{−max, 0, +max}` для подтяжки.
//! Проверку вызывает конфиг при старте (пакет P4): модель, при которой кривая «ломается» на краю ручки посреди
//! жеста, отвергается заранее.

use look_contract::{LookKnob, LookSettings};

use super::model::{InvalidLookModel, LookModelSettings};

/// Проверка краёв не прошла.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToneEdgeError {
    #[error("модель вида: {0}")]
    Model(#[from] InvalidLookModel),
    #[error("кривая тона не строится при крайних ручках: {0}")]
    CurveUnusable(String),
}
use super::tone_amounts;
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;
use crate::tone::ToneCurve;
use crate::tone::base::ToneBase;

/// Значения ручки на краях и в нуле.
const EDGE_VALUES: [f32; 3] = [-1.0, 0.0, 1.0];

/// Строится ли кривая основы `base` при всех краевых ручках тона и подтяжках AE `{−max, 0, +max}` ступеней.
pub fn validate_tone_edges(
    model: &LookModelSettings,
    base: &ToneBase,
    legacy: &crate::tone::ToneCurveSettings,
    output_gamut: DisplayGamut,
    max_residual_ev: f32,
) -> Result<(), ToneEdgeError> {
    model.validate()?;
    let describe = ToneEdgeError::CurveUnusable;
    for gain_ev in [-max_residual_ev, 0.0, max_residual_ev] {
        let gain = ExposureGain::from_ev(gain_ev)
            .map_err(|error| describe(format!("остаточная подтяжка {gain_ev} EV: {error}")))?;
        for contrast in EDGE_VALUES {
            for midtones in EDGE_VALUES {
                for shadows in EDGE_VALUES {
                    for highlights in EDGE_VALUES {
                        let corner = |value: f32| LookKnob::new(value).unwrap_or(LookKnob::NEUTRAL);
                        let settings = LookSettings {
                            contrast: corner(contrast),
                            midtones: corner(midtones),
                            shadows: corner(shadows),
                            highlights: corner(highlights),
                            ..LookSettings::NEUTRAL
                        };
                        let knobbed = base.with_knobs(legacy, &tone_amounts(model, &settings));
                        let label = format!(
                            "ручки контраст {contrast:+}, средние {midtones:+}, тени {shadows:+}, света {highlights:+}, \
                             подтяжка AE {gain_ev:+} EV"
                        );
                        // Все модели строятся одним путём; прежняя основа проверяет ещё и свою кривую.
                        knobbed
                            .base
                            .resolve(gain, output_gamut)
                            .map_err(|error| describe(format!("{label}: {error}")))?;
                        if matches!(knobbed.base, ToneBase::Legacy) {
                            ToneCurve::resolve(&knobbed.legacy_curve.with_exposure_gain(gain))
                                .map_err(|error| describe(format!("{label}: {error}")))?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
