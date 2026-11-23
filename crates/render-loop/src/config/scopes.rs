//! Секция `viewfinder.scopes`: цветовые приборы поверх видоискателя (сессия 14h, решения 183, 188–190).
//!
//! Что здесь живёт: только бюджет и настройки модели приборов. Сам прибор (тип, оси, размер картинки)
//! приходит командой `ViewfinderCommand::Scopes` от экрана, а не из конфига.

use scope_contract::MAX_SCOPE_IMAGE_SIDE_PX;
use scope_model::{ScopeModelSettings, ScopeSettingsError};
use serde::{Deserialize, Serialize};

/// Наибольшая допустимая настройка «каждый N-й кадр»: дальше прибор обновлялся бы реже раза в секунду на 60 к/с.
pub const MAX_SCOPES_EVERY_NTH_FRAME: u32 = 60;

/// Секция `viewfinder.scopes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScopesConfig {
    /// Настройки модели: размеры сеток, усиление, прореживание входа, правило показа (`scope-model`).
    pub model: ScopeModelSettings,
    /// Считать приборы на каждом N-м новом кадре камеры. 1 — на каждом (решение 190: прибор всегда про кадр на
    /// экране); больше 1 — только рычаг бюджета, менять по замеру 14b.
    pub every_nth_frame: u32,
    /// Наибольшая сторона картинки прибора, которую рендер согласен делать, пикселей. `[estimate]` 2048: часть
    /// экрана телефона (1080×2412) с запасом; больше — команда отвергается, а не молча урезается.
    pub max_picture_side_px: u32,
}

impl Default for ScopesConfig {
    fn default() -> Self {
        Self {
            model: ScopeModelSettings::default(),
            every_nth_frame: 1,
            max_picture_side_px: 2048,
        }
    }
}

/// Что не так с секцией `viewfinder.scopes`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScopesConfigError {
    /// «Каждый N-й кадр» вне `1..=MAX_SCOPES_EVERY_NTH_FRAME`.
    #[error("viewfinder.scopes.every_nth_frame должен лежать в 1..={MAX_SCOPES_EVERY_NTH_FRAME}, а не {0}")]
    EveryNthFrameOutOfRange(u32),
    /// Предел стороны картинки нулевой или больше предела словаря `scope-contract`.
    #[error("viewfinder.scopes.max_picture_side_px должен лежать в 1..={MAX_SCOPE_IMAGE_SIDE_PX}, а не {0}")]
    MaxPictureSideOutOfRange(u32),
    /// Настройки модели приборов отвергнуты самой моделью.
    #[error("viewfinder.scopes.model: {0}")]
    Model(#[from] ScopeSettingsError),
}

impl ScopesConfig {
    /// Проверяет секцию целиком (модель проверяет себя сама).
    pub fn validate(&self) -> Result<(), ScopesConfigError> {
        if !(1..=MAX_SCOPES_EVERY_NTH_FRAME).contains(&self.every_nth_frame) {
            return Err(ScopesConfigError::EveryNthFrameOutOfRange(self.every_nth_frame));
        }
        if !(1..=MAX_SCOPE_IMAGE_SIDE_PX).contains(&self.max_picture_side_px) {
            return Err(ScopesConfigError::MaxPictureSideOutOfRange(self.max_picture_side_px));
        }
        self.model.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_compute_every_frame() {
        let config = ScopesConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.every_nth_frame, 1, "решение 190: по умолчанию каждый кадр");
    }

    #[test]
    fn every_nth_frame_outside_its_range_is_rejected() {
        for bad in [0, MAX_SCOPES_EVERY_NTH_FRAME + 1] {
            let config = ScopesConfig {
                every_nth_frame: bad,
                ..ScopesConfig::default()
            };
            assert_eq!(config.validate(), Err(ScopesConfigError::EveryNthFrameOutOfRange(bad)));
        }
        let edge = ScopesConfig {
            every_nth_frame: MAX_SCOPES_EVERY_NTH_FRAME,
            ..ScopesConfig::default()
        };
        assert_eq!(edge.validate(), Ok(()));
    }

    #[test]
    fn picture_side_limit_outside_its_range_is_rejected() {
        for bad in [0, MAX_SCOPE_IMAGE_SIDE_PX + 1] {
            let config = ScopesConfig {
                max_picture_side_px: bad,
                ..ScopesConfig::default()
            };
            assert_eq!(config.validate(), Err(ScopesConfigError::MaxPictureSideOutOfRange(bad)));
        }
    }

    #[test]
    fn broken_model_is_reported_as_model_error() {
        let config = ScopesConfig {
            model: ScopeModelSettings {
                trace_columns: 0,
                ..ScopeModelSettings::default()
            },
            ..ScopesConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ScopesConfigError::Model(ScopeSettingsError::TraceColumnsOutOfRange {
                columns: 0
            }))
        );
    }

    #[test]
    fn unknown_field_is_rejected_and_partial_json_keeps_defaults() {
        assert!(serde_json::from_str::<ScopesConfig>(r#"{"every_nth":2}"#).is_err());
        let parsed: ScopesConfig = serde_json::from_str(r#"{"every_nth_frame":3}"#).expect("секция читается");
        assert_eq!(parsed.every_nth_frame, 3);
        assert_eq!(parsed.model, ScopeModelSettings::default());
        assert_eq!(parsed.max_picture_side_px, ScopesConfig::default().max_picture_side_px);
    }
}
