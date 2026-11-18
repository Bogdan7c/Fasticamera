//! Снимок состояния движка: всегда актуальная правда о камере для UI, приложения и smoke.

use std::sync::Arc;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capabilities::FrameDuration;
use camera_contract::capture_result::{ResultKey, SensorTimestamp};
use camera_contract::catalog::CameraCatalog;
use camera_contract::stream::ConfiguredStream;
use serde::Serialize;

use crate::backend::SessionGeneration;
use crate::state::CameraState;
use crate::three_a_status::ThreeAStatus;

/// Где приложение: Android отдаёт камеру только приложению на переднем плане.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppPresence {
    Foreground,
    Background,
}

/// Сколько раз HAL не отдал проверяемый ключ результата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MissingKeyCount {
    pub key: ResultKey,
    pub results: u64,
}

/// Темп текущей сессии по меткам сенсора (`SENSOR_TIMESTAMP`, не по колбэкам и не по
/// `SENSOR_FRAME_DURATION`, который в режиме CamX ложный). Обнуляется на новой сессии.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamStats {
    /// Сессия, к которой относятся числа; `None` — сессии ещё не было.
    pub session: Option<SessionGeneration>,
    /// Длительность кадра цели (для порога пропуска).
    pub target_frame_duration: Option<FrameDuration>,
    pub results_completed: u64,
    pub results_without_timestamp: u64,
    /// Метка не больше предыдущей (повтор или откат) — интервал не считается.
    pub timestamps_out_of_order: u64,
    /// Интервалы между соседними по приходу результатами с растущими метками.
    pub intervals: u64,
    /// Интервалы длиннее `gap_ratio × target_frame_duration`.
    pub gaps: u64,
    /// Число кадров, потерянных в пропусках: `round(интервал / длительность) − 1` по каждому.
    pub frames_missed_in_gaps: u64,
    pub first_timestamp: Option<SensorTimestamp>,
    pub last_timestamp: Option<SensorTimestamp>,
    /// Средняя частота: `intervals / (last − first)`; `None`, пока интервалов нет.
    pub measured_frames_per_second: Option<f64>,
    pub min_interval_nanos: Option<u64>,
    pub max_interval_nanos: Option<u64>,
    /// Последний `SENSOR_FRAME_DURATION` от HAL — только для отчёта, темп по нему не считается.
    pub last_reported_frame_duration: Option<FrameDuration>,
    pub missing_keys: Vec<MissingKeyCount>,
    pub capture_failures: u64,
    pub captures_flushed: u64,
}

impl StreamStats {
    /// Пустая статистика (сессии не было).
    pub fn empty() -> Self {
        Self {
            session: None,
            target_frame_duration: None,
            results_completed: 0,
            results_without_timestamp: 0,
            timestamps_out_of_order: 0,
            intervals: 0,
            gaps: 0,
            frames_missed_in_gaps: 0,
            first_timestamp: None,
            last_timestamp: None,
            measured_frames_per_second: None,
            min_interval_nanos: None,
            max_interval_nanos: None,
            last_reported_frame_duration: None,
            missing_keys: Vec::new(),
            capture_failures: 0,
            captures_flushed: 0,
        }
    }
}

/// Счётчики движка за всю жизнь процесса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CameraCounters {
    pub commands_handled: u64,
    /// События бэкенда прошлых устройств/сессий, отброшенные по поколению.
    pub stale_backend_events: u64,
    /// Результаты кадров, вытесненные из переполненной очереди бэкенда.
    pub dropped_capture_events: u64,
    /// События наружу, вытесненные из переполненной очереди.
    pub dropped_outward_events: u64,
    pub devices_opened: u64,
    pub sessions_configured: u64,
    pub recovery_attempts: u64,
    pub recoveries: u64,
}

/// Снимок состояния движка. Без `Serialize`: форма отчёта — забота потребителя
/// (каталог разделяется через `Arc`), части снимка сериализуются по отдельности.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraStatus {
    pub state: CameraState,
    pub presence: AppPresence,
    /// Каталог с ролями; пустой, пока не прочитан или если чтение провалилось.
    pub catalog: Arc<CameraCatalog>,
    /// Текущая сконфигурированная сессия.
    pub configured: Option<ConfiguredStream>,
    pub stream: StreamStats,
    /// Последняя причина недоступности (остаётся после восстановления — для отчёта).
    pub last_unavailable: Option<CameraUnavailableReason>,
    pub counters: CameraCounters,
    /// Автоматика 3A и ручные параметры (08).
    pub three_a: ThreeAStatus,
}
