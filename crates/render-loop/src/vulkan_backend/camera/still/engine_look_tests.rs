//! Вид картинки в снимке (решение 178, инвариант 3): проявление берёт значения вида ИЗ ПЛАНА снимка, то есть
//! на момент нажатия, и для обычного кадра, и для кадра со вспышкой. Видоискатель сюда не заходит вообще.

use look_contract::{LookKnob, LookSettings};

use super::entry::Stage;
use super::test_support::{EngineRig, MAIN, StreamRig, engine_rig, ms, plan};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Вид, по которому легко узнать «свой»: у каждой ручки своё значение.
fn distinctive_look() -> LookSettings {
    LookSettings {
        contrast: knob(0.25),
        saturation: knob(-0.5),
        warmth: knob(0.75),
        ..LookSettings::NEUTRAL
    }
}

/// Значения вида, с которыми проявляется снимок, дошедший до ожидания цели кодера.
fn developed_look(rig: &EngineRig) -> LookSettings {
    match &rig.engine.queue[0].stage {
        Stage::AwaitingTarget { inputs, .. } => inputs.params.look().clone(),
        _ => panic!("снимок должен ждать цель кодера после Secured"),
    }
}

fn rig_with_shown_frames() -> EngineRig {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(3);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let feed = rig.stream.feed_for(&frames, 0);
    rig.engine.ingest_feed(feed);
    let timings: Vec<PastPresentation> = (0..3)
        .map(|index| PastPresentation {
            present_id: index,
            frame_index: u64::from(index),
            actual_present: ms(10 * i64::from(index)),
        })
        .collect();
    rig.engine.ingest_presentations(&timings);
    rig
}

#[test]
fn a_shown_frame_is_developed_with_the_look_from_its_plan() {
    let mut rig = rig_with_shown_frames();
    let mut still_plan = plan(1, 15);
    still_plan.look = distinctive_look();
    rig.begin(still_plan);
    rig.run(3);

    assert_eq!(developed_look(&rig), distinctive_look());
}

#[test]
fn two_stills_in_a_row_each_keep_their_own_look() {
    // Правка вида между нажатиями не должна просочиться в уже поставленный снимок (инвариант 3).
    let mut rig = rig_with_shown_frames();
    let mut first = plan(1, 15);
    first.look = distinctive_look();
    let mut second = plan(2, 15);
    second.look = LookSettings::NEUTRAL;
    rig.begin(first);
    rig.begin(second);
    rig.run(6);

    let looks: Vec<LookSettings> = rig
        .engine
        .queue
        .iter()
        .filter_map(|entry| match &entry.stage {
            Stage::AwaitingTarget { inputs, .. } => Some(inputs.params.look().clone()),
            _ => None,
        })
        .collect();
    assert_eq!(looks, vec![distinctive_look(), LookSettings::NEUTRAL]);
}

#[test]
fn a_still_without_a_look_keeps_the_neutral_look() {
    let mut rig = rig_with_shown_frames();
    rig.begin(plan(1, 15));
    rig.run(3);

    assert!(developed_look(&rig).is_neutral());
}
