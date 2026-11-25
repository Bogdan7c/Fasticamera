//! Секция `viewfinder.look_model`: модель вида картинки (сессия 14a, решения 98–101, 178).
//!
//! Модель вида — это НЕ значения ручек (их крутит человек, они приходят командой `SetLook` и в плане снимка),
//! а правила, как ручки переводятся в формулы: диапазоны каждой ручки, набор полос оттенка, окно кожи для
//! сочности, предел размера таблицы LUT. Сама модель и её проверка живут в `isp-model::look`; здесь —
//! только проверка при старте в связке с остальным конфигом видоискателя.
//!
//! # Почему проверка учитывает AE и модель вида
//! Ручки тона (контраст, средние, тени, света) кривую основы НЕ трогают: это отдельная стадия вида после основы
//! (`isp-model::look::tone_shape`), и она растёт при любых величинах ручек по построению. Поэтому углы ручек
//! тона перебирать не нужно; проверяется сама модель вида (диапазоны ручек в допустимых пределах) и подтяжка AE:
//! остаточная подтяжка двигает серый самой кривой основы, и на её краю кривая может перестать строиться.
//! Тогда ISP молча оставлял бы прежние параметры на каждом кадре. Поэтому при старте перебираются края подтяжки AE
//! для каждой основы и для обеих целей вывода (видоискатель — Display P3, снимок — sRGB). Плохая модель или
//! основа — ошибка запуска с понятной причиной.
//!
//! Основа проверяется ВСЯ четвёрка (agx, aces, filmic, legacy), а не выбранная в `viewfinder.tone.base`: основа —
//! часть вида и переключается на ходу, поэтому действующей может стать любая.

use isp_model::image::DisplayGamut;
use isp_model::look::model::LookModelSettings;
use isp_model::look::validation::{ToneEdgeError, validate_tone_edges};
use look_contract::ToneBaseModel;

use super::ViewfinderToneConfig;

/// Цели вывода, для которых модель обязана строиться: экран (P3) и снимок (sRGB, решение 142).
const CHECKED_OUTPUT_GAMUTS: [DisplayGamut; 2] = [DisplayGamut::DisplayP3, DisplayGamut::Srgb];

/// Проверяет модель вида при старте: сама модель и строимость кривой основы на краях подтяжки AE.
pub(super) fn validate_look_model(
    model: &LookModelSettings,
    tone: &ViewfinderToneConfig,
    max_residual_ev: f32,
) -> Result<(), LookConfigError> {
    let legacy = tone.curve_settings();
    // ВСЕ модели, а не только выбранная в конфиге: основа — часть вида (решение 195) и меняется на ходу свайпом,
    // поэтому любая модель может оказаться действующей. Ошибка запуска лучше, чем кадры без параметров после свайпа.
    for base_model in ToneBaseModel::ALL {
        let base = tone.tone_base_for(base_model);
        for gamut in CHECKED_OUTPUT_GAMUTS {
            validate_tone_edges(model, &base, &legacy, gamut, max_residual_ev).map_err(|source| LookConfigError {
                base_model,
                gamut,
                source,
            })?;
        }
    }
    Ok(())
}

/// Модель вида не годится для этого конфига видоискателя.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("viewfinder.look_model (основа {base_model:?}, цель вывода {gamut:?}): {source}")]
pub struct LookConfigError {
    /// Для какой модели основы модель вида не строится (проверяются все четыре).
    pub base_model: ToneBaseModel,
    /// Для какой цели вывода модель не строится.
    pub gamut: DisplayGamut,
    /// Что именно не так.
    #[source]
    pub source: ToneEdgeError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use isp_model::look::range::KnobRange;

    #[test]
    fn default_model_is_valid_for_default_tone_and_ae() {
        let tone = ViewfinderToneConfig::default();
        assert_eq!(validate_look_model(&LookModelSettings::default(), &tone, 0.5), Ok(()));
    }

    /// Проверяются ВСЕ модели основы: умолчания проходят при любой выбранной в конфиге, а сломанная на краю подтяжки AE
    /// секция НЕвыбранной модели отвергается при загрузке (её может выбрать свайп), с названием модели в ошибке.
    #[test]
    fn every_base_model_is_checked_at_the_ae_edges_whichever_is_selected() {
        use super::super::ToneBaseChoice;
        use isp_model::tone::aces::AcesSettings;
        let model = LookModelSettings::default();
        for base in [
            ToneBaseChoice::Agx,
            ToneBaseChoice::Aces,
            ToneBaseChoice::Filmic,
            ToneBaseChoice::Legacy,
        ] {
            let tone = ViewfinderToneConfig {
                base,
                ..ViewfinderToneConfig::default()
            };
            assert_eq!(validate_look_model(&model, &tone, 0.5), Ok(()), "{base:?}");
        }
        // Серый сцены 0,8 у ACES при подтяжке −0,5 EV выходит за единицу (0,8 / 2^−0,5 ≈ 1,13): кривая на краю
        // подтяжки AE не строится, хотя без подтяжки строится — и когда ACES выбрана, и когда нет.
        let bright_gray = AcesSettings {
            scene_middle_gray: 0.8,
            ..AcesSettings::default()
        };
        for selected in [ToneBaseChoice::Aces, ToneBaseChoice::Agx] {
            let tone = ViewfinderToneConfig {
                base: selected,
                aces: bright_gray,
                ..ViewfinderToneConfig::default()
            };
            let error = validate_look_model(&model, &tone, 0.5).expect_err("край подтяжки AE ломает ACES");
            assert_eq!(error.base_model, ToneBaseModel::Aces, "выбрана {selected:?}");
            assert!(error.to_string().contains("основа ACES"), "{error}");
            assert!(
                error.to_string().contains("подтяжка -0.5 EV"),
                "ломает именно край подтяжки AE: {error}"
            );
        }
    }

    #[test]
    fn tone_range_beyond_the_limit_is_rejected_at_startup() {
        // Сила ручки тона больше предела (дроби теряют точность f32 у края отрезка): модель вида отвергается.
        let mut model = LookModelSettings::default();
        model.tone.contrast = KnobRange::symmetric(7.0);
        let tone = ViewfinderToneConfig::default();
        let error = validate_look_model(&model, &tone, 0.5).expect_err("слишком сильная ручка контраста");
        assert!(matches!(error.source, ToneEdgeError::Model(_)), "{error}");
        assert!(error.to_string().contains("tone.contrast"), "{error}");
    }

    #[test]
    fn strongest_allowed_tone_knobs_never_make_the_startup_check_fail() {
        // Ручки тона кривую основы не трогают: даже на самых сильных допустимых краях запуск проходит.
        let strongest = KnobRange::symmetric(6.0);
        let mut model = LookModelSettings::default();
        model.tone.contrast = strongest;
        model.tone.midtones = strongest;
        model.tone.shadows = strongest;
        model.tone.highlights = strongest;
        let tone = ViewfinderToneConfig::default();
        assert_eq!(validate_look_model(&model, &tone, 0.5), Ok(()));
    }
}
