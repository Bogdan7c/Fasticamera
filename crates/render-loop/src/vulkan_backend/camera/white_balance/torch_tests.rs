//! Белая светодиода по кадрам с фонариком (14d): оценка кадра с фонариком уходит вспышке, а не белой видоискателя.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use super::test_support::{
    directive_of, frame, gray_scene, identity_of, kelvin_of, new_stream, runner, single_colour_scene,
};
use super::*;
use camera_contract::white::WhiteTransition;

#[test]
fn a_torch_frame_gives_the_led_white_without_touching_the_viewfinder_white() {
    let (mut runner, links) = runner();
    links
        .directives
        .publish(directive_of(5000.0, WhiteTransition::Immediate, None));
    let stream = new_stream();
    // Несколько кадров при дневном свете: белая видоискателя стоит.
    for index in 0..6 {
        runner.plan_frame(&frame(&stream, index));
    }
    let applied_before = runner.diagnostics().applied_white;

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
    assert_eq!(after.applied_white, applied_before, "применённая белая не сдвинулась");
    assert_eq!(after.torch_white_estimates, 1);
}

#[test]
fn a_torch_frame_without_gray_candidates_is_counted_and_gives_no_white() {
    let (mut runner, _) = runner();
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
    let (mut runner, _) = runner();
    let first = new_stream();
    let passport = frame(&first, 0);
    let plan = runner.plan_frame(&passport);
    runner.ingest_torch_statistics(plan.stream, identity_of(&passport), &gray_scene(2800.0));
    // Новый поток до того, как вспышка забрала белую: старая чужая.
    runner.plan_frame(&frame(&new_stream(), 1));
    assert!(runner.drain_torch_whites().is_empty());
}
