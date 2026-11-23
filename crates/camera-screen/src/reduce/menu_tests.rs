//! Тесты меню Controls, подменю, значков над кадром, режимов и смены соотношения.

use camera_contract::three_a::SensorPoint;
use camera_contract::viewfinder_crop::CropAspect;

use crate::action::ScreenAction;
use crate::config::{ScreenConfig, ToolsConfig};
use crate::effect::{Effect, ScreenCameraIntent};
use crate::focus::FocusIndicator;
use crate::modes::CaptureMode;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::{AspectRatio, FlashMode, GridSetting, LevelSetting, RawCapture, TimerSetting};
use crate::state::PanelState;
use crate::test_support::{Screen, at_ms, facts_with_point};
use crate::tools::{ToolId, ToolOption};

#[test]
fn menu_opens_from_button_and_from_tapping_the_current_mode_and_closes_by_swipe_down() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);

    let tap_mode = screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    assert_eq!(tap_mode.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
}

#[test]
fn swipe_down_closes_any_card_whatever_its_origin() {
    // Карточка Exposure со значка над кадром.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    let closed = screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(closed.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.panel(), PanelState::Closed);

    // Карточка Timer и карточка Exposure из меню — тем же правилом.
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn swipe_down_with_nothing_open_is_no_change() {
    let mut screen = Screen::ready();
    let outcome = screen.step(ScreenAction::ControlsMenuCloseRequested).outcome;
    assert_eq!(outcome, ActionOutcome::NoChange(NoChangeReason::NothingToClose));
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn menu_item_opens_its_submenu_and_picking_an_option_applies_it_and_keeps_the_card_open() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));

    let picked = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds10,
    )));

    assert_eq!(picked.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.settings().timer, TimerSetting::Seconds10);
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));
}

#[test]
fn every_menu_tool_has_a_working_submenu_option() {
    let options = [
        ToolOption::Flash(FlashMode::On),
        ToolOption::Raw(RawCapture::On),
        ToolOption::Timer(TimerSetting::Seconds5),
        ToolOption::Aspect(AspectRatio::Square),
        ToolOption::Grid(GridSetting::On),
        ToolOption::Level(LevelSetting::On),
    ];
    // Один экран на все инструменты: после выбора карточка остаётся, «‹» возвращает в меню.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    for option in options {
        screen.step(ScreenAction::ControlsItemSelected(option.tool()));
        let picked = screen.step(ScreenAction::SubmenuOptionSelected(option));
        assert_eq!(picked.outcome, ActionOutcome::Applied, "{option:?}");
        assert_eq!(screen.state.panel(), PanelState::Submenu(option.tool()), "{option:?}");
        screen.step(ScreenAction::SubmenuBackRequested);
        assert_eq!(screen.state.panel(), PanelState::ControlsMenu, "{option:?}");
    }
    let settings = screen.state.settings();
    assert_eq!(settings.flash, FlashMode::On);
    assert_eq!(settings.raw, RawCapture::On);
    assert_eq!(settings.timer, TimerSetting::Seconds5);
    assert_eq!(settings.aspect, AspectRatio::Square);
    assert_eq!(settings.grid, GridSetting::On);
    assert_eq!(settings.level, LevelSetting::On);
}

#[test]
fn exposure_menu_item_opens_the_exposure_panel_not_a_submenu() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);
}

#[test]
fn submenu_back_returns_to_the_menu_without_changing_settings() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    let settings = *screen.state.settings();
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
    assert_eq!(*screen.state.settings(), settings);
}

#[test]
fn automatic_flash_is_selectable_from_the_card() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Flash));

    let picked = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Flash(FlashMode::Auto)));

    assert_eq!(picked.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.settings().flash, FlashMode::Auto);
    assert_eq!(screen.state.rejections().get(Rejection::UnsupportedOption), 0);
}

#[test]
fn flash_icon_cycles_off_auto_on_off() {
    let mut screen = Screen::ready();
    let mut seen = vec![screen.state.settings().flash];
    for _ in 0..4 {
        screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
        seen.push(screen.state.settings().flash);
    }
    assert_eq!(
        seen,
        [
            FlashMode::Off,
            FlashMode::Auto,
            FlashMode::On,
            FlashMode::Off,
            FlashMode::Auto
        ]
    );
}

#[test]
fn raw_icon_toggles() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Raw));
    assert_eq!(screen.state.settings().raw, RawCapture::On);
    screen.step(ScreenAction::StripToolTapped(ToolId::Raw));
    assert_eq!(screen.state.settings().raw, RawCapture::Off);
}

#[test]
fn timer_icon_is_hidden_until_the_timer_is_on_and_then_opens_the_timer_submenu() {
    let mut screen = Screen::ready();
    assert!(!screen.state.visible_strip_tools().contains(&ToolId::Timer));
    let hidden = screen.step(ScreenAction::StripToolTapped(ToolId::Timer));
    assert_eq!(hidden.outcome, ActionOutcome::NoChange(NoChangeReason::IconNotShown));
    assert_eq!(screen.state.panel(), PanelState::Closed);

    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(
        screen.state.visible_strip_tools(),
        [ToolId::Flash, ToolId::Raw, ToolId::Timer, ToolId::Exposure]
    );

    screen.step(ScreenAction::StripToolTapped(ToolId::Timer));
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));
}

#[test]
fn exposure_icon_opens_the_panel_and_a_second_tap_closes_it() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn strip_icons_are_blocked_while_the_menu_is_open_and_unknown_icons_are_refused() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let blocked = screen.step(ScreenAction::StripToolTapped(ToolId::Flash));
    assert_eq!(blocked.outcome, ActionOutcome::Rejected(Rejection::PanelBlocksAction));
    assert_eq!(screen.state.settings().flash, FlashMode::Off);

    screen.step(ScreenAction::ControlsMenuCloseRequested);
    let unknown = screen.step(ScreenAction::StripToolTapped(ToolId::Aspect));
    assert_eq!(unknown.outcome, ActionOutcome::Rejected(Rejection::ToolNotInStrip));
}

#[test]
fn menu_item_outside_the_modes_tool_set_or_with_menu_closed_is_refused() {
    let tools = ToolsConfig {
        controls_menu: vec![ToolId::Flash, ToolId::Aspect],
        ..ToolsConfig::default()
    };
    let mut screen = Screen::unprepared(ScreenConfig {
        tools,
        ..ScreenConfig::default()
    });
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let missing = screen.step(ScreenAction::ControlsItemSelected(ToolId::Grid));
    assert_eq!(missing.outcome, ActionOutcome::Rejected(Rejection::ToolNotInMenu));
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);

    screen.step(ScreenAction::ControlsMenuCloseRequested);
    let closed = screen.step(ScreenAction::ControlsItemSelected(ToolId::Flash));
    assert_eq!(closed.outcome, ActionOutcome::Rejected(Rejection::ToolNotInMenu));
}

#[test]
fn option_from_another_tool_or_without_a_submenu_is_refused() {
    let mut screen = Screen::ready();
    let none = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Raw(RawCapture::On)));
    assert_eq!(none.outcome, ActionOutcome::Rejected(Rejection::OptionNotInSubmenu));

    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Flash));
    let wrong = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    assert_eq!(wrong.outcome, ActionOutcome::Rejected(Rejection::OptionNotInSubmenu));
    assert_eq!(screen.state.settings().timer, TimerSetting::Off);
}

#[test]
fn changing_the_aspect_sends_new_framing_and_hides_the_focus_indicator() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));

    let changed = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(
        AspectRatio::SixteenByNine,
    )));

    let expected = screen.state.layout().expect("раскладка");
    let expected = crate::test_support::plain_framing(expected, AspectRatio::SixteenByNine);
    assert_eq!(changed.effects, [Effect::Viewfinder(expected.clone())]);
    assert_eq!(expected.zoom.aspect, CropAspect::SIXTEEN_BY_NINE);
    // Место — кадр 16:9 из раскладки (≈ 102–885 pt), а не кадр 4:3.
    assert!((f64::from(expected.destination.top) * 982.67 - 102.4).abs() < 0.2);
    assert_eq!(screen.state.settings().aspect, AspectRatio::SixteenByNine);
    assert_eq!(screen.state.framing_intent(), Some(expected));
}

#[test]
fn changing_the_aspect_with_a_visible_indicator_also_resets_the_cameras_point() {
    let mut screen = Screen::ready();
    screen.tap_center(at_ms(0));
    screen.step(ScreenAction::CameraFactsChanged(facts_with_point(
        SensorPoint::CENTER,
        1,
    )));
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));

    let changed = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(
        AspectRatio::Square,
    )));

    assert_eq!(screen.state.focus().indicator(), FocusIndicator::Hidden);
    assert!(matches!(
        changed.effects.as_slice(),
        [
            Effect::Camera(ScreenCameraIntent::ResetPointOfInterest),
            Effect::Viewfinder(_)
        ]
    ));
}

#[test]
fn choosing_the_same_aspect_sends_no_framing() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    let same = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(
        AspectRatio::FourByThree,
    )));
    assert!(same.effects.is_empty());
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Aspect));
}

#[test]
fn tapping_after_an_aspect_change_uses_the_new_frame_for_the_sensor_point() {
    // Функциональный путь: смена соотношения → тап по центру нового кадра → центр кадрирования 16:9.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(
        AspectRatio::SixteenByNine,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);

    let transition = screen.tap_center(at_ms(0));

    let [Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(sensor))] = transition.effects.as_slice() else {
        panic!("ожидался фокус, получено {:?}", transition.effects);
    };
    assert!((sensor.x() - 0.5).abs() < 1e-5 && (sensor.y() - 0.5).abs() < 1e-5);
}

#[test]
fn only_implemented_modes_are_in_the_carousel_and_others_are_refused() {
    let mut screen = Screen::ready();
    let labels: Vec<&str> = screen.state.mode_carousel().iter().map(|mode| mode.label).collect();
    assert_eq!(labels, ["PHOTO", "CALIBRATE"]);

    let before = screen.state.clone();
    let video = screen.step(ScreenAction::ModeSelected(CaptureMode::Video));
    assert_eq!(video.outcome, ActionOutcome::Rejected(Rejection::UnknownMode));
    assert_eq!(screen.state.mode(), before.mode());
    assert_eq!(screen.state.rejections().get(Rejection::UnknownMode), 1);
}

/// Включает таймер через меню и возвращает экран с закрытой панелью и видимым значком таймера.
fn screen_with_timer_icon() -> Screen {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
    screen
}

#[test]
fn option_picked_in_the_card_opened_from_the_icon_keeps_it_open_and_the_shutter_usable() {
    let mut screen = screen_with_timer_icon();
    screen.step(ScreenAction::StripToolTapped(ToolId::Timer));
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));

    let picked = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds10,
    )));

    assert_eq!(picked.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.settings().timer, TimerSetting::Seconds10);
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Timer));
    let shot = screen.step(ScreenAction::ShutterTapped { at: at_ms(0) });
    assert_eq!(
        shot.outcome,
        ActionOutcome::Applied,
        "затвор снимает при открытой карточке"
    );
}

#[test]
fn back_from_the_submenu_opened_from_the_icon_closes_the_panel_and_keeps_the_setting() {
    let mut screen = screen_with_timer_icon();
    screen.step(ScreenAction::StripToolTapped(ToolId::Timer));
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
    assert_eq!(screen.state.settings().timer, TimerSetting::Seconds3);
}

#[test]
fn the_origin_of_the_submenu_is_remembered_per_entry_not_left_over_from_a_previous_one() {
    let mut screen = screen_with_timer_icon();
    // Сначала подменю со значка, закрыто тапом мимо.
    screen.step(ScreenAction::StripToolTapped(ToolId::Timer));
    screen.tap_center(at_ms(0));
    assert_eq!(screen.state.panel(), PanelState::Closed);
    // Теперь та же карточка из меню Controls: левая кнопка ведёт в меню, а не закрывает панель.
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Off,
    )));
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
}
