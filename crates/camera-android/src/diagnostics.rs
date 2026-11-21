//! Диагностика бэкенда: счётчики и отказы, которые приложение читает снимком.
//!
//! Один общий объект `Arc<BackendDiagnostics>`: его отдаёт `backend_factory` вместе с фабрикой,
//! пишут колбэки NDK (атомарные счётчики, без блокировок на горячем пути) и сам бэкенд
//! (каталог, отказы — под коротким замком), читает приложение через [`BackendDiagnostics::snapshot`].
//! Тип нейтральный и сериализуемый, поэтому проверяется на ПК.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use camera_contract::ids::CameraId;
use serde::Serialize;

use crate::characteristics::CharacteristicNote;
use crate::companion_stream::CompanionStreamStatus;
use crate::reader_counters::{ReaderCounters, ReaderSnapshot};
use crate::request_controls_plan::PlanNote;
use crate::request_origin::OriginLookup;
use crate::standby_diagnostics::{StandbyDiagnostics, StandbySlotSnapshot};
use crate::vendor::VendorTagReport;

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
    companion: CompanionStreamStatus,
    plan_notes: BTreeMap<&'static str, u64>,
}

/// Общая диагностика бэкенда.
#[derive(Debug)]
pub struct BackendDiagnostics {
    failure_capacity: usize,
    reader: ReaderCounters,
    callback_panics: AtomicU64,
    callbacks_after_close: AtomicU64,
    unmatched_frame_numbers: AtomicU64,
    retained_session_contexts: AtomicU64,
    leaked_availability_contexts: AtomicU64,
    leaked_session_state_contexts: AtomicU64,
    results_without_request_context: AtomicU64,
    results_with_unregistered_request: AtomicU64,
    evicted_request_origins: AtomicU64,
    unreadable_three_a_keys: AtomicU64,
    companion_stray_images: AtomicU64,
    standby: StandbyDiagnostics,
    state: Mutex<SlowState>,
}

/// Снимок диагностики для приложения (сериализуется в JSON).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnosticsSnapshot {
    /// Счётчики reader-ов: кадры, возвраты, утечки, пробы CPU-доступа.
    pub reader: ReaderSnapshot,
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
    /// Результаты, у которых NDK не вернул контекст запроса (происхождение стало `Unknown`).
    /// Не ноль — значит `ACaptureRequest_setUserContext` не доживает до колбэков (проверка smoke `3a`).
    pub results_without_request_context: u64,
    /// Результаты с ключом, которого нет в таблице происхождений (запись снята или вытеснена).
    pub results_with_unregistered_request: u64,
    /// Записи таблицы происхождений, вытесненные при переполнении.
    pub evicted_request_origins: u64,
    /// Ключи состояния 3A в результатах, присутствующие, но непригодные.
    pub unreadable_three_a_keys: u64,
    /// Образы, пришедшие во вспомогательный поток 3A (должно быть 0: в него не шлют запросов).
    pub companion_stray_images: u64,
    /// Состояние вспомогательного потока 3A последней сессии. `disabled_by_config`/`unavailable`
    /// значит, что автоматика HAL (AF, AE-состояние, баланс) мертва.
    pub three_a_companion: CompanionStreamStatus,
    /// Сколько раз при построении запросов желаемое не удалось записать (по виду заметки).
    pub request_plan_notes: BTreeMap<&'static str, u64>,
    pub cameras: Vec<CameraCatalogReport>,
    pub skipped_cameras: Vec<SkippedCamera>,
    /// Последние отказы платформы (старые вытесняются).
    pub recent_failures: Vec<RecordedFailure>,
    pub dropped_failures: u64,
    /// Счётчики тёплых камер по ячейкам (13b); ячейки без событий не выводятся.
    pub standby: Vec<StandbySlotSnapshot>,
}

impl BackendDiagnostics {
    pub fn new(failure_history_capacity: usize) -> Self {
        Self {
            failure_capacity: failure_history_capacity.max(1),
            reader: ReaderCounters::default(),
            callback_panics: AtomicU64::new(0),
            callbacks_after_close: AtomicU64::new(0),
            unmatched_frame_numbers: AtomicU64::new(0),
            retained_session_contexts: AtomicU64::new(0),
            leaked_availability_contexts: AtomicU64::new(0),
            leaked_session_state_contexts: AtomicU64::new(0),
            results_without_request_context: AtomicU64::new(0),
            results_with_unregistered_request: AtomicU64::new(0),
            evicted_request_origins: AtomicU64::new(0),
            unreadable_three_a_keys: AtomicU64::new(0),
            companion_stray_images: AtomicU64::new(0),
            standby: StandbyDiagnostics::default(),
            state: Mutex::new(SlowState {
                cameras: Vec::new(),
                skipped_cameras: Vec::new(),
                failures: VecDeque::new(),
                dropped_failures: 0,
                companion: CompanionStreamStatus::NotConfigured,
                plan_notes: BTreeMap::new(),
            }),
        }
    }

    fn slow_state(&self) -> MutexGuard<'_, SlowState> {
        // Отравление замка (паника другого потока под замком) диагностику не ломает.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Счётчики reader-ов (пишут слушатель `AImageReader`, возврат кадров и закрытие reader-ов).
    pub fn reader(&self) -> &ReaderCounters {
        &self.reader
    }

    /// Счётчики тёплых камер (13b): пишут колбэки результатов и поток камеры.
    pub fn standby(&self) -> &StandbyDiagnostics {
        &self.standby
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

    /// Итог поиска происхождения результата: неопознанное считается по причине.
    pub fn record_origin_lookup(&self, lookup: OriginLookup) {
        match lookup {
            OriginLookup::Known(_) => {}
            OriginLookup::NoContext => {
                self.results_without_request_context.fetch_add(1, Ordering::Relaxed);
            }
            OriginLookup::Unregistered => {
                self.results_with_unregistered_request.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Таблица происхождений была полна: вытеснена самая старая запись.
    pub fn record_evicted_request_origin(&self) {
        self.evicted_request_origins.fetch_add(1, Ordering::Relaxed);
    }

    /// В результате `count` ключей состояния 3A были, но не прочитались.
    pub fn record_unreadable_three_a_keys(&self, count: usize) {
        self.unreadable_three_a_keys
            .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// Во вспомогательном потоке 3A нашлись образы (их не должно быть); все удалены.
    pub fn record_companion_stray_images(&self, count: u64) {
        self.companion_stray_images.fetch_add(count, Ordering::Relaxed);
    }

    /// Состояние вспомогательного потока последней сессии (заменяет прежнее).
    pub fn record_companion_status(&self, status: CompanionStreamStatus) {
        self.slow_state().companion = status;
    }

    /// Что из желаемого в запросе не удалось записать: счётчик по виду заметки.
    pub fn record_plan_notes(&self, notes: &[PlanNote]) {
        if notes.is_empty() {
            return;
        }
        let mut state = self.slow_state();
        for note in notes {
            let count = state.plan_notes.entry(note.name()).or_insert(0);
            *count = count.saturating_add(1);
        }
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
            reader: self.reader.snapshot(),
            callback_panics: self.callback_panics.load(Ordering::Relaxed),
            callbacks_after_close: self.callbacks_after_close.load(Ordering::Relaxed),
            unmatched_frame_numbers: self.unmatched_frame_numbers.load(Ordering::Relaxed),
            retained_session_contexts: self.retained_session_contexts.load(Ordering::Relaxed),
            leaked_availability_contexts: self.leaked_availability_contexts.load(Ordering::Relaxed),
            leaked_session_state_contexts: self.leaked_session_state_contexts.load(Ordering::Relaxed),
            results_without_request_context: self.results_without_request_context.load(Ordering::Relaxed),
            results_with_unregistered_request: self.results_with_unregistered_request.load(Ordering::Relaxed),
            evicted_request_origins: self.evicted_request_origins.load(Ordering::Relaxed),
            unreadable_three_a_keys: self.unreadable_three_a_keys.load(Ordering::Relaxed),
            companion_stray_images: self.companion_stray_images.load(Ordering::Relaxed),
            three_a_companion: state.companion,
            request_plan_notes: state.plan_notes.clone(),
            cameras: state.cameras.clone(),
            skipped_cameras: state.skipped_cameras.clone(),
            recent_failures: state.failures.iter().cloned().collect(),
            dropped_failures: state.dropped_failures,
            standby: self.standby.snapshot(),
        }
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
