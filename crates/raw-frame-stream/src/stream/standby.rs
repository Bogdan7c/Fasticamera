//! Тёплые («standby») эпохи: вторая камера запущена заранее, чтобы обмен с показываемой
//! прошёл без `StreamInactive` и без перезапуска потока (сессия 13b).
//!
//! # Владение
//! Хаб ничего не знает о камерах и ячейках бэкенда — только об эпохах. Показываемая эпоха живёт
//! в полях [`HubState`](super::state::HubState) как раньше; тёплые — в [`StandbyEpochs`].
//!
//! # Инварианты
//! * Кадр тёплой эпохи НИКОГДА не попадает в ячейку видоискателя, кольцо и очереди потребителей:
//!   он сразу возвращается камере (`standby_frames_returned`). Тёплая камера нужна, чтобы
//!   её сенсор и конвейер работали, а не чтобы показывать.
//! * Результаты тёплой эпохи копятся в ограниченном буфере (`max_pending_frames`): после обмена
//!   первые кадры новой показываемой спарятся с результатами, пришедшими до обмена.
//! * Обмен ([`HubState::promote_standby`]) атомарен под замком хаба: `active` остаётся `true`,
//!   поэтому видоискатель не видит `StreamInactive` из-за обмена.
//! * Счётчик эпох общий и монотонный (`last_issued_epoch`), даже если показываемая эпоха
//!   после обмена «старше» выданных: номера не повторяются.

use std::collections::VecDeque;
use std::sync::Arc;

use camera_contract::frame_passport::{FrameCaptureDetails, StreamPassport};

use super::deferred::Deferred;
use super::state::{BufferedResult, HubState};
use super::viewfinder_cell::PrimingAfterFlush;
use super::{StreamEpoch, StreamStart};
use crate::counters::StreamAccounting;
use crate::producer_frame::ProducerFrame;

/// Итог [`RawFrameProducer::begin_standby_stream`](super::RawFrameProducer::begin_standby_stream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandbyBegin {
    /// Тёплая эпоха начата: эпоха и ПОЛНЫЙ план пула (после обмена любой reader может стать показываемым).
    Started(StreamStart),
    /// Тёплых эпох уже столько, сколько разрешает конфиг (`max_standby_streams`); ничего не создано.
    LimitReached { limit: u32 },
}

/// Итог [`RawFrameProducer::promote_standby`](super::RawFrameProducer::promote_standby).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandbyPromotion {
    /// Обмен выполнен.
    Promoted {
        /// Новая показываемая эпоха (бывшая тёплая).
        shown: StreamEpoch,
        /// Бывшая показываемая, ставшая тёплой; `None`, если показываемой не было (поток не
        /// активен): тогда тёплая просто становится показываемой.
        demoted_to_standby: Option<StreamEpoch>,
    },
    /// Такой тёплой эпохи нет (чужая, закрытая или уже показываемая); состояние не менялось.
    NoSuchStandby,
}

/// Одна тёплая эпоха: всё, что хаб о ней помнит.
struct StandbyEpoch {
    epoch: StreamEpoch,
    /// Паспорт нужен при обмене: он становится паспортом показываемого потока.
    passport: Arc<StreamPassport>,
    /// Ёмкость кольца, посчитанная для этого паспорта при старте тёплой.
    ring_capacity: usize,
    /// Результаты, пришедшие раньше обмена (старейший спереди); метка сенсора есть всегда.
    recent_results: VecDeque<BufferedResult>,
}

/// Все тёплые эпохи хаба.
#[derive(Default)]
pub(super) struct StandbyEpochs {
    entries: Vec<StandbyEpoch>,
}

impl StandbyEpochs {
    pub(super) fn contains(&self, epoch: StreamEpoch) -> bool {
        self.entries.iter().any(|entry| entry.epoch == epoch)
    }

    fn take(&mut self, epoch: StreamEpoch) -> Option<StandbyEpoch> {
        let position = self.entries.iter().position(|entry| entry.epoch == epoch)?;
        Some(self.entries.remove(position))
    }
}

impl HubState {
    pub(super) fn begin_standby_stream(&mut self, passport: Arc<StreamPassport>) -> StandbyBegin {
        let limit = self.config.max_standby_streams;
        if self.standby.entries.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
            StreamAccounting::bump(&self.accounting.standby_begins_refused);
            return StandbyBegin::LimitReached { limit };
        }
        let epoch = self.allocate_epoch();
        let ring_capacity = self.ring_capacity_for(&passport);
        self.standby.entries.push(StandbyEpoch {
            epoch,
            passport,
            ring_capacity,
            recent_results: VecDeque::new(),
        });
        StreamAccounting::bump(&self.accounting.standby_streams_begun);
        StandbyBegin::Started(StreamStart {
            epoch,
            // План полный, как у показываемого потока: после обмена этот reader станет основным.
            pool: self.pool_plan_for(ring_capacity),
        })
    }

    /// Закрывает тёплую эпоху: её накопленные результаты так и не найдут кадров. Чужая — no-op.
    pub(super) fn end_standby_stream(&mut self, epoch: StreamEpoch) {
        let Some(closed) = self.standby.take(epoch) else { return };
        for _ in closed.recent_results {
            StreamAccounting::bump(&self.accounting.results_unpaired);
        }
    }

    /// Кадр тёплой эпохи: сразу камере, потребителям не достаётся. `false` — эпоха не тёплая.
    pub(super) fn return_standby_frame(
        &mut self,
        epoch: StreamEpoch,
        frame: Box<dyn ProducerFrame>,
        deferred: &mut Deferred,
    ) -> Result<(), Box<dyn ProducerFrame>> {
        if !self.standby.contains(epoch) {
            return Err(frame);
        }
        StreamAccounting::bump(&self.accounting.standby_frames_returned);
        deferred.return_untouched_later(frame);
        Ok(())
    }

    /// Результат тёплой эпохи: в её ограниченный буфер. Вызывающий сначала проверяет
    /// `standby.contains(epoch)`; `NotStandby` — защитный ответ, если эпоха всё же не тёплая.
    pub(super) fn buffer_standby_result(
        &mut self,
        epoch: StreamEpoch,
        details: FrameCaptureDetails,
    ) -> StandbyResultStored {
        let capacity = usize::try_from(self.config.max_pending_frames)
            .unwrap_or(usize::MAX)
            .max(1);
        let accounting = Arc::clone(&self.accounting);
        let Some(entry) = self.standby.entries.iter_mut().find(|entry| entry.epoch == epoch) else {
            return StandbyResultStored::NotStandby;
        };
        let Some(sensor_timestamp) = details.result.sensor_timestamp else {
            StreamAccounting::bump(&accounting.results_unpaired);
            return StandbyResultStored::WithoutTimestamp;
        };
        while entry.recent_results.len() >= capacity {
            entry.recent_results.pop_front();
            StreamAccounting::bump(&accounting.standby_results_evicted);
        }
        entry.recent_results.push_back(BufferedResult {
            sensor_timestamp,
            details,
        });
        StandbyResultStored::Buffered
    }

    /// Reader тёплой эпохи ответил «буферов нет»: считаем отдельно от показываемого.
    pub(super) fn note_standby_pool_exhausted(&self, epoch: StreamEpoch) {
        if self.standby.contains(epoch) {
            StreamAccounting::bump(&self.accounting.standby_pool_exhausted);
        }
    }

    /// Атомарный обмен: `standby_epoch` становится показываемой, прежняя показываемая — тёплой.
    ///
    /// Всё, что держала прежняя показываемая (ожидающие пары, ячейка, кольцо, очереди), отпускается
    /// без показа и считается в `frames_released_by_promotion` (кроме того — в обычных счётчиках
    /// этих мест, чтобы их балансы сходились). `active` остаётся `true`; если показываемой не
    /// было (`active == false`), тёплая просто становится показываемой.
    pub(super) fn promote_standby(&mut self, standby_epoch: StreamEpoch, deferred: &mut Deferred) -> StandbyPromotion {
        let Some(promoted) = self.standby.take(standby_epoch) else {
            return StandbyPromotion::NoSuchStandby;
        };
        let demoted = self.demote_shown_epoch(deferred);
        // Результаты тёплой переезжают в спаривание показываемой (хвост буфера ограничен её лимитом).
        for result in promoted.recent_results {
            self.store_promoted_result(result);
        }
        self.epoch = promoted.epoch;
        self.active = true;
        self.ring_capacity = promoted.ring_capacity;
        self.passport = Some(promoted.passport);
        StreamAccounting::bump(&self.accounting.standby_promotions);
        StandbyPromotion::Promoted {
            shown: promoted.epoch,
            demoted_to_standby: demoted,
        }
    }

    /// Отпускает всё у текущей показываемой эпохи и переводит её в тёплые. `None`, если её не было.
    fn demote_shown_epoch(&mut self, deferred: &mut Deferred) -> Option<StreamEpoch> {
        if !self.active {
            // Показываемой нет: держать нечего (после `end_stream` всё уже отпущено).
            return None;
        }
        let held_frames = self.pending.len()
            + self.viewfinder.queued()
            + self.ring.len()
            + self.lossless.iter().map(|queue| queue.frames.len()).sum::<usize>();
        // Запас буфера видоискателя сохраняется: у нового показываемого первый кадр без повторного набора.
        self.release_everything(PrimingAfterFlush::Keep, deferred);
        self.accounting.add(
            &self.accounting.frames_released_by_promotion,
            u64::try_from(held_frames).unwrap_or(u64::MAX),
        );
        let passport = self.passport.clone()?;
        self.standby.entries.push(StandbyEpoch {
            epoch: self.epoch,
            passport,
            ring_capacity: self.ring_capacity,
            recent_results: VecDeque::new(),
        });
        Some(self.epoch)
    }

    fn store_promoted_result(&mut self, result: BufferedResult) {
        let capacity = usize::try_from(self.config.max_buffered_results)
            .unwrap_or(usize::MAX)
            .max(1);
        while self.buffered_results.len() >= capacity {
            self.buffered_results.pop_front();
            StreamAccounting::bump(&self.accounting.results_unpaired);
        }
        self.buffered_results.push_back(result);
    }
}

/// Что стало с результатом тёплой эпохи (внутренний итог, наружу переводится в `ResultOutcome`).
pub(super) enum StandbyResultStored {
    Buffered,
    WithoutTimestamp,
    NotStandby,
}
