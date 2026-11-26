//! Тесты карточки инструмента (решение 103): подменю и Exposure — нижняя карточка с происхождением,
//! левой кнопкой, поднятым затвором и переключением значками.

use crate::action::ScreenAction;
use crate::outcome::{ActionOutcome, Rejection};
use crate::settings::{FlashMode, TimerSetting};
use crate::shutter::ShutterState;
use crate::state::{PanelState, ShutterVisibility};
use crate::test_support::{Screen, at_ms};
use crate::tools::{ToolId, ToolOption};

/// Экран с включённым таймером (значок таймера виден), панель закрыта.
fn screen_with_timer_icon() -> Screen {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    screen
}

fn open_exposure_from_menu(screen: &mut Screen) {
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Exposure));
}

#[test]
fn exposure_card_from_menu_returns_to_the_menu_by_the_left_button() {
    let mut screen = Screen::ready();
    open_exposure_from_menu(&mut screen);
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);

    screen.step(ScreenAction::SubmenuBackRequested);

    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
}

#[test]
fn exposure_card_from_the_icon_is_closed_by_the_left_button() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn exposure_close_request_closes_the_card_regardless_of_origin() {
    let mut screen = Screen::ready();
    open_exposure_from_menu(&mut screen);
    screen.step(ScreenAction::ExposurePanelCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn swipe_down_closes_the_exposure_card_opened_from_the_menu() {
    let mut screen = Screen::ready();
    open_exposure_from_menu(&mut screen);
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn origin_does_not_leak_between_cards() {
    let mut screen = Screen::ready();
    // Со значка (origin = StripIcon), закрыть тапом по видоискателю.
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    screen.tap_center(at_ms(0));
    // Та же карточка из меню: левая кнопка ведёт в меню.
    open_exposure_from_menu(&mut screen);
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
}

#[test]
fn shutter_is_raised_and_shoots_while_a_card_is_open() {
    let mut screen = Screen::ready();
    open_exposure_from_menu(&mut screen);
    assert_eq!(screen.state.shutter_visibility(), ShutterVisibility::RaisedAboveCard);
    assert!(screen.state.is_tool_card_open());
    assert!(screen.state.is_bottom_row_hidden());

    let shot = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });

    assert_eq!(shot.outcome, ActionOutcome::Applied);
    assert_ne!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(
        screen.state.panel(),
        PanelState::ExposurePanel,
        "снимок карточку не закрывает"
    );
}

#[test]
fn shutter_is_rejected_under_the_menu_and_visibility_follows_the_panel() {
    let mut screen = Screen::ready();
    assert_eq!(screen.state.shutter_visibility(), ShutterVisibility::Visible);
    assert!(!screen.state.is_bottom_row_hidden());

    screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(screen.state.shutter_visibility(), ShutterVisibility::HiddenUnderDrawer);
    assert!(!screen.state.is_tool_card_open());
    // 14p: шторка — полупрозрачная поверхность, поэтому нижний ряд под ней растворяется, а не просвечивает.
    assert!(screen.state.is_bottom_row_hidden());
    let shot = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert_eq!(shot.outcome, ActionOutcome::Rejected(Rejection::ShutterHiddenByMenu));

    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    assert_eq!(screen.state.shutter_visibility(), ShutterVisibility::RaisedAboveCard);
}

#[test]
fn strip_tap_while_a_card_is_open_switches_to_that_tools_card() {
    let mut screen = screen_with_timer_icon();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);

    let switched = screen.step(ScreenAction::StripToolTapped(ToolId::Timer));

    assert_eq!(switched.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));
    // И обратно: Exposure из открытой карточки таймера.
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);
    // Повторный тап по значку открытой карточки закрывает её.
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn strip_switch_from_a_menu_card_makes_the_left_button_close_instead_of_returning_to_the_menu() {
    let mut screen = screen_with_timer_icon();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    screen.step(ScreenAction::StripToolTapped(ToolId::Timer));
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn strip_toggle_icon_works_over_an_open_card_without_closing_it() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    assert_eq!(screen.state.settings().flash, FlashMode::Auto);
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);
}

#[test]
fn viewfinder_tap_closes_any_card_without_touching_settings() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Flash));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Flash(FlashMode::On)));
    let settings = *screen.state.settings();
    screen.tap_center(at_ms(0));
    assert_eq!(screen.state.panel(), PanelState::Closed);
    assert_eq!(*screen.state.settings(), settings);
}

#[test]
fn card_returns_to_menu_only_for_a_card_opened_from_the_menu() {
    let mut screen = Screen::ready();
    assert!(!screen.state.card_returns_to_menu(), "карточки нет — вернуться некуда");
    open_exposure_from_menu(&mut screen);
    assert!(screen.state.card_returns_to_menu(), "из меню: «‹» ведёт в меню");
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert!(!screen.state.card_returns_to_menu(), "закрыто");
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert!(
        !screen.state.card_returns_to_menu(),
        "со значка: меню за карточкой нет, кнопка «×»"
    );
}
