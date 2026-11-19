//! Секция `camera.scenarios.glass`: параметры smoke-сценария `glass` (сессия 11) — стекло поверх
//! живого видоискателя.
//!
//! Режим камеры, роль, сроки запуска и пороги самого видоискателя (частота сенсора, «ровно один раз»,
//! джиттер-буфер) берутся из `camera.scenarios.viewfinder`: сценарий стартует тот же продуктовый
//! видоискатель и судит его теми же порогами. Здесь только то, что относится к стеклу: длина окна,
//! бюджет GPU-времени кадра UI, допуск пропусков и сама демо-сцена.

use serde::{Deserialize, Serialize};

use crate::scenario::camera::glass_showcase::config::GlassShowcaseConfig;
use crate::scenario::camera::glass_showcase::error::ShowcaseConfigError;

/// Секция `camera.scenarios.glass`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GlassScenarioConfig {
    /// Длина окна наблюдения за анимацией стекла, мс. `[doc]` критерий выхода 11a: «0 пропусков кадров
    /// за 30 секунд анимации» (`user/foundation/11a-glass-phone-acceptance.md`).
    pub duration_ms: u64,
    /// Доля периода кадра, которую вправе занять GPU-время UI (p95). `[estimate]` 0,25 — «запас до бюджета
    /// кадра минус видоискатель»: при 60 Гц ≈ 4,2 мс. Подгонка, не Apple. Порог — параметр, а не вердикт
    /// «медленно»: на Adreno мс GPU растягивает DVFS (решение 56), поэтому число калибруется на телефоне в 11a.
    pub ui_gpu_budget_fraction: f64,
    /// Сколько кадров может показаться позже задуманного vsync за окно. `[doc]` 11a: 0 пропусков.
    pub max_missed_frames: u64,
    /// Демо-сцена: раскладка, сценарий времени, вид, свет от наклона.
    pub showcase: GlassShowcaseConfig,
}

impl Default for GlassScenarioConfig {
    fn default() -> Self {
        Self {
            duration_ms: 30_000,
            ui_gpu_budget_fraction: 0.25,
            max_missed_frames: 0,
            showcase: GlassShowcaseConfig::default(),
        }
    }
}

/// Неверная секция `camera.scenarios.glass`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GlassScenarioConfigError {
    #[error("glass.duration_ms должно быть больше нуля")]
    ZeroDuration,
    #[error("glass.ui_gpu_budget_fraction = {value}: нужна доля от 0 (не включая) до 1")]
    BudgetFractionOutOfRange { value: f64 },
    #[error("glass.showcase: {0}")]
    Showcase(#[from] ShowcaseConfigError),
}

impl GlassScenarioConfig {
    /// Бюджет GPU-времени кадра UI при частоте показа `refresh_hz`, мс: период кадра × доля.
    /// Частота не из головы: сценарий передаёт `render.preferred_frame_rate_hz` (решение 36).
    pub fn ui_gpu_budget_ms(&self, refresh_hz: f64) -> f64 {
        1_000.0 / refresh_hz * self.ui_gpu_budget_fraction
    }

    pub fn validate(&self) -> Result<(), GlassScenarioConfigError> {
        if self.duration_ms == 0 {
            return Err(GlassScenarioConfigError::ZeroDuration);
        }
        // Через сравнение: NaN не проходит и отвергается.
        let fraction = self.ui_gpu_budget_fraction;
        if !(fraction.is_finite() && fraction > 0.0 && fraction <= 1.0) {
            return Err(GlassScenarioConfigError::BudgetFractionOutOfRange { value: fraction });
        }
        self.showcase.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_budget_is_a_quarter_of_the_frame_period() {
        let config = GlassScenarioConfig::default();
        assert_eq!(config.validate(), Ok(()));
        let budget = config.ui_gpu_budget_ms(60.0);
        assert!((budget - 4.1667).abs() < 1e-3, "{budget}");
        assert!((config.ui_gpu_budget_ms(120.0) - budget / 2.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_values_are_rejected_with_their_own_reason() {
        let zero = GlassScenarioConfig {
            duration_ms: 0,
            ..GlassScenarioConfig::default()
        };
        assert_eq!(zero.validate(), Err(GlassScenarioConfigError::ZeroDuration));
        for value in [0.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
            let config = GlassScenarioConfig {
                ui_gpu_budget_fraction: value,
                ..GlassScenarioConfig::default()
            };
            assert!(
                matches!(
                    config.validate(),
                    Err(GlassScenarioConfigError::BudgetFractionOutOfRange { .. })
                ),
                "{value}"
            );
        }
        let mut broken_showcase = GlassScenarioConfig::default();
        broken_showcase.showcase.layout.edge_margin_pt = -1.0;
        assert!(matches!(
            broken_showcase.validate(),
            Err(GlassScenarioConfigError::Showcase(_))
        ));
    }

    #[test]
    fn unknown_fields_are_refused_and_partial_json_keeps_defaults() {
        assert!(serde_json::from_str::<GlassScenarioConfig>(r#"{"typo": 1}"#).is_err());
        let parsed: GlassScenarioConfig = serde_json::from_str(r#"{"duration_ms": 5000}"#).expect("разбор");
        assert_eq!(parsed.duration_ms, 5000);
        assert_eq!(parsed.ui_gpu_budget_fraction, 0.25);
    }
}
