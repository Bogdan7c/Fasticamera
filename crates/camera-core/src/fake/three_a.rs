//! Fake для автоматики 3A: сценарии состояний AF, результаты триггера и серий, отказы отправки.

use std::time::Duration;

use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::three_a::{AfState, BurstId, PointRequestId, RequestOrigin, ThreeAResultState};

use super::{FakeBackendControl, FakeEventDelivery};
use crate::backend::{BackendEvent, RequestFailure};
use crate::request_controls::ExposurePlan;

/// Интервал между метками кадров в результатах, которые строит fake сам (60 к/с).
pub(super) const FAKE_FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

impl FakeBackendControl {
    /// Следующий `submit_focus_trigger` вернёт этот итог (очередь; пустая — успех).
    pub fn push_trigger_result(&self, result: Result<(), RequestFailure>) {
        self.lock().trigger_script.push_back(result);
    }

    /// Следующий `submit_bracket` вернёт этот итог (очередь; пустая — успех).
    pub fn push_bracket_result(&self, result: Result<(), RequestFailure>) {
        self.lock().bracket_script.push_back(result);
    }

    /// Состояния AF для следующих результатов, по одному на результат (повторяющиеся кадры и
    /// кадр триггера берут из общей очереди по порядку). Когда очередь пуста, HAL «не сообщает»
    /// AF (`None`) — как в RAW-only сессии.
    pub fn push_af_states(&self, states: impl IntoIterator<Item = AfState>) {
        self.lock().af_script.extend(states);
    }

    /// Номер серии, которую бэкенд принял последней.
    pub fn last_submitted_burst(&self) -> Option<BurstId> {
        self.lock().submitted_brackets.last().map(|(burst, _)| *burst)
    }

    /// Результат кадра, на который пришёл триггер тапа `request` (`RequestOrigin::FocusTrigger`).
    pub fn complete_focus_trigger_frame(&self, request: PointRequestId) -> FakeEventDelivery {
        self.deliver_result(RequestOrigin::FocusTrigger { request })
    }

    /// Результаты кадров серии `burst` с этими индексами — **в этом порядке прихода**
    /// (проверка, что движок собирает по индексу, а не по приходу). Экспозиция кадра — та,
    /// что серия просила. Серия, которую бэкенд не принимал, — `NothingToAttachTo`.
    pub fn complete_bracket_frames(&self, burst: BurstId, arrival_order: &[u32]) -> FakeEventDelivery {
        for index in arrival_order {
            if self.deliver_result(RequestOrigin::Burst { burst, index: *index }) != FakeEventDelivery::Sent {
                return FakeEventDelivery::NothingToAttachTo;
            }
        }
        FakeEventDelivery::Sent
    }

    pub(super) fn deliver_result(&self, origin: RequestOrigin) -> FakeEventDelivery {
        let Some(session) = self.current_session_generation() else {
            return FakeEventDelivery::NothingToAttachTo;
        };
        let result = self.next_result(origin, FAKE_FRAME_INTERVAL);
        self.send_event(BackendEvent::CaptureCompleted { session, result })
    }

    /// Построить следующий результат: номер и метка продолжаются, экспозиция берётся из плана
    /// запроса (повторяющегося — из `Sensor`, кадра серии — из серии).
    pub(super) fn next_result(&self, origin: RequestOrigin, interval: Duration) -> CaptureResultMeta {
        let interval_nanos = i64::try_from(interval.as_nanos()).unwrap_or(i64::MAX);
        let mut state = self.lock();
        let frame = state.next_frame_number;
        let timestamp = state.next_timestamp_nanos;
        state.next_frame_number += 1;
        state.next_timestamp_nanos = timestamp.saturating_add(interval_nanos);
        let af_state = state.af_script.pop_front();
        let (exposure_time, iso) = match origin {
            RequestOrigin::Burst { burst, index } => state
                .submitted_brackets
                .iter()
                .find(|(submitted, _)| *submitted == burst)
                .and_then(|(_, bracket)| {
                    bracket
                        .frames()
                        .get(usize::try_from(index).unwrap_or(usize::MAX))
                        .copied()
                })
                .map_or((Duration::from_millis(8), 100), |frame| {
                    (frame.exposure_time.duration(), frame.iso.value())
                }),
            // Кадр вспышки несёт то, что отправили: выдержку и ISO своего разового запроса.
            RequestOrigin::FlashStill { shot } => state
                .submitted_flash_stills
                .iter()
                .find(|(submitted, _)| *submitted == shot)
                .and_then(|(_, controls)| match controls.exposure {
                    ExposurePlan::Sensor { exposure_time, iso } => Some((exposure_time.duration(), iso.value())),
                    ExposurePlan::VendorAuto { .. } => None,
                })
                .unwrap_or((Duration::from_millis(8), 100)),
            RequestOrigin::Repeating | RequestOrigin::FocusTrigger { .. } | RequestOrigin::Unknown => {
                match state.repeating.map(|(_, controls)| controls.exposure) {
                    Some(ExposurePlan::Sensor { exposure_time, iso }) => (exposure_time.duration(), iso.value()),
                    Some(ExposurePlan::VendorAuto { .. }) | None => (Duration::from_millis(8), 100),
                }
            }
        };
        CaptureResultMeta {
            frame_number: FrameNumber(frame),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(timestamp)),
            exposure_time: Some(exposure_time),
            sensitivity_iso: Some(iso),
            reported_frame_duration: None,
            rolling_shutter_skew: Some(Duration::from_micros(13_660)),
            three_a: ThreeAResultState {
                af_state,
                ..ThreeAResultState::unreported(origin)
            },
        }
    }
}
