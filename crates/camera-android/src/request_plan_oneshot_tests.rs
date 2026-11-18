//! Разовые запросы: триггер фокуса и кадры серии (общие помощники — в `request_plan_tests`).

use camera_contract::control::FocusDistance;
use camera_contract::fixtures::front_camera;
use camera_contract::three_a::AeAfLock;
use camera_core::request_controls::{AfTrigger, ExposurePlan, FocusPlan};

use super::tests::{
    auto_exposure, bracket_of, bytes, camx_60, controls, every_mechanism, has_ae_range, has_vendor_pair, inputs,
    main_camera, plan_of, public_entries, region,
};
use super::*;

#[test]
fn trigger_request_is_the_repeating_plan_plus_exactly_one_trigger_entry_for_every_lever() {
    let camera = main_camera();
    let focus = FocusPlan::SingleShot {
        region: region(0.5, 0.5, 0.1),
    };
    let plan_controls = controls(auto_exposure(), focus);
    for mechanism in every_mechanism() {
        let repeating = plan_of(mechanism, &plan_controls, &camera).plan;
        assert!(!public_entries(&repeating).contains_key("ACAMERA_CONTROL_AF_TRIGGER"));
        for (trigger, code) in [
            (AfTrigger::Start, tags::AF_TRIGGER_START),
            (AfTrigger::Cancel, tags::AF_TRIGGER_CANCEL),
        ] {
            let mut trigger_inputs = inputs(mechanism, &plan_controls, &camera);
            trigger_inputs.trigger = Some(trigger);
            let triggered = plan_request(&trigger_inputs).plan;
            assert_eq!(public_entries(&triggered)["ACAMERA_CONTROL_AF_TRIGGER"], &bytes(code));
            assert_eq!(triggered.entries.len(), repeating.entries.len() + 1);
            assert_eq!(triggered.entries[..repeating.entries.len()], repeating.entries[..]);
            // Рычаг частоты и вендорские ключи — в каждом типе запроса (CamX «липкий», зонд 08).
            assert_eq!(has_vendor_pair(&triggered), has_vendor_pair(&repeating));
            assert_eq!(has_ae_range(&triggered), has_ae_range(&repeating));
        }
    }
    let camx_trigger = {
        let mut trigger_inputs = inputs(camx_60(), &plan_controls, &camera);
        trigger_inputs.trigger = Some(AfTrigger::Start);
        plan_request(&trigger_inputs).plan
    };
    assert!(has_vendor_pair(&camx_trigger), "CamX-ключи есть в триггере");
}

#[test]
fn trigger_is_not_written_for_a_fixed_focus_camera() {
    let front = front_camera().capabilities;
    let plan_controls = controls(auto_exposure(), FocusPlan::FixedLens);
    let mut trigger_inputs = inputs(FrameRateMechanism::HalDefault, &plan_controls, &front);
    trigger_inputs.trigger = Some(AfTrigger::Start);
    let request = plan_request(&trigger_inputs);
    assert!(!public_entries(&request.plan).contains_key("ACAMERA_CONTROL_AF_TRIGGER"));
    assert_eq!(request.notes, vec![PlanNote::FocusIgnoredOnFixedLens]);
}

#[test]
fn every_bracket_frame_has_its_own_exposure_and_the_stream_lever_for_every_mechanism() {
    let camera = main_camera();
    // Базовый план — вендорский AE с блокировкой: в кадрах серии он обязан смениться ручным сенсором.
    let base = controls(
        ExposurePlan::VendorAuto {
            compensation_steps: 6,
            lock: AeAfLock::Locked,
            region: Some(region(0.5, 0.5, 0.1)),
        },
        FocusPlan::Manual {
            distance: FocusDistance::from_diopters(1.0).unwrap(),
        },
    );
    let bracket = bracket_of(&[(2, 100), (8, 400), (40, 1600)]);
    for mechanism in every_mechanism() {
        let repeating = plan_of(mechanism, &base, &camera).plan;
        let requests = plan_bracket(&inputs(mechanism, &base, &camera), &bracket);
        assert_eq!(requests.len(), 3);
        for (index, (request, expected)) in requests.iter().zip(bracket.frames()).enumerate() {
            let entries = public_entries(&request.plan);
            assert_eq!(
                entries["ACAMERA_CONTROL_AE_MODE"],
                &bytes(tags::AE_MODE_OFF),
                "кадр {index}"
            );
            assert_eq!(
                entries["ACAMERA_SENSOR_EXPOSURE_TIME"],
                &EntryValue::Int64s(vec![
                    i64::try_from(expected.exposure_time.duration().as_nanos()).unwrap()
                ]),
                "кадр {index}"
            );
            assert_eq!(
                entries["ACAMERA_SENSOR_SENSITIVITY"],
                &EntryValue::Int32s(vec![i32::try_from(expected.iso.value()).unwrap()]),
                "кадр {index}"
            );
            assert!(
                !entries.contains_key("ACAMERA_CONTROL_AE_LOCK"),
                "ключи вендорского AE ушли"
            );
            assert!(!entries.contains_key("ACAMERA_CONTROL_AF_TRIGGER"));
            // Фокус из базового плана сохранён.
            assert_eq!(entries["ACAMERA_LENS_FOCUS_DISTANCE"], &EntryValue::Floats(vec![1.0]));
            // Рычаг частоты и вендорские ключи — такие же, как у повторяющегося запроса.
            assert_eq!(
                has_vendor_pair(&request.plan),
                has_vendor_pair(&repeating),
                "кадр {index}"
            );
            assert_eq!(has_ae_range(&request.plan), has_ae_range(&repeating), "кадр {index}");
            assert_eq!(request.plan.template, repeating.template);
        }
    }
    // Кадр с выдержкой 40 мс при 60 к/с: длительность кадра растёт до выдержки.
    let long = &plan_bracket(&inputs(camx_60(), &base, &camera), &bracket)[2];
    assert_eq!(
        public_entries(&long.plan)["ACAMERA_SENSOR_FRAME_DURATION"],
        &EntryValue::Int64s(vec![40_000_000])
    );
}
