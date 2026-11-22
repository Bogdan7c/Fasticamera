//! Диагностика службы записи снимков: счётчики для smoke-сценария `capture` и экрана отладки.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use still_contract::phase::StillFailure;

use super::admission::StillAdmission;

/// Счётчики, которые ведёт служба (кроме учёта допуска — им владеет [`StillAdmission`]).
#[derive(Default)]
pub(super) struct StillCounters {
    saved: AtomicU64,
    failed_no_frame: AtomicU64,
    failed_develop: AtomicU64,
    failed_encode: AtomicU64,
    failed_store: AtomicU64,
    failed_unavailable: AtomicU64,
    timed_out: AtomicU64,
    late_events: AtomicU64,
    rejected_events: AtomicU64,
    discarded_drafts: AtomicU64,
    raw_lost_after_primary_published: AtomicU64,
    exif_build_failures: AtomicU64,
    worker_panics: AtomicU64,
}

impl StillCounters {
    pub(super) fn record_saved(&self) {
        self.saved.fetch_add(1, Ordering::Relaxed);
    }

    /// Считает срыв по причине.
    pub(super) fn record_failure(&self, failure: &StillFailure) {
        let counter = match failure {
            StillFailure::NoFrame(_) => &self.failed_no_frame,
            StillFailure::Develop(_) => &self.failed_develop,
            StillFailure::Encode(_) => &self.failed_encode,
            StillFailure::Store(_) => &self.failed_store,
            StillFailure::ServiceUnavailable => &self.failed_unavailable,
            // Отказ допуска не доходит до службы как снимок: он считается в `StillAdmission`.
            StillFailure::QueueFull => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_timeout(&self) {
        self.timed_out.fetch_add(1, Ordering::Relaxed);
    }

    /// Событие рендера пришло для снимка, которого служба уже не ждёт.
    pub(super) fn record_late_event(&self) {
        self.late_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Событие рендера отброшено из-за переполнения очереди.
    pub(super) fn record_rejected_event(&self) {
        self.rejected_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_discarded_draft(&self) {
        self.discarded_drafts.fetch_add(1, Ordering::Relaxed);
    }

    /// Основной файл опубликован, а DNG — нет (см. порядок публикации в `worker::finish`).
    pub(super) fn record_raw_lost_after_primary_published(&self) {
        self.raw_lost_after_primary_published.fetch_add(1, Ordering::Relaxed);
    }

    /// Блок Exif не собрался: снимок записан без метаданных.
    pub(super) fn record_exif_build_failure(&self) {
        self.exif_build_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Рабочий поток поймал панику и остановил службу.
    pub(super) fn record_worker_panic(&self) {
        self.worker_panics.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn snapshot(&self, admission: &StillAdmission) -> StillServiceStats {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        StillServiceStats {
            accepted: admission.accepted_total(),
            saved: load(&self.saved),
            failed_no_frame: load(&self.failed_no_frame),
            failed_develop: load(&self.failed_develop),
            failed_encode: load(&self.failed_encode),
            failed_store: load(&self.failed_store),
            failed_unavailable: load(&self.failed_unavailable),
            timed_out: load(&self.timed_out),
            late_events: load(&self.late_events),
            rejected_events: load(&self.rejected_events),
            discarded_drafts: load(&self.discarded_drafts),
            raw_lost_after_primary_published: load(&self.raw_lost_after_primary_published),
            exif_build_failures: load(&self.exif_build_failures),
            worker_panics: load(&self.worker_panics),
            admission_refusals: admission.refused_total(),
            pending_now: u64::try_from(admission.pending_count()).unwrap_or(u64::MAX),
        }
    }
}

/// Снимок счётчиков службы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct StillServiceStats {
    /// Допущено снимков.
    pub accepted: u64,
    /// Сохранено (все файлы опубликованы).
    pub saved: u64,
    pub failed_no_frame: u64,
    pub failed_develop: u64,
    pub failed_encode: u64,
    pub failed_store: u64,
    /// Сорвано из-за остановки службы или недоступной платформы.
    pub failed_unavailable: u64,
    /// Сколько срывов случилось из-за истёкшего срока (они же входят в `failed_*`).
    pub timed_out: u64,
    /// События рендера, пришедшие после конца снимка (учтены и проигнорированы).
    pub late_events: u64,
    /// События рендера, отброшенные переполнением очереди.
    pub rejected_events: u64,
    /// Удалённые черновики хранилища.
    pub discarded_drafts: u64,
    /// Основной файл опубликован, DNG — нет (снимок `Saved` без raw).
    pub raw_lost_after_primary_published: u64,
    /// Блок Exif не собрался: снимок записан БЕЗ метаданных (не срыв, но потеря данных).
    pub exif_build_failures: u64,
    /// Паники рабочего потока (после паники служба остановлена, живые снимки сорваны).
    pub worker_panics: u64,
    /// Отказы допуска (очередь полна, повтор номера, служба остановлена).
    pub admission_refusals: u64,
    /// Сколько снимков ждут сохранения прямо сейчас.
    pub pending_now: u64,
}
