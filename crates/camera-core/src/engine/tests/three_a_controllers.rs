//! Движок с чужими исполнителями 3A: всё, что он делает, идёт через трейты, а не через
//! знание о вендорской реализации. Плюс пробуждение внешнего исполнителя (`refresh`).

use std::time::Duration;

use camera_contract::control::{ExposureTime, Iso};
use camera_contract::three_a::{AeAfLock, PointRequestId, RequestOrigin};

use super::three_a_support::*;
use super::*;
use crate::command::{UnsupportedCommand, UnsupportedThreeA};
use crate::request_controls::{AfTrigger, ExposurePlan, FocusPlan};
use crate::three_a_control::{FocusTriggerRequest, TriggerDelivery};
use crate::three_a_status::{AdjustmentReason, FocusOutcome};
use crate::three_a_vendor::scripted::{
    ScriptedExposure, ScriptedExposureController, ScriptedFocus, ScriptedFocusController,
};
use crate::three_a_wiring::ThreeAWiring;

fn manual_plan(exposure_ms: u64, iso: u32) -> ExposurePlan {
    ExposurePlan::Sensor {
        exposure_time: ExposureTime::new(Duration::from_millis(exposure_ms)).expect("выдержка"),
        iso: Iso::new(iso).expect("ISO"),
    }
}

const AUTO_PLAN: ExposurePlan = ExposurePlan::VendorAuto {
    compensation_steps: 0,
    lock: AeAfLock::Unlocked,
    region: None,
};

fn rig_with_scripted_exposure() -> (Rig, ScriptedExposure) {
    let (controller, script) = ScriptedExposureController::new(AUTO_PLAN);
    let config = CameraEngineConfig::default();
    let focus = crate::three_a_vendor::VendorFocus::new(config.three_a.focus_marker_grace_frames);
    let wiring = ThreeAWiring::vendor(&config.three_a).with_controllers(Box::new(controller), Box::new(focus));
    let mut rig = Rig::with_wiring(config, wiring);
    rig.start_streaming_on_wide();
    (rig, script)
}

#[test]
fn refresh_that_reports_a_change_rebuilds_the_request_exactly_once() {
    let (mut rig, script) = rig_with_scripted_exposure();
    assert_eq!(
        rig.fake.repeating_request().map(|(_, plan)| plan.exposure),
        Some(AUTO_PLAN)
    );

    script.publish_new_plan(manual_plan(5, 300), Vec::new());
    rig.engine.refresh_three_a(rig.now);
    rig.pump();
    let calls = rig.fake.take_calls();
    assert_eq!(
        call_names(&calls),
        vec!["set_repeating_request"],
        "ровно одна перестройка"
    );
    assert_eq!(repeating_plans(&calls)[0].exposure, manual_plan(5, 300));

    // Второе пробуждение без нового решения: исполнитель спрошен, запрос не тронут.
    rig.engine.refresh_three_a(rig.now);
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(script.refresh_calls(), 2);
}

#[test]
fn refresh_that_changes_nothing_in_the_plan_does_not_touch_the_backend() {
    let (mut rig, script) = rig_with_scripted_exposure();
    // Исполнитель говорит «изменилось», но план тот же — запрос не перестраивается.
    script.publish_new_plan(AUTO_PLAN, Vec::new());
    rig.engine.refresh_three_a(rig.now);
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn adjustments_from_a_foreign_exposure_controller_reach_the_event_and_the_status() {
    let (mut rig, script) = rig_with_scripted_exposure();
    let adjustment = crate::three_a_status::ControlAdjustment {
        parameter: crate::three_a_status::ControlParameter::Iso,
        requested: 25_600.0,
        applied: 6_400.0,
        reason: AdjustmentReason::AboveCameraRange,
    };
    script.publish_new_plan(manual_plan(5, 6_400), vec![adjustment]);
    rig.engine.refresh_three_a(rig.now);
    rig.pump();
    assert_eq!(adjustment_events(&rig.fresh_events()), vec![vec![adjustment]]);
    assert_eq!(rig.engine.status().three_a.adjustments, vec![adjustment]);
}

#[test]
fn engine_forwards_every_result_including_bursts_and_resets_on_stream_end() {
    let (mut rig, script) = rig_with_scripted_exposure();
    rig.fake.complete_frames(2, Duration::from_millis(16));
    rig.pump();
    rig.feed_result(result_meta(
        99,
        RequestOrigin::FocusTrigger {
            request: PointRequestId(1),
        },
        None,
    ));
    rig.pump();
    let origins: Vec<RequestOrigin> = script
        .observed_frames()
        .iter()
        .map(|frame| frame.three_a.origin)
        .collect();
    assert_eq!(origins.len(), 3);
    assert_eq!(
        origins[2],
        RequestOrigin::FocusTrigger {
            request: PointRequestId(1)
        }
    );
    let resets_before = script.resets();
    rig.send(CameraCommand::StopStreaming);
    assert!(script.resets() > resets_before, "конец потока — сброс исполнителя");
}

#[test]
fn exposure_controller_refusal_at_reconcile_degrades_to_auto_and_never_halts() {
    let (mut rig, script) = rig_with_scripted_exposure();
    script.fail_planning(UnsupportedThreeA::ManualSensor);
    // Команда, перестраивающая запрос: исполнитель отказал, а команде отвечать нечем — но
    // сначала кандидат проверяется и команда получает честный отказ.
    assert_eq!(
        rig.send(CameraCommand::SetThreeAProfile {
            profile: camera_contract::three_a::ThreeAProfile::Video
        }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor))
    );
    // Без команды (пробуждение) — мягкая деградация: поток идёт, план — автомат.
    script.publish_new_plan(manual_plan(5, 300), Vec::new());
    rig.engine.refresh_three_a(rig.now);
    assert_eq!(
        rig.fake.repeating_request().map(|(_, plan)| plan.exposure),
        Some(manual_plan(5, 300))
    );
    script.publish_new_plan(AUTO_PLAN, Vec::new());
    script.fail_planning(UnsupportedThreeA::ManualSensor);
    rig.engine.refresh_three_a(rig.now);
    rig.pump();
    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(
        rig.fake.repeating_request().map(|(_, plan)| plan.exposure),
        Some(AUTO_PLAN)
    );
}

fn rig_with_scripted_focus() -> (Rig, ScriptedFocus) {
    let (controller, script) = ScriptedFocusController::new(FocusPlan::Continuous { region: None });
    let config = CameraEngineConfig::default();
    let wiring = ThreeAWiring::vendor(&config.three_a)
        .with_controllers(Box::new(crate::three_a_vendor::VendorExposure), Box::new(controller));
    let mut rig = Rig::with_wiring(config, wiring);
    rig.start_streaming_on_wide();
    (rig, script)
}

#[test]
fn engine_sends_the_trigger_a_foreign_focus_controller_asks_for_and_reports_its_outcome() {
    let (mut rig, script) = rig_with_scripted_focus();
    let request = PointRequestId(77);
    script.queue_trigger(FocusTriggerRequest {
        trigger: AfTrigger::Start,
        request,
    });
    // Любая перестройка запроса даёт исполнителю шанс выдать триггер; движок его отправляет.
    rig.send(CameraCommand::SetThreeAProfile {
        profile: camera_contract::three_a::ThreeAProfile::Video,
    });
    let calls = rig.fake.take_calls();
    assert_eq!(triggers(&calls), vec![(AfTrigger::Start, request)]);
    assert_eq!(
        script.deliveries(),
        vec![(
            FocusTriggerRequest {
                trigger: AfTrigger::Start,
                request
            },
            TriggerDelivery::Submitted
        )],
        "исполнитель узнал, что триггер ушёл"
    );

    script.queue_outcome(request, FocusOutcome::Focused);
    rig.fake.complete_frames(1, Duration::from_millis(16));
    rig.pump();
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::Focused)]
    );

    // Подтверждённый триггер больше не выдаётся (исполнитель убрал его из очереди).
    rig.send(CameraCommand::SetThreeAProfile {
        profile: camera_contract::three_a::ThreeAProfile::Photo,
    });
    assert!(triggers(&rig.fake.take_calls()).is_empty());
}

#[test]
fn engine_asks_the_focus_controller_about_the_timeout_of_its_own_trigger() {
    let (mut rig, script) = rig_with_scripted_focus();
    let request = PointRequestId(5);
    script.queue_trigger(FocusTriggerRequest {
        trigger: AfTrigger::Start,
        request,
    });
    rig.send(CameraCommand::SetThreeAProfile {
        profile: camera_contract::three_a::ThreeAProfile::Video,
    });
    rig.fresh_events();
    rig.advance(Duration::from_millis(
        CameraEngineConfig::default().three_a.focus_settle_timeout_ms,
    ));
    assert_eq!(script.timed_out_requests(), vec![request]);
    assert_eq!(
        focus_settled(&rig.fresh_events()),
        vec![(request, FocusOutcome::TimedOut)]
    );
}

#[test]
fn tracked_focus_region_from_outside_rebuilds_the_request_once_without_a_command() {
    // Задел фокус-трекинга (решение 69): регион пришёл от внешнего трекера, команды нет.
    let (mut rig, script) = rig_with_scripted_focus();
    rig.fake.take_calls();
    let region = camera_contract::three_a::MeteringRegion::new(
        camera_contract::three_a::SensorPoint::new(0.2, 0.7).unwrap(),
        0.18,
    )
    .unwrap();
    script.publish_tracked_plan(FocusPlan::Continuous { region: Some(region) });

    rig.engine.refresh_three_a(rig.now);
    let calls = rig.fake.take_calls();
    let rebuilt: Vec<_> = calls
        .iter()
        .filter_map(|call| match call {
            crate::fake::BackendCall::SetRepeatingRequest { controls, .. } => Some(controls.focus),
            _ => None,
        })
        .collect();
    assert_eq!(
        rebuilt,
        vec![FocusPlan::Continuous { region: Some(region) }],
        "ровно одна перестройка с новым регионом трекера"
    );

    // Второе пробуждение без нового региона ничего не трогает, но исполнителя спрашивают.
    rig.engine.refresh_three_a(rig.now);
    assert!(rig.fake.take_calls().is_empty());
    assert_eq!(script.refresh_calls(), 2);
}
