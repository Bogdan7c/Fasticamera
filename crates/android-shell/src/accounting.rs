//! Учёт оболочки: живые глобальные ссылки JNI и счётчики сбоев.
//!
//! Нейтральная часть (тестируется на ПК). Android-модули только сообщают сюда о событиях.

use std::sync::atomic::{AtomicU64, Ordering};

use shell_contract::diagnostics::ShellCounters;
use shell_contract::event_queue::ShellQueueStats;

/// Счётчики оболочки. Все — монотонные с начала процесса.
#[derive(Debug, Default)]
pub struct ShellAccounting {
    global_refs_created: AtomicU64,
    global_refs_deleted: AtomicU64,
    activity_attachments: AtomicU64,
    surface_release_timeouts: AtomicU64,
    platform_command_failures: AtomicU64,
    native_callback_failures: AtomicU64,
}

impl ShellAccounting {
    pub const fn new() -> Self {
        Self {
            global_refs_created: AtomicU64::new(0),
            global_refs_deleted: AtomicU64::new(0),
            activity_attachments: AtomicU64::new(0),
            surface_release_timeouts: AtomicU64::new(0),
            platform_command_failures: AtomicU64::new(0),
            native_callback_failures: AtomicU64::new(0),
        }
    }

    pub fn record_global_ref_created(&self) {
        self.global_refs_created.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_global_ref_deleted(&self) {
        self.global_refs_deleted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_activity_attached(&self) {
        self.activity_attachments.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_surface_release_timeout(&self) {
        self.surface_release_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_platform_command_failure(&self) {
        self.platform_command_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_native_callback_failure(&self) {
        self.native_callback_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Создано минус удалено. Насыщается на нуле: «удалено больше, чем создано» — ошибка учёта,
    /// но не повод паниковать; она видна, если сравнить с ожидаемым значением.
    pub fn live_global_refs(&self) -> u64 {
        let created = self.global_refs_created.load(Ordering::Relaxed);
        let deleted = self.global_refs_deleted.load(Ordering::Relaxed);
        created.saturating_sub(deleted)
    }

    /// Снимок для отчётов вместе со счётчиками очереди событий.
    pub fn snapshot(&self, queue: ShellQueueStats) -> ShellCounters {
        ShellCounters {
            live_global_refs: self.live_global_refs(),
            activity_attachments: self.activity_attachments.load(Ordering::Relaxed),
            surface_release_timeouts: self.surface_release_timeouts.load(Ordering::Relaxed),
            touch_samples_dropped: queue.touch_samples_dropped,
            critical_events_lost: queue.critical_events_lost,
            platform_command_failures: self.platform_command_failures.load(Ordering::Relaxed),
            native_callback_failures: self.native_callback_failures.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_refs_is_created_minus_deleted() {
        let accounting = ShellAccounting::new();
        accounting.record_global_ref_created();
        accounting.record_global_ref_created();
        accounting.record_global_ref_deleted();
        assert_eq!(accounting.live_global_refs(), 1);
        accounting.record_global_ref_deleted();
        accounting.record_global_ref_deleted();
        assert_eq!(accounting.live_global_refs(), 0, "насыщение, не переполнение");
    }

    #[test]
    fn snapshot_combines_own_and_queue_counters() {
        let accounting = ShellAccounting::new();
        accounting.record_activity_attached();
        accounting.record_surface_release_timeout();
        accounting.record_platform_command_failure();
        accounting.record_native_callback_failure();
        let snapshot = accounting.snapshot(ShellQueueStats {
            delivered: 10,
            touch_samples_dropped: 3,
            critical_events_lost: 1,
        });
        assert_eq!(
            snapshot,
            ShellCounters {
                live_global_refs: 0,
                activity_attachments: 1,
                surface_release_timeouts: 1,
                touch_samples_dropped: 3,
                critical_events_lost: 1,
                platform_command_failures: 1,
                native_callback_failures: 1,
            }
        );
    }
}
