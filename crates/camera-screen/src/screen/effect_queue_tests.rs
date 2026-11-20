//! Очередь эффектов: порядок, предел, учёт переполнения, пустая очередь.

use shell_contract::haptics::HapticPattern;

use super::effect_queue::{EffectQueue, PushOutcome};
use crate::effect::Effect;

#[test]
fn drain_returns_effects_in_order_and_empties_the_queue() {
    let mut queue = EffectQueue::new(4);
    assert_eq!(queue.push(Effect::OpenSystemGallery), PushOutcome::Queued);
    assert_eq!(
        queue.push(Effect::Haptic(HapticPattern::MediumImpact)),
        PushOutcome::Queued
    );
    assert_eq!(
        queue.drain(),
        [Effect::OpenSystemGallery, Effect::Haptic(HapticPattern::MediumImpact)]
    );
    assert!(queue.drain().is_empty(), "второй вызов подряд — пусто");
}

#[test]
fn overflow_drops_the_new_effect_and_counts_it() {
    let mut queue = EffectQueue::new(1);
    assert_eq!(queue.push(Effect::OpenSystemGallery), PushOutcome::Queued);
    assert_eq!(
        queue.push(Effect::Haptic(HapticPattern::MediumImpact)),
        PushOutcome::Overflowed
    );
    assert_eq!(queue.overflowed(), 1);
    assert_eq!(queue.queued(), 1);
    // Порядок и содержимое принятого не пострадали.
    assert_eq!(queue.drain(), [Effect::OpenSystemGallery]);
    // После опустошения очередь снова принимает.
    assert_eq!(queue.push(Effect::OpenSystemGallery), PushOutcome::Queued);
}

#[test]
fn draining_an_empty_queue_allocates_nothing_and_a_full_drain_keeps_every_effect() {
    let mut queue = EffectQueue::new(3);
    assert_eq!(
        queue.drain().capacity(),
        0,
        "пустой опрос каждый кадр не выделяет память"
    );
    for _ in 0..3 {
        assert_eq!(queue.push(Effect::OpenSystemGallery), PushOutcome::Queued);
    }
    assert_eq!(queue.drain().len(), 3, "хранилище отдано целиком");
    assert!(queue.drain().is_empty());
    assert_eq!(queue.queued(), 3, "счётчик принятых переживает опустошение");
}

#[test]
fn a_newer_lens_intent_replaces_the_pending_one_even_in_a_full_queue() {
    use camera_contract::ids::LensFacing;
    use camera_contract::zoom::LensIntent;

    let lens = |value: f32| {
        Effect::Lens(LensIntent {
            facing: LensFacing::Back,
            ratio: crate::test_support::ratio(value),
            gesture: camera_contract::zoom::ZoomGesture::Continuous,
        })
    };
    // Очередь на один эффект уже занята намерением: итоговое значение колеса не должно потеряться на переполнении.
    let mut queue = EffectQueue::new(1);
    assert_eq!(queue.push(lens(1.2)), PushOutcome::Queued);
    assert_eq!(queue.push(lens(1.7)), PushOutcome::Queued);
    assert_eq!(queue.push(lens(2.0)), PushOutcome::Queued);
    assert_eq!(queue.overflowed(), 0);
    assert_eq!(queue.drain(), [lens(2.0)], "осталось только последнее намерение");
    // Чужой эффект на полной очереди по-прежнему отбрасывается и считается.
    assert_eq!(queue.push(lens(1.0)), PushOutcome::Queued);
    assert_eq!(queue.push(Effect::OpenSystemGallery), PushOutcome::Overflowed);
}

#[test]
fn lens_intents_do_not_replace_other_kinds_and_keep_their_place_in_order() {
    use camera_contract::ids::LensFacing;
    use camera_contract::zoom::LensIntent;

    let lens = |value: f32| {
        Effect::Lens(LensIntent {
            facing: LensFacing::Back,
            ratio: crate::test_support::ratio(value),
            gesture: camera_contract::zoom::ZoomGesture::Continuous,
        })
    };
    let mut queue = EffectQueue::new(4);
    queue.push(Effect::OpenSystemGallery);
    queue.push(lens(1.1));
    queue.push(Effect::Haptic(HapticPattern::SelectionTick));
    queue.push(lens(1.9));
    assert_eq!(
        queue.drain(),
        [
            Effect::OpenSystemGallery,
            lens(1.9),
            Effect::Haptic(HapticPattern::SelectionTick)
        ]
    );
}
