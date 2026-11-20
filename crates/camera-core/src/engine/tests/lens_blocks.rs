//! Блокировка роли после провала смены: явная просьба (тап, переворот) пробует снова, плавный
//! жест блок уважает; провал возврата откатывает опору гистерезиса вместе с целью.

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::fixtures::camera_id;
use camera_contract::ids::{LensFacing, LensRole};

use super::lens_support::{LensRig, ultra_wide_id};
use super::*;
use crate::backend::OpenFailure;

fn fail_next_open(lens: &mut LensRig) {
    lens.rig.fake.push_open_result(Err(OpenFailure::Unavailable {
        reason: CameraUnavailableReason::DeviceFailure,
    }));
}

fn failed_switch_events(events: &[CameraEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, CameraEvent::LensSwitchFailed { .. }))
        .count()
}

/// Стенд: основная стримит, переворот на фронт провалился, цель вернулась на основную.
fn after_failed_front_flip() -> (LensRig, usize) {
    let mut lens = LensRig::streaming_on_wide();
    fail_next_open(&mut lens);
    lens.intent_discrete(LensFacing::Front, 1.0);
    let failures = failed_switch_events(&lens.rig.fresh_events());
    assert_eq!(failures, 1, "первый переворот провалился");
    lens.frames(1);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));
    (lens, failures)
}

#[test]
fn discrete_flip_to_the_blocked_front_is_retried_and_fails_again_with_a_second_event() {
    let (mut lens, _) = after_failed_front_flip();
    lens.rig.fake.take_calls();
    fail_next_open(&mut lens);

    lens.intent_discrete(LensFacing::Front, 1.0);

    assert_eq!(
        lens.open_cameras(),
        vec!["1", "2"],
        "фронт пробуется снова, затем возврат"
    );
    assert_eq!(
        failed_switch_events(&lens.rig.fresh_events()),
        1,
        "экран снова получает откат"
    );
    let status = lens.rig.engine.status().lens;
    assert_eq!(status.counters.switches_failed, 2);
    assert_eq!(status.blocked.map(|block| block.role), Some(LensRole::Front));
}

#[test]
fn discrete_retry_that_succeeds_clears_the_block_and_shows_the_front() {
    let (mut lens, _) = after_failed_front_flip();
    lens.intent_discrete(LensFacing::Front, 1.0);
    lens.frames(1);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("1")));
    assert!(lens.rig.engine.status().lens.blocked.is_none());
}

#[test]
fn continuous_intents_in_the_blocked_range_make_no_backend_calls_and_keep_the_block() {
    let (mut lens, _) = after_failed_front_flip();
    lens.rig.fake.take_calls();
    lens.rig.fresh_events(); // событие возврата кадра прежней камеры не относится к проверке
    for _ in 0..5 {
        lens.intent(LensFacing::Front, 1.0);
    }
    assert!(lens.rig.fake.take_calls().is_empty(), "колесо не зацикливает повторы");
    assert!(lens.rig.fresh_events().is_empty());
    assert_eq!(
        lens.rig.engine.status().lens.blocked.map(|block| block.role),
        Some(LensRole::Front)
    );
}

#[test]
fn continuous_intent_selecting_another_role_clears_the_block() {
    let (mut lens, _) = after_failed_front_flip();
    lens.intent(LensFacing::Back, 1.2);
    assert!(lens.rig.engine.status().lens.blocked.is_none());
}

#[test]
fn failed_switch_from_a_back_lens_rolls_the_hysteresis_anchor_back_to_it() {
    let mut lens = LensRig::streaming_on_wide();
    fail_next_open(&mut lens);
    lens.intent(LensFacing::Back, 0.6); // ширик провалился, цель вернулась на основную
    lens.frames(1);
    lens.intent_discrete(LensFacing::Front, 1.0); // блок ширика снят намерением другой роли
    lens.frames(1);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("1")));
    lens.rig.fake.take_calls();

    // Опора — основная (на ней стояли), а не провалившийся ширик: 1,02 даёт основную.
    lens.intent(LensFacing::Back, 1.02);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));
}

#[test]
fn failed_switch_from_front_rolls_the_anchor_back_to_the_last_streaming_back_lens() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent_discrete(LensFacing::Front, 1.0);
    lens.frames(1);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("1")));
    fail_next_open(&mut lens);
    lens.intent_discrete(LensFacing::Back, 0.6); // ширик провалился, цель вернулась на фронт
    lens.frames(1);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("1")));
    lens.rig.fake.take_calls();

    // Опора — основная (стримила последней), а не ширик, который не открылся.
    lens.intent(LensFacing::Back, 1.02);
    assert_eq!(lens.rig.fake.open_camera(), Some(camera_id("2")));
    assert_ne!(lens.rig.fake.open_camera(), Some(ultra_wide_id()));
}
