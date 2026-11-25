//! Замеры предвспышки на стороне AE: линейные замеры по кадрам без светодиода и с фонариком, их ярлыки по
//! результату самого кадра, недопуск светодиодного света в контроллер видоискателя, расписание статистики.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::flash::{
    FlashDirective, FlashFramePlan, FlashModeEcho, FlashResult, FlashShare, FlashShotId, FlashState, PreflashLight,
    PreflashSamples,
};
use camera_contract::three_a::RequestOrigin;

use super::test_support::*;
use super::*;
use scene_estimation::exposure::{CompletedExposureMeasurement, ExposureMeasurement};

const TORCH: FlashResult = FlashResult {
    mode: Some(FlashModeEcho::Torch),
    state: Some(FlashState::Fired),
};
const PARTIAL: FlashResult = FlashResult {
    mode: Some(FlashModeEcho::Single),
    state: Some(FlashState::Partial),
};
const SINGLE_FIRED: FlashResult = FlashResult {
    mode: Some(FlashModeEcho::Single),
    state: Some(FlashState::Fired),
};

/// Кадр со светом `flash`: план, затем приём статистики этого кадра (как после fence слота).
fn step_lit(harness: &mut Harness, exposure: Exposure, flash: FlashResult, scene: &AwbStatistics) -> ExposureFramePlan {
    let origin = if flash == SINGLE_FIRED {
        RequestOrigin::FlashStill { shot: FlashShotId(1) }
    } else {
        RequestOrigin::Repeating
    };
    let passport = flash_frame_at(&harness.stream, harness.next_index, Some(exposure), origin, flash);
    harness.next_index += 1;
    let plan = harness.runner.plan_frame(&passport);
    harness
        .runner
        .ingest_statistics(plan.stream, identity_of(&passport), &facts_of(&passport), scene);
    plan
}

fn samples(harness: &Harness) -> PreflashSamples {
    harness
        .proposals
        .preflash_newer_than(None)
        .expect("замеры предвспышки опубликованы")
        .1
}

fn publish_flash(harness: &Harness, flash: FlashDirective) {
    harness.directives.publish(ExposureDirective {
        flash,
        ..ExposureDirective::DEFAULT
    });
}

#[test]
fn ambient_and_torch_frames_become_separate_linear_samples_labelled_by_their_own_result() {
    let mut harness = Harness::new();
    step_lit(
        &mut harness,
        UNIT_EXPOSURE,
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    let ambient_only = samples(&harness);
    assert!(ambient_only.ambient.is_some() && ambient_only.torch.is_none());

    step_lit(&mut harness, UNIT_EXPOSURE, TORCH, &uniform_scene(0.3));
    let both = samples(&harness);
    let ambient = both.ambient.expect("замер без фонарика остался в ящике");
    let torch = both.torch.expect("замер с фонариком");
    assert_eq!(
        (ambient.light, torch.light),
        (PreflashLight::Ambient, PreflashLight::Torch)
    );
    // Линейно: произведение кадра × цель / среднее. Кадр 1 с·ISO: 0,075 / 0,1 и 0,075 / 0,3.
    assert_close_ev(ambient.mid_gray_product.seconds_iso(), 0.75, 0.01, "без фонарика");
    assert_close_ev(torch.mid_gray_product.seconds_iso(), 0.25, 0.01, "с фонариком");
    assert_eq!(torch.frame_product.seconds_iso(), 1.0);
    assert_eq!(torch.measured_on, CameraTag::of(&harness.stream.camera));
    assert!(
        torch.frame > ambient.frame,
        "метки кадров разные, по каждому свой замер"
    );
}

#[test]
fn the_same_scene_at_half_and_full_exposure_gives_the_same_mid_gray_product() {
    let mut harness = Harness::new();
    // Вдвое меньшая выдержка — вдвое меньше линейный свет кадра (то же место, та же сцена).
    step_lit(
        &mut harness,
        exposure_product(1.0),
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    let full = samples(&harness).ambient.unwrap();
    step_lit(
        &mut harness,
        exposure_product(0.5),
        FlashResult::UNREPORTED,
        &uniform_scene(0.05),
    );
    let half = samples(&harness).ambient.unwrap();
    assert_ne!(full.frame, half.frame);
    assert_close_ev(
        half.mid_gray_product.seconds_iso(),
        full.mid_gray_product.seconds_iso(),
        0.01,
        "линейность по выдержке",
    );
}

#[test]
fn lit_frames_are_measured_for_the_flash_but_never_move_the_viewfinder_controller() {
    let mut harness = Harness::new();
    harness.steps(4, Some(UNIT_EXPOSURE), &uniform_scene(0.1));
    let before = harness.runner.diagnostics();
    let target_before = harness.latest_target();

    for _ in 0..4 {
        step_lit(&mut harness, UNIT_EXPOSURE, TORCH, &uniform_scene(0.5));
    }

    let after = harness.runner.diagnostics();
    assert_eq!(after.counters.measurements_withheld_for_led_light, 4);
    assert_eq!(
        after.counters.accepted, before.counters.accepted,
        "замеры светодиодных кадров контроллер не принимал"
    );
    assert_eq!(
        harness.latest_target(),
        target_before,
        "цель видоискателя не сдвинулась к свету фонарика"
    );
    assert!(samples(&harness).torch.is_some(), "а вспышке они достались");
    // Обычный кадр после предвспышки снова питает контроллер.
    harness.step(Some(UNIT_EXPOSURE), &uniform_scene(0.1));
    assert!(harness.runner.diagnostics().counters.accepted > before.counters.accepted);
}

#[test]
fn partial_and_single_flash_frames_give_neither_a_sample_nor_controller_input() {
    let mut harness = Harness::new();
    harness.steps(2, Some(UNIT_EXPOSURE), &uniform_scene(0.1));
    let published = harness.runner.diagnostics().counters.preflash_samples_published;
    let accepted = harness.runner.diagnostics().counters.accepted;
    step_lit(&mut harness, UNIT_EXPOSURE, PARTIAL, &uniform_scene(0.6));
    step_lit(&mut harness, UNIT_EXPOSURE, SINGLE_FIRED, &uniform_scene(0.6));
    let counters = harness.runner.diagnostics().counters;
    assert_eq!(counters.preflash_samples_published, published);
    assert_eq!(counters.accepted, accepted);
    assert_eq!(counters.measurements_withheld_for_led_light, 2);
    assert!(samples(&harness).torch.is_none());
}

#[test]
fn active_preflash_demands_statistics_on_every_frame_and_inactive_does_not() {
    let mut harness = Harness::new();
    harness.steps(6, Some(UNIT_EXPOSURE), &uniform_scene(0.1));
    // Оценка есть, холодный старт закончен: обычно статистику ведёт расписание.
    assert_eq!(
        step_lit(
            &mut harness,
            UNIT_EXPOSURE,
            FlashResult::UNREPORTED,
            &uniform_scene(0.1)
        )
        .statistics,
        StatisticsDemand::Periodic
    );
    publish_flash(
        &harness,
        FlashDirective {
            preflash: PreflashMetering::Active { shot: FlashShotId(1) },
            planned: None,
        },
    );
    for _ in 0..3 {
        assert_eq!(
            step_lit(&mut harness, UNIT_EXPOSURE, TORCH, &uniform_scene(0.3)).statistics,
            StatisticsDemand::EveryFrame
        );
    }
    publish_flash(&harness, FlashDirective::INACTIVE);
    // Покадровая лента вокруг кадра со вспышкой: после предвспышки ещё ровно столько кадров «каждый кадр»,
    // потом снова расписание.
    for frame in 0..POST_FLASH_EVERY_FRAME_FRAMES {
        assert_eq!(
            step_lit(
                &mut harness,
                UNIT_EXPOSURE,
                FlashResult::UNREPORTED,
                &uniform_scene(0.1)
            )
            .statistics,
            StatisticsDemand::EveryFrame,
            "кадр {frame} после предвспышки"
        );
    }
    assert_eq!(
        step_lit(
            &mut harness,
            UNIT_EXPOSURE,
            FlashResult::UNREPORTED,
            &uniform_scene(0.1)
        )
        .statistics,
        StatisticsDemand::Periodic
    );
}

#[test]
fn flash_plans_are_forwarded_to_the_still_engine_once_each() {
    let mut harness = Harness::new();
    let plan = FlashFramePlan {
        shot: FlashShotId(3),
        ambient_frame: camera_contract::capture_result::SensorTimestamp::from_nanos(1),
        mid_gray_product: ExposureProduct::from_seconds_iso(0.2).unwrap(),
        flash_share: FlashShare::new(0.6),
    };
    let with_plan = FlashDirective {
        preflash: PreflashMetering::Inactive,
        planned: Some(plan),
    };
    publish_flash(&harness, with_plan);
    step_lit(
        &mut harness,
        UNIT_EXPOSURE,
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    assert_eq!(harness.runner.drain_flash_plans(), vec![plan]);
    // То же указание новым поколением (обратная связь `applied` обновляет его постоянно): повторно не уходит.
    publish_flash(&harness, with_plan);
    step_lit(
        &mut harness,
        UNIT_EXPOSURE,
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    assert!(harness.runner.drain_flash_plans().is_empty());
    let next = FlashFramePlan {
        shot: FlashShotId(4),
        ..plan
    };
    publish_flash(
        &harness,
        FlashDirective {
            planned: Some(next),
            ..with_plan
        },
    );
    step_lit(
        &mut harness,
        UNIT_EXPOSURE,
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    assert_eq!(harness.runner.drain_flash_plans(), vec![next]);
}

#[test]
fn ambient_level_is_green_per_unit_product_of_the_ambient_frame_only() {
    let mut harness = Harness::new();
    step_lit(
        &mut harness,
        exposure_product(2.0),
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    let levels = harness.runner.drain_ambient_levels();
    let [level] = levels.as_slice() else {
        panic!("ожидался один уровень света сцены: {levels:?}");
    };
    // Зелёный 0,1 при произведении 2 с·ISO.
    assert!(
        (level.green_per_product - 0.05).abs() < 1e-3,
        "{}",
        level.green_per_product
    );
    assert!(harness.runner.drain_ambient_levels().is_empty(), "отдаётся один раз");
    // Кадр с фонариком уровень сцены не меняет.
    step_lit(&mut harness, UNIT_EXPOSURE, TORCH, &uniform_scene(0.6));
    assert!(harness.runner.drain_ambient_levels().is_empty());
}

#[test]
fn every_ambient_level_is_kept_per_frame_not_only_the_latest() {
    let mut harness = Harness::new();
    for scene in [0.1, 0.2, 0.4] {
        step_lit(
            &mut harness,
            UNIT_EXPOSURE,
            FlashResult::UNREPORTED,
            &uniform_scene(scene),
        );
    }
    let levels = harness.runner.drain_ambient_levels();
    assert_eq!(levels.len(), 3, "по уровню на каждый кадр без светодиода");
    assert!(levels.windows(2).all(|pair| pair[0].frame < pair[1].frame));
    let greens: Vec<f64> = levels.iter().map(|level| level.green_per_product).collect();
    assert!(
        (greens[0] - 0.1).abs() < 1e-3 && (greens[2] - 0.4).abs() < 1e-3,
        "{greens:?}"
    );
}

#[test]
fn a_measurement_with_zero_luminance_gives_no_linear_sample_but_is_counted() {
    let harness = Harness::new();
    let mut meter = PreflashMeter::default();
    let done_with_luminance = |mean_luminance: f32, nanos: i64| CompletedExposureMeasurement {
        frame: FrameTag {
            stream_generation: 0,
            sequence: 0,
            sensor_timestamp_nanos: nanos,
        },
        area: camera_contract::auto_exposure::MeteredArea::WholeFrame,
        frame_product: ExposureProduct::from_seconds_iso(1.0).unwrap(),
        outcome: Ok(ExposureMeasurement {
            required_product: ExposureProduct::from_seconds_iso(1.0).unwrap(),
            quality: camera_contract::auto_exposure::MeasurementQuality::Measured,
            scene_log2_luminance: 0.0,
            clipped_fraction: 0.0,
            dark_fraction: 0.0,
            used_weight: 1.0,
            protection_ev: 0.0,
            mean_luminance,
            mean_green_level: 0.1,
        }),
    };
    let target = ExposureTarget::new(0.075).expect("цель");
    let black = meter.complete(&done_with_luminance(0.0, 10), &harness.stream, target, 0.0);
    assert!(black.publish.is_none(), "нулевая яркость замера не даёт");
    assert_eq!((meter.not_computable, meter.samples_published), (1, 0));
    // Нормальный кадр счётчик не двигает и замер даёт.
    let normal = meter.complete(&done_with_luminance(0.1, 20), &harness.stream, target, 0.0);
    assert!(normal.publish.is_some());
    assert_eq!((meter.not_computable, meter.samples_published), (1, 1));
}

#[test]
fn outside_preflash_samples_are_stored_without_waking_the_engine_and_during_it_they_wake_it() {
    let mut harness = Harness::new();
    // Получателя толчка нет: каждый толчок учитывается как «пропущенный» — так видно, был ли он.
    let nudges = |harness: &Harness| harness.proposals.refresh_signal().missed_before_bind();
    let before = nudges(&harness);
    step_lit(
        &mut harness,
        UNIT_EXPOSURE,
        FlashResult::UNREPORTED,
        &uniform_scene(0.1),
    );
    assert!(samples(&harness).ambient.is_some(), "замер без фонарика лежит в ящике");
    assert_eq!(nudges(&harness), before, "вне предвспышки движок не будят");

    publish_flash(
        &harness,
        FlashDirective {
            preflash: PreflashMetering::Active { shot: FlashShotId(2) },
            planned: None,
        },
    );
    step_lit(&mut harness, UNIT_EXPOSURE, TORCH, &uniform_scene(0.3));
    assert!(samples(&harness).torch.is_some());
    assert!(nudges(&harness) > before, "во время предвспышки замер будит движок");
}
