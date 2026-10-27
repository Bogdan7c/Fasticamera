//! Сводка диагностики бэкенда для строк результата сценариев камеры.
//!
//! Полный снимок (`DiagnosticsSnapshot`, с вендорскими тегами и заметками разбора по каждой камере) в
//! строку результата не помещается; сюда попадают только числа и отказы, по которым выносится вердикт.
//! Полные данные сценарии выкладывают строками стадий `detail_*` (см. `detail.rs`).

use camera_android::DiagnosticsSnapshot;
use camera_android::diagnostics::{RecordedFailure, SkippedCamera};
use camera_android::reader_counters::ReaderSnapshot;
use serde::Serialize;

/// Сколько закрытых reader-ов допустимо застать ещё не удалёнными в конце прогона: удаление ленивое
/// (на следующем вызове бэкенда), поэтому последний закрытый может ждать своей очереди.
const READERS_AWAITING_REAP_ALLOWED: u64 = 1;

/// Сводка диагностики бэкенда для результата (полная — в `detail_backend`).
#[derive(Debug, Serialize)]
pub struct BackendSummary<'snapshot> {
    pub callback_panics: u64,
    pub callbacks_after_close: u64,
    pub unmatched_frame_numbers: u64,
    /// Для скольких камер записан разбор вендорских тегов и заметок.
    pub cameras_with_vendor_report: usize,
    pub skipped_cameras: &'snapshot [SkippedCamera],
    pub failures_recorded: usize,
    pub last_failure: Option<&'snapshot RecordedFailure>,
    pub dropped_failures: u64,
    /// Жизнь reader-ов за весь процесс: закрытые сессии не должны оставлять reader-ы и буферы.
    pub readers: ReaderLifecycleSummary,
}

/// Учёт жизни reader-ов: reader закрытой сессии удаляется только после возврата всех его кадров
/// (сессия 05), иначе GPU читал бы освобождённую память. Здесь видно, что удаление действительно
/// происходит, а не копится.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReaderLifecycleSummary {
    pub retired: u64,
    pub deleted: u64,
    pub leaked: u64,
    pub images_leaked: u64,
}

impl ReaderLifecycleSummary {
    pub fn of(reader: &ReaderSnapshot) -> Self {
        Self {
            retired: reader.readers_retired,
            deleted: reader.readers_deleted,
            leaked: reader.readers_leaked,
            images_leaked: reader.images_leaked,
        }
    }

    /// Нарушения жизни reader-ов: утечки reader-ов и буферов, и закрытые reader-ы, которые копятся
    /// неудалёнными (больше [`READERS_AWAITING_REAP_ALLOWED`]).
    pub fn violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.leaked > 0 {
            violations.push(format!(
                "утекло reader-ов: {} (кадры не вернулись за отведённое время)",
                self.leaked
            ));
        }
        if self.images_leaked > 0 {
            violations.push(format!("утекло буферов камеры: {}", self.images_leaked));
        }
        let awaiting = self.retired.saturating_sub(self.deleted + self.leaked);
        if awaiting > READERS_AWAITING_REAP_ALLOWED {
            violations.push(format!(
                "закрытых reader-ов ждут удаления: {awaiting} (закрыто {}, удалено {})",
                self.retired, self.deleted
            ));
        }
        violations
    }
}

impl<'snapshot> BackendSummary<'snapshot> {
    pub fn of(snapshot: &'snapshot DiagnosticsSnapshot) -> Self {
        Self {
            callback_panics: snapshot.callback_panics,
            callbacks_after_close: snapshot.callbacks_after_close,
            unmatched_frame_numbers: snapshot.unmatched_frame_numbers,
            cameras_with_vendor_report: snapshot.cameras.len(),
            skipped_cameras: &snapshot.skipped_cameras,
            failures_recorded: snapshot.recent_failures.len(),
            last_failure: snapshot.recent_failures.last(),
            dropped_failures: snapshot.dropped_failures,
            readers: ReaderLifecycleSummary::of(&snapshot.reader),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle(retired: u64, deleted: u64, leaked: u64, images_leaked: u64) -> ReaderLifecycleSummary {
        ReaderLifecycleSummary {
            retired,
            deleted,
            leaked,
            images_leaked,
        }
    }

    #[test]
    fn all_closed_readers_deleted_is_clean() {
        assert!(lifecycle(5, 5, 0, 0).violations().is_empty());
    }

    #[test]
    fn one_reader_awaiting_lazy_reap_is_allowed_but_two_are_not() {
        assert!(lifecycle(5, 4, 0, 0).violations().is_empty());
        let violations = lifecycle(5, 3, 0, 0).violations();
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("ждут удаления: 2"), "{violations:?}");
    }

    #[test]
    fn leaked_readers_and_buffers_are_each_reported() {
        let violations = lifecycle(3, 2, 1, 4).violations();
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(violations[0].contains("reader-ов: 1"), "{violations:?}");
        assert!(violations[1].contains("буферов камеры: 4"), "{violations:?}");
    }
}
