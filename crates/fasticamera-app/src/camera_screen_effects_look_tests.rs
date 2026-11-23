//! Вид картинки в исполнителе эффектов экрана (решение 178, инвариант 3): намерение → команда рендеру; запрос
//! снимка получает значения вида НА МОМЕНТ обработки нажатия, а правка сразу после нажатия запрос не меняет.

use look_contract::{LookIntent, LookSettings};

use super::*;
use crate::look_service::LookHandle;
use crate::look_test_support::{StubPersistence, contrast_look, handle_with, wheels_look};
use crate::viewfinder_control::RenderAdjustment;

fn behavior_with_library(stub: &StubPersistence) -> (Behavior, LookHandle) {
    let handle = handle_with(stub.clone());
    (
        Behavior {
            look: Some(handle.clone()),
            ..Behavior::default()
        },
        handle,
    )
}

fn captured_looks(rig: &Rig) -> Vec<LookSettings> {
    rig.capture.looks.lock().expect("замок").clone()
}

#[test]
fn a_look_effect_reaches_the_render_as_set_look_and_asks_for_a_file_write() {
    let stub = StubPersistence::accepting();
    let (behavior, handle) = behavior_with_library(&stub);
    let (mut executor, rig) = build_executor(behavior);

    let feedback = executor.execute(vec![Effect::Look(LookIntent::SetLook(contrast_look(0.5)))]);

    assert!(feedback.is_empty());
    let sent: Vec<_> = rig
        .render
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetLook(look)) => Some((*look).clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![contrast_look(0.5)]);
    assert_eq!(handle.active_settings(), contrast_look(0.5));
    assert_eq!(stub.requested().len(), 1);
    let counters = executor.counters().look;
    assert_eq!((counters.look_changed, counters.render.delivered), (1, 1));
}

#[test]
fn the_shot_request_gets_the_look_of_the_press_moment_and_a_later_edit_does_not_change_it() {
    let stub = StubPersistence::accepting();
    let (behavior, handle) = behavior_with_library(&stub);
    let (mut executor, rig) = build_executor(behavior);

    // Порядок эффектов одного кадра: вид A → нажатие → вид B. Нажатие обязано увидеть A.
    executor.execute(vec![
        Effect::Look(LookIntent::SetLook(contrast_look(0.5))),
        Effect::Capture(shot(1)),
        Effect::Look(LookIntent::SetLook(contrast_look(-0.5))),
    ]);
    assert_eq!(captured_looks(&rig), vec![contrast_look(0.5)]);
    assert_eq!(handle.active_settings(), contrast_look(-0.5), "а сам вид уже другой");

    // Следующее нажатие видит уже новое значение.
    executor.execute(vec![Effect::Capture(shot(2))]);
    assert_eq!(captured_looks(&rig), vec![contrast_look(0.5), contrast_look(-0.5)]);
}

#[test]
fn a_press_before_any_look_intent_carries_the_active_preset_values_not_neutral() {
    let stub = StubPersistence::accepting();
    let (behavior, handle) = behavior_with_library(&stub);
    let (mut executor, rig) = build_executor(behavior);

    executor.execute(vec![Effect::Capture(shot(1))]);

    assert_eq!(captured_looks(&rig), vec![handle.active_settings()]);
    assert_ne!(
        captured_looks(&rig)[0],
        LookSettings::NEUTRAL,
        "по умолчанию активен «Как глаз»: запрос несёт его значения"
    );
}

#[test]
fn without_a_library_the_press_carries_the_neutral_look_and_intents_are_counted_as_dropped() {
    let (mut executor, rig) = build_executor(Behavior::default());

    executor.execute(vec![
        Effect::Look(LookIntent::SetLook(contrast_look(0.5))),
        Effect::Capture(shot(1)),
    ]);

    assert_eq!(captured_looks(&rig), vec![LookSettings::NEUTRAL]);
    assert_eq!(executor.counters().look.without_library, 1);
    assert!(rig.render.recorded().is_empty());
}

#[test]
fn a_rejected_look_intent_does_not_change_the_look_of_the_next_shot() {
    let stub = StubPersistence::accepting();
    let (behavior, handle) = behavior_with_library(&stub);
    let (mut executor, rig) = build_executor(behavior);
    executor.execute(vec![Effect::Look(LookIntent::SetLook(contrast_look(0.5)))]);

    executor.execute(vec![
        Effect::Look(LookIntent::OverwritePreset(look_contract::UserPresetId(404))),
        Effect::Capture(shot(1)),
    ]);

    assert_eq!(captured_looks(&rig), vec![contrast_look(0.5)]);
    assert_eq!(executor.counters().look.rejected.not_found, 1);
    assert_eq!(handle.active_settings(), contrast_look(0.5));
}

#[test]
fn write_outcomes_are_polled_on_every_frame_and_counted() {
    let stub = StubPersistence::accepting();
    let (behavior, _handle) = behavior_with_library(&stub);
    let (mut executor, _rig) = build_executor(behavior);
    executor.execute(vec![Effect::Look(LookIntent::SetLook(contrast_look(0.5)))]);
    stub.push_outcome(look_presets::SaveOutcome::Saved);

    assert_eq!(executor.counters().look.writes.saved, 0, "до кадра итог не опрошен");
    executor.begin_frame(MonotonicNanos::from_nanos(16_000_000));

    assert_eq!(executor.counters().look.writes.saved, 1);
}

#[test]
fn the_shot_request_carries_the_color_wheels_of_the_press_moment() {
    let stub = StubPersistence::accepting();
    let (behavior, handle) = behavior_with_library(&stub);
    let (mut executor, rig) = build_executor(behavior);

    // Колёса → нажатие → правка вида без колёс: снимок обязан взять колёса момента нажатия.
    executor.execute(vec![
        Effect::Look(LookIntent::SetLook(wheels_look())),
        Effect::Capture(shot(1)),
        Effect::Look(LookIntent::SetLook(LookSettings::NEUTRAL)),
    ]);

    let looks = captured_looks(&rig);
    assert_eq!(looks, vec![wheels_look()]);
    assert!(!looks[0].wheels.is_neutral(), "в запрос снимка попали колёса");
    assert!(
        handle.active_settings().wheels.is_neutral(),
        "а вид после нажатия уже без колёс"
    );
}
