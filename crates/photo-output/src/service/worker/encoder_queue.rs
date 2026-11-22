//! Ограничение числа открытых кодеров HEIF и очередь снимков, ждущих своего места.
//!
//! Рендер проявляет снимки строго по одному, а аппаратные кодеры дороги (память, лимит одновременных
//! экземпляров на телефоне). Поэтому служба открывает кодер не на каждый `Secured` сразу, а пока есть
//! место (`max_open_encoders`); остальные снимки ждут в очереди по порядку `Secured` (FIFO), и
//! рендер получает их цель позже. DNG и JPEG от очереди не зависят.
//!
//! Владение: место занимает [`EncoderSlot`] — он живёт вместе с открытым кодером и освобождает место
//! при уничтожении (в любом пути: `finish`, `abort`, паника), поэтому утечка места без утечки
//! самого кодера невозможна.

use std::collections::VecDeque;
use std::sync::Arc;

use shell_contract::still_media::{EncodeError, HeifSurfaceEncoder};
use std::time::Duration;
use still_contract::develop::PrimaryTargetOffer;
use still_contract::phase::StillFailure;
use still_contract::plan::StillId;

use super::Worker;
use super::job::{Developing, Job, Stage};
use crate::service::stats::StillCounters;

/// Занятое место под кодер. Освобождается при уничтожении.
pub(super) struct EncoderSlot {
    counters: Arc<StillCounters>,
}

impl EncoderSlot {
    fn occupy(counters: &Arc<StillCounters>) -> Self {
        counters.encoder_opened();
        Self {
            counters: Arc::clone(counters),
        }
    }
}

impl Drop for EncoderSlot {
    fn drop(&mut self) {
        self.counters.encoder_closed();
    }
}

/// Открытый кодер вместе с занятым им местом.
pub(super) struct OpenEncoder {
    encoder: Box<dyn HeifSurfaceEncoder>,
    /// Место освобождается после `finish`/`abort`: кодер до тех пор считается открытым.
    _slot: EncoderSlot,
}

impl OpenEncoder {
    pub(super) fn finish(self, timeout: Duration) -> Result<(), EncodeError> {
        self.encoder.finish(timeout)
    }

    pub(super) fn abort(self) {
        self.encoder.abort();
    }
}

/// Где у снимка кодер.
pub(super) enum EncoderSeat {
    /// Кодер не нужен (JPEG).
    NotNeeded,
    /// Нужен, места ещё нет: снимок стоит в очереди [`EncoderQueue`].
    Waiting,
    /// Открыт.
    Open(OpenEncoder),
}

impl EncoderSeat {
    pub(super) fn is_waiting(&self) -> bool {
        matches!(self, Self::Waiting)
    }
}

/// Очередь снимков, ждущих места под кодер, по порядку `Secured`.
///
/// Очередь ленивая: снимок, который закончился, пока ждал (срок, отказ рендера), из неё не удаляется
/// сразу — [`Worker::open_waiting_encoders`] пропускает такие номера при выборке.
#[derive(Default)]
pub(super) struct EncoderQueue {
    waiting: VecDeque<StillId>,
}

impl EncoderQueue {
    pub(super) fn enqueue(&mut self, id: StillId) {
        self.waiting.push_back(id);
    }

    fn pop_next(&mut self) -> Option<StillId> {
        self.waiting.pop_front()
    }
}

impl Worker {
    /// Есть ли свободное место под ещё один кодер.
    fn has_free_encoder_slot(&self) -> bool {
        let limit = u64::try_from(self.config.max_open_encoders).unwrap_or(u64::MAX);
        self.counters.open_encoders() < limit
    }

    /// Открывает кодеры ждущим снимкам, пока есть места, строго по очереди. Зовётся после каждой
    /// команды и после обработки `Secured`: место могло освободиться на любом пути завершения снимка.
    pub(super) fn open_waiting_encoders(&mut self) {
        while self.has_free_encoder_slot() {
            let Some(id) = self.encoder_queue.pop_next() else {
                return;
            };
            let Some(Job { ticket, stage }) = self.take_job(id) else {
                continue; // Снимок уже закончился сам, пока ждал.
            };
            let Stage::Developing(mut developing) = stage else {
                self.put_job(id, Job { ticket, stage });
                continue;
            };
            if !developing.encoder.is_waiting() {
                self.put_job(
                    id,
                    Job {
                        ticket,
                        stage: Stage::Developing(developing),
                    },
                );
                continue;
            }
            match self.open_encoder_and_offer(id, &mut developing) {
                Ok(()) => self.put_job(
                    id,
                    Job {
                        ticket,
                        stage: Stage::Developing(developing),
                    },
                ),
                Err(failure) => self.fail_parts(id, ticket, Stage::Developing(developing), failure),
            }
        }
    }

    /// Открывает кодер снимка и отдаёт рендеру его поверхность. Отказ платформы — `Encode` и ответ
    /// рендеру «цели нет» (он отпустит кадр); отказ самого рендера принять цель — `ServiceUnavailable`.
    pub(super) fn open_encoder_and_offer(&self, id: StillId, developing: &mut Developing) -> Result<(), StillFailure> {
        let target = match self.open_heif_encoder(developing) {
            Ok(target) => target,
            Err(failure) => {
                self.offer_unavailable(id);
                return Err(failure);
            }
        };
        match self
            .deps
            .targets
            .offer_primary_target(PrimaryTargetOffer::Ready { id, target })
        {
            Ok(()) => Ok(()),
            Err(unavailable) => {
                tracing::warn!(?id, %unavailable, "рендер не принял цель для снимка");
                Err(StillFailure::ServiceUnavailable)
            }
        }
    }

    /// Занимает место под открытый кодер.
    pub(super) fn seat_for_open_encoder(&self, encoder: Box<dyn HeifSurfaceEncoder>) -> EncoderSeat {
        EncoderSeat::Open(OpenEncoder {
            encoder,
            _slot: EncoderSlot::occupy(&self.counters),
        })
    }

    /// Если рендер ещё ждёт цель снимка, которого служба срывает, — отвечает ему «цели нет», чтобы он
    /// сразу отпустил кадр, а не ждал свой срок.
    pub(super) fn release_pending_target(&self, id: StillId, stage: &Stage) {
        if let Stage::Developing(developing) = stage
            && developing.encoder.is_waiting()
        {
            self.offer_unavailable(id);
        }
    }
}
