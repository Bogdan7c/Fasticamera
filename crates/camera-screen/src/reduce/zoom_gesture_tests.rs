//! Тесты жеста намерения зума (сессия 13, правки): какой `ZoomGesture` уходит движку, с каким направлением
//! строится кадрирование, и что неудача смены объектива не теряется, если активной камеры в фактах нет.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, ZoomGesture};

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::outcome::ActionOutcome;
use crate::test_support::{Screen, at_ms, ratio, zoom_facts, zoom_facts_with_failure};

fn lens_intents(effects: &[Effect]) -> Vec<LensIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Lens(intent) => Some(*intent),
            _ => None,
        })
        .collect()
}

/// Направление в последней команде кадрирования.
fn framing_facing(effects: &[Effect]) -> Option<LensFacing> {
    effects.iter().rev().find_map(|effect| match effect {
        Effect::Viewfinder(intent) => Some(intent.zoom.facing),
        _ => None,
    })
}

fn ratio_of(screen: &Screen) -> f32 {
    screen.state.zoom().ratio().value()
}

/// Прогоняет кадры по 16 мс от `from_ms` и собирает эффекты всех кадров.
fn run_frames(screen: &mut Screen, from_ms: i64, frames: i64) -> Vec<Effect> {
    let mut effects = Vec::new();
    for step in 1..=frames {
        effects.extend(screen.step(ScreenAction::Tick(at_ms(from_ms + step * 16))).effects);
    }
    effects
}

fn framing_count(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Viewfinder(_)))
        .count()
}

#[test]
fn a_stop_tap_publishes_exactly_one_discrete_intent_for_the_whole_glide() {
    let mut screen = Screen::zoomed(LensRole::UltraWide);
    // Упор «1×» (индекс 1) с ширика: ровно одно намерение с коэффициентом упора, переход его не повторяет.
    let started = screen.step(ScreenAction::ZoomStopTapped {
        stop: 1,
        at: at_ms(1000),
    });
    let mut effects = started.effects;
    effects.extend(run_frames(&mut screen, 1000, 25));
    assert!(!screen.state.zoom().is_gliding(), "переход должен завершиться");
    let intents = lens_intents(&effects);
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].gesture, ZoomGesture::Discrete);
    assert!((intents[0].ratio.value() - 1.0).abs() < 1.0e-4, "{:?}", intents[0]);
    assert!(framing_count(&effects) > 3, "кадрирование идёт по шагам перехода");
    assert!((ratio_of(&screen) - 1.0).abs() < 1.0e-4);
}

#[test]
fn a_new_tap_during_the_glide_publishes_one_more_intent_and_the_last_wins() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let first = screen.step(ScreenAction::ZoomStopTapped {
        stop: 0,
        at: at_ms(1000),
    });
    assert_eq!(lens_intents(&first.effects).len(), 1);
    let mut effects = run_frames(&mut screen, 1000, 5);
    assert!(screen.state.zoom().is_gliding(), "переход ещё идёт");
    // Новый тап посреди перехода — на «2×» (индекс 2): одно новое намерение, дальше тишина.
    let second = screen.step(ScreenAction::ZoomStopTapped {
        stop: 2,
        at: at_ms(1100),
    });
    effects.extend(second.effects);
    effects.extend(run_frames(&mut screen, 1100, 25));
    let intents = lens_intents(&effects);
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert!((intents[0].ratio.value() - 2.0).abs() < 1.0e-4, "{:?}", intents[0]);
    assert!((ratio_of(&screen) - 2.0).abs() < 1.0e-4);
}

#[test]
fn a_wheel_drag_publishes_a_continuous_intent_on_every_change() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(10) });
    let mut intents = Vec::new();
    for (index, requested) in [1.2_f32, 1.4, 1.6, 1.8].into_iter().enumerate() {
        let moved = screen.step(ScreenAction::ZoomWheelDragged {
            requested: ratio(requested),
            at: at_ms(20 + index as i64 * 16),
        });
        intents.extend(lens_intents(&moved.effects));
    }
    assert_eq!(intents.len(), 4, "{intents:?}");
    assert!(intents.iter().all(|intent| intent.gesture == ZoomGesture::Continuous));
}

#[test]
fn wheel_and_pinch_publish_continuous_intents() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(10) });
    let wheel = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.7),
        at: at_ms(20),
    });
    assert_eq!(lens_intents(&wheel.effects).len(), 1);
    assert_eq!(lens_intents(&wheel.effects)[0].gesture, ZoomGesture::Continuous);
    screen.step(ScreenAction::ZoomWheelReleased { at: at_ms(30) });

    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(40) });
    let pinch = screen.step(ScreenAction::ZoomPinchChanged {
        scale: 1.5,
        at: at_ms(50),
    });
    assert_eq!(lens_intents(&pinch.effects).len(), 1);
    assert_eq!(lens_intents(&pinch.effects)[0].gesture, ZoomGesture::Continuous);
}

#[test]
fn the_flip_publishes_a_discrete_intent_in_both_directions() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let to_front = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(100) });
    let mut front_effects = to_front.effects;
    front_effects.extend(run_frames(&mut screen, 100, 25));
    assert_eq!(lens_intents(&front_effects).len(), 1, "переворот = одно намерение");
    assert_eq!(lens_intents(&front_effects)[0].gesture, ZoomGesture::Discrete);
    let to_back = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(200) });
    let mut back_effects = to_back.effects;
    back_effects.extend(run_frames(&mut screen, 200, 25));
    assert_eq!(lens_intents(&back_effects).len(), 1, "переворот = одно намерение");
    assert_eq!(lens_intents(&back_effects)[0].gesture, ZoomGesture::Discrete);
}

#[test]
fn the_framing_carries_the_direction_the_ratio_is_expressed_in() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(10) });
    let back = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.4),
        at: at_ms(20),
    });
    assert_eq!(framing_facing(&back.effects), Some(LensFacing::Back));
    assert_eq!(
        screen.state.framing_intent().expect("кадрирование").zoom.facing,
        LensFacing::Back
    );

    let front = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(1000) });
    assert_eq!(framing_facing(&front.effects), Some(LensFacing::Front));
    assert_eq!(
        screen.state.framing_intent().expect("кадрирование").zoom.facing,
        LensFacing::Front
    );
}

#[test]
fn a_failure_reported_without_an_active_lens_still_snaps_when_the_active_lens_appears() {
    let mut screen = Screen::zoomed(LensRole::UltraWide);
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(10) });
    screen.step(ScreenAction::ZoomPinchChanged {
        scale: 20.0,
        at: at_ms(20),
    });
    assert!((ratio_of(&screen) - 10.0).abs() < 1.0e-3);

    // Неудача №1 пришла, а активной камеры в этих фактах нет: подгонять не к чему, неудача не «съедена».
    let mut without_active = zoom_facts_with_failure(LensRole::UltraWide, 1, "2");
    without_active.lens.active = None;
    screen.step(ScreenAction::CameraFactsChanged(without_active));
    assert!((ratio_of(&screen) - 10.0).abs() < 1.0e-3, "откатывать пока не к чему");

    // Те же факты уже с активным ширик: откат происходит теперь (его предел 0,593 × 10).
    screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::UltraWide,
        1,
        "2",
    )));
    assert!((ratio_of(&screen) - 5.93).abs() < 1.0e-2, "{}", ratio_of(&screen));

    // И после отката та же неудача больше не срабатывает.
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(30) });
    screen.step(ScreenAction::ZoomPinchChanged {
        scale: 20.0,
        at: at_ms(40),
    });
    screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::UltraWide,
        1,
        "2",
    )));
    assert!(
        (ratio_of(&screen) - 10.0).abs() < 1.0e-3,
        "учтённая неудача не откатывает снова"
    );
}

#[test]
fn flipping_again_after_a_failed_flip_publishes_the_front_intent_again() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let first = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(100) });
    assert_eq!(first.outcome, ActionOutcome::Applied);
    assert_eq!(lens_intents(&first.effects)[0].facing, LensFacing::Front);
    assert_eq!(screen.state.zoom().facing(), LensFacing::Front);

    // Фронтальная не открылась: активна по-прежнему основная задняя, пришла неудача №1 на камеру «1».
    let failed = screen.step(ScreenAction::CameraFactsChanged(zoom_facts_with_failure(
        LensRole::Wide,
        1,
        "1",
    )));
    assert_eq!(
        screen.state.zoom().facing(),
        LensFacing::Back,
        "экран откатился на заднюю"
    );
    assert!(lens_intents(&failed.effects).is_empty(), "откат без эха-намерения");

    // Повторный переворот не подавляется экраном: снова Discrete-намерение на фронтальную.
    let second = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(5000) });
    assert_eq!(second.outcome, ActionOutcome::Applied);
    let intents = lens_intents(&second.effects);
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].facing, LensFacing::Front);
    assert_eq!(intents[0].gesture, ZoomGesture::Discrete);
    assert_eq!(screen.state.zoom().last_flip().expect("метка").sequence, 2);
    // Контроль: факты без новой неудачи ничего не откатывают.
    screen.step(ScreenAction::CameraFactsChanged(zoom_facts(LensRole::Front)));
    assert_eq!(screen.state.zoom().facing(), LensFacing::Front);
}
