//! Ручные параметры, компенсация, явные поправки «ограничено до …», (баланс белого теперь у владельца белого, см. `white_*`).

use std::time::Duration;

use camera_contract::control::{Control, ExposureBias, ExposureTime, FocusDistance, Iso, ManualControl};
use camera_contract::fixtures::oneplus11_visible_cameras;
use camera_contract::three_a::{PointOfInterest, ThreeAProfile};

use super::three_a_support::*;
use super::*;
use crate::command::{UnsupportedCommand, UnsupportedThreeA};
use crate::request_controls::{ExposurePlan, FocusPlan};
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter};

fn iso(value: u32) -> Control<Iso> {
    Control::Manual(Iso::new(value).expect("ISO"))
}

fn shutter(duration: Duration) -> Control<ExposureTime> {
    Control::Manual(ExposureTime::new(duration).expect("выдержка"))
}

fn fully_manual(iso_value: u32, exposure: Duration) -> ManualControl {
    ManualControl {
        iso: iso(iso_value),
        shutter: shutter(exposure),
        ..ManualControl::AUTOMATIC
    }
}

fn bias(ev: f32) -> ExposureBias {
    ExposureBias::from_ev(ev).expect("конечное")
}

fn current_plan(rig: &Rig) -> RequestControls {
    rig.fake.repeating_request().expect("запрос идёт").1
}

use crate::request_controls::RequestControls;

#[test]
fn manual_exposure_reaches_the_request_and_the_result_reports_it_back() {
    let mut rig = streaming_rig();
    // 1/1000 с, ISO 200 — внутри диапазонов и кадра 60 к/с.
    let controls = fully_manual(200, Duration::from_millis(1));
    assert_eq!(
        rig.send(CameraCommand::ApplyControls { controls }),
        CommandOutcome::Accepted
    );

    assert_eq!(
        current_plan(&rig).exposure,
        ExposurePlan::Sensor {
            exposure_time: ExposureTime::new(Duration::from_millis(1)).expect("выдержка"),
            iso: Iso::new(200).expect("ISO"),
        }
    );
    rig.fake.complete_frames(1, Duration::from_millis(16));
    rig.pump();
    let status = rig.engine.status().three_a;
    let last = status.last_result.expect("результат кадра попал в снимок");
    assert_eq!(last.exposure_time, Some(Duration::from_millis(1)));
    assert_eq!(last.sensitivity_iso, Some(200));
    assert!(status.adjustments.is_empty(), "всё применено как есть");
    assert!(adjustment_events(&rig.fresh_events()).is_empty());
}

#[test]
fn iso_below_camera_minimum_is_raised_with_an_explicit_adjustment() {
    let mut rig = streaming_rig();
    let controls = fully_manual(50, Duration::from_millis(4));
    rig.send(CameraCommand::ApplyControls { controls });

    let expected = ControlAdjustment {
        parameter: ControlParameter::Iso,
        requested: 50.0,
        applied: 100.0,
        reason: AdjustmentReason::BelowCameraRange,
    };
    assert!(matches!(current_plan(&rig).exposure, ExposurePlan::Sensor { iso, .. } if iso.value() == 100));
    let events = rig.fresh_events();
    assert_eq!(adjustment_events(&events), vec![vec![expected]]);
    let status = rig.engine.status().three_a;
    assert_eq!(status.adjustments, vec![expected]);
    assert_eq!(status.controls, controls, "желаемое — как просил пользователь");
}

#[test]
fn iso_above_camera_maximum_is_lowered() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::ApplyControls {
        controls: fully_manual(12_800, Duration::from_millis(4)),
    });
    assert!(matches!(current_plan(&rig).exposure, ExposurePlan::Sensor { iso, .. } if iso.value() == 6_400));
    assert_eq!(
        rig.engine.status().three_a.adjustments,
        vec![ControlAdjustment {
            parameter: ControlParameter::Iso,
            requested: 12_800.0,
            applied: 6_400.0,
            reason: AdjustmentReason::AboveCameraRange,
        }]
    );
}

#[test]
fn shutter_longer_than_the_60_fps_frame_is_limited_to_the_frame() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::ApplyControls {
        controls: fully_manual(100, Duration::from_millis(100)),
    });
    let frame = mode_raw10_60().frame_rate.frame_duration().as_duration();
    assert!(matches!(
        current_plan(&rig).exposure,
        ExposurePlan::Sensor { exposure_time, .. } if exposure_time.duration() == frame
    ));
    assert_eq!(
        adjustment_events(&rig.fresh_events()),
        vec![vec![ControlAdjustment {
            parameter: ControlParameter::Shutter,
            requested: 100_000_000.0,
            applied: 16_666_667.0,
            reason: AdjustmentReason::LimitedByFrameDuration,
        }]]
    );
}

#[test]
fn same_adjustment_is_announced_once_but_stays_in_the_status() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::ApplyControls {
        controls: fully_manual(50, Duration::from_millis(4)),
    });
    assert_eq!(adjustment_events(&rig.fresh_events()).len(), 1);
    // Посторонняя команда перестраивает запрос, но поправка та же — повторного события нет.
    rig.send(CameraCommand::SetThreeAProfile {
        profile: ThreeAProfile::Video,
    });
    rig.send(CameraCommand::FocusAndExposeAt { point: center() });
    assert!(adjustment_events(&rig.fresh_events()).is_empty());
    assert_eq!(rig.engine.status().three_a.adjustments.len(), 1);
    // Снял ручной режим — поправок нет, и событие не нужно.
    rig.send(CameraCommand::ApplyControls {
        controls: ManualControl::AUTOMATIC,
    });
    assert!(adjustment_events(&rig.fresh_events()).is_empty());
    assert!(rig.engine.status().three_a.adjustments.is_empty());
}

#[test]
fn semi_automatic_exposure_is_unsupported_and_changes_nothing() {
    let mut rig = streaming_rig();
    let before = rig.engine.status().three_a.controls;
    let semi_auto = ManualControl {
        iso: iso(400),
        ..ManualControl::AUTOMATIC
    };
    assert_eq!(
        rig.send(CameraCommand::ApplyControls { controls: semi_auto }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::SemiAutomaticExposure))
    );
    assert!(rig.fake.take_calls().is_empty(), "бэкенд не тронут");
    assert_eq!(
        rig.engine.status().three_a.controls,
        before,
        "желаемые параметры прежние"
    );
    let shutter_only = ManualControl {
        shutter: shutter(Duration::from_millis(2)),
        ..ManualControl::AUTOMATIC
    };
    assert_eq!(
        rig.send(CameraCommand::ApplyControls { controls: shutter_only }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::SemiAutomaticExposure))
    );
}

#[test]
fn manual_exposure_is_unsupported_on_a_camera_without_manual_sensor() {
    let mut rig = streaming_rig_with_edited_wide(|camera| {
        camera.capabilities.manual_sensor = camera_contract::capabilities::ManualSensorRanges::NotSupported;
    });
    assert_eq!(
        rig.send(CameraCommand::ApplyControls {
            controls: fully_manual(100, Duration::from_millis(4))
        }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor))
    );
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn switching_to_a_camera_without_manual_sensor_degrades_to_auto_with_explicit_adjustments() {
    let mut cameras = oneplus11_visible_cameras();
    for camera in &mut cameras {
        if camera.id.as_str() == "3" {
            camera.capabilities.manual_sensor = camera_contract::capabilities::ManualSensorRanges::NotSupported;
        }
    }
    let fake = FakeBackendControl::new();
    fake.set_cameras(cameras);
    let mut rig = Rig::over_fake(CameraEngineConfig::default(), AppPresence::Foreground, fake);
    rig.engine.load_catalog(rig.now);
    rig.pump();
    rig.start_streaming_on_wide();
    let controls = fully_manual(200, Duration::from_millis(4));
    rig.send(CameraCommand::ApplyControls { controls });
    rig.fresh_events();

    assert_eq!(
        rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.state_name(), "streaming", "поток не упал из-за ручных значений");
    assert!(matches!(current_plan(&rig).exposure, ExposurePlan::VendorAuto { .. }));
    let events = rig.fresh_events();
    let adjustments = adjustment_events(&events).pop().expect("поправки объявлены");
    assert_eq!(adjustments.len(), 2);
    assert!(
        adjustments
            .iter()
            .all(|adjustment| adjustment.reason == AdjustmentReason::NotSupportedByCamera && adjustment.applied == 0.0)
    );
    assert_eq!(rig.engine.status().three_a.controls, controls, "желаемое сохранено");
}

#[test]
fn manual_focus_is_clamped_and_unsupported_on_the_fixed_focus_front_camera() {
    let mut rig = streaming_rig();
    let far_too_close = ManualControl {
        focus: Control::Manual(FocusDistance::from_diopters(15.0).expect("конечная")),
        ..ManualControl::AUTOMATIC
    };
    rig.send(CameraCommand::ApplyControls {
        controls: far_too_close,
    });
    // Ближайший фокус основной камеры в фикстуре — 10 дптр.
    assert!(matches!(
        current_plan(&rig).focus,
        FocusPlan::Manual { distance } if distance.diopters() == 10.0
    ));
    assert_eq!(
        adjustment_events(&rig.fresh_events()),
        vec![vec![ControlAdjustment {
            parameter: ControlParameter::Focus,
            requested: 15.0,
            applied: 10.0,
            reason: AdjustmentReason::AboveCameraRange,
        }]]
    );

    // Ручной фокус в пределах камеры — без поправок; затем фронтальная камера его не умеет.
    let reachable = ManualControl {
        focus: Control::Manual(FocusDistance::from_diopters(2.0).expect("конечная")),
        ..ManualControl::AUTOMATIC
    };
    rig.send(CameraCommand::ApplyControls { controls: reachable });
    assert!(adjustment_events(&rig.fresh_events()).is_empty());
    rig.send(CameraCommand::SwitchTo { role: LensRole::Front });
    assert_eq!(current_plan(&rig).focus, FocusPlan::FixedLens);
    assert_eq!(
        adjustment_events(&rig.fresh_events()),
        vec![vec![ControlAdjustment {
            parameter: ControlParameter::Focus,
            requested: 2.0,
            applied: 0.0,
            reason: AdjustmentReason::NotSupportedByCamera,
        }]]
    );
}

#[test]
fn exposure_compensation_is_exact_on_the_sixth_stop_grid_and_quantized_off_it() {
    let mut rig = streaming_rig();
    let steps = |rig: &Rig| match current_plan(rig).exposure {
        ExposurePlan::VendorAuto { compensation_steps, .. } => compensation_steps,
        other => panic!("ожидался вендорский AE, а {other:?}"),
    };

    // +0,5 EV = ровно 3 шага по 1/6: поправки нет.
    rig.send(CameraCommand::SetExposureBias { bias: bias(0.5) });
    assert_eq!(steps(&rig), 3);
    assert!(rig.engine.status().three_a.adjustments.is_empty());
    assert!(adjustment_events(&rig.fresh_events()).is_empty());

    // +0,55 EV = 3,3 шага → 3, и это сказано явно.
    rig.send(CameraCommand::SetExposureBias { bias: bias(0.55) });
    assert_eq!(steps(&rig), 3);
    let adjustment = rig.engine.status().three_a.adjustments[0];
    assert_eq!(adjustment.reason, AdjustmentReason::QuantizedToStep);
    assert!(
        (adjustment.applied - 0.5).abs() < 1e-9,
        "применено 0,5 EV, а не {}",
        adjustment.applied
    );

    // За диапазоном камеры (±3 EV = 18 шагов) — обрезка с причиной.
    rig.send(CameraCommand::SetExposureBias { bias: bias(4.0) });
    assert_eq!(steps(&rig), 18);
    assert_eq!(
        rig.engine.status().three_a.adjustments[0].reason,
        AdjustmentReason::AboveCameraRange
    );
    rig.send(CameraCommand::SetExposureBias { bias: bias(-4.0) });
    assert_eq!(steps(&rig), -18);
    assert_eq!(
        rig.engine.status().three_a.adjustments[0].reason,
        AdjustmentReason::BelowCameraRange
    );
    assert_eq!(
        rig.send(CameraCommand::SetExposureBias { bias: bias(-4.0) }),
        CommandOutcome::AlreadyInState
    );
}

#[test]
fn exposure_compensation_is_ignored_with_a_note_when_exposure_is_fully_manual() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::SetExposureBias { bias: bias(1.0) });
    rig.send(CameraCommand::ApplyControls {
        controls: ManualControl {
            exposure_bias: bias(1.0),
            ..fully_manual(100, Duration::from_millis(4))
        },
    });
    assert_eq!(
        rig.engine.status().three_a.adjustments,
        vec![ControlAdjustment {
            parameter: ControlParameter::ExposureBias,
            requested: 1.0,
            applied: 0.0,
            reason: AdjustmentReason::IgnoredWhileExposureFullyManual,
        }]
    );
}

#[test]
fn set_exposure_bias_does_not_touch_focus_point_or_triggers() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() });
    let status_before = rig.engine.status().three_a;
    let focus_plan_before = current_plan(&rig).focus;
    rig.fake.take_calls();

    rig.send(CameraCommand::SetExposureBias { bias: bias(1.0) });

    let calls = rig.fake.take_calls();
    assert!(triggers(&calls).is_empty(), "компенсация не перезапускает фокус");
    assert_eq!(current_plan(&rig).focus, focus_plan_before);
    let status_after = rig.engine.status().three_a;
    assert_eq!(status_after.point, status_before.point);
    assert_eq!(status_after.focus, status_before.focus);
    assert_eq!(status_after.lock, status_before.lock);
    assert_eq!(status_after.controls.exposure_bias, bias(1.0));
}

#[test]
fn close_forgets_the_whole_three_a_intent() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    rig.send(CameraCommand::SetExposureBias { bias: bias(1.0) });
    rig.send(CameraCommand::Close);
    let status = rig.engine.status().three_a;
    assert_eq!(status.point, PointOfInterest::WholeFrame);
    assert_eq!(status.controls, ManualControl::AUTOMATIC);
    assert_eq!(status.lock, camera_contract::three_a::AeAfLock::Unlocked);
}
