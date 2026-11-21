//! Спаривание кадра и результата строго по равенству `SENSOR_TIMESTAMP` (решение 42).
//!
//! Чужие метаданные не подставляются никогда: нет пары — честное `Missing`.

use std::time::{Duration, Instant};

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::{FrameCaptureDetails, FrameCaptureMeta, FrameSequence, MissingCaptureMeta};

use super::deferred::Deferred;
use super::standby::StandbyResultStored;
use super::state::{BufferedResult, HubState, PendingFrame};
use super::{OfferOutcome, OfferedFrame, ResultOutcome, StreamEpoch};
use crate::counters::StreamAccounting;

/// Порядок действий в вызовах (важно для тестов и ожиданий):
/// * кадр: сперва истекают старые ожидания, потом ищется пара — так публикации идут от старых к новым;
/// * результат: сперва ищется пара (опоздавший на миллисекунду результат всё же спасает кадр),
///   потом истекают ожидания.
impl HubState {
    pub(super) fn offer_frame(
        &mut self,
        epoch: StreamEpoch,
        offered: OfferedFrame,
        now: Instant,
        deferred: &mut Deferred,
    ) -> OfferOutcome {
        StreamAccounting::bump(&self.accounting.frames_offered);
        if !self.accepts(epoch) {
            // Тёплая эпоха: кадр сразу камере; чужая/закрытая — устаревшая.
            let Err(frame) = self.return_standby_frame(epoch, offered.frame, deferred) else {
                return OfferOutcome::ReturnedAsStandby;
            };
            StreamAccounting::bump(&self.accounting.stale_epoch_frames);
            deferred.return_untouched_later(frame);
            return OfferOutcome::StaleEpoch;
        }
        self.expire_pending(now, deferred);
        let pending = PendingFrame {
            sequence: self.allocate_sequence(),
            sensor_timestamp: offered.sensor_timestamp,
            acquire_fence: offered.acquire_fence,
            frame: offered.frame,
            arrived_at: now,
        };
        let paired_result = self
            .buffered_results
            .iter()
            .position(|buffered| buffered.sensor_timestamp == pending.sensor_timestamp)
            .and_then(|position| self.buffered_results.remove(position));
        if let Some(buffered) = paired_result {
            // Результат пришёл раньше кадра: кадр не ждал ни мгновения.
            self.accounting.note_pairing_wait_micros(0);
            self.publish_paired(pending, buffered.details, deferred);
            return OfferOutcome::PublishedPaired;
        }
        self.pending.push_back(pending);
        self.publish_overflowed_pending(deferred);
        OfferOutcome::AwaitingResult
    }

    pub(super) fn offer_capture_result(
        &mut self,
        epoch: StreamEpoch,
        details: FrameCaptureDetails,
        now: Instant,
        deferred: &mut Deferred,
    ) -> ResultOutcome {
        StreamAccounting::bump(&self.accounting.results_offered);
        if !self.accepts(epoch) {
            if !self.standby.contains(epoch) {
                StreamAccounting::bump(&self.accounting.stale_epoch_results);
                return ResultOutcome::StaleEpoch;
            }
            return match self.buffer_standby_result(epoch, details) {
                StandbyResultStored::Buffered => ResultOutcome::BufferedForStandby,
                StandbyResultStored::WithoutTimestamp => ResultOutcome::WithoutTimestamp,
                StandbyResultStored::NotStandby => ResultOutcome::StaleEpoch,
            };
        }
        let Some(sensor_timestamp) = details.result.sensor_timestamp else {
            StreamAccounting::bump(&self.accounting.results_unpaired);
            return ResultOutcome::WithoutTimestamp;
        };
        let outcome = self.pair_or_buffer_result(sensor_timestamp, details, now, deferred);
        self.expire_pending(now, deferred);
        outcome
    }

    fn pair_or_buffer_result(
        &mut self,
        sensor_timestamp: SensorTimestamp,
        details: FrameCaptureDetails,
        now: Instant,
        deferred: &mut Deferred,
    ) -> ResultOutcome {
        let waiting = self
            .pending
            .iter()
            .position(|pending| pending.sensor_timestamp == sensor_timestamp);
        if let Some(pending) = waiting.and_then(|position| self.pending.remove(position)) {
            let waited = now.saturating_duration_since(pending.arrived_at);
            self.accounting.note_pairing_wait_micros(duration_micros(waited));
            self.publish_paired(pending, details, deferred);
            return ResultOutcome::PairedWithFrame;
        }
        // Кадра ещё нет. Буфер ограничен: вытесняем старейший результат — его кадр так
        // и не пришёл (или придёт слишком поздно).
        let capacity = usize::try_from(self.config.max_buffered_results).unwrap_or(usize::MAX);
        while self.buffered_results.len() >= capacity.max(1) {
            self.buffered_results.pop_front();
            StreamAccounting::bump(&self.accounting.results_unpaired);
        }
        self.buffered_results.push_back(BufferedResult {
            sensor_timestamp,
            details,
        });
        ResultOutcome::Buffered
    }

    /// Публикует без метаданных кадры, прождавшие пару не меньше `pairing_timeout_ms`.
    pub(super) fn expire_pending(&mut self, now: Instant, deferred: &mut Deferred) {
        let timeout = Duration::from_millis(u64::from(self.config.pairing_timeout_ms));
        while let Some(oldest) = self.pending.front() {
            if now.saturating_duration_since(oldest.arrived_at) < timeout {
                break;
            }
            let Some(expired) = self.pending.pop_front() else { break };
            StreamAccounting::bump(&self.accounting.frames_missing_meta_timeout);
            self.publish_missing(expired, MissingCaptureMeta::PairingTimedOut, deferred);
        }
    }

    /// Больше `max_pending_frames` ждать нельзя: каждый ждущий держит буфер из пула.
    fn publish_overflowed_pending(&mut self, deferred: &mut Deferred) {
        let limit = usize::try_from(self.config.max_pending_frames).unwrap_or(usize::MAX);
        while self.pending.len() > limit {
            let Some(oldest) = self.pending.pop_front() else { break };
            StreamAccounting::bump(&self.accounting.frames_missing_meta_timeout);
            self.publish_missing(oldest, MissingCaptureMeta::PairingTimedOut, deferred);
        }
    }

    fn publish_paired(&mut self, pending: PendingFrame, details: FrameCaptureDetails, deferred: &mut Deferred) {
        StreamAccounting::bump(&self.accounting.frames_paired);
        self.publish(pending, FrameCaptureMeta::Paired(details), deferred);
    }

    fn publish_missing(&mut self, pending: PendingFrame, reason: MissingCaptureMeta, deferred: &mut Deferred) {
        self.publish(pending, FrameCaptureMeta::Missing { reason }, deferred);
    }

    fn allocate_sequence(&mut self) -> FrameSequence {
        let sequence = FrameSequence(self.next_sequence);
        self.next_sequence += 1;
        sequence
    }
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
