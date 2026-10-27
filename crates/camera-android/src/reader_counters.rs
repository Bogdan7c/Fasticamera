//! Счётчики `AImageReader` с лизингом кадров (замена временного RAW-приёмника сессий 03–04).
//!
//! Атомарные, без блокировок: их пишут слушатель reader-а (чужой поток NDK), поток рендера (при
//! возврате кадра) и поток камеры (при закрытии reader-ов). Читает приложение снимком
//! ([`ReaderCounters::snapshot`]). Каждый сброс или утечка обязаны быть здесь посчитаны —
//! «молча» ничего не пропадает (инвариант 4 README сессии 05).

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

/// Счётчики reader-ов камеры.
#[derive(Debug, Default)]
pub struct ReaderCounters {
    frames_acquired: AtomicU64,
    acquire_errors: AtomicU64,
    last_acquire_error: AtomicI32,
    pool_exhausted: AtomicU64,
    fence_merge_failures: AtomicU64,
    images_returned: AtomicU64,
    images_leaked: AtomicU64,
    readers_retired: AtomicU64,
    readers_deleted: AtomicU64,
    readers_leaked: AtomicU64,
    cpu_access_probe_copies: AtomicU64,
    cpu_access_probe_failures: AtomicU64,
    arrival_age: ArrivalAgeCounters,
}

/// Атомарные накопители «возраста кадра при приходе в reader» (микросекунды).
#[derive(Debug)]
struct ArrivalAgeCounters {
    samples: AtomicU64,
    total_micros: AtomicU64,
    /// `u64::MAX`, пока выборок нет; в снимке превращается в `None`.
    min_micros: AtomicU64,
    max_micros: AtomicU64,
    clock_failures: AtomicU64,
    negative: AtomicU64,
}

impl Default for ArrivalAgeCounters {
    fn default() -> Self {
        Self {
            samples: AtomicU64::new(0),
            total_micros: AtomicU64::new(0),
            min_micros: AtomicU64::new(u64::MAX),
            max_micros: AtomicU64::new(0),
            clock_failures: AtomicU64::new(0),
            negative: AtomicU64::new(0),
        }
    }
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

impl ReaderCounters {
    /// Кадр забран у reader-а и предложен потоку.
    pub fn record_frame_acquired(&self) {
        bump(&self.frames_acquired);
    }

    /// Сбой `acquire*` (кроме штатных «буферов нет» и «все на руках»). Сохраняется последний код.
    pub fn record_acquire_error(&self, status: i32) {
        bump(&self.acquire_errors);
        self.last_acquire_error.store(status, Ordering::Relaxed);
    }

    /// Reader ответил «все `maxImages` на руках»: кадр остался в очереди reader-а.
    pub fn record_pool_exhausted(&self) {
        bump(&self.pool_exhausted);
    }

    /// Не удалось слить fence-ы освобождения: буфер не возвращён (см. `images_leaked`).
    pub fn record_fence_merge_failure(&self) {
        bump(&self.fence_merge_failures);
    }

    /// Кадр возвращён камере (`AImage_deleteAsync`).
    pub fn record_image_returned(&self) {
        bump(&self.images_returned);
    }

    /// Кадр не возвращён и не будет (слияние fence-ов не удалось или объект кадра потерян).
    pub fn record_image_leaked(&self) {
        bump(&self.images_leaked);
    }

    /// Reader закрытой сессии ждёт возврата своих кадров вместо немедленного удаления.
    pub fn record_reader_retired(&self) {
        bump(&self.readers_retired);
    }

    /// Reader удалён (все его кадры были возвращены).
    pub fn record_reader_deleted(&self) {
        bump(&self.readers_deleted);
    }

    /// Reader не удалён: кадры не вернулись за отведённое время, он оставлен до конца процесса.
    pub fn record_reader_leaked(&self) {
        bump(&self.readers_leaked);
    }

    /// Отладочная проба скопировала первую строку кадра через CPU.
    pub fn record_cpu_probe_copy(&self) {
        bump(&self.cpu_access_probe_copies);
    }

    /// Проба не смогла прочитать кадр (замок, fence): сбой виден, а не молчалив.
    pub fn record_cpu_probe_failure(&self) {
        bump(&self.cpu_access_probe_failures);
    }

    /// Возраст кадра при приходе в reader: `CLOCK_BOOTTIME сейчас − SENSOR_TIMESTAMP`
    /// (начало экспозиции). Это доля камерного конвейера HAL в общей задержке до экрана.
    pub fn record_arrival_age(&self, age: Duration) {
        // Насыщение вместо паники: возраст в сотни тысяч лет сюда не придёт, но и обрезка не молчит о типе.
        let micros = u64::try_from(age.as_micros()).unwrap_or(u64::MAX);
        let counters = &self.arrival_age;
        counters.samples.fetch_add(1, Ordering::Relaxed);
        counters.total_micros.fetch_add(micros, Ordering::Relaxed);
        counters.min_micros.fetch_min(micros, Ordering::Relaxed);
        counters.max_micros.fetch_max(micros, Ordering::Relaxed);
    }

    /// Часы `CLOCK_BOOTTIME` не прочитались: возраст кадра не измерен (выборки нет).
    pub fn record_arrival_clock_failure(&self) {
        bump(&self.arrival_age.clock_failures);
    }

    /// Метка сенсора позже «сейчас» (из будущего): возраст отрицателен, выборки нет.
    pub fn record_arrival_age_negative(&self) {
        bump(&self.arrival_age.negative);
    }

    pub(crate) fn snapshot(&self) -> ReaderSnapshot {
        let errors = self.acquire_errors.load(Ordering::Relaxed);
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        ReaderSnapshot {
            frames_acquired: load(&self.frames_acquired),
            acquire_errors: errors,
            last_acquire_error: (errors > 0).then(|| self.last_acquire_error.load(Ordering::Relaxed)),
            pool_exhausted: load(&self.pool_exhausted),
            fence_merge_failures: load(&self.fence_merge_failures),
            images_returned: load(&self.images_returned),
            images_leaked: load(&self.images_leaked),
            readers_retired: load(&self.readers_retired),
            readers_deleted: load(&self.readers_deleted),
            readers_leaked: load(&self.readers_leaked),
            cpu_access_probe_copies: load(&self.cpu_access_probe_copies),
            cpu_access_probe_failures: load(&self.cpu_access_probe_failures),
            arrival_age: self.arrival_age.snapshot(),
        }
    }
}

impl ArrivalAgeCounters {
    fn snapshot(&self) -> ArrivalAgeSnapshot {
        let samples = self.samples.load(Ordering::Relaxed);
        let has_samples = samples > 0;
        ArrivalAgeSnapshot {
            samples,
            total_micros: self.total_micros.load(Ordering::Relaxed),
            min_micros: has_samples.then(|| self.min_micros.load(Ordering::Relaxed)),
            max_micros: has_samples.then(|| self.max_micros.load(Ordering::Relaxed)),
            clock_failures: self.clock_failures.load(Ordering::Relaxed),
            negative: self.negative.load(Ordering::Relaxed),
        }
    }
}

/// Снимок возраста кадров при приходе в reader (измерение, не вердикт).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ArrivalAgeSnapshot {
    /// Сколько кадров измерено.
    pub samples: u64,
    pub total_micros: u64,
    /// Минимальный возраст; `None`, пока выборок нет.
    pub min_micros: Option<u64>,
    /// Максимальный возраст; `None`, пока выборок нет.
    pub max_micros: Option<u64>,
    /// Сколько раз не прочитались часы `CLOCK_BOOTTIME`.
    pub clock_failures: u64,
    /// Сколько кадров пришло с меткой сенсора «из будущего».
    pub negative: u64,
}

impl ArrivalAgeSnapshot {
    /// Средний возраст в микросекундах; `None`, если выборок нет.
    pub fn mean_micros(&self) -> Option<u64> {
        self.total_micros.checked_div(self.samples)
    }
}

/// Снимок счётчиков reader-ов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReaderSnapshot {
    pub frames_acquired: u64,
    pub acquire_errors: u64,
    /// Код последнего сбоя `acquire*`, если сбои были.
    pub last_acquire_error: Option<i32>,
    pub pool_exhausted: u64,
    pub fence_merge_failures: u64,
    pub images_returned: u64,
    pub images_leaked: u64,
    pub readers_retired: u64,
    pub readers_deleted: u64,
    pub readers_leaked: u64,
    /// Сколько раз отладочная проба реально прочитала пиксели через CPU.
    pub cpu_access_probe_copies: u64,
    pub cpu_access_probe_failures: u64,
    /// Возраст кадра при приходе в reader (начало экспозиции → наш колбэк).
    pub arrival_age: ArrivalAgeSnapshot,
}

#[cfg(test)]
#[path = "reader_counters_tests.rs"]
mod tests;
