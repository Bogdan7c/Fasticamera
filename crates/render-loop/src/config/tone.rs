//! Секция `viewfinder.tone`: ручки тона видоискателя до сессии 14a (решение 98, сессия 12b).
//!
//! # Что здесь можно крутить, а что нет
//! Только то, что меняет вид картинки на экране, но не экспозицию:
//! - `contrast` — крутизна S-кривой в середине (1 — без S-образности);
//! - `display_middle_gray` — насколько светлой выходит на экране середина (средний серый сцены).
//!
//! Средний серый **сцены** (`scene_middle_gray`) сюда намеренно не вынесен: это цель нашего AE (08a).
//! Ручка на нём заставила бы AE пересчитать выдержку, а экспозицию уже крутит EV. Поэтому
//! [`ViewfinderToneConfig::curve_settings`] всегда берёт его, белый сцены и сохранение оттенка из
//! умолчания `isp-model` — тот же источник, что у цели AE (`IspParamsCache::base_scene_middle_gray`).
//!
//! # Почему проверка учитывает AE
//! Кривая строится для каждого кадра с остаточной подтяжкой AE (`k` в пределах ±`max_residual_ev`):
//! подтяжка сдвигает средний серый сцены (`scene_middle_gray / k`). Сочетание контраста и средних тонов,
//! которое строится при `k = 1`, может не строиться при крайнем `k` — тогда ISP молча показывал бы
//! прежние параметры. Поэтому секция проверяется при старте на обоих краях подтяжки и в середине:
//! плохое значение — ошибка запуска с именем поля, а не тихий отказ на каждом кадре.
//!
//! В 14a основа тона меняется на AgX (решение 101) и эти ручки подбираются заново.

use isp_model::exposure::ExposureGain;
use isp_model::tone::{InvalidToneCurve, ToneCurve, ToneCurveSettings};
use serde::{Deserialize, Serialize};

/// Ручки тона видоискателя (см. модуль).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderToneConfig {
    /// Контраст S-кривой (> 0).
    pub contrast: f32,
    /// Линейная яркость экрана (0…1), в которую превращается средний серый сцены.
    pub display_middle_gray: f32,
}

impl Default for ViewfinderToneConfig {
    /// Умолчание `isp-model` (оценка сессии 06): контраст 1,35, середина экрана 0,20.
    fn default() -> Self {
        let model = ToneCurveSettings::default();
        Self {
            contrast: model.contrast,
            display_middle_gray: model.display_middle_gray,
        }
    }
}

/// Секция тона не годится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ToneConfigError {
    /// Кривая с такими ручками не строится (при указанной остаточной подтяжке AE).
    #[error("viewfinder.tone: {source} (при остаточной подтяжке AE {residual_ev:+.2} EV)")]
    CurveUnusable {
        /// При какой подтяжке AE кривая не строится, EV.
        residual_ev: f32,
        /// Что именно не так с кривой.
        #[source]
        source: InvalidToneCurve,
    },
    /// Предел подтяжки AE не переводится в конечный множитель (проверить тон на краях нельзя).
    #[error("viewfinder.tone: предел подтяжки AE {max_residual_ev} EV не даёт конечного множителя")]
    ResidualLimitUnusable {
        /// Предел из `viewfinder.auto_exposure.controller.max_residual_ev`.
        max_residual_ev: f32,
    },
}

impl ViewfinderToneConfig {
    /// Полные настройки кривой: ручки секции поверх умолчания `isp-model`. Средний серый сцены, белый и
    /// сохранение оттенка всегда из умолчания (см. модуль: средний серый сцены — цель AE, не ручка).
    pub fn curve_settings(&self) -> ToneCurveSettings {
        ToneCurveSettings {
            contrast: self.contrast,
            display_middle_gray: self.display_middle_gray,
            ..ToneCurveSettings::default()
        }
    }

    /// Проверяет, что кривая строится без подтяжки и на обоих краях `±max_residual_ev` (см. модуль).
    /// Строимость монотонна по подтяжке, поэтому краёв достаточно.
    pub fn validate(&self, max_residual_ev: f32) -> Result<(), ToneConfigError> {
        let settings = self.curve_settings();
        for residual_ev in [0.0, -max_residual_ev, max_residual_ev] {
            let gain = ExposureGain::from_ev(residual_ev)
                .map_err(|_| ToneConfigError::ResidualLimitUnusable { max_residual_ev })?;
            ToneCurve::resolve(&settings.with_exposure_gain(gain))
                .map_err(|source| ToneConfigError::CurveUnusable { residual_ev, source })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

    use super::*;

    /// Предел подтяжки AE по умолчанию (`ExposureControllerConfig::max_residual_ev`).
    const DEFAULT_RESIDUAL_EV: f32 = 0.5;

    #[test]
    fn default_matches_the_isp_model_default_and_is_valid() {
        let config = ViewfinderToneConfig::default();
        assert_eq!(config.curve_settings(), ToneCurveSettings::default());
        assert_eq!(config.validate(DEFAULT_RESIDUAL_EV), Ok(()));
    }

    #[test]
    fn knobs_never_move_the_scene_middle_gray_that_auto_exposure_aims_at() {
        let config = ViewfinderToneConfig {
            contrast: 1.8,
            display_middle_gray: 0.24,
        };
        let settings = config.curve_settings();
        let model = ToneCurveSettings::default();
        assert_eq!(settings.contrast, 1.8);
        assert_eq!(settings.display_middle_gray, 0.24);
        assert_eq!(settings.scene_middle_gray, model.scene_middle_gray, "цель AE не ручка");
        assert_eq!(settings.scene_white, model.scene_white);
        assert_eq!(settings.hue_preservation, model.hue_preservation);
    }

    #[test]
    fn json_fills_missing_fields_from_defaults_and_rejects_typos() {
        let partial: ViewfinderToneConfig = serde_json::from_str(r#"{"contrast": 1.5}"#).unwrap();
        assert_eq!(partial.contrast, 1.5);
        assert_eq!(
            partial.display_middle_gray,
            ViewfinderToneConfig::default().display_middle_gray
        );
        assert!(serde_json::from_str::<ViewfinderToneConfig>(r#"{"contrats": 1.5}"#).is_err());
        assert!(
            serde_json::from_str::<ViewfinderToneConfig>(r#"{"scene_middle_gray": 0.1}"#).is_err(),
            "цель AE через конфиг тона не задаётся"
        );
    }

    #[test]
    fn non_positive_contrast_is_rejected_with_the_model_reason() {
        let config = ViewfinderToneConfig {
            contrast: 0.0,
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::CurveUnusable {
                residual_ev: 0.0,
                source: InvalidToneCurve::InvalidContrast { contrast: 0.0 },
            })
        );
    }

    #[test]
    fn display_middle_gray_outside_zero_one_is_rejected() {
        let config = ViewfinderToneConfig {
            display_middle_gray: 1.0,
            ..ViewfinderToneConfig::default()
        };
        assert!(matches!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::CurveUnusable {
                source: InvalidToneCurve::DisplayMiddleGrayOutOfRange { .. },
                ..
            })
        ));
    }

    /// Сочетание строится без подтяжки, но не при крайней затемняющей подтяжке AE: проверка обязана поймать
    /// его при старте (иначе ISP молча держал бы прежние параметры на тех кадрах).
    #[test]
    fn combination_that_breaks_only_at_the_darkening_residual_edge_is_rejected() {
        // Предел строимости: y_g > (g / w)^c. При g = 0,075, c = 0,5: без подтяжки 0,274, при k = 2^−0,5 — 0,326.
        let config = ViewfinderToneConfig {
            contrast: 0.5,
            display_middle_gray: 0.3,
        };
        assert_eq!(
            config.validate(0.0),
            Ok(()),
            "без подтяжки строится — значит, ловит именно край"
        );
        assert!(matches!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::CurveUnusable {
                residual_ev,
                source: InvalidToneCurve::DisplayMiddleGrayUnreachable { .. },
            }) if residual_ev < 0.0
        ));
    }

    #[test]
    fn residual_limit_without_a_finite_multiplier_is_its_own_error() {
        assert_eq!(
            ViewfinderToneConfig::default().validate(1.0e6),
            Err(ToneConfigError::ResidualLimitUnusable { max_residual_ev: 1.0e6 })
        );
    }
}
