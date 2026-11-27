//! Строка лога вида снимка (сессия 14o, «снимок = видоискатель по виду»).
//!
//! Когда движок снимка берёт план в работу, он пишет отпечаток рецепта (основа + ручки), которым проявит кадр.
//! Разбор `scripts/look_recipe_log.py` сверяет его со строкой нажатия приложения (`look_still_press`): вид, с
//! которым снимок ушёл из экрана, и вид, которым он проявлен, обязаны совпасть. Отпечаток считает та же функция, что
//! у приложения ([`look_contract::LookRecipe::fingerprint`]).

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};

/// Цель трассировки: общая с видом приложения, чтобы один фильтр logcat ловил обе стороны.
const LOG_TARGET: &str = "fasticamera::look";

/// Строка `look_still_render fp=<16 hex>` без префикса logcat.
pub(super) fn still_render_line(base: ToneBaseModel, settings: &LookSettings) -> String {
    let recipe = LookRecipe {
        base,
        settings: settings.clone(),
    };
    format!("look_still_render fp={}", recipe.fingerprint_hex())
}

/// Пишет строку уровнем `info` (release-фильтр её пропускает): раз на проявляемый кадр.
pub(super) fn log_still_render(base: ToneBaseModel, settings: &LookSettings) {
    tracing::info!(target: LOG_TARGET, "{}", still_render_line(base, settings));
}

#[cfg(test)]
mod tests {
    use look_contract::{GlobalKnob, LookKnob};

    use super::*;

    #[test]
    fn the_line_carries_the_fingerprint_of_base_and_settings_together() {
        let settings = LookSettings::NEUTRAL.with_knob(GlobalKnob::ALL[0], LookKnob::new(0.4).unwrap());
        let line = still_render_line(ToneBaseModel::Aces, &settings);
        let expected = LookRecipe {
            base: ToneBaseModel::Aces,
            settings: settings.clone(),
        }
        .fingerprint_hex();
        assert_eq!(line, format!("look_still_render fp={expected}"));
        assert_ne!(
            line,
            still_render_line(ToneBaseModel::Agx, &settings),
            "основа входит в отпечаток"
        );
    }
}
