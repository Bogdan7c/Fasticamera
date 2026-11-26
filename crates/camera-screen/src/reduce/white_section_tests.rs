//! Белое в разделе калибровки на редьюсере (решения 289–294): капсула открывает и закрывает нижнюю карточку,
//! шкалы и «Phone» шлют намерения, «Auto» и долгое нажатие отвергаются, смена режима карточку закрывает, а
//! желание паузы слежения следует за режимом.

use camera_contract::control::{ColorTemperature, Tint};
use camera_contract::white::{WhiteAdjustment, WhiteSourceId};

use crate::action::ScreenAction;
use crate::calibration::SectionCard;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::look_test_support::look_facts;
use crate::mode_behaviour::WhiteTrackingHold;
use crate::modes::CaptureMode;
use crate::outcome::{ActionOutcome, Rejection};
use crate::state::PanelState;
use crate::test_support::Screen;
use crate::white::WhiteAxis;
use crate::white_config::WhiteLightIndex;

/// Раздел с открытой карточкой белого (открыта тапом по капсуле).
fn section_with_white_card() -> Screen {
    let mut screen = Screen::calibrating(look_facts());
    let tap = screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(tap.outcome, ActionOutcome::Applied);
    screen
}

#[test]
fn the_capsule_tap_opens_the_white_card_and_a_second_tap_closes_it_without_touching_the_panel() {
    let mut screen = Screen::calibrating(look_facts());

    let opened = screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(opened.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.calibration().card(), SectionCard::White);
    assert!(screen.state.is_white_card_open());
    assert_eq!(
        screen.state.panel(),
        PanelState::Closed,
        "панель PHOTO в разделе не трогается"
    );
    assert!(opened.effects.is_empty(), "открытие намерений не порождает");

    let closed = screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(closed.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.calibration().card(), SectionCard::Closed);
    assert!(!screen.state.is_white_card_open());
}

#[test]
fn the_card_always_opens_on_the_kelvin_scale() {
    let mut screen = section_with_white_card();
    screen.step(ScreenAction::WhiteAxisSelected(WhiteAxis::Tint));
    assert_eq!(screen.state.white().axis(), WhiteAxis::Tint);
    screen.step(ScreenAction::WhiteButtonTapped);

    screen.step(ScreenAction::WhiteButtonTapped);

    assert_eq!(screen.state.white().axis(), WhiteAxis::Temperature);
}

#[test]
fn the_capsule_is_refused_while_the_presets_card_is_open_and_the_menu_button_while_the_white_card_is() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let refused = screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(refused.outcome, ActionOutcome::Rejected(Rejection::PanelBlocksAction));
    assert_eq!(
        screen.state.calibration().card(),
        SectionCard::Presets,
        "карточка пресетов на месте"
    );

    let mut screen = section_with_white_card();
    let refused = screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(refused.outcome, ActionOutcome::Rejected(Rejection::PanelBlocksAction));
    assert_eq!(
        screen.state.calibration().card(),
        SectionCard::White,
        "карточка белого на месте"
    );
}

#[test]
fn the_scrim_close_back_button_and_a_mode_change_each_close_the_white_card() {
    for close in [
        ScreenAction::ControlsMenuCloseRequested,
        ScreenAction::SubmenuBackRequested,
    ] {
        let mut screen = section_with_white_card();
        screen.step(close.clone());
        assert_eq!(screen.state.calibration().card(), SectionCard::Closed, "{close:?}");
    }
    let mut screen = section_with_white_card();
    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    assert_eq!(screen.state.calibration().card(), SectionCard::Closed);
    assert!(!screen.state.is_white_card_open());
}

#[test]
fn the_scales_and_phone_send_the_same_intents_as_in_photo_and_the_light_buttons_set_the_white() {
    let mut screen = section_with_white_card();
    let kelvin = ColorTemperature::from_kelvin(4300.0).expect("температура");
    let drag = screen.step(ScreenAction::WhiteTemperatureDragged(kelvin));
    assert_eq!(drag.outcome, ActionOutcome::Applied);
    assert_eq!(
        drag.effects,
        [Effect::Camera(ScreenCameraIntent::AdjustWhite(
            WhiteAdjustment::Temperature(kelvin)
        ))]
    );
    let tint = Tint::from_duv(0.004).expect("оттенок");
    let drag = screen.step(ScreenAction::WhiteTintDragged(tint));
    assert_eq!(
        drag.effects,
        [Effect::Camera(ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(
            tint
        )))]
    );
    let phone = screen.step(ScreenAction::WhitePhoneTapped);
    assert_eq!(
        phone.effects,
        [Effect::Camera(ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE))]
    );
    let light = screen.step(ScreenAction::WhiteLightSelected(WhiteLightIndex(0)));
    assert!(
        matches!(
            light.effects.as_slice(),
            [Effect::Camera(ScreenCameraIntent::SetWhite(_))]
        ),
        "{:?}",
        light.effects
    );
}

#[test]
fn white_card_actions_with_the_card_closed_are_refused_as_in_photo() {
    let mut screen = Screen::calibrating(look_facts());
    for action in [
        ScreenAction::WhitePhoneTapped,
        ScreenAction::WhiteAxisSelected(WhiteAxis::Tint),
        ScreenAction::WhiteTemperatureDragged(ColorTemperature::DAYLIGHT),
    ] {
        let refused = screen.step(action.clone());
        assert_eq!(
            refused.outcome,
            ActionOutcome::Rejected(Rejection::OptionNotInSubmenu),
            "{action:?}"
        );
        assert!(refused.effects.is_empty(), "{action:?}");
    }
}

#[test]
fn auto_and_the_long_press_stay_refused_even_with_the_white_card_open() {
    let mut screen = section_with_white_card();
    for action in [ScreenAction::WhiteAutoTapped, ScreenAction::WhiteButtonLongPressed] {
        let refused = screen.step(action.clone());
        assert_eq!(
            refused.outcome,
            ActionOutcome::Rejected(Rejection::NotAvailableInMode),
            "{action:?}"
        );
        assert!(refused.effects.is_empty(), "{action:?}: никаких команд камере");
    }
    assert_eq!(screen.state.calibration().card(), SectionCard::White);
}

#[test]
fn the_tracking_hold_follows_the_mode_and_survives_rotation_and_open_cards() {
    let mut screen = Screen::ready();
    assert_eq!(screen.state.white_tracking_hold(), WhiteTrackingHold::Running);

    screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    assert_eq!(
        screen.state.white_tracking_hold(),
        WhiteTrackingHold::PausedForCalibration
    );
    screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(
        screen.state.white_tracking_hold(),
        WhiteTrackingHold::PausedForCalibration
    );
    screen.rotate_to(shell_contract::orientation::DeviceOrientation::LandscapeLeft);
    assert_eq!(
        screen.state.white_tracking_hold(),
        WhiteTrackingHold::PausedForCalibration
    );

    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    assert_eq!(screen.state.white_tracking_hold(), WhiteTrackingHold::Running);
}

#[test]
fn the_screen_never_emits_the_hold_as_an_effect_the_app_polls_the_level() {
    let mut screen = Screen::ready();
    let mut all_effects = Vec::new();
    all_effects.extend(screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate)).effects);
    all_effects.extend(screen.step(ScreenAction::ModeSelected(CaptureMode::Photo)).effects);
    assert!(
        !all_effects
            .iter()
            .any(|effect| matches!(effect, Effect::Camera(ScreenCameraIntent::HoldWhiteTracking(_)))),
        "{all_effects:?}"
    );
}
