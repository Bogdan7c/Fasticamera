//! Конфиг движка камеры (секция `camera.engine` конфига приложения).

use camera_contract::stream::TargetFrameRate;
use serde::{Deserialize, Serialize};

use crate::standby_config::{StandbyConfig, StandbyConfigError};
use crate::three_a_own::OwnExposureConfig;
use crate::zoom::{ZoomConfig, ZoomConfigError};

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

/// Автоматика 3A: тап, блокировка, серии (сессия 08).
///
/// Все значения — данные, а не код; обоснование каждого рядом.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ThreeAConfig {
    /// Сторона региона замера/фокуса вокруг тапа как доля меньшей стороны сенсора.
    /// Квадрат тапа в iOS Камере — порядка 1/6–1/5 короткой стороны кадра: берём середину
    /// (≈ 1/5,5). Это **оценка по iOS, сверить со спецификацией 09**.
    pub metering_region_size_fraction: f32,
    /// Сколько ждать итога тапа от HAL, мс. Зонд 08: холодный захват AF занимает 27–34 кадра
    /// ≈ 0,55 с при 60 к/с. 1500 мс — почти втрое больше: не обрывает медленный скан в
    /// темноте, но и «тап без ответа» (RAW-only сессия не отдаёт `AF_STATE`) не висит вечно.
    pub focus_settle_timeout_ms: u64,
    /// Сколько результатов повторяющегося запроса подряд можно получить после отправки
    /// триггера, не увидев результат самого триггера, прежде чем считать, что метка потеряна
    /// (бэкенд не опознал запрос) и верить состоянию AF как есть. Конвейер Camera2 держит
    /// порядка 4–8 кадров в полёте, поэтому 8 кадров (≈ 133 мс при 60 к/с) — запас, после
    /// которого устаревший `FOCUSED_LOCKED` прошлого тапа уже вытеснен из конвейера.
    pub focus_marker_grace_frames: u32,
    /// Сбрасывать точку интереса и блокировку AE/AF при смене камеры. Оценка по iOS
    /// (фокус и замер не переносятся на другую камеру), **сверить со спецификацией 09**.
    pub reset_point_on_camera_switch: bool,
    /// Предел длины серии с брекетингом. Кадры серии живут в пуле RAW-кадров рядом с кадрами
    /// видоискателя и ожидающими парами (см. `PoolPlan` в `raw-frame-stream`, пул ≈ 7), поэтому
    /// серия длиннее четырёх вытеснила бы видоискатель. 4 покрывает типичный HDR-брекет (3)
    /// с запасом на один кадр. Это бюджет, а не возможность HAL: поднимать вместе с пулом.
    pub max_bracket_frames: usize,
    /// Сколько ждать результаты всех кадров серии с момента отправки, мс. Серия из 4 кадров
    /// с выдержками до секунды укладывается в ~5 с; 10 с — двойной запас на ночные выдержки,
    /// после чего серия честно объявляется проваленной (`ResultsTimedOut`).
    pub bracket_result_timeout_ms: u64,
    /// Наш AE на стороне движка (08a): стартовое значение, мерцание, пороги перестроения.
    pub exposure: OwnExposureConfig,
}

impl Default for ThreeAConfig {
    fn default() -> Self {
        Self {
            metering_region_size_fraction: 0.18,
            focus_settle_timeout_ms: 1_500,
            focus_marker_grace_frames: 8,
            reset_point_on_camera_switch: true,
            max_bracket_frames: 4,
            bracket_result_timeout_ms: 10_000,
            exposure: OwnExposureConfig::default(),
        }
    }
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
    pub three_a: ThreeAConfig,
    /// Шкала зума и выбор камеры (сессия 13).
    pub zoom: ZoomConfig,
    /// Тёплые камеры (13b): ёмкость и пауза до закрытия.
    pub standby: StandbyConfig,
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
    #[error("three_a.metering_region_size_fraction должен быть в (0, 1], а не {0}")]
    InvalidMeteringRegionSize(f32),
    #[error("three_a: {field} должен быть от 1 до {max}, а не {value}")]
    InvalidThreeALimit { field: &'static str, value: u64, max: u64 },
    #[error("three_a.exposure.{field} должен быть конечным и положительным (допуски — до 2 EV), а не {value}")]
    InvalidExposureSetting { field: &'static str, value: f64 },
    #[error("zoom: {0}")]
    InvalidZoom(#[from] ZoomConfigError),
    #[error("standby: {0}")]
    InvalidStandby(#[from] StandbyConfigError),
}

impl CameraEngineConfig {
    /// Предел ёмкостей очередей: защита от опечатки в конфиге, съедающей память.
    pub const MAX_QUEUE_CAPACITY: usize = 65_536;
    /// Предел одной задержки повтора.
    pub const MAX_BACKOFF_MS: u64 = 3_600_000;
    /// Предел любого ожидания автоматики 3A (итог тапа, результаты серии).
    pub const MAX_THREE_A_WAIT_MS: u64 = 60_000;
    /// Предел кадров серии, которые вообще можно разрешить конфигом.
    pub const MAX_THREE_A_BRACKET_FRAMES: u64 = 32;
    /// Предел «кадров на потерянную метку триггера».
    pub const MAX_FOCUS_MARKER_GRACE_FRAMES: u64 = 600;
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
        self.zoom.validate()?;
        self.standby.validate()?;
        self.validate_three_a()
    }

    /// Проверка секции `three_a`.
    fn validate_three_a(&self) -> Result<(), CameraEngineConfigError> {
        let three_a = &self.three_a;
        let fraction = three_a.metering_region_size_fraction;
        if !(fraction.is_finite() && fraction > 0.0 && fraction <= 1.0) {
            return Err(CameraEngineConfigError::InvalidMeteringRegionSize(fraction));
        }
        let limits = [
            (
                "focus_settle_timeout_ms",
                three_a.focus_settle_timeout_ms,
                Self::MAX_THREE_A_WAIT_MS,
            ),
            (
                "bracket_result_timeout_ms",
                three_a.bracket_result_timeout_ms,
                Self::MAX_THREE_A_WAIT_MS,
            ),
            (
                "max_bracket_frames",
                three_a.max_bracket_frames as u64,
                Self::MAX_THREE_A_BRACKET_FRAMES,
            ),
            (
                "focus_marker_grace_frames",
                u64::from(three_a.focus_marker_grace_frames),
                Self::MAX_FOCUS_MARKER_GRACE_FRAMES,
            ),
        ];
        for (field, value, max) in limits {
            if value == 0 || value > max {
                return Err(CameraEngineConfigError::InvalidThreeALimit { field, value, max });
            }
        }
        three_a.exposure.validate()
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
