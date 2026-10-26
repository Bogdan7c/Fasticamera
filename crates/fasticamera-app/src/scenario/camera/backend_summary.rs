//! Сводка диагностики бэкенда для строк результата сценариев камеры.
//!
//! Полный снимок (`DiagnosticsSnapshot`, с вендорскими тегами и заметками разбора по каждой камере) в
//! строку результата не помещается; сюда попадают только числа и отказы, по которым выносится вердикт.
//! Полные данные сценарии выкладывают строками стадий `detail_*` (см. `detail.rs`).

use camera_android::DiagnosticsSnapshot;
use camera_android::diagnostics::{RecordedFailure, SkippedCamera};
use serde::Serialize;

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
        }
    }
}
