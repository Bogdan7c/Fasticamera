//! План пула буферов: сколько `maxImages` нужно `AImageReader`, чтобы ни один потребитель
//! не остался без буфера, а HAL — без свободного места (инвариант 3 README).

use serde::Serialize;

/// Из чего складывается пул. Каждое слагаемое — буферы, которые одновременно могут быть
/// «на руках» у этой части системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PoolPlan {
    /// Ячейки видоискателя (`max(1, viewfinder_jitter_buffer.max_queued)` — верхняя граница обеих
    /// политик, переключаемых на ходу) + кадры у потока рендера (`viewfinder_render_holds`).
    /// См. `RawFrameStreamConfig::viewfinder_pool_cells`.
    pub viewfinder: u32,
    /// Кадры, ждущие пару (`max_pending_frames`).
    pub pending_pairs: u32,
    /// Кольцо снимков: `min(бюджет / размер кадра, потолок)`; 0 — выключено.
    pub snapshot_ring: u32,
    /// Доли кадров, выданные снимкам (`snapshot_capture_holds`): снимок держит буфер камеры,
    /// пока GPU не скопирует кадр (≈ 1–2 кадра), и может пережить вытеснение кадра из кольца.
    /// Считается только при включённом кольце (`snapshot_ring > 0`).
    pub snapshot_capture_holds: u32,
    /// Очереди потребителей «ни одного молча».
    pub lossless: u32,
    /// Запас (`pool_slack`).
    pub slack: u32,
}

impl PoolPlan {
    /// Сколько буферов снимки держат сверх самого кольца: без кольца снимков нет — 0.
    pub fn effective_snapshot_capture_holds(&self) -> u32 {
        if self.snapshot_ring == 0 {
            0
        } else {
            self.snapshot_capture_holds
        }
    }

    /// `maxImages` для `AImageReader`: сумма всех слагаемых (удержания снимков — только при кольце).
    pub fn reader_max_images(&self) -> u32 {
        self.viewfinder
            + self.pending_pairs
            + self.snapshot_ring
            + self.effective_snapshot_capture_holds()
            + self.lossless
            + self.slack
    }
}
