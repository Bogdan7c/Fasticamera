//! Счётчики тёплых камер по ячейкам (13b). Пишут колбэки NDK (атомарно, без замков) и поток
//! камеры; читает приложение снимком в составе `DiagnosticsSnapshot`.
//!
//! Счётчик принадлежит **ячейке**, а не сессии: после обмена ролей результаты бывшей основной
//! считаются в ячейке, в которую она переехала.

use std::sync::atomic::{AtomicU64, Ordering};

use camera_core::backend_standby::StandbySlotId;
use serde::Serialize;

/// Сколько ячеек диагностика различает (потолок `AndroidCameraConfig::standby_capacity`).
pub const MAX_STANDBY_SLOTS: usize = 4;

#[derive(Debug, Default)]
struct SlotCounters {
    sessions_started: AtomicU64,
    results_offered: AtomicU64,
    capture_failures: AtomicU64,
    open_failures: AtomicU64,
    configure_failures: AtomicU64,
    swaps: AtomicU64,
    swap_refusals: AtomicU64,
}

/// Счётчики всех ячеек.
#[derive(Debug, Default)]
pub struct StandbyDiagnostics {
    slots: [SlotCounters; MAX_STANDBY_SLOTS],
}

/// Снимок одной ячейки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StandbySlotSnapshot {
    pub slot: u8,
    /// Сконфигурированных сессий тёплой.
    pub sessions_started: u64,
    /// Результаты кадров тёплой, предложенные потоку кадров (движку они не доставляются).
    pub results_offered: u64,
    /// Провалы кадров тёплой (`CaptureFailed` не доставляется, только считается).
    pub capture_failures: u64,
    /// Отказы открытия устройства тёплой.
    pub open_failures: u64,
    /// Отказы создания сессии/запроса тёплой.
    pub configure_failures: u64,
    /// Удачные обмены ролей с этой ячейкой.
    pub swaps: u64,
    /// Обмены, в которых отказано (нет сессии / хаб не знает эпохи).
    pub swap_refusals: u64,
}

impl StandbyDiagnostics {
    fn counters(&self, slot: StandbySlotId) -> Option<&SlotCounters> {
        self.slots.get(slot.index())
    }

    fn bump(&self, slot: StandbySlotId, pick: impl FnOnce(&SlotCounters) -> &AtomicU64) {
        // Ячейка за пределами таблицы конфигом не допускается; на всякий случай — не паникуем.
        if let Some(counters) = self.counters(slot) {
            pick(counters).fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_session_started(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.sessions_started);
    }
    pub fn record_result_offered(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.results_offered);
    }
    pub fn record_capture_failure(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.capture_failures);
    }
    pub fn record_open_failure(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.open_failures);
    }
    pub fn record_configure_failure(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.configure_failures);
    }
    pub fn record_swap(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.swaps);
    }
    pub fn record_swap_refusal(&self, slot: StandbySlotId) {
        self.bump(slot, |c| &c.swap_refusals);
    }

    /// Снимок всех ячеек, в которых что-то происходило (пустые не шумят в отчёте).
    pub fn snapshot(&self) -> Vec<StandbySlotSnapshot> {
        self.slots
            .iter()
            .zip(0_u8..)
            .map(|(counters, slot)| StandbySlotSnapshot {
                slot,
                sessions_started: counters.sessions_started.load(Ordering::Relaxed),
                results_offered: counters.results_offered.load(Ordering::Relaxed),
                capture_failures: counters.capture_failures.load(Ordering::Relaxed),
                open_failures: counters.open_failures.load(Ordering::Relaxed),
                configure_failures: counters.configure_failures.load(Ordering::Relaxed),
                swaps: counters.swaps.load(Ordering::Relaxed),
                swap_refusals: counters.swap_refusals.load(Ordering::Relaxed),
            })
            .filter(|snapshot| *snapshot != StandbySlotSnapshot::empty(snapshot.slot))
            .collect()
    }
}

impl StandbySlotSnapshot {
    fn empty(slot: u8) -> Self {
        Self {
            slot,
            sessions_started: 0,
            results_offered: 0,
            capture_failures: 0,
            open_failures: 0,
            configure_failures: 0,
            swaps: 0,
            swap_refusals: 0,
        }
    }
}

#[cfg(test)]
#[path = "standby_diagnostics_tests.rs"]
mod tests;
