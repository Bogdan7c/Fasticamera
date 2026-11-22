//! Белая светодиода по кадрам с фонариком (14d): оценка кадра с фонариком уходит вспышке, а не контроллеру AWB.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use super::runner_tests::{
    frame, gray_scene, identity_of, kelvin_of, new_stream, process, runner, single_colour_scene,
};
use super::*;

#[test]
fn a_torch_frame_gives_the_led_white_without_touching_the_viewfinder_white_or_best_estimate() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    // Холодный старт и несколько кадров при дневном свете: белая видоискателя ≈ 5000 К.
    for index in 0..6 {
        process(&mut runner, &frame(&stream, index), &gray_scene(5000.0));
    }
    let daylight = runner.diagnostics();
    let scene_before = daylight.scene_white.expect("белая видоискателя");
    let estimate_before = daylight.last_estimate.expect("оценка видоискателя");

    // Кадр с фонариком: тёплый светодиод (2800 К).
    let torch_frame = frame(&stream, 6);
    let plan = runner.plan_frame(&torch_frame);
    runner.ingest_torch_statistics(plan.stream, identity_of(&torch_frame), &gray_scene(2800.0));

    let whites = runner.drain_torch_whites();
    assert_eq!(whites.len(), 1);
    assert!(
        (kelvin_of(whites[0].white) - 2800.0).abs() < 80.0,
        "{}",
        kelvin_of(whites[0].white)
    );
    assert_eq!(
        whites[0].frame, torch_frame.sensor_timestamp,
        "белая привязана к кадру с фонариком"
    );
    assert!(Arc::ptr_eq(&whites[0].stream, &stream));
    assert!(runner.drain_torch_whites().is_empty(), "отдаётся один раз");

    let after = runner.diagnostics();
    assert_eq!(
        after.scene_white,
        Some(scene_before),
        "белая видоискателя не сдвинулась"
    );
    assert_eq!(
        after.last_estimate,
        Some(estimate_before),
        "лучшая оценка холодного старта не испорчена"
    );
    assert_eq!(after.torch_white_estimates, 1);
    assert_eq!(after.decisions, daylight.decisions, "контроллер AWB оценку не видел");
}

#[test]
fn torch_statistics_are_estimated_even_when_the_awb_schedule_skips_the_frame() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    for index in 0..4 {
        process(&mut runner, &frame(&stream, index), &gray_scene(5000.0));
    }
    // Расписание прореживает кадры после первой оценки: найдём кадр, на котором статистику AWB не просили.
    let mut skipped = None;
    for index in 4..40 {
        let passport = frame(&stream, index);
        let plan = runner.plan_frame(&passport);
        if plan.statistics == StatisticsPlan::Skip {
            skipped = Some((passport, plan));
            break;
        }
    }
    let (passport, plan) = skipped.expect("прореживание пропускает кадры");
    runner.ingest_torch_statistics(plan.stream, identity_of(&passport), &gray_scene(3000.0));
    assert_eq!(runner.drain_torch_whites().len(), 1);
}

#[test]
fn a_torch_frame_without_gray_candidates_is_counted_and_gives_no_white() {
    let (mut runner, _, _) = runner();
    let stream = new_stream();
    let passport = frame(&stream, 0);
    let plan = runner.plan_frame(&passport);
    runner.ingest_torch_statistics(plan.stream, identity_of(&passport), &single_colour_scene());
    assert!(runner.drain_torch_whites().is_empty());
    let diagnostics = runner.diagnostics();
    assert_eq!(
        (diagnostics.torch_white_estimates, diagnostics.torch_white_failures),
        (0, 1)
    );
}

#[test]
fn torch_whites_of_a_previous_stream_are_dropped_with_the_stream() {
    let (mut runner, _, _) = runner();
    let first = new_stream();
    let passport = frame(&first, 0);
    let plan = runner.plan_frame(&passport);
    runner.ingest_torch_statistics(plan.stream, identity_of(&passport), &gray_scene(2800.0));
    // Новый поток до того, как вспышка забрала белую: старая чужая.
    runner.plan_frame(&frame(&new_stream(), 1));
    assert!(runner.drain_torch_whites().is_empty());
}
