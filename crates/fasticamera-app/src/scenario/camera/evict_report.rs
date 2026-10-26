//! Отчёт и вердикт `camera-evict`: пережила ли камера вытеснение системной камерой.
//!
//! Контракт со `scripts/smoke_checks.py`: имена полей [`EvictObservation`].

use camera_contract::availability::CameraUnavailableReason;
use camera_core::CameraCounters;
use serde::Serialize;

use super::engine_watch::RunFailure;
use super::progress::TimedNote;

/// Причины, с которыми Android отнимает камеру у нашего приложения при запуске системной камеры.
/// На OnePlus 11 ожидается `Evicted`; остальные две допустимы (зависит от порядка колбэков
/// системы), но в отчёте видно, какая именно пришла.
pub const ACCEPTED_EVICTION_REASONS: [CameraUnavailableReason; 3] = [
    CameraUnavailableReason::Evicted,
    CameraUnavailableReason::InUseByAnotherClient,
    CameraUnavailableReason::TooManyCamerasOpen,
];

/// Недоступность, которую увидел сценарий.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnavailableSeen {
    pub reason: CameraUnavailableReason,
    /// Миллисекунды от старта сценария.
    pub at_ms: u64,
}

/// Восстановление, которое увидел сценарий.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecoveredSeen {
    pub reason: CameraUnavailableReason,
    /// Сколько попыток понадобилось движку (поле события `Recovered`).
    pub attempts: u32,
    pub at_ms: u64,
    /// Время от события недоступности до восстановления.
    pub recovery_ms: u64,
}

/// Что наблюдалось за прогон.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvictObservation {
    pub failure: Option<RunFailure>,
    /// Камера, которую открыл сценарий (ID после разбора роли).
    pub camera: Option<String>,
    pub streaming_after_ms: Option<u64>,
    /// Первая недоступность после того, как поток пошёл.
    pub unavailable: Option<UnavailableSeen>,
    /// Остальные недоступности (повторные вытеснения, сбои) — для разбора.
    pub further_unavailable: Vec<CameraUnavailableReason>,
    pub recovered: Option<RecoveredSeen>,
    pub final_state: &'static str,
    /// Счётчики движка на конец: `recovery_attempts`, `recoveries`.
    pub counters: CameraCounters,
    /// Журнал событий движка с момента старта (первые `EVENTS_IN_REPORT_LIMIT`).
    pub events: Vec<TimedNote>,
    /// Сколько записей журнала не поместилось в `events`.
    pub events_dropped: usize,
}

/// Судит прогон (нарушения; пусто — камера пережила вытеснение).
pub fn evaluate_evict(observation: &EvictObservation) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &observation.failure {
        violations.push(failure.describe());
    }
    match &observation.unavailable {
        Some(seen) if ACCEPTED_EVICTION_REASONS.contains(&seen.reason) => {}
        Some(seen) => violations.push(format!(
            "камера стала недоступна по неожиданной причине {}, ожидалось одно из {:?}",
            seen.reason.name(),
            ACCEPTED_EVICTION_REASONS.map(CameraUnavailableReason::name)
        )),
        None if observation.failure.is_none() => violations.push("вытеснение не наблюдалось".to_owned()),
        None => {}
    }
    if observation.failure.is_none() {
        if observation.recovered.is_none() {
            violations.push("событие Recovered не пришло: самовосстановления не было".to_owned());
        }
        if observation.final_state != "streaming" {
            violations.push(format!(
                "в конце поток не идёт (состояние: {})",
                observation.final_state
            ));
        }
        if observation.counters.recoveries == 0 {
            violations.push("счётчик recoveries движка равен нулю".to_owned());
        }
    }
    violations
}

#[cfg(test)]
#[path = "evict_report_tests.rs"]
mod tests;
