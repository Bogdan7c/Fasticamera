//! Взаимодействие: повторный вход в подменю, пока старое закрывается, и повторный тап по рамке фокуса.

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::settings::TimerSetting;
use crate::state::PanelState;
use crate::tools::ToolId;

#[test]
fn re_entering_a_submenu_that_is_still_closing_keeps_it_alive_and_tappable() {
    let mut rig = Rig::new();
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
    rig.screen
        .handle_action(ScreenAction::ControlsItemSelected(ToolId::Timer));
    rig.run(700);
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Timer));
    // Назад — и сразу снова в тот же пункт, пока подменю лишь начало закрываться.
    rig.act(ScreenAction::SubmenuBackRequested);
    rig.now_ms += 3 * FRAME_MS;
    rig.frame();
    rig.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Timer));
    // Варианты принимают касания сразу, не дожидаясь конца морфа.
    let three_seconds = rig.center_with(|g| g.option_row(1));
    rig.tap(three_seconds);
    assert_eq!(
        rig.screen.state().settings().timer,
        TimerSetting::Seconds3,
        "подменю не «мёртвое»: тап по варианту сработал"
    );
}

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
