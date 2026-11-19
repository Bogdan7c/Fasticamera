//! Настройки оболочки: ёмкости, таймауты, частоты датчиков.
//!
//! Приходят в составе конфига нативной части (`RuntimeConfig`, поле `shell`) из Kotlin.
//! Каждое поле необязательно: отсутствующее берётся из значения по умолчанию ниже
//! (с объяснением, откуда число). Опечатка в имени поля — ошибка, а не молчаливый дефолт.

use std::num::NonZeroU32;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shell_contract::event_queue::ShellQueueConfig;
use shell_contract::motion::{GyroRequest, SensorRate};

use crate::gravity_source::{GravityConfig, GravityConfigError, GravitySettings};
use crate::orientation_classifier::{OrientationClassifierConfig, OrientationConfigError};

/// Минимальный период опроса запаса тепла: Android отвечает NaN, если спрашивать чаще раза
/// в секунду (документация `AThermal_getThermalHeadroom`).
const MIN_THERMAL_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShellConfig {
    /// Ёмкости очереди событий (касания / остальные).
    pub queue: ShellQueueConfig,
    /// Сколько `surfaceDestroyed` ждёт подтверждения от владельца рендера, мс. По умолчанию
    /// 500: отпустить swapchain — единицы миллисекунд, а дольше держать главный поток
    /// Android опасно (ANR через 5 с).
    pub surface_release_timeout_ms: u64,
    /// Ёмкость очереди команд к платформе (вибрация, разрешение…). По умолчанию 32:
    /// колесо зума даёт до ~30 тиков в секунду.
    pub command_queue_capacity: usize,
    /// Период опроса запаса тепла, мс (не меньше 1000). По умолчанию 2000.
    pub thermal_poll_interval_ms: u64,
    /// Частота гироскопа по умолчанию, Гц. 400 — требование V3 README.
    pub gyro_rate_hz: u32,
    /// Сколько миллисекунд истории гироскопа держит кольцо. 5 с ≈ 2200 показаний по 24 байта ≈ 55 КБ.
    pub gyro_retained_ms: u64,
    /// Частота акселерометра для ориентации, Гц. 50 — достаточно для поворота иконок.
    pub accelerometer_rate_hz: u32,
    /// Пороги классификатора ориентации.
    pub orientation: OrientationClassifierConfig,
    /// Гравитация для блика стекла: частота и фильтр отката на акселерометр.
    pub gravity: GravityConfig,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            queue: ShellQueueConfig::default(),
            surface_release_timeout_ms: 500,
            command_queue_capacity: 32,
            thermal_poll_interval_ms: 2000,
            gyro_rate_hz: 400,
            gyro_retained_ms: 5000,
            accelerometer_rate_hz: 50,
            orientation: OrientationClassifierConfig::default(),
            gravity: GravityConfig::default(),
        }
    }
}

/// Неверные настройки оболочки.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ShellConfigError {
    #[error("{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("thermal_poll_interval_ms = {0}: Android разрешает опрос не чаще раза в секунду (≥ 1000)")]
    ThermalPollTooFrequent(u64),
    #[error("ориентация: {0}")]
    Orientation(#[from] OrientationConfigError),
    #[error("гравитация: {0}")]
    Gravity(#[from] GravityConfigError),
}

/// Проверенные настройки в виде, удобном коду оболочки.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedShellConfig {
    pub queue: ShellQueueConfig,
    pub surface_release_timeout: Duration,
    pub command_queue_capacity: usize,
    pub thermal_poll_interval: Duration,
    pub default_gyro_request: GyroRequest,
    pub accelerometer_rate: SensorRate,
    pub orientation: OrientationClassifierConfig,
    pub gravity: GravitySettings,
}

impl ShellConfig {
    pub fn validate(&self) -> Result<ValidatedShellConfig, ShellConfigError> {
        let nonzero_rate = |hertz: u32, field| NonZeroU32::new(hertz).ok_or(ShellConfigError::Zero { field });
        if self.surface_release_timeout_ms == 0 {
            return Err(ShellConfigError::Zero {
                field: "surface_release_timeout_ms",
            });
        }
        if self.command_queue_capacity == 0 {
            return Err(ShellConfigError::Zero {
                field: "command_queue_capacity",
            });
        }
        let thermal_poll_interval = Duration::from_millis(self.thermal_poll_interval_ms);
        if thermal_poll_interval < MIN_THERMAL_POLL_INTERVAL {
            return Err(ShellConfigError::ThermalPollTooFrequent(self.thermal_poll_interval_ms));
        }
        if self.queue.touch_capacity == 0 || self.queue.critical_capacity == 0 {
            return Err(ShellConfigError::Zero {
                field: "queue.touch_capacity / queue.critical_capacity",
            });
        }
        Ok(ValidatedShellConfig {
            queue: self.queue,
            surface_release_timeout: Duration::from_millis(self.surface_release_timeout_ms),
            command_queue_capacity: self.command_queue_capacity,
            thermal_poll_interval,
            default_gyro_request: GyroRequest {
                rate: SensorRate::new(nonzero_rate(self.gyro_rate_hz, "gyro_rate_hz")?),
                retained: Duration::from_millis(self.gyro_retained_ms),
            },
            accelerometer_rate: SensorRate::new(nonzero_rate(self.accelerometer_rate_hz, "accelerometer_rate_hz")?),
            orientation: self.orientation.validate()?,
            gravity: self.gravity.validate()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_plan() {
        let validated = ShellConfig::default().validate().expect("дефолты валидны");
        assert_eq!(validated.default_gyro_request.rate.hertz(), 400);
        assert_eq!(validated.surface_release_timeout, Duration::from_millis(500));
        assert_eq!(validated.thermal_poll_interval, Duration::from_secs(2));
    }

    #[test]
    fn partial_json_fills_defaults_and_typo_is_error() {
        let partial: ShellConfig =
            serde_json::from_str(r#"{"gyro_rate_hz": 200, "queue": {"touch_capacity": 8}}"#).expect("частичный конфиг");
        assert_eq!(partial.gyro_rate_hz, 200);
        assert_eq!(partial.queue.touch_capacity, 8);
        assert_eq!(
            partial.queue.critical_capacity,
            ShellQueueConfig::default().critical_capacity
        );
        assert_eq!(partial.accelerometer_rate_hz, 50);
        assert!(serde_json::from_str::<ShellConfig>(r#"{"gyro_rate": 200}"#).is_err());
    }

    #[test]
    fn too_frequent_thermal_poll_is_rejected() {
        let config = ShellConfig {
            thermal_poll_interval_ms: 500,
            ..ShellConfig::default()
        };
        assert_eq!(config.validate(), Err(ShellConfigError::ThermalPollTooFrequent(500)));
    }

    #[test]
    fn zero_values_are_rejected_with_field_name() {
        let config = ShellConfig {
            gyro_rate_hz: 0,
            ..ShellConfig::default()
        };
        assert_eq!(config.validate(), Err(ShellConfigError::Zero { field: "gyro_rate_hz" }));
        let config = ShellConfig {
            command_queue_capacity: 0,
            ..ShellConfig::default()
        };
        assert!(matches!(config.validate(), Err(ShellConfigError::Zero { .. })));
    }
}
