//! Вспышка в запросах (14d): `FLASH_MODE` пишется в каждый запрос, разовый запрос строится только
//! при выполненных условиях. Общие помощники — в `request_plan_tests`.

use camera_contract::fixtures::front_camera;
use camera_contract::flash::FlashControl;
use camera_core::request_controls::{ExposurePlan, FocusPlan};

use super::tests::{
    auto_exposure, bracket_of, bytes, camx_60, continuous, controls, every_mechanism, has_ae_range, has_vendor_pair,
    inputs, main_camera, plan_of, public_entries, sensor,
};
use super::*;
use crate::flash_still_plan::{FlashStillRefusal, plan_flash_still};

const FLASH_MODE_NAME: &str = "ACAMERA_FLASH_MODE";

fn with_flash(base: RequestControls, flash: FlashControl) -> RequestControls {
    RequestControls { flash, ..base }
}

fn flash_byte(plan: &RequestPlan) -> Option<EntryValue> {
    public_entries(plan).get(FLASH_MODE_NAME).map(|value| (*value).clone())
}

#[test]
fn every_flash_control_writes_its_own_code_and_off_is_explicit() {
    let camera = main_camera();
    let base = controls(sensor(8, 400), continuous());
    for (flash, code) in [
        (FlashControl::Off, tags::FLASH_MODE_OFF),
        (FlashControl::Torch, tags::FLASH_MODE_TORCH),
        (FlashControl::Single, tags::FLASH_MODE_SINGLE),
    ] {
        for mechanism in every_mechanism() {
            let request = plan_of(mechanism, &with_flash(base, flash), &camera);
            assert_eq!(flash_byte(&request.plan), Some(bytes(code)), "{flash:?}");
            assert_eq!(request.notes, Vec::new(), "{flash:?}");
        }
    }
}

#[test]
fn flash_mode_is_written_into_the_repeating_trigger_and_every_bracket_request() {
    let camera = main_camera();
    // Вендорский AE: повторяющийся запрос с фонариком (предвспышка).
    let torch = with_flash(controls(auto_exposure(), continuous()), FlashControl::Torch);
    let mut trigger_inputs = inputs(camx_60(), &torch, &camera);
    trigger_inputs.trigger = Some(AfTrigger::Start);
    assert_eq!(
        flash_byte(&plan_request(&trigger_inputs).plan),
        Some(bytes(tags::FLASH_MODE_TORCH)),
        "триггер несёт фонарик базы"
    );
    // Серия берёт светодиод из базовых управлений, а не включает свой.
    let bracket = bracket_of(&[(2, 100), (8, 400)]);
    for flash in [FlashControl::Off, FlashControl::Torch] {
        let base = with_flash(controls(auto_exposure(), continuous()), flash);
        for request in plan_bracket(&inputs(camx_60(), &base, &camera), &bracket) {
            let expected = match flash {
                FlashControl::Torch => tags::FLASH_MODE_TORCH,
                _ => tags::FLASH_MODE_OFF,
            };
            assert_eq!(flash_byte(&request.plan), Some(bytes(expected)), "{flash:?}");
        }
    }
}

#[test]
fn single_flash_still_carries_frame_rate_and_vendor_keys_like_any_request() {
    let camera = main_camera();
    let single = with_flash(controls(sensor(8, 400), continuous()), FlashControl::Single);
    for mechanism in every_mechanism() {
        let repeating = plan_of(mechanism, &with_flash(single, FlashControl::Off), &camera).plan;
        let still = plan_flash_still(&inputs(mechanism, &single, &camera)).expect("условия выполнены");
        assert_eq!(flash_byte(&still.plan), Some(bytes(tags::FLASH_MODE_SINGLE)));
        assert_eq!(has_vendor_pair(&still.plan), has_vendor_pair(&repeating));
        assert_eq!(has_ae_range(&still.plan), has_ae_range(&repeating));
        assert_eq!(still.plan.template, repeating.template);
        // Остальное — те же записи, что у повторяющегося запроса: отличается только режим вспышки.
        assert_eq!(still.plan.entries.len(), repeating.entries.len());
        assert_eq!(
            public_entries(&still.plan)["ACAMERA_SENSOR_EXPOSURE_TIME"],
            public_entries(&repeating)["ACAMERA_SENSOR_EXPOSURE_TIME"]
        );
    }
    assert!(has_vendor_pair(
        &plan_flash_still(&inputs(camx_60(), &single, &camera)).unwrap().plan
    ));
}

#[test]
fn flash_still_ignores_an_af_trigger_in_the_inputs() {
    let camera = main_camera();
    let single = with_flash(controls(sensor(8, 400), continuous()), FlashControl::Single);
    let mut with_trigger = inputs(camx_60(), &single, &camera);
    with_trigger.trigger = Some(AfTrigger::Start);
    let still = plan_flash_still(&with_trigger).unwrap();
    assert!(!public_entries(&still.plan).contains_key("ACAMERA_CONTROL_AF_TRIGGER"));
}

#[test]
fn flash_still_is_refused_with_the_exact_reason() {
    let camera = main_camera();
    let front = front_camera().capabilities;
    let not_single = with_flash(controls(sensor(8, 400), continuous()), FlashControl::Torch);
    assert_eq!(
        plan_flash_still(&inputs(camx_60(), &not_single, &camera)).unwrap_err(),
        FlashStillRefusal::NotSingleFlash {
            actual: FlashControl::Torch
        }
    );
    let vendor_exposure = with_flash(controls(auto_exposure(), continuous()), FlashControl::Single);
    assert!(matches!(vendor_exposure.exposure, ExposurePlan::VendorAuto { .. }));
    assert_eq!(
        plan_flash_still(&inputs(camx_60(), &vendor_exposure, &camera)).unwrap_err(),
        FlashStillRefusal::ExposureNotSensor
    );
    let single_on_front = with_flash(controls(sensor(8, 400), FocusPlan::FixedLens), FlashControl::Single);
    assert_eq!(
        plan_flash_still(&inputs(camx_60(), &single_on_front, &front)).unwrap_err(),
        FlashStillRefusal::NoFlashUnit
    );
}

#[test]
fn standby_style_off_request_writes_an_explicit_off() {
    let camera = main_camera();
    let off = controls(auto_exposure(), continuous());
    assert_eq!(off.flash, FlashControl::Off);
    assert_eq!(
        flash_byte(&plan_of(FrameRateMechanism::HalDefault, &off, &camera).plan),
        Some(bytes(tags::FLASH_MODE_OFF))
    );
}

#[test]
fn camera_without_flash_gets_no_flash_key_and_an_honest_note_when_light_was_requested() {
    let front = front_camera().capabilities;
    let off = controls(auto_exposure(), FocusPlan::FixedLens);
    let quiet = plan_of(FrameRateMechanism::HalDefault, &off, &front);
    assert_eq!(flash_byte(&quiet.plan), None, "светодиода нет — ключ не нужен");
    assert!(
        !quiet
            .notes
            .iter()
            .any(|note| matches!(note, PlanNote::FlashUnavailable { .. })),
        "Off без вспышки — не повод для заметки"
    );
    let torch = with_flash(off, FlashControl::Torch);
    let asked = plan_of(FrameRateMechanism::HalDefault, &torch, &front);
    assert_eq!(flash_byte(&asked.plan), None);
    assert!(asked.notes.contains(&PlanNote::FlashUnavailable {
        requested: FlashControl::Torch
    }));
}
