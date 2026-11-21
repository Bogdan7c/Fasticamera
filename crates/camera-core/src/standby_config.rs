//! Конфиг «тёплых» камер (13b, секция `camera.engine.standby`).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Тёплые камеры: сколько держать и как долго после «не нужно».
///
/// Это данные, а не код: ёмкость зависит от телефона (ресурс HAL), пауза — от ощущения владельца.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StandbyConfig {
    /// Сколько тёплых камер держать одновременно (0 — тёплых нет вообще). Рабочая ёмкость —
    /// пересечение с `StandbyCamera::capacity()` бэкенда. `[estimate]` 1: на OnePlus 11 хватает
    /// ресурса HAL на основную и одну тёплую (замер сессии 13b), две тёплые не проверялись.
    pub max_cameras: usize,
    /// Через сколько мс после «не нужно» тёплые закрываются. Пауза нужна, чтобы колесо, закрытое
    /// и тут же открытое снова (или щипок с короткой паузой), не открывало и не закрывало камеры
    /// заново. `[estimate]` 1500 мс: телефон покажет настоящую цифру.
    pub release_delay_ms: u64,
}

impl Default for StandbyConfig {
    fn default() -> Self {
        Self {
            max_cameras: 1,
            release_delay_ms: 1_500,
        }
    }
}

/// Конфиг тёплых камер некорректен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StandbyConfigError {
    #[error("standby.max_cameras должен быть не больше {max}, а не {value}")]
    TooManyCameras { value: usize, max: usize },
    #[error("standby.release_delay_ms должен быть не больше {max} мс, а не {value}")]
    ReleaseDelayTooLong { value: u64, max: u64 },
}

impl StandbyConfig {
    /// Предел ёмкости: номер ячейки — `u8`, а камер у телефона единицы.
    pub const MAX_CAMERAS: usize = 8;
    /// Предел паузы до закрытия: минута — защита от опечатки, держащей камеры вечно.
    pub const MAX_RELEASE_DELAY_MS: u64 = 60_000;

    pub fn validate(&self) -> Result<(), StandbyConfigError> {
        if self.max_cameras > Self::MAX_CAMERAS {
            return Err(StandbyConfigError::TooManyCameras {
                value: self.max_cameras,
                max: Self::MAX_CAMERAS,
            });
        }
        if self.release_delay_ms > Self::MAX_RELEASE_DELAY_MS {
            return Err(StandbyConfigError::ReleaseDelayTooLong {
                value: self.release_delay_ms,
                max: Self::MAX_RELEASE_DELAY_MS,
            });
        }
        Ok(())
    }

    /// Пауза до закрытия тёплых камер.
    pub fn release_delay(&self) -> Duration {
        Duration::from_millis(self.release_delay_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_hold_one_camera() {
        let config = StandbyConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.max_cameras, 1);
        assert_eq!(config.release_delay(), Duration::from_millis(1_500));
    }

    #[test]
    fn limits_are_enforced_and_zero_cameras_means_off() {
        let off = StandbyConfig {
            max_cameras: 0,
            ..StandbyConfig::default()
        };
        assert_eq!(off.validate(), Ok(()));
        let too_many = StandbyConfig {
            max_cameras: 9,
            ..StandbyConfig::default()
        };
        assert_eq!(
            too_many.validate(),
            Err(StandbyConfigError::TooManyCameras { value: 9, max: 8 })
        );
        let too_long = StandbyConfig {
            release_delay_ms: 60_001,
            ..StandbyConfig::default()
        };
        assert_eq!(
            too_long.validate(),
            Err(StandbyConfigError::ReleaseDelayTooLong {
                value: 60_001,
                max: 60_000
            })
        );
    }

    #[test]
    fn parses_from_json_and_rejects_unknown_keys() {
        let parsed: StandbyConfig = serde_json::from_str(r#"{"max_cameras":2}"#).expect("разбор");
        assert_eq!(parsed.max_cameras, 2);
        assert_eq!(parsed.release_delay_ms, 1_500);
        assert!(serde_json::from_str::<StandbyConfig>(r#"{"max":2}"#).is_err());
    }
}
