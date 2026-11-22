//! Перевод итога выбора кадра кольцом (`raw_frame_stream::RingPick`) в словарь снимка
//! (`still_contract::frame_choice`).
//!
//! crate кольца намеренно не знает `still-contract` (решение 139), поэтому перевод — здесь. Он исчерпывающий
//! (без `_`): новая причина кольца не проскочит молча. Причины, у которых нет точного двойника в словаре,
//! отображаются в ближайшую по смыслу, но считаются ОТДЕЛЬНО ([`TranslatedPick::detail`]), чтобы замену
//! «кадр не дошёл до кольца» не выдавали за вытеснение.

use camera_contract::capture_result::SensorTimestamp;
use raw_frame_stream::{RawFrameLease, RingPick, RingSubstitutionReason, RingUnavailableReason};
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::{FrameChoice, NoFrameReason, SubstitutionReason};

/// Подробность перевода: что на самом деле сказало кольцо (для отдельных счётчиков).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickDetail {
    /// Перевод один к одному.
    Direct,
    /// Цели нет в кольце, хотя она не старше старейшего кадра (потерян): в словаре — `ShownFrameEvicted`.
    TargetLostBeforeRing,
    /// Кадры в кольце есть, но годных нет: в словаре — `RingEmpty`.
    NoEligibleFrame,
}

/// Результат перевода: либо кадр с описанием выбора, либо причина «кадра нет».
#[derive(Debug)]
pub enum TranslatedPick {
    /// Кадр взят (лизинг — доля, её надо вернуть).
    Frame {
        lease: RawFrameLease,
        choice: FrameChoice,
        detail: PickDetail,
    },
    /// Кадра нет.
    NoFrame { reason: NoFrameReason, detail: PickDetail },
}

/// Когда кадр появился на экране: известно только для доказанного выбора (`ShownFrame` после доказательства).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShownAt {
    /// Показ доказан записями драйвера: кадр впервые был на экране в этот момент (`MONOTONIC`).
    Proven(MonotonicNanos),
    /// Показ не доказан: выбор по оценке.
    Unknown,
}

/// Переводит итог кольца. `shown_at` нужен только для точного попадания.
pub fn translate_pick(pick: RingPick, shown_at: ShownAt) -> TranslatedPick {
    match pick {
        RingPick::Exact(lease) => {
            let sensor_timestamp = lease.passport().sensor_timestamp;
            let choice = match shown_at {
                ShownAt::Proven(shown_at) => FrameChoice::ShownAtPress {
                    sensor_timestamp,
                    shown_at,
                },
                // Точное попадание без доказанного показа: запрос `ShownFrame` без доказательства не строится,
                // но если кольцо всё же вернуло `Exact`, честно называем это оценкой, а не «показанным кадром».
                ShownAt::Unknown => FrameChoice::Substituted {
                    sensor_timestamp,
                    wanted: Some(sensor_timestamp),
                    reason: SubstitutionReason::PresentationTimingUnknown,
                },
            };
            TranslatedPick::Frame {
                lease,
                choice,
                detail: PickDetail::Direct,
            }
        }
        RingPick::Substituted {
            lease,
            reason,
            requested,
        } => {
            let (reason, detail) = substitution_reason(reason);
            let choice = FrameChoice::Substituted {
                sensor_timestamp: lease.passport().sensor_timestamp,
                wanted: wanted_stamp(requested),
                reason,
            };
            TranslatedPick::Frame { lease, choice, detail }
        }
        RingPick::Unavailable(reason) => {
            let (reason, detail) = no_frame_reason(reason);
            TranslatedPick::NoFrame { reason, detail }
        }
    }
}

fn wanted_stamp(requested: SensorTimestamp) -> Option<SensorTimestamp> {
    Some(requested)
}

fn substitution_reason(reason: RingSubstitutionReason) -> (SubstitutionReason, PickDetail) {
    match reason {
        RingSubstitutionReason::TargetEvicted => (SubstitutionReason::ShownFrameEvicted, PickDetail::Direct),
        RingSubstitutionReason::TargetWithoutMeta => {
            (SubstitutionReason::ShownFrameWithoutMetadata, PickDetail::Direct)
        }
        RingSubstitutionReason::TargetNotRepeating => (SubstitutionReason::ShownFrameNotRegular, PickDetail::Direct),
        RingSubstitutionReason::TargetNotInRing => {
            (SubstitutionReason::ShownFrameEvicted, PickDetail::TargetLostBeforeRing)
        }
        RingSubstitutionReason::EstimateRequested => {
            (SubstitutionReason::PresentationTimingUnknown, PickDetail::Direct)
        }
    }
}

fn no_frame_reason(reason: RingUnavailableReason) -> (NoFrameReason, PickDetail) {
    match reason {
        RingUnavailableReason::RingDisabled => (NoFrameReason::RingDisabled, PickDetail::Direct),
        RingUnavailableReason::RingEmpty => (NoFrameReason::RingEmpty, PickDetail::Direct),
        RingUnavailableReason::NoEligibleFrame => (NoFrameReason::RingEmpty, PickDetail::NoEligibleFrame),
    }
}

#[cfg(test)]
#[path = "choice_tests.rs"]
mod tests;
