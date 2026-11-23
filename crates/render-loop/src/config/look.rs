//! Секция `viewfinder.look_model`: модель вида картинки (сессия 14a, решения 98–101, 178).
//!
//! Модель вида — это НЕ значения ручек (их крутит человек, они приходят командой `SetLook` и в плане снимка),
//! а правила, как ручки переводятся в формулы: диапазоны каждой ручки, набор полос оттенка, окно кожи для
//! сочности, предел размера таблицы LUT. Сама модель и её проверка живут в `isp-model::look`; здесь —
//! только проверка при старте в связке с остальным конфигом видоискателя.
//!
//! # Почему проверка учитывает тон и AE
//! Ручки тона меняют параметры кривой основы (наклон, серый экрана, носок, плечо). На краю ручки кривая
//! может перестать строиться — особенно при крайней остаточной подтяжке AE, которая двигает серый сцены.
//! Тогда ISP молча оставлял бы прежние параметры на каждом кадре. Поэтому при старте перебираются все
//! крайние положения ручек тона × края подтяжки AE для выбранной основы и для обеих целей вывода
//! (видоискатель — Display P3, снимок — sRGB). Плохая модель — ошибка запуска с понятной причиной.
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

/// Проверяет модель вида при старте: сама модель и строимость кривой тона на краях ручек и подтяжки AE.
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

    #[test]
    fn default_model_is_valid_for_default_tone_and_ae() {
        let tone = ViewfinderToneConfig::default();
        assert_eq!(validate_look_model(&LookModelSettings::default(), &tone, 0.5), Ok(()));
    }

    /// Проверяются ВСЕ модели основы: умолчания проходят при любой выбранной в конфиге, а сломанная на краю ручки
    /// секция НЕвыбранной модели отвергается при загрузке (её может выбрать свайп), с названием модели в ошибке.
    #[test]
    fn every_base_model_is_checked_at_the_knob_edges_whichever_is_selected() {
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
        // Тесная ось теней и света у ACES ломает кривую на краю ручки — и когда ACES выбрана, и когда нет.
        let narrow = AcesSettings {
            shape_width_ev: 0.5,
            ..AcesSettings::default()
        };
        for selected in [ToneBaseChoice::Aces, ToneBaseChoice::Agx] {
            let tone = ViewfinderToneConfig {
                base: selected,
                aces: narrow,
                ..ViewfinderToneConfig::default()
            };
            let error = validate_look_model(&model, &tone, 0.5).expect_err("край ручки ломает ACES");
            assert_eq!(error.base_model, ToneBaseModel::Aces, "выбрана {selected:?}");
            assert!(error.to_string().contains("основа ACES"), "{error}");
        }
    }

    #[test]
    fn contrast_range_that_breaks_the_curve_is_rejected_at_startup() {
        // Слишком широкий диапазон контраста: при «−1» наклон AgX опускается ниже минимума, при котором
        // плечо ещё доходит до белого точки, — кривая на краю не строится, запуск должен отказать.
        let model = LookModelSettings {
            contrast_range_log2: 3.0,
            ..LookModelSettings::default()
        };
        let tone = ViewfinderToneConfig::default();
        let error = validate_look_model(&model, &tone, 0.5).expect_err("край ручки контраста ломает кривую");
        assert!(
            matches!(error.source, ToneEdgeError::CurveUnusable(_) | ToneEdgeError::Model(_)),
            "{error}"
        );
    }
}
