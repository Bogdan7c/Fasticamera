//! Конфиг движка камеры (секция `camera.engine` конфига приложения).

use camera_contract::stream::TargetFrameRate;
use serde::{Deserialize, Serialize};

/// Восстановление после недоступности (решение 39).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RecoveryConfig {
    /// Сколько попыток подряд без успеха, прежде чем сдаться (`Fatal::RecoveryExhausted`).
    /// Возврат на передний план и устойчивый поток (`stable_frames_before_reset`) обнуляют счётчик.
    pub max_consecutive_attempts: u32,
    /// Задержки повторов по таймеру (сбой устройства/службы), мс. Попытка N берёт элемент
    /// `min(N, len-1)`: последний повторяется.
    pub retry_backoff_ms: Vec<u64>,
    /// Сколько результатов кадров подряд должна отдать восстановленная сессия, чтобы счётчик
    /// подряд идущих попыток обнулился. Без этого платформа, закрывающая сессию сразу после
    /// каждой успешной настройки, крутила бы восстановление бесконечно. 30 — полсекунды потока
    /// на 60 к/с: достаточно, чтобы отличить живую сессию от закрытой «на старте», и мало, чтобы
    /// настоящее восстановление не висело в долгах.
    pub stable_frames_before_reset: u32,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            max_consecutive_attempts: 5,
            retry_backoff_ms: vec![200, 500, 1_000, 2_000],
            stable_frames_before_reset: 30,
        }
    }
}

/// Назначение ролей по углу обзора (по длинной стороне сенсора).
///
/// Пороги — данные, а не код. Замер smoke `camera-catalog` на OnePlus 11 (сессия 04):
/// основная 72,5°, фронт 71,0°, ширик 102,0°. Порог ширика 90° стоит примерно посередине
/// между основной и шириком (запас по ~12° в обе стороны); порог телевика 55° — ниже основной
/// на 17° (телевик 2× даёт ~40°).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RoleClassifierConfig {
    /// Задняя камера с углом не меньше этого — ширик.
    pub ultra_wide_min_fov_degrees: f64,
    /// Задняя камера с углом не больше этого — телевик.
    pub tele_max_fov_degrees: f64,
}

impl Default for RoleClassifierConfig {
    fn default() -> Self {
        Self {
            ultra_wide_min_fov_degrees: 90.0,
            tele_max_fov_degrees: 55.0,
        }
    }
}

/// Учёт темпа потока по меткам сенсора.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CadenceConfig {
    /// Интервал длиннее `gap_ratio × длительность кадра цели` считается пропуском.
    pub gap_ratio: f64,
}

impl Default for CadenceConfig {
    fn default() -> Self {
        Self { gap_ratio: 1.5 }
    }
}

/// Ёмкости очередей движка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct QueueConfig {
    /// Команды снаружи; полная очередь → `CommandRejected::QueueFull`.
    pub command_capacity: usize,
    /// Частые события бэкенда (результаты кадров); при переполнении вытесняются старые
    /// со счётчиком. Редкие события устройства не ограничены и не теряются.
    pub capture_event_capacity: usize,
    /// События наружу; при переполнении вытесняются старые со счётчиком.
    pub outward_event_capacity: usize,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            command_capacity: 64,
            capture_event_capacity: 256,
            outward_event_capacity: 128,
        }
    }
}

/// Поток камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeThreadConfig {
    /// Сколько ждать выхода потока камеры, если `CameraRuntime` уничтожен без `shutdown`, мс.
    /// Дольше не ждём: застрявший в платформе поток отпускается (бесконечных ожиданий нет).
    pub drop_shutdown_timeout_ms: u64,
}

impl Default for RuntimeThreadConfig {
    fn default() -> Self {
        Self {
            drop_shutdown_timeout_ms: 2_000,
        }
    }
}

/// Запасная частота кадров, если запрошенная недостижима (решение 23).
///
/// Это enum, а не голое число: «откат выключен» — осознанное значение, а не `0` или `None`
/// с неочевидным смыслом. Видоискатель основной камеры просит 4:3@60 через вендорские ключи;
/// без них движок один раз откатывается на `To { fps }` с честным событием `StreamDegraded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameRateFallback {
    /// Отката нет: недостижимая частота — обычный `StreamConfigurationFailed`.
    Disabled,
    /// Откатиться на эту частоту, к/с (должна быть валидной `TargetFrameRate`).
    To { fps: u32 },
}

impl Default for FrameRateFallback {
    /// 30 к/с — частота, которую основная камера отдаёт без вендорских ключей.
    fn default() -> Self {
        Self::To { fps: 30 }
    }
}

/// Поведение при недостижимом режиме потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StreamConfig {
    /// Запасная частота кадров (решение 23).
    pub frame_rate_fallback: FrameRateFallback,
}

/// Конфиг движка целиком.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraEngineConfig {
    pub recovery: RecoveryConfig,
    pub roles: RoleClassifierConfig,
    pub cadence: CadenceConfig,
    pub queues: QueueConfig,
    pub runtime: RuntimeThreadConfig,
    pub stream: StreamConfig,
}

/// Конфиг движка некорректен.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CameraEngineConfigError {
    #[error("recovery.max_consecutive_attempts должен быть больше нуля")]
    NoRecoveryAttempts,
    #[error("recovery.stable_frames_before_reset должен быть больше нуля")]
    NoStableFrames,
    #[error("recovery.retry_backoff_ms не может быть пустым")]
    EmptyBackoff,
    #[error("recovery.retry_backoff_ms: задержка {0} мс больше часа")]
    BackoffTooLong(u64),
    #[error("roles: пороги углов должны быть конечными и 0 < tele ({tele}) < ultra_wide ({ultra_wide}) < 180")]
    InvalidRoleThresholds { tele: f64, ultra_wide: f64 },
    #[error("cadence.gap_ratio должен быть конечным и больше 1, а не {0}")]
    InvalidGapRatio(f64),
    #[error("runtime.drop_shutdown_timeout_ms должен быть от 1 до {max} мс, а не {value}")]
    InvalidDropShutdownTimeout { value: u64, max: u64 },
    #[error("stream.frame_rate_fallback: запасная частота {0} к/с вне допустимых границ")]
    InvalidFrameRateFallback(u32),
    #[error("queues: ёмкости должны быть от 1 до {max}")]
    InvalidQueueCapacity { max: usize },
}

impl CameraEngineConfig {
    /// Предел ёмкостей очередей: защита от опечатки в конфиге, съедающей память.
    pub const MAX_QUEUE_CAPACITY: usize = 65_536;
    /// Предел одной задержки повтора.
    pub const MAX_BACKOFF_MS: u64 = 3_600_000;
    /// Предел ожидания выхода потока в `Drop`.
    pub const MAX_DROP_SHUTDOWN_TIMEOUT_MS: u64 = 60_000;

    pub fn validate(&self) -> Result<(), CameraEngineConfigError> {
        if self.recovery.max_consecutive_attempts == 0 {
            return Err(CameraEngineConfigError::NoRecoveryAttempts);
        }
        if self.recovery.stable_frames_before_reset == 0 {
            return Err(CameraEngineConfigError::NoStableFrames);
        }
        if self.recovery.retry_backoff_ms.is_empty() {
            return Err(CameraEngineConfigError::EmptyBackoff);
        }
        if let Some(too_long) = self
            .recovery
            .retry_backoff_ms
            .iter()
            .copied()
            .find(|delay| *delay > Self::MAX_BACKOFF_MS)
        {
            return Err(CameraEngineConfigError::BackoffTooLong(too_long));
        }
        let RoleClassifierConfig {
            ultra_wide_min_fov_degrees: ultra_wide,
            tele_max_fov_degrees: tele,
        } = self.roles;
        let thresholds_ok =
            tele.is_finite() && ultra_wide.is_finite() && 0.0 < tele && tele < ultra_wide && ultra_wide < 180.0;
        if !thresholds_ok {
            return Err(CameraEngineConfigError::InvalidRoleThresholds { tele, ultra_wide });
        }
        if !(self.cadence.gap_ratio.is_finite() && self.cadence.gap_ratio > 1.0) {
            return Err(CameraEngineConfigError::InvalidGapRatio(self.cadence.gap_ratio));
        }
        let capacities = [
            self.queues.command_capacity,
            self.queues.capture_event_capacity,
            self.queues.outward_event_capacity,
        ];
        if capacities
            .iter()
            .any(|capacity| *capacity == 0 || *capacity > Self::MAX_QUEUE_CAPACITY)
        {
            return Err(CameraEngineConfigError::InvalidQueueCapacity {
                max: Self::MAX_QUEUE_CAPACITY,
            });
        }
        let drop_timeout = self.runtime.drop_shutdown_timeout_ms;
        if drop_timeout == 0 || drop_timeout > Self::MAX_DROP_SHUTDOWN_TIMEOUT_MS {
            return Err(CameraEngineConfigError::InvalidDropShutdownTimeout {
                value: drop_timeout,
                max: Self::MAX_DROP_SHUTDOWN_TIMEOUT_MS,
            });
        }
        if let FrameRateFallback::To { fps } = self.stream.frame_rate_fallback {
            TargetFrameRate::new(fps).map_err(|_| CameraEngineConfigError::InvalidFrameRateFallback(fps))?;
        }
        Ok(())
    }

    /// Задержка перед попыткой восстановления номер `attempt` (с 1).
    pub fn backoff_for_attempt(&self, attempt: u32) -> u64 {
        let schedule = &self.recovery.retry_backoff_ms;
        let index = (attempt.saturating_sub(1) as usize).min(schedule.len().saturating_sub(1));
        schedule.get(index).copied().unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
