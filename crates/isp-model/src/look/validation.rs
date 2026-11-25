//! Проверка основ при старте: кривая основы обязана строиться при крайней остаточной подтяжке AE
//! (`±max_residual_ev`, решение 72).
//!
//! Ручки тона вида кривую основы больше не трогают (14l, [`super::tone_shape`]): их стадия растёт при любых
//! величинах по построению, поэтому перебирать углы ручек не нужно. Подтяжка AE по-прежнему двигает серый самой
//! кривой, и модель, у которой на краю подтяжки кривая «ломается» посреди съёмки, отвергается заранее.

use super::model::{InvalidLookModel, LookModelSettings};
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;
use crate::tone::ToneCurve;
use crate::tone::base::ToneBase;

/// Проверка краёв не прошла.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToneEdgeError {
    #[error("модель вида: {0}")]
    Model(#[from] InvalidLookModel),
    #[error("кривая тона не строится при крайней подтяжке AE: {0}")]
    CurveUnusable(String),
}

/// Годна ли модель вида и строится ли кривая основы `base` при подтяжках AE `{−max, 0, +max}` ступеней.
pub fn validate_tone_edges(
    model: &LookModelSettings,
    base: &ToneBase,
    legacy: &crate::tone::ToneCurveSettings,
    output_gamut: DisplayGamut,
    max_residual_ev: f32,
) -> Result<(), ToneEdgeError> {
    model.validate()?;
    for gain_ev in [-max_residual_ev, 0.0, max_residual_ev] {
        let unusable = |reason: String| ToneEdgeError::CurveUnusable(format!("подтяжка {gain_ev:+} EV: {reason}"));
        let gain = ExposureGain::from_ev(gain_ev).map_err(|error| unusable(error.to_string()))?;
        base.resolve(gain, output_gamut)
            .map_err(|error| unusable(error.to_string()))?;
        // У прежней основы числа живут в настройках её кривой, а не в варианте `ToneBase`.
        if matches!(base, ToneBase::Legacy) {
            ToneCurve::resolve(&legacy.with_exposure_gain(gain)).map_err(|error| unusable(error.to_string()))?;
        }
    }
    Ok(())
}
