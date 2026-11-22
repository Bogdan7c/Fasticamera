//! Предвспышка: какой замер без фонарика годится, перемеры и их сроки (14d, факты камеры 3 и зонда).

use camera_contract::auto_exposure::MeasurementQuality;
use camera_contract::flash::{FlashStillFailure, PreflashLight, PreflashSamples};

use super::flash_support::*;
use crate::flash_status::{FlashPhase, FlashStillReport};

fn assert_preflash(flash: &FlashRig, remeasures: u32) {
    assert_eq!(flash.phase(), FlashPhase::Preflash { shot: SHOT, remeasures });
}

#[test]
fn ambient_frame_exposed_after_the_torch_request_is_ignored_and_the_plan_uses_the_pre_request_one() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let requested = flash.repeating_product();
    let torch = sample(PreflashLight::Torch, 28, requested, 4.0, MeasurementQuality::Measured);

    // Камера 3: первый (частично освещённый) кадр после запроса пришёл «обычным» и дал очень светлый
    // замер без фонарика. Он снят ПОСЛЕ запроса: брать его как свет сцены нельзя, вклад фонарика ушёл бы в 0.
    let partially_lit = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame() + 5,
        requested,
        1.0,
        MeasurementQuality::Measured,
    );
    flash.publish_samples(PreflashSamples {
        ambient: Some(partially_lit),
        torch: Some(torch),
    });
    assert_preflash(&flash, 0);

    // Замер кадра ДО запроса дошёл позже (запаздывание замера): по нему и считается план.
    let before_request = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame(),
        requested,
        8.0,
        MeasurementQuality::Measured,
    );
    flash.publish_samples(PreflashSamples {
        ambient: Some(before_request),
        torch: Some(torch),
    });
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
    let planned = flash.directive().flash.planned.expect("план опубликован");
    assert_eq!(planned.ambient_frame, before_request.frame);
    let expected_product = 1.0 / (1.0 / 8.0 + 1.87 / 8.0);
    assert!(
        (planned.mid_gray_product.seconds_iso() - expected_product).abs() < 1e-9,
        "план по свету сцены 8, а не по частично освещённому кадру: {}",
        planned.mid_gray_product.seconds_iso()
    );
}

#[test]
fn a_later_ambient_frame_never_replaces_the_pre_request_one() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let requested = flash.repeating_product();
    let before_request = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame(),
        requested,
        8.0,
        MeasurementQuality::Measured,
    );
    flash.publish_samples(PreflashSamples {
        ambient: Some(before_request),
        torch: None,
    });
    // Позже приходит частично освещённый «обычный» кадр, затем замер с фонариком.
    let partially_lit = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame() + 5,
        requested,
        1.0,
        MeasurementQuality::Measured,
    );
    flash.publish_samples(PreflashSamples {
        ambient: Some(partially_lit),
        torch: None,
    });
    flash.publish_samples(PreflashSamples {
        ambient: Some(partially_lit),
        torch: Some(sample(
            PreflashLight::Torch,
            28,
            requested,
            4.0,
            MeasurementQuality::Measured,
        )),
    });
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
    let planned = flash.directive().flash.planned.expect("план опубликован");
    assert_eq!(planned.ambient_frame, before_request.frame);
}

#[test]
fn without_any_result_before_the_request_no_ambient_sample_can_be_trusted() {
    // Результатов повторяющегося запроса ещё не было: метки «до запроса» нет, любой замер без фонарика
    // мог быть снят уже под фонариком.
    let mut flash = FlashRig::unmeasured();
    flash.capture(SHOT);
    let requested = flash.repeating_product();
    flash.publish_samples(PreflashSamples {
        ambient: Some(sample(
            PreflashLight::Ambient,
            3,
            requested,
            8.0,
            MeasurementQuality::Measured,
        )),
        torch: Some(sample(
            PreflashLight::Torch,
            28,
            requested,
            4.0,
            MeasurementQuality::Measured,
        )),
    });
    assert_preflash(&flash, 0);
}

fn clipped_torch(frame: i64, frame_product: f64) -> camera_contract::flash::PreflashSample {
    sample(
        PreflashLight::Torch,
        frame,
        frame_product,
        1.0,
        MeasurementQuality::ClippedHighlights,
    )
}

#[test]
fn each_remeasure_extends_the_preflash_deadline_by_one_timeout() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let first = flash.repeating_product();
    flash.rig.advance(ms(400));
    flash.publish_samples(PreflashSamples {
        ambient: Some(sample(
            PreflashLight::Ambient,
            flash.pre_request_frame(),
            first,
            8.0,
            MeasurementQuality::Measured,
        )),
        torch: Some(clipped_torch(28, first)),
    });
    assert_preflash(&flash, 1);
    flash.rig.fresh_events();
    // Прежний срок (500 мс от запроса) прошёл, но перемер продлил его на 500 мс: до 1000 мс жив.
    flash.rig.advance(ms(599));
    assert_preflash(&flash, 1);
    flash.rig.advance(ms(1));
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(
        reports,
        vec![FlashStillReport::Failed {
            shot: SHOT,
            failure: FlashStillFailure::PreflashNotMeasured
        }]
    );
}

#[test]
fn clipped_sample_at_the_exposure_floor_plans_at_once_instead_of_remeasuring() {
    // Яркая сцена: AE стоит на нижней границе камеры, «на 3 EV меньше» даёт ту же выдержку и ISO.
    let mut flash = FlashRig::measured(1e-9);
    flash.capture(SHOT);
    let requested = flash.repeating_product();
    flash.publish_samples(PreflashSamples {
        ambient: Some(sample(
            PreflashLight::Ambient,
            flash.pre_request_frame(),
            requested,
            8.0,
            MeasurementQuality::Measured,
        )),
        torch: Some(clipped_torch(28, requested)),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Gap { shot: SHOT },
        "перемер не нужен: уменьшать некуда"
    );
    let counters = flash.rig.engine.status().three_a.flash.counters;
    assert_eq!(counters.preflash_remeasures, 0);
    assert_eq!(counters.planned_from_clipped_torch, 1);
}

#[test]
fn plan_from_a_still_clipped_torch_after_the_budget_is_counted_and_a_good_one_is_not() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let first = flash.repeating_product();
    let ambient = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame(),
        first,
        8.0,
        MeasurementQuality::Measured,
    );
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(clipped_torch(28, first)),
    });
    assert_preflash(&flash, 1);
    let lowered = flash.repeating_product();
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(clipped_torch(40, lowered)),
    });
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT }, "снимок не отменяется");
    assert_eq!(
        flash
            .rig
            .engine
            .status()
            .three_a
            .flash
            .counters
            .planned_from_clipped_torch,
        1
    );

    // Годный замер счётчик не двигает.
    let mut good = FlashRig::measured(8.0);
    good.capture(SHOT);
    good.publish_good_samples(8.0, 4.0);
    assert_eq!(good.phase(), FlashPhase::Gap { shot: SHOT });
    assert_eq!(
        good.rig
            .engine
            .status()
            .three_a
            .flash
            .counters
            .planned_from_clipped_torch,
        0
    );
}
