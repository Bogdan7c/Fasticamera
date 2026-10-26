//! Диагностика бэкенда: счётчики и отказы, которые приложение читает снимком.
//!
//! Один общий объект `Arc<BackendDiagnostics>`: его отдаёт `backend_factory` вместе с фабрикой,
//! пишут колбэки NDK (атомарные счётчики, без блокировок на горячем пути) и сам бэкенд
//! (каталог, отказы — под коротким замком), читает приложение через [`BackendDiagnostics::snapshot`].
//! Тип нейтральный и сериализуемый, поэтому проверяется на ПК.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use camera_contract::ids::CameraId;
use serde::Serialize;

use crate::characteristics::CharacteristicNote;
use crate::vendor::VendorTagReport;

/// Счётчики временного RAW-приёмника (до сессии 05: заменяется лизингом `RawFrameLease`).
#[derive(Debug, Default)]
pub struct RawSinkCounters {
    frames_acquired: AtomicU64,
    frames_released: AtomicU64,
    acquire_errors: AtomicU64,
    last_acquire_error: AtomicI32,
}

impl RawSinkCounters {
    pub fn record_frame_acquired(&self) {
        self.frames_acquired.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_frame_released(&self) {
        self.frames_released.fetch_add(1, Ordering::Relaxed);
    }

    /// Сбой `acquire*` (кроме штатного «буферов больше нет»). Сохраняется последний код.
    pub fn record_acquire_error(&self, status: i32) {
        self.acquire_errors.fetch_add(1, Ordering::Relaxed);
        self.last_acquire_error.store(status, Ordering::Relaxed);
    }

    fn snapshot(&self) -> RawSinkSnapshot {
        let errors = self.acquire_errors.load(Ordering::Relaxed);
        RawSinkSnapshot {
            frames_acquired: self.frames_acquired.load(Ordering::Relaxed),
            frames_released: self.frames_released.load(Ordering::Relaxed),
            acquire_errors: errors,
            last_acquire_error: (errors > 0).then(|| self.last_acquire_error.load(Ordering::Relaxed)),
        }
    }
}

/// Снимок счётчиков RAW-приёмника.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawSinkSnapshot {
    pub frames_acquired: u64,
    pub frames_released: u64,
    pub acquire_errors: u64,
    /// Код последнего сбоя `acquire*`, если сбои были.
    pub last_acquire_error: Option<i32>,
}

/// Камера, не попавшая в каталог, и причина.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedCamera {
    pub camera: String,
    pub reason: String,
}

/// Вендорские теги и заметки разбора одной камеры.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CameraCatalogReport {
    pub camera: String,
    pub vendor_tags: Vec<VendorTagReport>,
    pub notes: Vec<CharacteristicNote>,
}

/// Отказ платформы, записанный для отчёта.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedFailure {
    pub operation: String,
    pub code: i32,
    pub detail: String,
}

#[derive(Debug)]
struct SlowState {
    cameras: Vec<CameraCatalogReport>,
    skipped_cameras: Vec<SkippedCamera>,
    failures: VecDeque<RecordedFailure>,
    dropped_failures: u64,
}

/// Общая диагностика бэкенда.
#[derive(Debug)]
pub struct BackendDiagnostics {
    failure_capacity: usize,
    raw_sink: RawSinkCounters,
    callback_panics: AtomicU64,
    callbacks_after_close: AtomicU64,
    unmatched_frame_numbers: AtomicU64,
    retained_session_contexts: AtomicU64,
    leaked_availability_contexts: AtomicU64,
    leaked_session_state_contexts: AtomicU64,
    state: Mutex<SlowState>,
}

/// Снимок диагностики для приложения (сериализуется в JSON).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnosticsSnapshot {
    pub raw_sink: RawSinkSnapshot,
    /// Паники в колбэках NDK, пойманные на границе C (должно быть 0).
    pub callback_panics: u64,
    /// Колбэки, пришедшие после закрытия устройства или сессии и проигнорированные.
    pub callbacks_after_close: u64,
    /// Результаты кадров, для которых не нашёлся настоящий номер кадра (подставлен запасной).
    pub unmatched_frame_numbers: u64,
    /// Контексты колбэков сессий, которые намеренно не освобождены до конца процесса
    /// (`onClosed` может прийти из деструктора сессии уже после закрытия устройства).
    pub retained_session_contexts: u64,
    /// Контексты колбэков доступности камер, оставленные на весь процесс (NDK не гарантирует
    /// отсутствие запоздалых вызовов после снятия колбэка).
    pub leaked_availability_contexts: u64,
    /// Контексты состояния сессий, оставленные на весь процесс при разрушении бэкенда:
    /// `onClosed` сессии может прийти из её деструктора уже после этого (use-after-free иначе).
    pub leaked_session_state_contexts: u64,
    pub cameras: Vec<CameraCatalogReport>,
    pub skipped_cameras: Vec<SkippedCamera>,
    /// Последние отказы платформы (старые вытесняются).
    pub recent_failures: Vec<RecordedFailure>,
    pub dropped_failures: u64,
}

impl BackendDiagnostics {
    pub fn new(failure_history_capacity: usize) -> Self {
        Self {
            failure_capacity: failure_history_capacity.max(1),
            raw_sink: RawSinkCounters::default(),
            callback_panics: AtomicU64::new(0),
            callbacks_after_close: AtomicU64::new(0),
            unmatched_frame_numbers: AtomicU64::new(0),
            retained_session_contexts: AtomicU64::new(0),
            leaked_availability_contexts: AtomicU64::new(0),
            leaked_session_state_contexts: AtomicU64::new(0),
            state: Mutex::new(SlowState {
                cameras: Vec::new(),
                skipped_cameras: Vec::new(),
                failures: VecDeque::new(),
                dropped_failures: 0,
            }),
        }
    }

    fn slow_state(&self) -> MutexGuard<'_, SlowState> {
        // Отравление замка (паника другого потока под замком) диагностику не ломает.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Счётчики RAW-приёмника (пишет слушатель `AImageReader`).
    pub fn raw_sink(&self) -> &RawSinkCounters {
        &self.raw_sink
    }

    pub fn record_callback_panic(&self) {
        self.callback_panics.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_callback_after_close(&self) {
        self.callbacks_after_close.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_unmatched_frame_number(&self) {
        self.unmatched_frame_numbers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retained_session_context(&self) {
        self.retained_session_contexts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_leaked_availability_context(&self) {
        self.leaked_availability_contexts.fetch_add(1, Ordering::Relaxed);
    }

    /// Бэкенд разрушен, `count` контекстов состояния сессий намеренно оставлены в памяти.
    pub fn record_leaked_session_state_contexts(&self, count: u64) {
        self.leaked_session_state_contexts.fetch_add(count, Ordering::Relaxed);
    }

    /// Запомнить разбор камеры (заменяет прежнюю запись о ней при повторном чтении каталога).
    pub fn record_camera(&self, camera: &CameraId, vendor_tags: Vec<VendorTagReport>, notes: Vec<CharacteristicNote>) {
        let report = CameraCatalogReport {
            camera: camera.to_string(),
            vendor_tags,
            notes,
        };
        let mut state = self.slow_state();
        state.cameras.retain(|existing| existing.camera != report.camera);
        state.cameras.push(report);
    }

    /// Запомнить, что камера в каталог не попала (повторная запись о той же камере заменяется).
    pub fn record_skipped_camera(&self, camera: &str, reason: String) {
        let mut state = self.slow_state();
        state.skipped_cameras.retain(|existing| existing.camera != camera);
        state.skipped_cameras.push(SkippedCamera {
            camera: camera.to_owned(),
            reason,
        });
    }

    /// Начало нового чтения каталога: прежние записи о камерах устарели.
    pub fn reset_catalog(&self) {
        let mut state = self.slow_state();
        state.cameras.clear();
        state.skipped_cameras.clear();
    }

    /// Записать отказ платформы; при переполнении старейшая запись вытесняется и считается.
    pub fn record_failure(&self, operation: &str, code: i32, detail: &str) {
        let mut state = self.slow_state();
        if state.failures.len() == self.failure_capacity {
            state.failures.pop_front();
            state.dropped_failures = state.dropped_failures.saturating_add(1);
        }
        state.failures.push_back(RecordedFailure {
            operation: operation.to_owned(),
            code,
            detail: detail.to_owned(),
        });
    }

    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        let state = self.slow_state();
        DiagnosticsSnapshot {
            raw_sink: self.raw_sink.snapshot(),
            callback_panics: self.callback_panics.load(Ordering::Relaxed),
            callbacks_after_close: self.callbacks_after_close.load(Ordering::Relaxed),
            unmatched_frame_numbers: self.unmatched_frame_numbers.load(Ordering::Relaxed),
            retained_session_contexts: self.retained_session_contexts.load(Ordering::Relaxed),
            leaked_availability_contexts: self.leaked_availability_contexts.load(Ordering::Relaxed),
            leaked_session_state_contexts: self.leaked_session_state_contexts.load(Ordering::Relaxed),
            cameras: state.cameras.clone(),
            skipped_cameras: state.skipped_cameras.clone(),
            recent_failures: state.failures.iter().cloned().collect(),
            dropped_failures: state.dropped_failures,
        }
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
