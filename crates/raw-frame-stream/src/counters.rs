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
    /// Видоискатель не успел забрать кадр, пришёл новее — старый отпущен (норма `LatestWins`);
    /// при любой политике — и кадр, опубликованный позже уже выданного более нового.
    pub dropped_superseded: u64,
    /// Буфер видоискателя переполнен — старейший кадр отпущен (решение 53: часы камеры чуть
    /// быстрее экрана, ~1 кадр за оборот фазы). Не путать с `dropped_superseded`.
    pub dropped_jitter_overflow: u64,
    /// Опросы буфера, пока он заполнялся (старт, новая эпоха, переключение политики): рендер
    /// показывает прежнее изображение.
    pub viewfinder_polls_while_priming: u64,
    /// Опросы заполненного буфера, когда очередь пуста: повтор кадра на экране.
    pub viewfinder_underruns: u64,
    /// Кадры ячейки видоискателя, отпущенные концом потока или сменой эпохи.
    pub viewfinder_flushed: u64,
    /// Кадров в ячейке видоискателя сейчас. Баланс: `frames_published = viewfinder_delivered +
    /// dropped_superseded + dropped_jitter_overflow + viewfinder_flushed + viewfinder_queued`.
    pub viewfinder_queued: u64,
    /// Сколько раз политика видоискателя переключена на ходу.
    pub viewfinder_pacing_switches: u64,
    /// Кольцо снимков вытеснило старейший кадр (норма `KeepLast`).
    pub ring_evicted: u64,
    /// Выбор кадра из кольца для снимка: точное попадание.
    pub ring_picks_exact: u64,
    /// Выбор из кольца: выдан ближайший годный кадр вместо запрошенного (причина — в `RingPick`).
    pub ring_picks_substituted: u64,
    /// Выбор из кольца: выдать нечего (выключено, пусто, нет годного).
    pub ring_picks_unavailable: u64,
    /// Кадр со вспышкой (14d) найден в кольце по метке запроса (поиск не «выбор», замен нет).
    pub ring_flash_picks_found: u64,
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

    // --- Тёплые (standby) эпохи, сессия 13b ---
    /// Тёплых эпох начато.
    pub standby_streams_begun: u64,
    /// `begin_standby_stream` отказал: достигнут `max_standby_streams`.
    pub standby_begins_refused: u64,
    /// Кадры тёплых эпох, сразу возвращённые камере (в баланс буферов входят, см. выше).
    pub standby_frames_returned: u64,
    /// Удачных обменов `promote_standby`.
    pub standby_promotions: u64,
    /// Сколько кадров отпустил обмен у прежней показываемой эпохи (ожидающие пары + доли ячейки,
    /// кольца и очередей). Справочный счётчик: те же кадры учтены и в обычных (`viewfinder_flushed`,
    /// `frames_missing_meta_stopped`, `released_unused`), поэтому в балансе буферов не участвует.
    pub frames_released_by_promotion: u64,
    /// Результат тёплой эпохи вытеснен из её ограниченного буфера.
    pub standby_results_evicted: u64,
    /// Reader тёплой эпохи ответил «буферов нет» (отдельно от `dropped_pool_exhausted`).
    pub standby_pool_exhausted: u64,
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
    pub(crate) dropped_jitter_overflow: AtomicU64,
    pub(crate) viewfinder_polls_while_priming: AtomicU64,
    pub(crate) viewfinder_underruns: AtomicU64,
    pub(crate) viewfinder_flushed: AtomicU64,
    viewfinder_queued: AtomicU64,
    pub(crate) viewfinder_pacing_switches: AtomicU64,
    pub(crate) ring_evicted: AtomicU64,
    pub(crate) ring_picks_exact: AtomicU64,
    pub(crate) ring_picks_substituted: AtomicU64,
    pub(crate) ring_picks_unavailable: AtomicU64,
    pub(crate) ring_flash_picks_found: AtomicU64,
    pub(crate) lossless_defects: AtomicU64,
    released_after_fence: AtomicU64,
    released_unused: AtomicU64,
    leaked_leases: AtomicU64,
    frames_returned: AtomicU64,
    frames_leaked: AtomicU64,
    frames_outstanding: AtomicU64,
    max_frames_outstanding: AtomicU64,
    pub(crate) cpu_copies: AtomicU64,
    pub(crate) standby_streams_begun: AtomicU64,
    pub(crate) standby_begins_refused: AtomicU64,
    pub(crate) standby_frames_returned: AtomicU64,
    pub(crate) standby_promotions: AtomicU64,
    pub(crate) frames_released_by_promotion: AtomicU64,
    pub(crate) standby_results_evicted: AtomicU64,
    pub(crate) standby_pool_exhausted: AtomicU64,
}

/// Счётчики — монотонная статистика, порядок между ними не важен: `Relaxed` достаточно.
const ORDER: Ordering = Ordering::Relaxed;

impl StreamAccounting {
    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, ORDER);
    }

    /// Прибавляет `amount` к счётчику (для событий, считающих сразу пачку кадров).
    pub(crate) fn add(&self, counter: &AtomicU64, amount: u64) {
        counter.fetch_add(amount, ORDER);
    }

    pub(crate) fn note_pairing_wait_micros(&self, micros: u64) {
        self.max_pairing_wait_micros.fetch_max(micros, ORDER);
        self.total_pairing_wait_micros.fetch_add(micros, ORDER);
    }

    /// Уровень ячейки видоискателя (пишется под замком хаба после каждого её изменения).
    pub(crate) fn set_viewfinder_queued(&self, queued: usize) {
        self.viewfinder_queued
            .store(u64::try_from(queued).unwrap_or(u64::MAX), ORDER);
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
            dropped_jitter_overflow: self.dropped_jitter_overflow.load(ORDER),
            viewfinder_polls_while_priming: self.viewfinder_polls_while_priming.load(ORDER),
            viewfinder_underruns: self.viewfinder_underruns.load(ORDER),
            viewfinder_flushed: self.viewfinder_flushed.load(ORDER),
            viewfinder_queued: self.viewfinder_queued.load(ORDER),
            viewfinder_pacing_switches: self.viewfinder_pacing_switches.load(ORDER),
            ring_evicted: self.ring_evicted.load(ORDER),
            ring_picks_exact: self.ring_picks_exact.load(ORDER),
            ring_picks_substituted: self.ring_picks_substituted.load(ORDER),
            ring_picks_unavailable: self.ring_picks_unavailable.load(ORDER),
            ring_flash_picks_found: self.ring_flash_picks_found.load(ORDER),
            lossless_defects: self.lossless_defects.load(ORDER),
            released_after_fence: self.released_after_fence.load(ORDER),
            released_unused: self.released_unused.load(ORDER),
            leaked_leases: self.leaked_leases.load(ORDER),
            frames_returned: self.frames_returned.load(ORDER),
            frames_leaked: self.frames_leaked.load(ORDER),
            frames_outstanding: self.frames_outstanding.load(ORDER),
            max_frames_outstanding: self.max_frames_outstanding.load(ORDER),
            cpu_copies: self.cpu_copies.load(ORDER),
            standby_streams_begun: self.standby_streams_begun.load(ORDER),
            standby_begins_refused: self.standby_begins_refused.load(ORDER),
            standby_frames_returned: self.standby_frames_returned.load(ORDER),
            standby_promotions: self.standby_promotions.load(ORDER),
            frames_released_by_promotion: self.frames_released_by_promotion.load(ORDER),
            standby_results_evicted: self.standby_results_evicted.load(ORDER),
            standby_pool_exhausted: self.standby_pool_exhausted.load(ORDER),
        }
    }
}
