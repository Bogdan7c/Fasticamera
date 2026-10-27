//! Счётчики потока кадров: живые атомики внутри и снимок [`RawStreamCounters`] наружу.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// Снимок счётчиков для диагностики и smoke. Все значения — с создания потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RawStreamCounters {
    // --- Вход от производителя ---
    /// Буферов предложено производителем.
    pub frames_offered: u64,
    /// Результатов запроса предложено.
    pub results_offered: u64,
    /// Кадров/результатов прошлой эпохи (после смены сессии) — отпущены без раздачи.
    pub stale_epoch_frames: u64,
    pub stale_epoch_results: u64,
    /// Производитель не смог взять буфер: пул исчерпан (`AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED`).
    /// Признак ошибки расчёта пула, а не норма.
    pub dropped_pool_exhausted: u64,

    // --- Спаривание по SENSOR_TIMESTAMP ---
    pub frames_paired: u64,
    /// Кадр ушёл без метаданных: пара не пришла за `pairing_timeout_ms` (решение 42).
    pub frames_missing_meta_timeout: u64,
    /// Кадр ждал пару, когда поток остановили.
    pub frames_missing_meta_stopped: u64,
    /// Результат так и не нашёл свой кадр (вытеснен из буфера ожидания или опоздал).
    pub results_unpaired: u64,
    /// Самое долгое и суммарное ожидание пары (мкс): цена спаривания в задержке видоискателя.
    pub max_pairing_wait_micros: u64,
    pub total_pairing_wait_micros: u64,

    // --- Раздача потребителям ---
    pub frames_published: u64,
    pub viewfinder_delivered: u64,
    /// Видоискатель не успел забрать кадр, пришёл новее — старый отпущен (норма `LatestWins`).
    pub dropped_superseded: u64,
    /// Кольцо снимков вытеснило старейший кадр (норма `KeepLast`).
    pub ring_evicted: u64,
    /// Потребитель «ни одного молча» не принял кадр — брак (будущая запись).
    pub lossless_defects: u64,

    // --- Возврат буферов ---
    pub released_after_fence: u64,
    pub released_unused: u64,
    /// Лизинг выброшен без возврата (решение 43). Должно быть 0.
    pub leaked_leases: u64,
    /// Буферов отдано производителю / утекло — только тех, что стали лизингом.
    ///
    /// Баланс буферов: `frames_offered` равно сумме `frames_returned`, `frames_leaked`,
    /// `frames_outstanding`, `stale_epoch_frames`, `frames_missing_meta_stopped` и числа кадров,
    /// которые ждут пару прямо сейчас. Кадры чужой
    /// эпохи и кадры, ждавшие пару при остановке, возвращаются камере в обход лизинга
    /// и считаются только своими счётчиками.
    pub frames_returned: u64,
    pub frames_leaked: u64,
    /// Буферов в лизинге сейчас и максимум за всё время — сверка с планом пула.
    pub frames_outstanding: u64,
    pub max_frames_outstanding: u64,

    /// Копий пикселей через CPU (инвариант 1: на пути видоискателя должно быть 0).
    pub cpu_copies: u64,
}

/// Живые счётчики. Обновляются из колбэков камеры и потока рендера без замков.
#[derive(Debug, Default)]
pub(crate) struct StreamAccounting {
    pub(crate) frames_offered: AtomicU64,
    pub(crate) results_offered: AtomicU64,
    pub(crate) stale_epoch_frames: AtomicU64,
    pub(crate) stale_epoch_results: AtomicU64,
    pub(crate) dropped_pool_exhausted: AtomicU64,
    pub(crate) frames_paired: AtomicU64,
    pub(crate) frames_missing_meta_timeout: AtomicU64,
    pub(crate) frames_missing_meta_stopped: AtomicU64,
    pub(crate) results_unpaired: AtomicU64,
    pub(crate) max_pairing_wait_micros: AtomicU64,
    pub(crate) total_pairing_wait_micros: AtomicU64,
    pub(crate) frames_published: AtomicU64,
    pub(crate) viewfinder_delivered: AtomicU64,
    pub(crate) dropped_superseded: AtomicU64,
    pub(crate) ring_evicted: AtomicU64,
    pub(crate) lossless_defects: AtomicU64,
    released_after_fence: AtomicU64,
    released_unused: AtomicU64,
    leaked_leases: AtomicU64,
    frames_returned: AtomicU64,
    frames_leaked: AtomicU64,
    frames_outstanding: AtomicU64,
    max_frames_outstanding: AtomicU64,
    pub(crate) cpu_copies: AtomicU64,
}

/// Счётчики — монотонная статистика, порядок между ними не важен: `Relaxed` достаточно.
const ORDER: Ordering = Ordering::Relaxed;

impl StreamAccounting {
    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, ORDER);
    }

    pub(crate) fn note_pairing_wait_micros(&self, micros: u64) {
        self.max_pairing_wait_micros.fetch_max(micros, ORDER);
        self.total_pairing_wait_micros.fetch_add(micros, ORDER);
    }

    pub(crate) fn note_frame_leased(&self) {
        let outstanding = self.frames_outstanding.fetch_add(1, ORDER) + 1;
        self.max_frames_outstanding.fetch_max(outstanding, ORDER);
    }

    pub(crate) fn note_released_after_fence(&self) {
        Self::bump(&self.released_after_fence);
    }

    pub(crate) fn note_released_unused(&self) {
        Self::bump(&self.released_unused);
    }

    pub(crate) fn note_leaked_lease(&self) {
        Self::bump(&self.leaked_leases);
    }

    pub(crate) fn note_frame_returned(&self) {
        Self::bump(&self.frames_returned);
        self.frames_outstanding.fetch_sub(1, ORDER);
    }

    pub(crate) fn note_frame_leaked(&self) {
        Self::bump(&self.frames_leaked);
        self.frames_outstanding.fetch_sub(1, ORDER);
    }

    pub(crate) fn snapshot(&self) -> RawStreamCounters {
        RawStreamCounters {
            frames_offered: self.frames_offered.load(ORDER),
            results_offered: self.results_offered.load(ORDER),
            stale_epoch_frames: self.stale_epoch_frames.load(ORDER),
            stale_epoch_results: self.stale_epoch_results.load(ORDER),
            dropped_pool_exhausted: self.dropped_pool_exhausted.load(ORDER),
            frames_paired: self.frames_paired.load(ORDER),
            frames_missing_meta_timeout: self.frames_missing_meta_timeout.load(ORDER),
            frames_missing_meta_stopped: self.frames_missing_meta_stopped.load(ORDER),
            results_unpaired: self.results_unpaired.load(ORDER),
            max_pairing_wait_micros: self.max_pairing_wait_micros.load(ORDER),
            total_pairing_wait_micros: self.total_pairing_wait_micros.load(ORDER),
            frames_published: self.frames_published.load(ORDER),
            viewfinder_delivered: self.viewfinder_delivered.load(ORDER),
            dropped_superseded: self.dropped_superseded.load(ORDER),
            ring_evicted: self.ring_evicted.load(ORDER),
            lossless_defects: self.lossless_defects.load(ORDER),
            released_after_fence: self.released_after_fence.load(ORDER),
            released_unused: self.released_unused.load(ORDER),
            leaked_leases: self.leaked_leases.load(ORDER),
            frames_returned: self.frames_returned.load(ORDER),
            frames_leaked: self.frames_leaked.load(ORDER),
            frames_outstanding: self.frames_outstanding.load(ORDER),
            max_frames_outstanding: self.max_frames_outstanding.load(ORDER),
            cpu_copies: self.cpu_copies.load(ORDER),
        }
    }
}
