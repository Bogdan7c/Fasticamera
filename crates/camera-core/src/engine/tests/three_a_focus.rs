//! Тап, блокировка, сброс точки и итог фокуса — через движок и fake-бэкенд: проверяем то,
//! что дошло до запросов HAL и что увидел бы экран (события и снимок).

use std::time::Duration;

use camera_contract::three_a::{
    AeAfLock, AfModeSupport, AfState, PointOfInterest, PointRequestId, RequestOrigin, SensorPoint,
};

use super::three_a_support::*;
use super::*;
use crate::backend::{BackendFailure, RequestFailure};
use crate::command::{InvalidStateReason, UnsupportedCommand, UnsupportedThreeA};
use crate::request_controls::{AfTrigger, ExposurePlan, FocusPlan};
use crate::three_a_status::{FocusOutcome, FocusProgress};

fn tap(rig: &mut Rig, point: SensorPoint) -> PointRequestId {
    assert_eq!(
        rig.send(CameraCommand::FocusAndExposeAt { point }),
        CommandOutcome::Accepted
    );
    PointRequestId(rig.last_command_id().0)
}

#[test]
fn tap_sends_single_shot_request_and_one_trigger_and_scanning_is_visible() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, upper_left());

    let calls = rig.fake.take_calls();
    assert_eq!(
        call_names(&calls),
        vec!["set_repeating_request", "submit_focus_trigger"]
    );
    let plan = repeating_plans(&calls).remove(0);
    let FocusPlan::SingleShot { region } = plan.focus else {
        panic!("тап должен дать однократный AF, а дал {:?}", plan.focus);
    };
    assert_eq!(region.center(), upper_left());
    assert_eq!(
        region.size_fraction(),
        CameraEngineConfig::default().three_a.metering_region_size_fraction
    );
    assert!(
        matches!(plan.exposure, ExposurePlan::VendorAuto { region: Some(r), lock: AeAfLock::Unlocked, .. } if r == region),
        "замер экспозиции в той же точке: {:?}",
        plan.exposure
    );
    assert_eq!(triggers(&calls), vec![(AfTrigger::Start, request)]);

    let status = rig.engine.status().three_a;
    assert_eq!(status.focus, FocusProgress::Scanning { request });
    assert_eq!(status.counters.focus_triggers_sent, 1);
}

#[test]
fn focus_settles_focused_after_scan_and_is_reported_once() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    rig.fresh_events();
    rig.fake
        .push_af_states([AfState::ActiveScan, AfState::ActiveScan, AfState::FocusedLocked]);
    rig.fake.complete_focus_trigger_frame(request);
    rig.pump();
    assert_eq!(
        rig.engine.status().three_a.focus,
        FocusProgress::Scanning { request },
        "пока скан идёт, итога нет"
    );
    rig.fake.complete_frames(2, Duration::from_millis(16));
    rig.pump();
    // Ещё кадры после итога не порождают второго события.
    rig.fake
        .push_af_states([AfState::FocusedLocked, AfState::FocusedLocked]);
    rig.fake.complete_frames(2, Duration::from_millis(16));
    rig.pump();

    let events = rig.fresh_events();
    assert_eq!(focus_settled(&events), vec![(request, FocusOutcome::Focused)]);
    assert_eq!(
        rig.engine.status().three_a.focus,
        FocusProgress::Settled {
            request,
            outcome: FocusOutcome::Focused
        }
    );
}

#[test]
fn hal_reports_not_focused_locked_as_not_focused() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    rig.fresh_events();
    rig.fake
        .push_af_states([AfState::ActiveScan, AfState::NotFocusedLocked]);
    rig.fake.complete_focus_trigger_frame(request);
    rig.fake.complete_frames(1, Duration::from_millis(16));
    rig.pump();
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::NotFocused)]
    );
}

#[test]
fn stale_focused_locked_from_before_the_trigger_is_not_taken_for_the_result() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    rig.fresh_events();
    let trigger_origin = RequestOrigin::FocusTrigger { request };
    // Кадр триггера (номер 10) обогнал по невытесняемой полосе кадр потока 9 с устаревшим
    // FOCUSED_LOCKED прошлого тапа: порядок определяет номер кадра, а не приход.
    rig.feed_result(result_meta(10, trigger_origin, Some(AfState::ActiveScan)));
    rig.feed_result(result_meta(9, RequestOrigin::Repeating, Some(AfState::FocusedLocked)));
    assert!(
        focus_settled(&rig.fresh_events()).is_empty(),
        "устаревший итог проигнорирован"
    );
    assert_eq!(rig.engine.status().three_a.focus, FocusProgress::Scanning { request });

    rig.feed_result(result_meta(11, RequestOrigin::Repeating, Some(AfState::FocusedLocked)));
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::Focused)]
    );
}

#[test]
fn lost_trigger_marker_is_replaced_by_the_grace_frames_rule() {
    let mut config = CameraEngineConfig::default();
    config.three_a.focus_marker_grace_frames = 2;
    let mut rig = streaming_rig_with(config);
    let request = tap(&mut rig, center());
    rig.fresh_events();
    // Метка триггера не придёт. Первые два кадра потока — льготные (устаревшие), третий — верим.
    rig.feed_result(result_meta(1, RequestOrigin::Repeating, Some(AfState::FocusedLocked)));
    rig.feed_result(result_meta(2, RequestOrigin::Repeating, Some(AfState::FocusedLocked)));
    assert!(focus_settled(&rig.fresh_events()).is_empty());
    rig.feed_result(result_meta(3, RequestOrigin::Repeating, Some(AfState::FocusedLocked)));
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::Focused)]
    );
}

#[test]
fn tap_without_any_hal_answer_times_out_after_the_configured_time_exactly_once() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    rig.fresh_events();
    let timeout = Duration::from_millis(CameraEngineConfig::default().three_a.focus_settle_timeout_ms);

    rig.advance(timeout - Duration::from_millis(1));
    assert!(focus_settled(&rig.fresh_events()).is_empty(), "раньше срока итога нет");
    assert!(rig.engine.next_deadline().is_some(), "движок знает, когда проснуться");

    rig.advance(Duration::from_millis(1));
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::TimedOut)]
    );
    let status = rig.engine.status().three_a;
    assert_eq!(
        status.focus,
        FocusProgress::Settled {
            request,
            outcome: FocusOutcome::TimedOut
        }
    );
    assert_eq!(status.counters.focus_timeouts, 1);

    // Запоздавший итог от HAL уже ничего не публикует, и таймер второй раз не срабатывает.
    rig.fake.push_af_states([AfState::FocusedLocked]);
    rig.fake.complete_focus_trigger_frame(request);
    rig.advance(timeout);
    assert!(focus_settled(&rig.fresh_events()).is_empty());
}

#[test]
fn settled_tap_cancels_its_timer() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    rig.fake.push_af_states([AfState::ActiveScan, AfState::FocusedLocked]);
    rig.fake.complete_focus_trigger_frame(request);
    rig.fake.complete_frames(1, Duration::from_millis(16));
    rig.pump();
    rig.fresh_events();
    assert_eq!(rig.engine.next_deadline(), None, "после итога таймеров не осталось");
    rig.advance(Duration::from_secs(10));
    assert!(focus_settled(&rig.fresh_events()).is_empty());
    assert_eq!(rig.engine.status().three_a.counters.focus_timeouts, 0);
}

#[test]
fn repeating_reconcile_never_sends_the_same_trigger_twice() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    let mut all_calls = rig.fake.take_calls();
    // Любая команда, перестраивающая запрос, снова гонит `reconcile` с тем же тапом в намерении.
    for ev in [0.5, 0.0, -0.5] {
        let bias = camera_contract::control::ExposureBias::from_ev(ev).expect("конечное");
        rig.send(CameraCommand::SetExposureBias { bias });
        all_calls.extend(rig.fake.take_calls());
    }
    assert_eq!(
        triggers(&all_calls),
        vec![(AfTrigger::Start, request)],
        "триггер ровно один"
    );
    assert!(repeating_plans(&all_calls).len() >= 3, "запрос при этом перестраивался");
    assert!(
        repeating_plans(&all_calls)
            .iter()
            .all(|plan| matches!(plan.focus, FocusPlan::SingleShot { .. })),
        "а однократный AF держится"
    );
}

#[test]
fn each_new_tap_is_a_new_request_with_its_own_trigger() {
    let mut rig = streaming_rig();
    let first = tap(&mut rig, center());
    let second = tap(&mut rig, upper_left());
    assert_ne!(first, second);
    let calls = rig.fake.take_calls();
    assert_eq!(
        triggers(&calls),
        vec![(AfTrigger::Start, first), (AfTrigger::Start, second)]
    );
    assert_eq!(
        rig.engine.status().three_a.focus,
        FocusProgress::Scanning { request: second }
    );
}

#[test]
fn lock_holds_ae_and_af_through_scene_changes_and_reset_releases_it() {
    let mut rig = streaming_rig();
    assert_eq!(
        rig.send(CameraCommand::LockFocusAndExposureAt { point: center() }),
        CommandOutcome::Accepted
    );
    let calls = rig.fake.take_calls();
    let plan = repeating_plans(&calls).remove(0);
    assert!(matches!(
        plan.exposure,
        ExposurePlan::VendorAuto {
            lock: AeAfLock::Locked,
            ..
        }
    ));
    assert!(
        matches!(plan.focus, FocusPlan::SingleShot { .. }),
        "объектив держится в AF_MODE_AUTO"
    );

    // «Сцена меняется»: HAL шлёт кадры с другим AF/экспозицией, блокировка остаётся.
    rig.fake
        .push_af_states([AfState::PassiveScan, AfState::PassiveUnfocused]);
    rig.fake.complete_frames(5, Duration::from_millis(16));
    rig.pump();
    let status = rig.engine.status().three_a;
    assert_eq!(status.lock, AeAfLock::Locked);
    assert!(matches!(status.point, PointOfInterest::Point { .. }));
    assert!(
        rig.fake.take_calls().is_empty(),
        "блокировка не перестраивает запрос сама"
    );

    assert_eq!(rig.send(CameraCommand::ResetPointOfInterest), CommandOutcome::Accepted);
    let plan = repeating_plans(&rig.fake.take_calls()).remove(0);
    assert!(matches!(
        plan.exposure,
        ExposurePlan::VendorAuto {
            lock: AeAfLock::Unlocked,
            region: None,
            ..
        }
    ));
    assert_eq!(plan.focus, FocusPlan::Continuous { region: None });
    let status = rig.engine.status().three_a;
    assert_eq!(
        (status.point, status.lock),
        (PointOfInterest::WholeFrame, AeAfLock::Unlocked)
    );
    assert_eq!(status.focus, FocusProgress::Continuous { af_state: None });
    assert_eq!(
        rig.send(CameraCommand::ResetPointOfInterest),
        CommandOutcome::AlreadyInState
    );
}

#[test]
fn a_plain_tap_releases_a_previous_lock() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    tap(&mut rig, upper_left());
    assert_eq!(rig.engine.status().three_a.lock, AeAfLock::Unlocked);
}

#[test]
fn camera_switch_resets_point_and_lock_by_default() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    rig.fake.take_calls();

    assert_eq!(
        rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.state_name(), "streaming");
    let status = rig.engine.status().three_a;
    assert_eq!(
        (status.point, status.lock),
        (PointOfInterest::WholeFrame, AeAfLock::Unlocked)
    );
    let plan = repeating_plans(&rig.fake.take_calls())
        .pop()
        .expect("запрос на новой камере");
    assert_eq!(plan.focus, FocusPlan::Continuous { region: None });
    assert!(matches!(
        plan.exposure,
        ExposurePlan::VendorAuto {
            lock: AeAfLock::Unlocked,
            ..
        }
    ));
}

#[test]
fn camera_switch_keeps_point_and_lock_when_config_says_so_and_refocuses_on_the_new_camera() {
    let mut config = CameraEngineConfig::default();
    config.three_a.reset_point_on_camera_switch = false;
    let mut rig = streaming_rig_with(config);
    rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    let request = PointRequestId(rig.last_command_id().0);
    rig.fake.take_calls();

    rig.send(CameraCommand::SwitchTo {
        role: LensRole::UltraWide,
    });
    let status = rig.engine.status().three_a;
    assert_eq!(status.lock, AeAfLock::Locked);
    assert!(matches!(status.point, PointOfInterest::Point { request: kept, .. } if kept == request));
    let calls = rig.fake.take_calls();
    assert_eq!(
        triggers(&calls),
        vec![(AfTrigger::Start, request)],
        "новая камера — новый поток: фокус на точке запускается заново"
    );
}

#[test]
fn tap_on_the_fixed_focus_front_camera_only_meters() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::SwitchTo { role: LensRole::Front });
    rig.fake.take_calls();

    assert_eq!(
        rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() }),
        CommandOutcome::Accepted
    );
    let calls = rig.fake.take_calls();
    assert!(triggers(&calls).is_empty(), "фокусироваться нечем: триггера нет");
    let plan = repeating_plans(&calls).remove(0);
    assert_eq!(plan.focus, FocusPlan::FixedLens);
    assert!(
        matches!(plan.exposure, ExposurePlan::VendorAuto { region: Some(_), .. }),
        "а замер экспозиции в точке есть"
    );
    assert_eq!(rig.engine.status().three_a.focus, FocusProgress::FixedLens);
}

#[test]
fn tap_lock_and_bracket_need_a_running_stream() {
    let mut rig = Rig::new();
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.fake.take_calls();
    let not_streaming = CommandOutcome::InvalidState(InvalidStateReason::NotStreaming);
    assert_eq!(
        rig.send(CameraCommand::FocusAndExposeAt { point: center() }),
        not_streaming
    );
    assert_eq!(
        rig.send(CameraCommand::LockFocusAndExposureAt { point: center() }),
        not_streaming
    );
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(rig.engine.status().three_a.point, PointOfInterest::WholeFrame);
}

#[test]
fn lock_is_unsupported_without_ae_lock_and_changes_nothing() {
    let mut rig = streaming_rig_with_edited_wide(|camera| {
        camera.capabilities.three_a.ae_lock = camera_contract::three_a::LockSupport::NotAvailable;
    });
    assert_eq!(
        rig.send(CameraCommand::LockFocusAndExposureAt { point: center() }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ExposureLock))
    );
    assert!(rig.fake.take_calls().is_empty(), "бэкенд не тронут");
    let status = rig.engine.status().three_a;
    assert_eq!(
        (status.point, status.lock),
        (PointOfInterest::WholeFrame, AeAfLock::Unlocked)
    );
    assert_eq!(
        status.focus,
        FocusProgress::Continuous { af_state: None },
        "исполнитель фокуса тоже не тронут"
    );
    // А обычный тап на этой камере работает.
    assert_eq!(
        rig.send(CameraCommand::FocusAndExposeAt { point: center() }),
        CommandOutcome::Accepted
    );
}

#[test]
fn tap_is_unsupported_without_af_mode_auto() {
    let mut rig = streaming_rig_with_edited_wide(|camera| {
        camera
            .capabilities
            .three_a
            .af_modes
            .retain(|mode| *mode != AfModeSupport::Auto);
    });
    assert_eq!(
        rig.send(CameraCommand::FocusAndExposeAt { point: center() }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::SingleShotFocus))
    );
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(rig.engine.status().three_a.point, PointOfInterest::WholeFrame);
}

#[test]
fn rejected_trigger_reports_not_delivered_and_keeps_the_stream_running() {
    // Отказ самого разового запроса (`Backend`) сессию не ломает (ревью 08, как у серии):
    // поток идёт, тап получает явный итог, а не молчание до восстановления.
    let mut rig = streaming_rig();
    rig.fake
        .push_trigger_result(Err(RequestFailure::Backend(BackendFailure {
            operation: "ACameraCaptureSession_capture",
            code: -10005,
            detail: "тест".to_owned(),
        })));
    let request = tap(&mut rig, center());
    let status = rig.engine.status();
    assert_eq!(status.three_a.counters.focus_trigger_failures, 1);
    assert_eq!(status.three_a.counters.focus_triggers_sent, 0);
    assert_eq!(status.state.name(), "streaming", "поток не уходит в восстановление");
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::TriggerNotDelivered)]
    );
    rig.advance(Duration::from_secs(30));
    assert!(
        focus_settled(&rig.fresh_events()).is_empty(),
        "итог объявлен ровно один раз, таймер не взведён"
    );
}

#[test]
fn trigger_rejected_because_the_device_is_gone_sends_the_session_into_recovery() {
    let mut rig = streaming_rig();
    rig.fake.push_trigger_result(Err(RequestFailure::DeviceDisconnected));
    let request = tap(&mut rig, center());
    let status = rig.engine.status();
    assert_eq!(status.state.name(), "error", "потеря устройства — восстановление");
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::TriggerNotDelivered)]
    );
}

#[test]
fn losing_the_stream_forgets_the_pending_tap_timer() {
    let mut rig = streaming_rig();
    tap(&mut rig, center());
    rig.fresh_events();
    rig.fake.disconnect_current_device();
    rig.pump();
    assert_eq!(rig.state_name(), "error");
    assert_eq!(
        rig.engine.next_deadline(),
        None,
        "срок тапа потерянного потока снят: просыпаться незачем"
    );
    rig.advance(Duration::from_secs(10));
    assert!(
        focus_settled(&rig.fresh_events()).is_empty(),
        "таймер тапа потерянного потока не стреляет"
    );
}

#[test]
fn recovery_restarts_a_persisted_tap_on_the_new_session() {
    let mut rig = streaming_rig();
    let request = tap(&mut rig, center());
    rig.fake.disconnect_current_device();
    rig.pump();
    rig.fake.take_calls();
    // Камера снова свободна: движок восстанавливает поток и запускает тап заново.
    let camera = rig.fake.open_camera();
    assert!(camera.is_none(), "устройство закрыто после вытеснения");
    rig.feed(BackendEvent::CameraAvailability {
        camera: camera_contract::fixtures::camera_id("2"),
        availability: crate::backend::CameraAvailability::Available,
    });
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(triggers(&rig.fake.take_calls()), vec![(AfTrigger::Start, request)]);
}
