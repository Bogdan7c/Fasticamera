//! Настройки UI-прохода поверх видоискателя (секция `ui_overlay` конфига потока рендера, сессия 11).
//!
//! Только то, что нужно GPU-рендеру UI при создании: пределы кадра и пирамида размытия фона. Сам проход
//! по умолчанию выключен — включает его установка источника кадра UI (`UiOverlayCommand::Install`),
//! а не конфиг. Значения по умолчанию и их происхождение описаны в `ui-render-vulkan::config`.

use serde::{Deserialize, Serialize};
use ui_render_vulkan::{BackdropPyramidConfig, ConfigError as UiRenderConfigError, UiRenderLimits};

/// Секция `ui_overlay`; отсутствующие поля — из [`Default`], неизвестные — ошибка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UiOverlayConfig {
    /// Пределы одного кадра UI (фигуры, глифы, группы стекла). Превышение — честный отказ кадра UI.
    pub limits: UiRenderLimits,
    /// Пирамида размытия фона под стеклом.
    pub pyramid: BackdropPyramidConfig,
}

impl UiOverlayConfig {
    /// Проверяет числа теми же правилами, по которым рендер UI проверит их при создании: ошибка
    /// ловится при старте потока, а не на первом кадре с установленным источником.
    pub fn validate(&self) -> Result<(), UiOverlayConfigError> {
        self.limits.validate()?;
        self.pyramid.validate()?;
        Ok(())
    }
}

/// Что не так в секции `ui_overlay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ui_overlay: {0}")]
pub struct UiOverlayConfigError(#[from] pub UiRenderConfigError);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_broken_numbers_are_rejected() {
        assert_eq!(UiOverlayConfig::default().validate(), Ok(()));
        let zero_limit = UiOverlayConfig {
            limits: UiRenderLimits {
                max_glass_groups: 0,
                ..UiRenderLimits::default()
            },
            ..UiOverlayConfig::default()
        };
        assert_eq!(
            zero_limit.validate(),
            Err(UiOverlayConfigError(UiRenderConfigError::ZeroLimit {
                field: "max_glass_groups"
            }))
        );
        let no_levels = UiOverlayConfig {
            pyramid: BackdropPyramidConfig {
                levels: 0,
                ..BackdropPyramidConfig::default()
            },
            ..UiOverlayConfig::default()
        };
        assert_eq!(
            no_levels.validate(),
            Err(UiOverlayConfigError(UiRenderConfigError::PyramidLevelsOutOfRange {
                levels: 0
            }))
        );
    }

    #[test]
    fn section_reads_from_json_with_defaults_and_rejects_unknown_keys() {
        let config: UiOverlayConfig =
            serde_json::from_str(r#"{"pyramid": {"levels": 3}, "limits": {"max_glass_groups": 8}}"#).unwrap();
        assert_eq!(config.pyramid.levels, 3);
        assert_eq!(config.limits.max_glass_groups, 8);
        assert_eq!(
            config.limits.max_shape_instances,
            UiRenderLimits::default().max_shape_instances
        );
        assert!(serde_json::from_str::<UiOverlayConfig>(r#"{"limit": {}}"#).is_err());
    }
}
