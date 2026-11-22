//! Вспышка скрыта, пока камера её не поддерживает (инвариант 6, решение 138): ни значка, ни пункта меню,
//! ни касаний, и в снимок уходит `FlashMode::Off`.

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::facts::{CameraFacts, FlashAvailability};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::FlashMode;
use crate::state::PanelState;
use crate::test_support::{Screen, at_ms, live_facts};
use crate::tools::{ToolId, ToolOption};

fn without_flash() -> CameraFacts {
    CameraFacts {
        flash: FlashAvailability::Unavailable,
        ..live_facts()
    }
}

fn screen_without_flash() -> Screen {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(without_flash()));
    screen
}

#[test]
fn flash_is_listed_only_while_the_camera_reports_it() {
    let mut screen = Screen::ready();
    assert!(screen.state.visible_strip_tools().contains(&ToolId::Flash));
    assert!(screen.state.menu_tools().contains(&ToolId::Flash));

    screen.step(ScreenAction::CameraFactsChanged(without_flash()));
    assert!(!screen.state.visible_strip_tools().contains(&ToolId::Flash));
    assert!(!screen.state.menu_tools().contains(&ToolId::Flash));
    // Остальное на месте и идёт подряд в прежнем порядке.
    assert_eq!(screen.state.visible_strip_tools(), [ToolId::Raw, ToolId::Exposure]);
    assert_eq!(screen.state.menu_tools()[0], ToolId::Aspect);
}

#[test]
fn flash_icon_and_menu_item_ignore_taps_when_there_is_no_flash() {
    let mut screen = screen_without_flash();

    let strip = screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    assert_eq!(strip.outcome, ActionOutcome::NoChange(NoChangeReason::IconNotShown));
    assert_eq!(screen.state.settings().flash, FlashMode::Off);

    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let item = screen.step(ScreenAction::ControlsItemSelected(ToolId::Flash));
    assert_eq!(item.outcome, ActionOutcome::Rejected(Rejection::ToolNotInMenu));
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
}

#[test]
fn flash_option_cannot_be_applied_without_a_flash() {
    let mut screen = screen_without_flash();
    // Недостижимо через UI, но редьюсер не доверяет вызывающему: панель подставлена напрямую.
    screen.state.panel = PanelState::Submenu(ToolId::Flash);

    let option = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Flash(FlashMode::On)));

    assert_eq!(option.outcome, ActionOutcome::Rejected(Rejection::UnsupportedOption));
    assert_eq!(screen.state.settings().flash, FlashMode::Off);
}

#[test]
fn shot_requests_never_ask_for_flash_without_it_even_if_on_was_chosen_earlier() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    assert_eq!(screen.state.settings().flash, FlashMode::On);
    let with_flash = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert!(matches!(with_flash.effects.first(), Some(Effect::Capture(shot)) if shot.flash == FlashMode::On));
    screen.step(ScreenAction::CaptureSecured {
        request: crate::shutter::ShotRequestId(1),
    });

    // Вспышка пропала: выбор «On» остаётся в настройках, но в снимок не идёт.
    screen.step(ScreenAction::CameraFactsChanged(without_flash()));
    let without = screen.step(ScreenAction::ShutterTapped { at: at_ms(100) });

    assert!(matches!(without.effects.first(), Some(Effect::Capture(shot)) if shot.flash == FlashMode::Off));
    assert_eq!(
        screen.state.settings().flash,
        FlashMode::On,
        "выбор пользователя не сброшен"
    );
}

#[test]
fn the_flash_card_closes_when_the_flash_disappears() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Flash));
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Flash));

    screen.step(ScreenAction::CameraFactsChanged(without_flash()));

    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn a_new_screen_starts_without_flash_until_the_camera_reports_it() {
    let screen = Screen::unprepared(crate::ScreenConfig::default());
    assert_eq!(screen.state.camera_facts().flash, FlashAvailability::Unavailable);
    assert_eq!(screen.state.effective_flash(), FlashMode::Off);
}

/// Снимок несёт выбор пользователя как есть (включая `Auto`: решает его приложение, решение 159);
/// без вспышки у камеры — всегда `Off`.
#[test]
fn shot_request_carries_the_chosen_flash_mode_when_available_and_off_when_not() {
    for (mode, taps) in [(FlashMode::Off, 0), (FlashMode::Auto, 1), (FlashMode::On, 2)] {
        let mut screen = Screen::ready();
        for _ in 0..taps {
            screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
        }
        assert_eq!(screen.state.settings().flash, mode);
        let shot = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
        assert!(
            matches!(shot.effects.first(), Some(Effect::Capture(request)) if request.flash == mode),
            "{mode:?}: снимок должен нести выбранный режим"
        );
    }

    // Auto выбран, затем вспышка пропала: в снимок идёт Off, выбор пользователя цел.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    screen.step(ScreenAction::CameraFactsChanged(without_flash()));
    assert_eq!(screen.state.effective_flash(), FlashMode::Off);
    let shot = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert!(matches!(shot.effects.first(), Some(Effect::Capture(request)) if request.flash == FlashMode::Off));
    assert_eq!(screen.state.settings().flash, FlashMode::Auto);
}

#[test]
fn a_new_screen_starts_with_flash_off_by_default() {
    // Решение 160: при запуске вспышка выключена.
    assert_eq!(crate::settings::CaptureSettings::default().flash, FlashMode::Off);
    assert_eq!(Screen::ready().state.settings().flash, FlashMode::Off);
}
