//! Поток кадров: вход от производителя, спаривание, раздача потребителям.
//!
//! Публичные типы и ручки; логика — в подмодулях:
//! `hub` (замок и порядок замков), `state` (эпохи, пул), `pairing` (спаривание),
//! `publish` (раздача), `viewfinder_cell` (политика видоискателя), `deferred` (возвраты вне замка).

mod deferred;
mod hub;
mod pairing;
mod publish;
mod ring_pick;
mod standby;
mod state;
#[cfg(test)]
mod tests;
mod viewfinder_cell;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::FlashShotId;
use camera_contract::frame_passport::{FrameCaptureDetails, FrameSequence, StreamPassport};
use serde::Serialize;
use shell_contract::sync_fence::GpuWaitFence;

use crate::config::RawFrameStreamConfig;
use crate::counters::RawStreamCounters;
use crate::counters::StreamAccounting;
use crate::lease::RawFrameLease;
use crate::pool::PoolPlan;
use crate::producer_frame::ProducerFrame;
use crate::ring_pick::{FlashRingPick, RingPick, RingPickRequest};
use crate::viewfinder_pacing::{ViewfinderPacing, ViewfinderPacingSwitch};
use viewfinder_cell::CellTake;

pub use standby::{StandbyBegin, StandbyPromotion};

/// Номер сессии потока. Кадры и результаты прошлой эпохи не смешиваются с новой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct StreamEpoch(pub u64);

/// Итог [`RawFrameProducer::begin_stream`]: эпоха и сколько буферов нужно reader-у.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamStart {
    pub epoch: StreamEpoch,
    pub pool: PoolPlan,
}

/// Буфер, который производитель отдаёт в лизинг.
pub struct OfferedFrame {
    /// `AImage_getTimestamp` — та же метка, что `SENSOR_TIMESTAMP` результата.
    pub sensor_timestamp: SensorTimestamp,
    pub acquire_fence: GpuWaitFence,
    pub frame: Box<dyn ProducerFrame>,
}

/// Что стало с предложенным кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferOutcome {
    /// Результат уже был — кадр сразу роздан с метаданными.
    PublishedPaired,
    /// Ждёт свой результат (не дольше `pairing_timeout_ms`).
    AwaitingResult,
    /// Кадр прошлой эпохи или потока нет — отпущен производителю без раздачи.
    StaleEpoch,
    /// Эпоха тёплая (standby): кадр сразу возвращён камере, потребителям не достался.
    ReturnedAsStandby,
}

/// Что стало с предложенным результатом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultOutcome {
    /// Нашёлся кадр с той же меткой — роздан с метаданными.
    PairedWithFrame,
    /// Кадра ещё нет — результат ждёт его.
    Buffered,
    /// Результат без метки сенсора: спарить нельзя, посчитан как неспаренный.
    WithoutTimestamp,
    /// Результат прошлой эпохи — отброшен.
    StaleEpoch,
    /// Эпоха тёплая: результат лежит в её буфере до обмена (старые вытесняются).
    BufferedForStandby,
}

/// Ответ видоискателю.
#[derive(Debug)]
pub enum ViewfinderPoll {
    /// Кадр для этого vsync: самый свежий (`LatestWins`) или старейший непоказанный (буфер с
    /// запасом). Один и тот же кадр не выдаётся дважды.
    NewFrame(RawFrameLease),
    /// Поток идёт, кадра для этого vsync нет (пусто, буфер заполняется или запас исчерпан) —
    /// показывать прежний.
    NoNewFrame,
    /// Потока нет (не начат или остановлен): держать прошлый кадр нельзя — reader ждёт
    /// возврата всех буферов, чтобы удалиться.
    StreamInactive,
}

/// Почему потребитель «ни одного молча» не получил кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefectReason {
    /// Очередь потребителя полна — кадр отпущен, это брак записи.
    QueueFull,
}

/// Событие брака: кадр, который не дошёл до потребителя «ни одного молча».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StreamDefect {
    pub sequence: FrameSequence,
    pub sensor_timestamp: SensorTimestamp,
    pub reason: DefectReason,
}

use hub::StreamHub;

/// Поток кадров. Создаётся в composition root; производитель и потребители получают свои ручки.
#[derive(Clone)]
pub struct RawFrameStream {
    hub: Arc<StreamHub>,
}

impl RawFrameStream {
    pub fn new(config: RawFrameStreamConfig) -> Self {
        Self {
            hub: Arc::new(StreamHub::new(config)),
        }
    }

    /// Ручка производителя (камера). Можно клонировать для колбэков кадров и результатов.
    pub fn producer(&self) -> RawFrameProducer {
        RawFrameProducer {
            hub: Arc::clone(&self.hub),
        }
    }

    /// Ручка видоискателя (политика — [`ViewfinderPacing`]). Потребитель один; повторный вызов
    /// отдаёт ту же ячейку.
    pub fn viewfinder(&self) -> ViewfinderFrames {
        ViewfinderFrames {
            hub: Arc::clone(&self.hub),
        }
    }

    /// Ручка кольца снимков (`KeepLast(N)`); N считается при каждом `begin_stream` из бюджета.
    pub fn snapshot_ring(&self) -> SnapshotRingFrames {
        SnapshotRingFrames {
            hub: Arc::clone(&self.hub),
        }
    }

    /// Подключает потребителя «ни одного молча» с очередью `capacity`. Его очередь входит
    /// в план пула со следующего `begin_stream`.
    pub fn attach_lossless_consumer(&self, capacity: NonZeroUsize) -> LosslessFrames {
        let index = self.hub.with_state(|state, _| state.attach_lossless(capacity.get()));
        LosslessFrames {
            hub: Arc::clone(&self.hub),
            index,
        }
    }

    pub fn counters(&self) -> RawStreamCounters {
        self.hub.accounting.snapshot()
    }

    /// Переключает политику видоискателя на ходу (A/B на телефоне, решение 53). Зовёт только
    /// владелец потока (composition root). Лизинги не теряются и не выдаются дважды: при переходе
    /// к `LatestWins` в ячейке остаётся самый новый кадр (прочие — `dropped_superseded`), при
    /// переходе к буферу он заново заполняется. План пула не меняется: он уже рассчитан на обе.
    pub fn set_viewfinder_pacing(&self, pacing: ViewfinderPacing) -> ViewfinderPacingSwitch {
        self.hub
            .with_state(|state, deferred| state.viewfinder.switch_to(pacing, &state.accounting, deferred))
    }

    /// Действующая политика видоискателя.
    pub fn viewfinder_pacing(&self) -> ViewfinderPacing {
        self.hub.with_state(|state, _| state.viewfinder.pacing())
    }
}

/// Вход потока для камеры. Все методы короткие и не ждут других потоков (зовутся из колбэков NDK).
#[derive(Clone)]
pub struct RawFrameProducer {
    hub: Arc<StreamHub>,
}

impl RawFrameProducer {
    /// Новая сессия камеры: всё, что держала прошлая эпоха в ожидании/ячейке/кольце, отпускается;
    /// возвращает эпоху и план пула для `maxImages`.
    pub fn begin_stream(&self, passport: Arc<StreamPassport>) -> StreamStart {
        self.hub
            .with_state(|state, deferred| state.begin_stream(passport, deferred))
    }

    /// Тёплая эпоха для второй камеры: номер из общего счётчика и ПОЛНЫЙ план пула (после обмена
    /// её reader станет показываемым). Показываемую эпоху не трогает. Сверх `max_standby_streams`
    /// — [`StandbyBegin::LimitReached`], ничего не создаётся.
    pub fn begin_standby_stream(&self, passport: Arc<StreamPassport>) -> StandbyBegin {
        self.hub.with_state(|state, _| state.begin_standby_stream(passport))
    }

    /// Конец тёплой эпохи: накопленные результаты отпускаются. Чужая или закрытая — no-op.
    pub fn end_standby_stream(&self, epoch: StreamEpoch) {
        self.hub.with_state(|state, _| state.end_standby_stream(epoch))
    }

    /// Атомарный обмен: тёплая становится показываемой, прежняя показываемая — тёплой. Всё, что
    /// держала прежняя показываемая, отпускается без показа; `StreamInactive` из-за обмена не бывает.
    pub fn promote_standby(&self, standby_epoch: StreamEpoch) -> StandbyPromotion {
        self.hub
            .with_state(|state, deferred| state.promote_standby(standby_epoch, deferred))
    }

    pub fn offer_frame(&self, epoch: StreamEpoch, frame: OfferedFrame, now: Instant) -> OfferOutcome {
        self.hub
            .with_state(|state, deferred| state.offer_frame(epoch, frame, now, deferred))
    }

    pub fn offer_capture_result(
        &self,
        epoch: StreamEpoch,
        details: FrameCaptureDetails,
        now: Instant,
    ) -> ResultOutcome {
        self.hub
            .with_state(|state, deferred| state.offer_capture_result(epoch, details, now, deferred))
    }

    /// Reader ответил «все буферы на руках» — посчитать (`dropped_pool_exhausted`).
    pub fn note_pool_exhausted(&self, epoch: StreamEpoch) {
        self.hub.with_state(|state, _| {
            // Считаем только текущую эпоху: отказ reader-а прошлой сессии к новой не относится.
            if state.accepts(epoch) {
                StreamAccounting::bump(&state.accounting.dropped_pool_exhausted);
            } else {
                // Тёплая эпоха считается отдельно; чужая — не считается нигде.
                state.note_standby_pool_exhausted(epoch);
            }
        })
    }

    /// Пиксели кадра скопированы через CPU (отладочная проба инварианта 1).
    pub fn note_cpu_copy(&self) {
        StreamAccounting::bump(&self.hub.accounting.cpu_copies)
    }

    /// Отпустить кадры, чьё ожидание пары истекло к `now` (зовётся и из опроса видоискателя).
    pub fn expire_pending(&self, now: Instant) {
        self.hub
            .with_state(|state, deferred| state.expire_pending(now, deferred))
    }

    /// Сессия закончилась: ожидающие пары кадры отпускаются (`frames_missing_meta_stopped`),
    /// ячейка (`viewfinder_flushed`) и кольцо пустеют, видоискатель получает `StreamInactive`.
    pub fn end_stream(&self, epoch: StreamEpoch) {
        self.hub.with_state(|state, deferred| state.end_stream(epoch, deferred))
    }
}

/// Потребитель видоискателя (политика — [`ViewfinderPacing`]).
pub struct ViewfinderFrames {
    hub: Arc<StreamHub>,
}

impl ViewfinderFrames {
    /// Кадр для ОДНОГО показа на vsync. Звать ровно один раз на кадр, который будет показан:
    /// при буфере с запасом каждый вызов расходует кадр очереди, лишний опрос «съел» бы кадр
    /// следующего vsync. Попутно истекают ожидания пары к `now`.
    pub fn poll_for_vsync(&self, now: Instant) -> ViewfinderPoll {
        self.hub.with_state(|state, deferred| {
            state.expire_pending(now, deferred);
            match state.viewfinder.take(state.active, &state.accounting) {
                CellTake::Frame(lease) => ViewfinderPoll::NewFrame(lease),
                CellTake::Nothing if state.active => ViewfinderPoll::NoNewFrame,
                CellTake::Nothing => ViewfinderPoll::StreamInactive,
            }
        })
    }
}

/// Потребитель кольца снимков (`KeepLast(N)`).
pub struct SnapshotRingFrames {
    hub: Arc<StreamHub>,
}

impl SnapshotRingFrames {
    /// Текущая ёмкость кольца (0 — выключено).
    pub fn capacity(&self) -> usize {
        self.hub.with_state(|state, _| state.ring_capacity)
    }

    /// Неразрушающий выбор одного кадра для снимка (решение 139): кольцо сохраняет свою долю
    /// и вытесняет кадр как обычно, наружу уходит отдельная доля — её надо вернуть явно.
    /// Правило выбора и замены — в [`RingPickRequest`]/[`RingPick`] и `stream/ring_pick.rs`.
    pub fn pick(&self, request: RingPickRequest) -> RingPick {
        self.hub.with_state(|state, _| state.pick_from_ring(request))
    }

    /// Найти в кольце кадр разового запроса со вспышкой снимка `shot` (14d): строго по метке
    /// запроса, без замены другим кадром. Доля отдельная, её надо вернуть явно.
    pub fn pick_flash_frame(&self, shot: FlashShotId) -> FlashRingPick {
        self.hub.with_state(|state, _| state.pick_flash_frame_from_ring(shot))
    }

    /// Забрать все кадры кольца, от старых к новым.
    pub fn take_all(&self) -> Vec<RawFrameLease> {
        self.hub.with_state(|state, _| state.ring.drain(..).collect())
    }
}

/// Потребитель «ни одного молча» (`NeverDropSilently`): каждый кадр либо в очереди, либо брак.
pub struct LosslessFrames {
    hub: Arc<StreamHub>,
    /// Номер очереди этого потребителя в хабе.
    index: usize,
}

impl LosslessFrames {
    /// Следующий кадр очереди (FIFO).
    pub fn take_next(&self) -> Option<RawFrameLease> {
        self.hub
            .with_state(|state, _| state.lossless.get_mut(self.index)?.frames.pop_front())
    }

    /// События брака с прошлого вызова.
    pub fn drain_defects(&self) -> Vec<StreamDefect> {
        self.hub.with_state(|state, _| {
            state
                .lossless
                .get_mut(self.index)
                .map(|queue| std::mem::take(&mut queue.defects))
                .unwrap_or_default()
        })
    }
}
