//! Кэш раскладки текста: подписи раскладываются при первом показе, на повторных кадрах — нет, динамические
//! (EV, секунды) перекладываются только при смене значения.

use camera_contract::control::ExposureBias;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::state::PanelState;
use crate::tools::ToolId;

#[test]
fn static_labels_are_laid_out_once_and_repeat_frames_reuse_them() {
    let mut rig = Rig::new();
    // Панель Exposure: подпись EXPOSURE, число EV, значки; затем меню: подписи пунктов.
    rig.screen
        .handle_action(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(800);
    let warmed = rig.screen.diagnostics().text_layouts;
    assert!(warmed > 0, "подписи показаны, значит разложены");
    rig.run(500);
    assert_eq!(
        rig.screen.diagnostics().text_layouts,
        warmed,
        "30 кадров с теми же подписями не добавили ни одной раскладки"
    );
    // Меню Controls: 9 подписей пунктов + подписи внутри кругов — один раз.
    rig.screen.handle_action(ScreenAction::ExposurePanelCloseRequested);
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(800);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    let with_menu = rig.screen.diagnostics().text_layouts;
    assert!(with_menu > warmed, "подписи меню разложены при первом показе");
    rig.run(500);
    assert_eq!(rig.screen.diagnostics().text_layouts, with_menu);
}

#[test]
fn a_dynamic_value_is_laid_out_once_per_value_and_cached_when_it_returns() {
    let mut rig = Rig::new();
    rig.screen
        .handle_action(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(800);
    let ev = |value: f32, at_ms: i64| ScreenAction::ExposureDragged {
        requested: ExposureBias::from_ev(value).expect("EV"),
        at: ms(at_ms),
    };
    let base = rig.screen.diagnostics().text_layouts;
    // Новое значение: число на панели и под значком вверху — две новые подписи (разные роли текста).
    let now = rig.now_ms;
    rig.act(ev(2.0 / 3.0, now));
    rig.run(800);
    let after_change = rig.screen.diagnostics().text_layouts;
    assert!(after_change > base, "новое значение EV разложено");
    rig.run(500);
    assert_eq!(
        rig.screen.diagnostics().text_layouts,
        after_change,
        "значение держится — раскладки нет"
    );
    // Вернулись к 0 и снова к 2/3: оба текста уже в кэше.
    let now = rig.now_ms;
    rig.act(ev(0.0, now));
    rig.run(800);
    let back_to_zero = rig.screen.diagnostics().text_layouts;
    let now = rig.now_ms;
    rig.act(ev(2.0 / 3.0, now));
    rig.run(800);
    assert_eq!(
        rig.screen.diagnostics().text_layouts,
        back_to_zero,
        "ранее виденное значение берётся из кэша"
    );
}

#[test]
fn a_full_cache_is_cleared_instead_of_growing_without_bound() {
    let mut config = crate::config::ScreenConfig::default();
    config.widgets.text_run_cache_capacity = 2;
    let mut rig = Rig::with(config, ui_core::time::MotionPreference::Full);
    rig.screen
        .handle_action(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(800);
    // Больше двух разных подписей на одном экране: кэш обязан очищаться, а экран — продолжать рисовать.
    assert!(rig.screen.diagnostics().text_cache_evictions > 0);
    assert!(text_count(&rig.last) > 0, "подписи на экране при тесном кэше");
}
