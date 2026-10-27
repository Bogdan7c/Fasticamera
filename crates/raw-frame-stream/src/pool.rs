//! План пула буферов: сколько `maxImages` нужно `AImageReader`, чтобы ни один потребитель
//! не остался без буфера, а HAL — без свободного места (инвариант 3 README).

use serde::Serialize;

/// Из чего складывается пул. Каждое слагаемое — буферы, которые одновременно могут быть
/// «на руках» у этой части системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PoolPlan {
    /// Ячейка видоискателя (1) + кадры у потока рендера (`viewfinder_render_holds`).
    pub viewfinder: u32,
    /// Кадры, ждущие пару (`max_pending_frames`).
    pub pending_pairs: u32,
    /// Кольцо снимков: `min(бюджет / размер кадра, потолок)`; 0 — выключено.
    pub snapshot_ring: u32,
    /// Очереди потребителей «ни одного молча».
    pub lossless: u32,
    /// Запас (`pool_slack`).
    pub slack: u32,
}

impl PoolPlan {
    /// `maxImages` для `AImageReader`: сумма всех слагаемых.
    pub fn reader_max_images(&self) -> u32 {
        self.viewfinder + self.pending_pairs + self.snapshot_ring + self.lossless + self.slack
    }
}
