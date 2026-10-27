//! Раздача готового кадра потребителям: ячейка видоискателя, кольцо, очереди «ни одного молча».

use camera_contract::frame_passport::{FrameCaptureMeta, RawFramePassport};

use super::state::HubState;
use super::state::PendingFrame;
use super::{DefectReason, StreamDefect};
use crate::counters::StreamAccounting;
use crate::lease::{RawFrameLease, SharedFrame};
use crate::producer_frame::ProducerFrame;

use super::deferred::Deferred;

impl HubState {
    /// Превращает ожидающий кадр в лизинг с паспортом и раздаёт его доли потребителям.
    ///
    /// Исходный лизинг — временный держатель хаба: он отпускается последним, поэтому буфер
    /// не уйдёт камере, пока хаб раздаёт доли, и уйдёт сразу, если никто долю не взял.
    pub(super) fn publish(&mut self, pending: PendingFrame, capture: FrameCaptureMeta, deferred: &mut Deferred) {
        let Some(stream) = self.passport.clone() else {
            // Недостижимо (активный поток всегда с паспортом), но кадр не теряем.
            deferred.return_untouched_later(pending.frame);
            return;
        };
        let passport = RawFramePassport {
            stream,
            sequence: pending.sequence,
            sensor_timestamp: pending.sensor_timestamp,
            capture,
        };
        let frame: Box<dyn ProducerFrame> = pending.frame;
        let lease = SharedFrame::lease(passport, frame, pending.acquire_fence, self.accounting.clone());
        StreamAccounting::bump(&self.accounting.frames_published);

        self.give_to_viewfinder(&lease, deferred);
        self.give_to_ring(&lease, deferred);
        self.give_to_lossless_queues(&lease, deferred);
        deferred.release_unused_later(lease);
    }

    /// `LatestWins`: в ячейке остаётся кадр с бо́льшим `sequence` (при публикации не по порядку
    /// прибытия новый кадр может оказаться старше того, что уже лежит).
    fn give_to_viewfinder(&mut self, lease: &RawFrameLease, deferred: &mut Deferred) {
        let incoming = lease.share();
        let loser = match self.viewfinder_slot.take() {
            Some(current) if current.passport().sequence > incoming.passport().sequence => {
                self.viewfinder_slot = Some(current);
                incoming
            }
            Some(current) => {
                self.viewfinder_slot = Some(incoming);
                current
            }
            None => {
                self.viewfinder_slot = Some(incoming);
                return;
            }
        };
        StreamAccounting::bump(&self.accounting.dropped_superseded);
        deferred.release_unused_later(loser);
    }

    /// `KeepLast(N)`: кольцо хранится по возрастанию `sequence`; переполнение выбрасывает старейший.
    fn give_to_ring(&mut self, lease: &RawFrameLease, deferred: &mut Deferred) {
        if self.ring_capacity == 0 {
            return;
        }
        let incoming = lease.share();
        let incoming_sequence = incoming.passport().sequence;
        let position = self
            .ring
            .iter()
            .rposition(|held| held.passport().sequence < incoming_sequence)
            .map_or(0, |index| index + 1);
        self.ring.insert(position, incoming);
        while self.ring.len() > self.ring_capacity {
            let Some(oldest) = self.ring.pop_front() else { break };
            StreamAccounting::bump(&self.accounting.ring_evicted);
            deferred.release_unused_later(oldest);
        }
    }

    /// `NeverDropSilently`: кадр либо в очереди, либо брак с событием — тихо не теряется.
    fn give_to_lossless_queues(&mut self, lease: &RawFrameLease, deferred: &mut Deferred) {
        for queue in &mut self.lossless {
            let share = lease.share();
            if queue.frames.len() >= queue.capacity {
                StreamAccounting::bump(&self.accounting.lossless_defects);
                queue.defects.push(StreamDefect {
                    sequence: share.passport().sequence,
                    sensor_timestamp: share.passport().sensor_timestamp,
                    reason: DefectReason::QueueFull,
                });
                deferred.release_unused_later(share);
            } else {
                queue.frames.push_back(share);
            }
        }
    }
}
