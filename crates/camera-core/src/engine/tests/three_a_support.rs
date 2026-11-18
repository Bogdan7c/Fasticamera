//! Общие помощники тестов 3A: стенды, разбор журнала вызовов и событий, ручные результаты кадров.

use std::time::Duration;

use camera_contract::capabilities::CameraDescriptor;
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::oneplus11_visible_cameras;
use camera_contract::three_a::{AfState, PointRequestId, RequestOrigin, SensorPoint, ThreeAResultState};

use super::*;
use crate::request_controls::{AfTrigger, RequestControls};
use crate::three_a_status::{BracketReport, ControlAdjustment, FocusOutcome};

/// Тап в центр кадра.
pub(super) fn center() -> SensorPoint {
    SensorPoint::CENTER
}

/// Тап в точку вне центра (чтобы регион отличался от «по умолчанию»).
pub(super) fn upper_left() -> SensorPoint {
    SensorPoint::new(0.25, 0.25).expect("точка внутри кадра")
}

/// Стенд, уже стримящий с основной камеры в режиме RAW10 60 к/с.
pub(super) fn streaming_rig() -> Rig {
    let mut rig = Rig::new();
    rig.start_streaming_on_wide();
    rig
}

/// То же с изменённым конфигом.
pub(super) fn streaming_rig_with(config: CameraEngineConfig) -> Rig {
    let mut rig = Rig::with_config(config, AppPresence::Foreground);
    rig.start_streaming_on_wide();
    rig
}

/// Все триггеры из журнала вызовов.
pub(super) fn triggers(calls: &[BackendCall]) -> Vec<(AfTrigger, PointRequestId)> {
    calls
        .iter()
        .filter_map(|call| match call {
            BackendCall::SubmitFocusTrigger { trigger, request, .. } => Some((*trigger, *request)),
            _ => None,
        })
        .collect()
}

/// Планы всех повторяющихся запросов из журнала вызовов.
pub(super) fn repeating_plans(calls: &[BackendCall]) -> Vec<RequestControls> {
    calls
        .iter()
        .filter_map(|call| match call {
            BackendCall::SetRepeatingRequest { controls, .. } => Some(*controls),
            _ => None,
        })
        .collect()
}

/// Итоги тапов из событий.
pub(super) fn focus_settled(events: &[CameraEvent]) -> Vec<(PointRequestId, FocusOutcome)> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::FocusSettled { request, outcome } => Some((*request, *outcome)),
            _ => None,
        })
        .collect()
}

/// Поправки из событий `ControlsAdjusted` (каждое событие — отдельным списком).
pub(super) fn adjustment_events(events: &[CameraEvent]) -> Vec<Vec<ControlAdjustment>> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::ControlsAdjusted { adjustments } => Some(adjustments.clone()),
            _ => None,
        })
        .collect()
}

/// Отчёты о сериях из событий.
pub(super) fn bracket_reports(events: &[CameraEvent]) -> Vec<BracketReport> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::BracketCaptured { report } => Some(report.clone()),
            _ => None,
        })
        .collect()
}

/// Результат кадра, собранный вручную (когда важны номер кадра и порядок прихода).
pub(super) fn result_meta(frame: i64, origin: RequestOrigin, af_state: Option<AfState>) -> CaptureResultMeta {
    CaptureResultMeta {
        frame_number: FrameNumber(frame),
        sensor_timestamp: Some(SensorTimestamp::from_nanos(1_000_000_000 + frame * 16_666_667)),
        exposure_time: Some(Duration::from_millis(8)),
        sensitivity_iso: Some(100),
        reported_frame_duration: None,
        rolling_shutter_skew: None,
        three_a: ThreeAResultState {
            af_state,
            ..ThreeAResultState::unreported(origin)
        },
    }
}

impl Rig {
    /// Результат кадра текущей сессии напрямую в движок.
    pub(super) fn feed_result(&mut self, result: CaptureResultMeta) {
        let session = self.fake.current_session_generation().expect("сессия есть");
        self.feed(BackendEvent::CaptureCompleted { session, result });
    }
}

/// Стенд, уже стримящий с основной камеры, у которой тест что-то поменял в характеристиках.
pub(super) fn streaming_rig_with_edited_wide(edit: impl FnOnce(&mut CameraDescriptor)) -> Rig {
    let mut cameras = oneplus11_visible_cameras();
    if let Some(wide_camera) = cameras.iter_mut().find(|camera| camera.id.as_str() == "2") {
        edit(wide_camera);
    }
    let fake = FakeBackendControl::new();
    fake.set_cameras(cameras);
    let mut rig = Rig::over_fake(CameraEngineConfig::default(), AppPresence::Foreground, fake);
    rig.engine.load_catalog(rig.now);
    rig.pump();
    rig.start_streaming_on_wide();
    rig
}
