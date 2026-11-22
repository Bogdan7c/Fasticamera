//! Состояние хаба под замком: эпоха, ожидающие пары, ячейки потребителей.
//!
//! Все методы тут — «решения под замком»: они ничего не возвращают камере сами, а складывают
//! возвраты в [`Deferred`]. Спаривание — в `pairing.rs`, раздача — в `publish.rs`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::{FrameCaptureDetails, FrameSequence, StreamPassport};
use camera_contract::raw_format::RawFormat;
use shell_contract::sync_fence::GpuWaitFence;

use super::deferred::Deferred;
use super::standby::StandbyEpochs;
use super::viewfinder_cell::{PrimingAfterFlush, ViewfinderCell};
use super::{StreamDefect, StreamEpoch, StreamStart};
use crate::config::RawFrameStreamConfig;
use crate::counters::StreamAccounting;
use crate::lease::RawFrameLease;
use crate::pool::PoolPlan;
use crate::producer_frame::ProducerFrame;

/// Кадр, ждущий результат с той же меткой.
pub(super) struct PendingFrame {
    pub(super) sequence: FrameSequence,
    pub(super) sensor_timestamp: SensorTimestamp,
    pub(super) acquire_fence: GpuWaitFence,
    pub(super) frame: Box<dyn ProducerFrame>,
    pub(super) arrived_at: Instant,
}

/// Результат, пришедший раньше своего кадра. Метка всегда есть (без неё не буферизуем).
pub(super) struct BufferedResult {
    pub(super) sensor_timestamp: SensorTimestamp,
    pub(super) details: FrameCaptureDetails,
}

/// Очередь одного потребителя «ни одного молча».
pub(super) struct LosslessQueue {
    pub(super) capacity: usize,
    pub(super) frames: VecDeque<RawFrameLease>,
    pub(super) defects: Vec<StreamDefect>,
}

pub(super) struct HubState {
    pub(super) config: RawFrameStreamConfig,
    pub(super) accounting: Arc<StreamAccounting>,
    /// Показываемая эпоха (после обмена с тёплой может быть меньше `last_issued_epoch`).
    pub(super) epoch: StreamEpoch,
    /// Последний выданный номер эпохи (показываемой или тёплой): общий монотонный счётчик.
    pub(super) last_issued_epoch: u64,
    /// Тёплые эпохи (см. `standby.rs`).
    pub(super) standby: StandbyEpochs,
    /// Между `begin_stream` и `end_stream`. До первого начала и после конца — false.
    pub(super) active: bool,
    pub(super) passport: Option<Arc<StreamPassport>>,
    /// Счётчик последовательности на всю жизнь хаба (не обнуляется при смене эпохи):
    /// номера кадров разных эпох не пересекаются, по ним видно порядок и пропуски.
    pub(super) next_sequence: u64,
    /// В порядке прибытия (старейший спереди).
    pub(super) pending: VecDeque<PendingFrame>,
    /// В порядке прибытия; вытесняется старейший.
    pub(super) buffered_results: VecDeque<BufferedResult>,
    /// Ячейка видоискателя с действующей политикой (`LatestWins` или буфер с запасом).
    pub(super) viewfinder: ViewfinderCell,
    /// По возрастанию `sequence` (старейший спереди).
    pub(super) ring: VecDeque<RawFrameLease>,
    pub(super) ring_capacity: usize,
    pub(super) lossless: Vec<LosslessQueue>,
}

/// Размер одного RAW-кадра в байтах для расчёта бюджета кольца.
/// RAW10 упакован 4 пикселя в 5 байт (ширина*высота*5/4); RAW16 — 2 байта на пиксель.
fn frame_bytes(passport: &StreamPassport) -> u64 {
    let pixels = passport.buffer_size.pixel_count();
    match passport.raw_format {
        RawFormat::Raw10 => pixels.saturating_mul(5) / 4,
        RawFormat::Raw16 => pixels.saturating_mul(2),
    }
}

impl HubState {
    pub(super) fn new(config: RawFrameStreamConfig, accounting: Arc<StreamAccounting>) -> Self {
        Self {
            viewfinder: ViewfinderCell::new(config.viewfinder_pacing, config.viewfinder_jitter_buffer),
            config,
            accounting,
            epoch: StreamEpoch(0),
            last_issued_epoch: 0,
            standby: StandbyEpochs::default(),
            active: false,
            passport: None,
            next_sequence: 0,
            pending: VecDeque::new(),
            buffered_results: VecDeque::new(),
            ring: VecDeque::new(),
            ring_capacity: 0,
            lossless: Vec::new(),
        }
    }

    /// Принимаем ли мы события этой эпохи.
    pub(super) fn accepts(&self, epoch: StreamEpoch) -> bool {
        self.active && self.epoch == epoch
    }

    pub(super) fn attach_lossless(&mut self, capacity: usize) -> usize {
        self.lossless.push(LosslessQueue {
            capacity,
            frames: VecDeque::new(),
            defects: Vec::new(),
        });
        self.lossless.len() - 1
    }

    pub(super) fn begin_stream(&mut self, passport: Arc<StreamPassport>, deferred: &mut Deferred) -> StreamStart {
        self.release_everything(PrimingAfterFlush::Refill, deferred);
        self.epoch = self.allocate_epoch();
        self.active = true;
        self.ring_capacity = self.ring_capacity_for(&passport);
        self.passport = Some(passport);
        StreamStart {
            epoch: self.epoch,
            pool: self.pool_plan_for(self.ring_capacity),
        }
    }

    /// Выдаёт следующий номер эпохи из общего счётчика (показываемые и тёплые не пересекаются).
    pub(super) fn allocate_epoch(&mut self) -> StreamEpoch {
        self.last_issued_epoch += 1;
        StreamEpoch(self.last_issued_epoch)
    }

    pub(super) fn end_stream(&mut self, epoch: StreamEpoch, deferred: &mut Deferred) {
        if !self.accepts(epoch) {
            return;
        }
        self.release_everything(PrimingAfterFlush::Refill, deferred);
        self.active = false;
        self.ring_capacity = 0;
    }

    /// Сколько кадров вместит кольцо: `min(бюджет / размер кадра, потолок)`.
    pub(super) fn ring_capacity_for(&self, passport: &StreamPassport) -> usize {
        let bytes_per_frame = frame_bytes(passport);
        if bytes_per_frame == 0 {
            return 0;
        }
        let by_budget = self.config.snapshot_ring_budget_bytes / bytes_per_frame;
        let capped = by_budget.min(u64::from(self.config.snapshot_ring_max_frames));
        // Потолок — u32, так что значение всегда влезает; saturating на всякий случай.
        usize::try_from(capped).unwrap_or(usize::MAX)
    }

    /// План пула при данной ёмкости кольца (для показываемой и для тёплой эпохи одинаково).
    pub(super) fn pool_plan_for(&self, ring_capacity: usize) -> PoolPlan {
        let lossless_total: usize = self.lossless.iter().map(|queue| queue.capacity).sum();
        PoolPlan {
            viewfinder: self.config.viewfinder_pool_cells(),
            pending_pairs: self.config.max_pending_frames,
            snapshot_ring: u32::try_from(ring_capacity).unwrap_or(u32::MAX),
            // Удержания снимков нужны только при включённом кольце.
            snapshot_capture_holds: if ring_capacity == 0 {
                0
            } else {
                self.config.snapshot_capture_holds
            },
            lossless: u32::try_from(lossless_total).unwrap_or(u32::MAX),
            slack: self.config.pool_slack,
        }
    }

    /// Отпускает всё, что держит поток: ожидающие кадры, результаты, ячейку, кольцо, очереди.
    ///
    /// `priming` — набирает ли буфер видоискателя запас заново (конец потока) или сохраняет его (обмен ролей).
    pub(super) fn release_everything(&mut self, priming: PrimingAfterFlush, deferred: &mut Deferred) {
        for pending in self.pending.drain(..) {
            StreamAccounting::bump(&self.accounting.frames_missing_meta_stopped);
            deferred.return_untouched_later(pending.frame);
        }
        for _ in self.buffered_results.drain(..) {
            StreamAccounting::bump(&self.accounting.results_unpaired);
        }
        self.viewfinder.flush_with(priming, &self.accounting, deferred);
        for lease in self.ring.drain(..) {
            deferred.release_unused_later(lease);
        }
        for queue in &mut self.lossless {
            for lease in queue.frames.drain(..) {
                deferred.release_unused_later(lease);
            }
        }
    }
}
