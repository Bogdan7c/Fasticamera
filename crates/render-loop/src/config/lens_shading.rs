//! Секция `viewfinder.lens_shading`: коррекция виньетирования по нашей калибровке камеры (сессия 14f).
//!
//! По умолчанию коррекция всегда включена и полная (решение владельца). Выключить или ослабить её можно
//! только конфигом; пользовательский переключатель — сессия 16, а политика «ослаблять при слабом свете» —
//! бэклог. Сила остаётся покадровым управлением обработкой (`ProcessingControls::lens_shading`), чтобы
//! политика могла стать динамической без переделки ISP. Видоискатель и снимок читают ОДНУ секцию, поэтому
//! картинка снимка совпадает с видоискателем.
//!
//! Сила проверяется при разборе (`CorrectionStrength` принимает только 0…1): значение вне диапазона — ошибка
//! запуска с именем поля, а не молчаливое обрезание.

use isp_model::lens_shading::{CorrectionStrength, LensShadingControl};
use serde::{Deserialize, Serialize};

/// Включена ли коррекция виньетирования.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensShadingMode {
    /// Корректируем с силой `strength`. По умолчанию.
    #[default]
    Enabled,
    /// Не корректируем (сравнения, отладка): углы остаются тёмными.
    Disabled,
}

/// Настройки коррекции виньетирования.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderLensShadingConfig {
    pub mode: LensShadingMode,
    /// Сила коррекции 0…1: 1 — углы поднимаются до яркости центра полностью.
    pub strength: CorrectionStrength,
}

impl Default for ViewfinderLensShadingConfig {
    fn default() -> Self {
        Self {
            mode: LensShadingMode::Enabled,
            strength: CorrectionStrength::FULL,
        }
    }
}

impl ViewfinderLensShadingConfig {
    /// Управление обработкой кадра, которое ISP применяет к каждому кадру.
    pub fn control(&self) -> LensShadingControl {
        match self.mode {
            LensShadingMode::Enabled => LensShadingControl::Enabled(self.strength),
            LensShadingMode::Disabled => LensShadingControl::Disabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<ViewfinderLensShadingConfig, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn default_is_always_full_correction() {
        assert_eq!(
            ViewfinderLensShadingConfig::default().control(),
            LensShadingControl::Enabled(CorrectionStrength::FULL)
        );
        assert_eq!(parse("{}").unwrap(), ViewfinderLensShadingConfig::default());
    }

    #[test]
    fn mode_and_strength_reach_the_control() {
        let weakened = parse(r#"{"strength": 0.25}"#).unwrap();
        assert_eq!(
            weakened.control(),
            LensShadingControl::Enabled(CorrectionStrength::new(0.25).unwrap())
        );
        let off = parse(r#"{"mode": "disabled", "strength": 0.25}"#).unwrap();
        assert_eq!(off.control(), LensShadingControl::Disabled);
    }

    #[test]
    fn bad_strength_unknown_mode_and_unknown_fields_are_startup_errors() {
        for bad in [
            r#"{"strength": 1.5}"#,
            r#"{"strength": -0.1}"#,
            r#"{"mode": "sometimes"}"#,
            r#"{"gain": 2}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad} должен отвергаться");
        }
    }

    #[test]
    fn section_roundtrips_through_json() {
        let config = ViewfinderLensShadingConfig {
            mode: LensShadingMode::Disabled,
            strength: CorrectionStrength::new(0.5).unwrap(),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(parse(&json).unwrap(), config);
    }
}
