//! Приращения накопительных счётчиков за прогон `raw-stream`.
//!
//! Счётчики потока, читателей и показа живут столько же, сколько процесс, и накапливают итоги
//! прошлых сценариев. Вердикт судит только то, что произошло за прогон: разность снимков «после»
//! и «до». Измерения-максимумы и «сейчас» (например, `max_frames_outstanding`) не вычитаются —
//! вычитать максимум бессмысленно, а нужен именно пик за жизнь процесса: более высокий пик
//! прошлого сценария не должен быть выше плана пула, иначе это тоже нарушение.

use camera_android::reader_counters::{ArrivalAgeSnapshot, ReaderSnapshot};
use raw_frame_stream::RawStreamCounters;
use render_loop::camera_diagnostics::CameraStreamDiagnostics;
use serde::Serialize;

/// Разность `after - before` с насыщением в ноль: счётчик не должен уменьшаться, но «меньше, чем
/// было» (чужой сброс) не оборачивается, а даёт ноль.
fn grew(before: u64, after: u64) -> u64 {
    after.saturating_sub(before)
}

/// Приращение счётчиков потока кадров. Максимумы и «сейчас» берутся из снимка «после».
pub fn stream_delta(before: &RawStreamCounters, after: &RawStreamCounters) -> RawStreamCounters {
    RawStreamCounters {
        frames_offered: grew(before.frames_offered, after.frames_offered),
        results_offered: grew(before.results_offered, after.results_offered),
        stale_epoch_frames: grew(before.stale_epoch_frames, after.stale_epoch_frames),
        stale_epoch_results: grew(before.stale_epoch_results, after.stale_epoch_results),
        dropped_pool_exhausted: grew(before.dropped_pool_exhausted, after.dropped_pool_exhausted),
        frames_paired: grew(before.frames_paired, after.frames_paired),
        frames_missing_meta_timeout: grew(before.frames_missing_meta_timeout, after.frames_missing_meta_timeout),
        frames_missing_meta_stopped: grew(before.frames_missing_meta_stopped, after.frames_missing_meta_stopped),
        results_unpaired: grew(before.results_unpaired, after.results_unpaired),
        max_pairing_wait_micros: after.max_pairing_wait_micros,
        total_pairing_wait_micros: grew(before.total_pairing_wait_micros, after.total_pairing_wait_micros),
        frames_published: grew(before.frames_published, after.frames_published),
        viewfinder_delivered: grew(before.viewfinder_delivered, after.viewfinder_delivered),
        dropped_superseded: grew(before.dropped_superseded, after.dropped_superseded),
        dropped_jitter_overflow: grew(before.dropped_jitter_overflow, after.dropped_jitter_overflow),
        viewfinder_polls_while_priming: grew(
            before.viewfinder_polls_while_priming,
            after.viewfinder_polls_while_priming,
        ),
        viewfinder_underruns: grew(before.viewfinder_underruns, after.viewfinder_underruns),
        viewfinder_flushed: grew(before.viewfinder_flushed, after.viewfinder_flushed),
        // Измерение «сейчас» (кадров в очереди), а не накопление: берётся из снимка «после».
        viewfinder_queued: after.viewfinder_queued,
        viewfinder_pacing_switches: grew(before.viewfinder_pacing_switches, after.viewfinder_pacing_switches),
        ring_evicted: grew(before.ring_evicted, after.ring_evicted),
        // Выбор кадра из кольца для снимка (решение 139): накопления.
        ring_picks_exact: grew(before.ring_picks_exact, after.ring_picks_exact),
        ring_picks_substituted: grew(before.ring_picks_substituted, after.ring_picks_substituted),
        ring_picks_unavailable: grew(before.ring_picks_unavailable, after.ring_picks_unavailable),
        lossless_defects: grew(before.lossless_defects, after.lossless_defects),
        released_after_fence: grew(before.released_after_fence, after.released_after_fence),
        released_unused: grew(before.released_unused, after.released_unused),
        leaked_leases: grew(before.leaked_leases, after.leaked_leases),
        frames_returned: grew(before.frames_returned, after.frames_returned),
        frames_leaked: grew(before.frames_leaked, after.frames_leaked),
        frames_outstanding: after.frames_outstanding,
        max_frames_outstanding: after.max_frames_outstanding,
        cpu_copies: grew(before.cpu_copies, after.cpu_copies),
        // Тёплые камеры (13b): все — накопления, считаются приращением.
        standby_streams_begun: grew(before.standby_streams_begun, after.standby_streams_begun),
        standby_begins_refused: grew(before.standby_begins_refused, after.standby_begins_refused),
        standby_frames_returned: grew(before.standby_frames_returned, after.standby_frames_returned),
        standby_promotions: grew(before.standby_promotions, after.standby_promotions),
        frames_released_by_promotion: grew(before.frames_released_by_promotion, after.frames_released_by_promotion),
        standby_results_evicted: grew(before.standby_results_evicted, after.standby_results_evicted),
        standby_pool_exhausted: grew(before.standby_pool_exhausted, after.standby_pool_exhausted),
    }
}

/// Приращение счётчиков читателей `AImageReader`. Код последней ошибки берётся из снимка «после».
pub fn reader_delta(before: &ReaderSnapshot, after: &ReaderSnapshot) -> ReaderSnapshot {
    ReaderSnapshot {
        frames_acquired: grew(before.frames_acquired, after.frames_acquired),
        acquire_errors: grew(before.acquire_errors, after.acquire_errors),
        last_acquire_error: after.last_acquire_error,
        pool_exhausted: grew(before.pool_exhausted, after.pool_exhausted),
        fence_merge_failures: grew(before.fence_merge_failures, after.fence_merge_failures),
        images_returned: grew(before.images_returned, after.images_returned),
        images_leaked: grew(before.images_leaked, after.images_leaked),
        readers_retired: grew(before.readers_retired, after.readers_retired),
        readers_deleted: grew(before.readers_deleted, after.readers_deleted),
        readers_leaked: grew(before.readers_leaked, after.readers_leaked),
        cpu_access_probe_copies: grew(before.cpu_access_probe_copies, after.cpu_access_probe_copies),
        cpu_access_probe_failures: grew(before.cpu_access_probe_failures, after.cpu_access_probe_failures),
        arrival_age: arrival_age_delta(&before.arrival_age, &after.arrival_age),
    }
}

/// Приращение замера возраста кадров. Выборки, сумма и сбои вычитаются; min/max — за всё время
/// процесса (из снимка «после»): минимум и максимум вычитанием не получить.
fn arrival_age_delta(before: &ArrivalAgeSnapshot, after: &ArrivalAgeSnapshot) -> ArrivalAgeSnapshot {
    ArrivalAgeSnapshot {
        samples: grew(before.samples, after.samples),
        total_micros: grew(before.total_micros, after.total_micros),
        min_micros: after.min_micros,
        max_micros: after.max_micros,
        clock_failures: grew(before.clock_failures, after.clock_failures),
        negative: grew(before.negative, after.negative),
    }
}

/// Счётчики показа кадров камеры рендером за прогон (без истории показов: она для задержки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RenderCameraDelta {
    /// Разных кадров поставлено на показ.
    pub camera_frames_presented: u64,
    pub camera_frame_redraws: u64,
    pub camera_frames_imported: u64,
    /// Новых кадров, которые не удалось подготовить (импорт, fence, паспорт).
    pub camera_frame_failures: u64,
    pub import_cache_hits: u64,
    pub import_cache_misses: u64,
    pub release_fences_exported: u64,
    pub release_export_failures: u64,
    pub leases_released_after_idle: u64,
    /// Лизинги, брошенные при зависшем GPU (учтённая утечка).
    pub leases_leaked_on_teardown: u64,
}

impl RenderCameraDelta {
    pub fn between(before: &CameraStreamDiagnostics, after: &CameraStreamDiagnostics) -> Self {
        Self {
            camera_frames_presented: grew(before.camera_frames_presented, after.camera_frames_presented),
            camera_frame_redraws: grew(before.camera_frame_redraws, after.camera_frame_redraws),
            camera_frames_imported: grew(before.camera_frames_imported, after.camera_frames_imported),
            camera_frame_failures: grew(before.camera_frame_failures, after.camera_frame_failures),
            import_cache_hits: grew(before.import_cache_hits, after.import_cache_hits),
            import_cache_misses: grew(before.import_cache_misses, after.import_cache_misses),
            release_fences_exported: grew(before.release_fences_exported, after.release_fences_exported),
            release_export_failures: grew(before.release_export_failures, after.release_export_failures),
            leases_released_after_idle: grew(before.leases_released_after_idle, after.leases_released_after_idle),
            leases_leaked_on_teardown: grew(before.leases_leaked_on_teardown, after.leases_leaked_on_teardown),
        }
    }
}

#[cfg(test)]
#[path = "raw_stream_deltas_tests.rs"]
mod tests;
