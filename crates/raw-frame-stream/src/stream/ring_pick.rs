//! Неразрушающий выбор кадра из кольца снимков (решение 139). Решение принимается под замком
//! хаба; кольцо при этом сохраняет свою долю кадра, наружу уходит только `share()`.
//!
//! # Правило выбора
//! * Годный кадр: метаданные `Paired` И происхождение результата `RequestOrigin::Repeating`
//!   (кадры серии, фокус-триггера и неопознанные пропускаются: их экспозиция не «то, что видел
//!   пользователь»).
//! * Точное попадание: метка совпала и кадр годен.
//! * Кадр со вспышкой (14d) ищется отдельно ([`HubState::pick_flash_frame_from_ring`]): строго по
//!   `RequestOrigin::FlashStill { shot }`, без замены; обычные выборы его не видят.
//! * Замена: ближайший годный кадр по |Δt| к цели; при равенстве расстояний — более ранний
//!   (раньше значит «не позже, чем увидел пользователь»: задержка нажатия на экране делает
//!   более поздний кадр заведомо менее честным).

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::FlashShotId;
use camera_contract::frame_passport::FrameCaptureMeta;
use camera_contract::three_a::RequestOrigin;

use super::state::HubState;
use crate::counters::StreamAccounting;
use crate::lease::RawFrameLease;
use crate::ring_pick::{FlashRingPick, RingPick, RingPickRequest, RingSubstitutionReason, RingUnavailableReason};

/// Чем кадр кольца для снимка: годен или почему нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eligibility {
    Eligible,
    WithoutMeta,
    NotRepeating,
}

fn eligibility(lease: &RawFrameLease) -> Eligibility {
    match &lease.passport().capture {
        FrameCaptureMeta::Missing { .. } => Eligibility::WithoutMeta,
        FrameCaptureMeta::Paired(details) => match details.result.three_a.origin {
            RequestOrigin::Repeating => Eligibility::Eligible,
            RequestOrigin::FocusTrigger { .. }
            | RequestOrigin::Burst { .. }
            | RequestOrigin::FlashStill { .. }
            | RequestOrigin::Unknown => Eligibility::NotRepeating,
        },
    }
}

impl HubState {
    pub(super) fn pick_from_ring(&self, request: RingPickRequest) -> RingPick {
        let outcome = self.choose_from_ring(request);
        match &outcome {
            RingPick::Exact(_) => StreamAccounting::bump(&self.accounting.ring_picks_exact),
            RingPick::Substituted { .. } => StreamAccounting::bump(&self.accounting.ring_picks_substituted),
            RingPick::Unavailable(_) => StreamAccounting::bump(&self.accounting.ring_picks_unavailable),
        }
        outcome
    }

    /// Найти кадр разового запроса со вспышкой `shot`: точное совпадение метки запроса и
    /// метаданные `Paired` (метка запроса живёт в результате). Ближайшего/другого кадра НЕ бывает.
    pub(super) fn pick_flash_frame_from_ring(&self, shot: FlashShotId) -> FlashRingPick {
        if self.ring_capacity == 0 {
            return FlashRingPick::RingDisabled;
        }
        let found = self.ring.iter().find(|held| match &held.passport().capture {
            FrameCaptureMeta::Paired(details) => details.result.three_a.origin == (RequestOrigin::FlashStill { shot }),
            FrameCaptureMeta::Missing { .. } => false,
        });
        match found {
            Some(held) => {
                StreamAccounting::bump(&self.accounting.ring_flash_picks_found);
                FlashRingPick::Found(held.share())
            }
            None => FlashRingPick::NotInRingYet,
        }
    }

    fn choose_from_ring(&self, request: RingPickRequest) -> RingPick {
        if self.ring_capacity == 0 {
            return RingPick::Unavailable(RingUnavailableReason::RingDisabled);
        }
        if self.ring.is_empty() {
            return RingPick::Unavailable(RingUnavailableReason::RingEmpty);
        }
        let target = request.target();
        let target_frame = self.ring.iter().find(|held| held.passport().sensor_timestamp == target);

        let reason = match (request, target_frame) {
            (RingPickRequest::ShownFrame(_), Some(held)) => match eligibility(held) {
                Eligibility::Eligible => return RingPick::Exact(held.share()),
                Eligibility::WithoutMeta => RingSubstitutionReason::TargetWithoutMeta,
                Eligibility::NotRepeating => RingSubstitutionReason::TargetNotRepeating,
            },
            (RingPickRequest::ShownFrame(_), None) if self.is_older_than_ring(target) => {
                RingSubstitutionReason::TargetEvicted
            }
            (RingPickRequest::ShownFrame(_), None) => RingSubstitutionReason::TargetNotInRing,
            (RingPickRequest::NearestToEstimate(_), _) => RingSubstitutionReason::EstimateRequested,
        };

        match self.nearest_eligible(target) {
            Some(lease) => RingPick::Substituted {
                lease: lease.share(),
                reason,
                requested: target,
            },
            None => RingPick::Unavailable(RingUnavailableReason::NoEligibleFrame),
        }
    }

    /// Цель старше всех кадров кольца: её вытеснило (кольцо хранит последние N).
    fn is_older_than_ring(&self, target: SensorTimestamp) -> bool {
        self.ring
            .iter()
            .map(|held| held.passport().sensor_timestamp)
            .min()
            .is_some_and(|oldest| target < oldest)
    }

    /// Ближайший годный кадр по |Δt|; при равенстве — с меньшей меткой (более ранний).
    fn nearest_eligible(&self, target: SensorTimestamp) -> Option<&RawFrameLease> {
        self.ring
            .iter()
            .filter(|held| eligibility(held) == Eligibility::Eligible)
            .min_by_key(|held| {
                let stamp = held.passport().sensor_timestamp;
                (stamp.as_nanos().abs_diff(target.as_nanos()), stamp)
            })
    }
}
