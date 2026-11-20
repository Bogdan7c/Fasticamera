//! Взаимодействие: повторный вход в подменю, пока старое закрывается, и повторный тап по рамке фокуса.

use super::test_rig::*;
use crate::action::ScreenAction;

#[test]
fn a_repeated_tap_on_the_exact_same_point_pulses_the_focus_frame_again() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(600);
    let settled = rig.screen.focus_frame_scale();
    // Тот же пиксель, новый тап: рамка появляется заново (масштаб снова далёк от покоя).
    rig.tap(center);
    let pulsing = rig.screen.focus_frame_scale();
    assert!(
        (pulsing - settled).abs() > 0.02,
        "повторный тап не перезапустил анимацию: покой {settled}, после тапа {pulsing}"
    );
    rig.run(600);
    assert!(
        (rig.screen.focus_frame_scale() - settled).abs() < 0.01,
        "анимация снова улеглась"
    );
}

#[test]
fn dragging_the_sun_does_not_restart_the_focus_frame_animation() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(600);
    let settled = rig.screen.focus_frame_scale();
    // Перетаскивание «солнца» продлевает показ индикатора (его метка времени меняется), но это не новый тап.
    for step in 1..=3 {
        rig.act(ScreenAction::ExposureDragged {
            requested: camera_contract::control::ExposureBias::from_ev(step as f32 / 3.0).expect("EV"),
            at: ms(rig.now_ms),
        });
        assert!(
            (rig.screen.focus_frame_scale() - settled).abs() < 0.01,
            "рамка не должна пульсировать от продления срока"
        );
    }
}

/// «Солнце» рамки фокуса в pt для рамки с центром `center`.
fn sun_of(center: ui_core::geometry::Point) -> ui_core::geometry::Point {
    let tokens = ui_kit::tokens::UiKitTokens::default();
    ui_core::geometry::Point::new(
        center.x + tokens.focus.frame_width_pt * 0.5 + tokens.focus.sun_offset_right_of_frame_pt,
        center.y - tokens.focus.sun_offset_above_center_pt,
    )
}

#[test]
fn a_finger_resting_on_the_sun_keeps_the_focus_frame_visible_past_the_indicator_lifetime() {
    use shell_contract::touch::TouchPhase;
    use ui_kit::focus_frame::FocusPhase;

    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(300);
    let sun = sun_of(center);
    // Палец берёт «солнце» и чуть сдвигается (в пределах одного шага 1/3 EV), дальше лежит неподвижно.
    let held_at = ui_core::geometry::Point::new(sun.x, sun.y - 12.0);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, sun, start);
    rig.touch(TouchPhase::Moved, held_at, start + 16);
    rig.now_ms = start + 16;
    rig.run(300);
    let steps_when_settled = rig.screen.state().exposure().steps();
    // 5 с (индикатор живёт 3 с) палец шлёт события на одном месте, кадры идут.
    for _ in 0..312 {
        rig.touch(TouchPhase::Moved, held_at, rig.now_ms + 1);
        rig.run(16);
    }
    assert_eq!(
        rig.screen.state().exposure().steps(),
        steps_when_settled,
        "палец неподвижен: шаг не меняется"
    );
    assert!(
        matches!(
            rig.screen.state().focus().indicator(),
            crate::focus::FocusIndicator::Shown { .. }
        ),
        "индикатор под пальцем погас"
    );
    assert_eq!(
        rig.screen.focus_frame_phase(),
        Some(FocusPhase::Active),
        "рамка исчезла под пальцем"
    );
    assert_eq!(rig.screen.diagnostics().ignored_sun_pans, 0);

    // Палец поднят: срок индикатора отсчитывается от последнего события и заканчивается.
    let release = rig.now_ms;
    rig.touch(TouchPhase::Ended, held_at, release);
    rig.run(3_100);
    assert_eq!(
        rig.screen.state().focus().indicator(),
        crate::focus::FocusIndicator::Hidden
    );
    // Затухание рамки идёт пружиной: даём ей закончиться.
    rig.run(2_000);
    assert_eq!(
        rig.screen.focus_frame_phase(),
        Some(FocusPhase::Hidden),
        "рамка должна погаснуть вслед за индикатором"
    );
}

#[test]
fn a_sun_pan_after_the_indicator_is_hidden_neither_revives_the_frame_nor_resets_the_stage() {
    use shell_contract::touch::TouchPhase;
    use ui_kit::focus_frame::FocusPhase;

    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(300);
    let sun = sun_of(center);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, sun, start);
    rig.touch(
        TouchPhase::Moved,
        ui_core::geometry::Point::new(sun.x, sun.y - 20.0),
        start + 16,
    );
    rig.now_ms = start + 32;
    rig.frame();
    let rebuilds = rig.screen.diagnostics().stage_rebuilds;
    // Камера пропала: индикатор скрыт в состоянии, а палец ещё на «солнце» и шлёт события до следующего кадра.
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(crate::facts::CameraFacts::default()));
    assert_eq!(
        rig.screen.state().focus().indicator(),
        crate::focus::FocusIndicator::Hidden
    );
    rig.touch(
        TouchPhase::Moved,
        ui_core::geometry::Point::new(sun.x, sun.y - 40.0),
        start + 48,
    );
    rig.now_ms = start + 64;
    rig.frame();
    rig.run(1_000);

    let diagnostics = rig.screen.diagnostics();
    assert_eq!(diagnostics.stage_rebuilds, rebuilds, "сцена не должна сбрасываться");
    assert!(diagnostics.ignored_sun_pans >= 1, "пан после скрытия не учтён");
    assert_eq!(
        rig.screen.focus_frame_phase(),
        Some(FocusPhase::Hidden),
        "рамка ожила без индикатора"
    );
}
