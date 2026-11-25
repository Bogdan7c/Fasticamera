//! Панель белого на редьюсере: намерения движку, правила карточки и отказы. Белого экран не хранит, поэтому
//! проверяем наблюдаемое: какие эффекты ушли, какая панель открыта, какой номер отказа и какой итог у действия.

use camera_contract::control::{ColorTemperature, Tint};
use camera_contract::white::{HeldWhite, WhiteAdjustment, WhiteOrigin, WhiteSourceId};
use shell_contract::haptics::HapticPattern;

use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::{CameraFacts, FlashAvailability};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::TimerSetting;
use crate::state::PanelState;
use crate::test_support::{Screen, live_facts};
use crate::tools::ToolId;
use crate::white::WhiteAxis;
use crate::white_config::WhiteLightIndex;
use crate::white_facts::{WhiteFacts, WhiteTakeFacts, WhiteTakeState};

const WHITE_CARD: PanelState = PanelState::Submenu(ToolId::White);

fn take_phone() -> Effect {
    Effect::Camera(ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE))
}

fn error_haptic() -> Effect {
    Effect::Haptic(HapticPattern::Error)
}

fn temperature(kelvin: f64) -> ColorTemperature {
    ColorTemperature::from_kelvin(kelvin).expect("температура теста")
}

/// Экран с открытой карточкой белого (открыта тапом по кнопке белого).
fn screen_with_white_card() -> Screen {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(screen.state.panel(), WHITE_CARD);
    screen
}

fn facts_with_take(serial: u64, state: WhiteTakeState) -> CameraFacts {
    CameraFacts {
        white: WhiteFacts {
            take: Some(WhiteTakeFacts { serial, state }),
            ..WhiteFacts::STARTING
        },
        ..live_facts()
    }
}

// ---- кнопка и карточка ----

#[test]
fn white_is_not_a_strip_icon_nor_a_menu_item_and_needs_neither_flash_nor_a_rear_camera() {
    let mut screen = Screen::ready();
    // Верхний ряд прежний: белого в нём нет, тап «как по значку» отвергается.
    assert!(!screen.state.visible_strip_tools().contains(&ToolId::White));
    let as_icon = screen.step(ScreenAction::StripToolTapped(ToolId::White));
    assert_eq!(as_icon.outcome, ActionOutcome::Rejected(Rejection::ToolNotInStrip));
    assert_eq!(screen.state.panel(), PanelState::Closed);
    // Кнопка белого работает и без вспышки.
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        flash: FlashAvailability::Unavailable,
        ..live_facts()
    }));
    assert_eq!(
        screen.step(ScreenAction::WhiteButtonTapped).outcome,
        ActionOutcome::Applied
    );
    assert_eq!(screen.state.panel(), WHITE_CARD);
    // Пункта меню Controls у белого нет: шторка не меняется.
    assert!(!screen.state.menu_tools().contains(&ToolId::White));
}

#[test]
fn tapping_the_white_button_opens_the_card_and_a_second_tap_closes_it() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(screen.state.panel(), WHITE_CARD);
    screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn white_button_tap_switches_from_another_card_and_the_cross_closes_a_card_opened_by_the_button() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(screen.state.panel(), WHITE_CARD);
    // «×» карточки, открытой кнопкой, закрывает панель (меню Controls за ней нет).
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn white_button_tap_is_refused_while_the_controls_menu_is_open() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let tap = screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(tap.outcome, ActionOutcome::Rejected(Rejection::PanelBlocksAction));
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);
}

#[test]
fn the_card_always_opens_on_the_kelvin_scale() {
    let mut screen = screen_with_white_card();
    screen.step(ScreenAction::WhiteAxisSelected(WhiteAxis::Tint));
    assert_eq!(screen.state.white().axis(), WhiteAxis::Tint);
    screen.step(ScreenAction::WhiteButtonTapped);
    screen.step(ScreenAction::WhiteButtonTapped);
    assert_eq!(screen.state.panel(), WHITE_CARD);
    assert_eq!(screen.state.white().axis(), WhiteAxis::Temperature);
}

#[test]
fn selecting_the_axis_already_shown_changes_nothing() {
    let mut screen = screen_with_white_card();
    let again = screen.step(ScreenAction::WhiteAxisSelected(WhiteAxis::Temperature));
    assert_eq!(again.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
}

// ---- долгое нажатие на кнопку ----

#[test]
fn long_press_takes_the_phone_white_with_a_haptic_and_does_not_open_the_panel() {
    let mut screen = Screen::ready();
    let press = screen.step(ScreenAction::WhiteButtonLongPressed);
    assert_eq!(press.outcome, ActionOutcome::Applied);
    assert_eq!(
        press.effects,
        [take_phone(), Effect::Haptic(HapticPattern::MediumImpact)]
    );
    assert_eq!(screen.state.panel(), PanelState::Closed);
}

#[test]
fn long_press_keeps_an_open_white_card_open() {
    let mut screen = screen_with_white_card();
    let press = screen.step(ScreenAction::WhiteButtonLongPressed);
    assert_eq!(
        press.effects,
        [take_phone(), Effect::Haptic(HapticPattern::MediumImpact)]
    );
    assert_eq!(screen.state.panel(), WHITE_CARD);
}

#[test]
fn long_press_takes_the_white_but_leaves_another_open_card_alone() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::StripToolTapped(ToolId::Exposure));
    let press = screen.step(ScreenAction::WhiteButtonLongPressed);
    assert_eq!(
        press.effects,
        [take_phone(), Effect::Haptic(HapticPattern::MediumImpact)]
    );
    assert_eq!(screen.state.panel(), PanelState::ExposurePanel);
}

#[test]
fn long_press_is_refused_under_the_controls_menu_without_effects() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let press = screen.step(ScreenAction::WhiteButtonLongPressed);
    assert_eq!(press.outcome, ActionOutcome::Rejected(Rejection::PanelBlocksAction));
    assert!(press.effects.is_empty());
}

#[test]
fn long_press_still_sends_the_intent_while_the_camera_is_not_live() {
    // Решение о «камеры нет» принимает движок (ответит «не спрашивал»); экран его не дублирует.
    let mut screen = Screen::unprepared(ScreenConfig::default());
    let press = screen.step(ScreenAction::WhiteButtonLongPressed);
    assert_eq!(press.outcome, ActionOutcome::Applied);
    assert_eq!(press.effects[0], take_phone());
}

// ---- действия карточки ----

#[test]
fn phone_button_takes_the_phone_white_without_a_haptic() {
    let mut screen = screen_with_white_card();
    let tap = screen.step(ScreenAction::WhitePhoneTapped);
    assert_eq!(tap.effects, [take_phone()]);
    assert_eq!(screen.state.panel(), WHITE_CARD);
}

#[test]
fn light_button_sets_the_white_with_the_numbers_from_the_config() {
    let mut screen = screen_with_white_card();
    let lamp_index = screen
        .config
        .white
        .lights
        .iter()
        .position(|light| light.label == "Lamp")
        .expect("в конфиге есть Lamp");
    let tap = screen.step(ScreenAction::WhiteLightSelected(WhiteLightIndex(lamp_index)));
    let [Effect::Camera(ScreenCameraIntent::SetWhite(setting))] = tap.effects.as_slice() else {
        panic!("ждали одно намерение SetWhite, получили {:?}", tap.effects);
    };
    assert_eq!(setting.temperature.kelvin(), 3_200.0);
    assert_eq!(setting.tint.duv(), 0.0);
}

#[test]
fn light_button_with_an_unknown_index_is_a_distinct_refusal_that_is_counted() {
    let mut screen = screen_with_white_card();
    let beyond_the_list = WhiteLightIndex(screen.config.white.lights.len());
    let tap = screen.step(ScreenAction::WhiteLightSelected(beyond_the_list));
    assert_eq!(tap.outcome, ActionOutcome::Rejected(Rejection::UnknownWhiteLight));
    assert!(tap.effects.is_empty());
    assert_eq!(screen.state.rejections().get(Rejection::UnknownWhiteLight), 1);
}

#[test]
fn scales_send_a_pointwise_adjustment_of_one_axis() {
    let mut screen = screen_with_white_card();
    let kelvin = screen.step(ScreenAction::WhiteTemperatureDragged(temperature(4_300.0)));
    assert_eq!(
        kelvin.effects,
        [Effect::Camera(ScreenCameraIntent::AdjustWhite(
            WhiteAdjustment::Temperature(temperature(4_300.0))
        ))]
    );
    let tint = Tint::from_duv(-0.01).expect("оттенок");
    let tint_move = screen.step(ScreenAction::WhiteTintDragged(tint));
    assert_eq!(
        tint_move.effects,
        [Effect::Camera(ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(
            tint
        )))]
    );
}

#[test]
fn dragging_back_to_the_value_the_facts_still_show_is_still_a_real_edit() {
    // Факты отстают на кадр: палец вернулся к значению, которое они ещё показывают. Это правка, а не «без изменений».
    let mut screen = screen_with_white_card();
    let held = WhiteFacts::STARTING.held.setting.temperature;
    let drag = screen.step(ScreenAction::WhiteTemperatureDragged(held));
    assert_eq!(drag.outcome, ActionOutcome::Applied);
    assert_eq!(drag.effects.len(), 1);
}

#[test]
fn card_actions_are_refused_without_effects_while_the_card_is_closed() {
    let actions = [
        ScreenAction::WhitePhoneTapped,
        ScreenAction::WhiteLightSelected(WhiteLightIndex(0)),
        ScreenAction::WhiteAxisSelected(WhiteAxis::Tint),
        ScreenAction::WhiteTemperatureDragged(ColorTemperature::DAYLIGHT),
        ScreenAction::WhiteTintDragged(Tint::NEUTRAL),
    ];
    for action in actions {
        let mut screen = Screen::ready();
        let name = format!("{action:?}");
        let transition = screen.step(action);
        assert_eq!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::OptionNotInSubmenu),
            "{name}"
        );
        assert!(transition.effects.is_empty(), "{name}");
        assert_eq!(screen.state.white().axis(), WhiteAxis::Temperature, "{name}");
    }
}

#[test]
fn card_actions_are_refused_while_a_different_card_is_open() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    let tap = screen.step(ScreenAction::WhitePhoneTapped);
    assert_eq!(tap.outcome, ActionOutcome::Rejected(Rejection::OptionNotInSubmenu));
    assert_eq!(screen.state.settings().timer, TimerSetting::Off);
}

#[test]
fn the_screen_never_stores_the_white_it_sends() {
    // Рука послала белое, но факты движка не менялись: экран продолжает показывать то, что держит движок.
    let mut screen = screen_with_white_card();
    screen.step(ScreenAction::WhiteTemperatureDragged(temperature(3_000.0)));
    screen.step(ScreenAction::WhiteLightSelected(WhiteLightIndex(0)));
    assert_eq!(screen.state.camera_facts().white.held, WhiteFacts::STARTING.held);
}

// ---- отказы источника ----

#[test]
fn a_refusal_shakes_the_button_and_vibrates_exactly_once_per_command_number() {
    let mut screen = Screen::unprepared(ScreenConfig::default());
    let first = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        3,
        WhiteTakeState::Refused,
    )));
    // Первые факты после создания экрана: отказ принят как уже виденный, молча.
    assert!(!first.effects.contains(&error_haptic()));
    assert_eq!(screen.state.white().refusal_serial(), 0);

    let fresh = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        4,
        WhiteTakeState::Refused,
    )));
    assert_eq!(
        fresh.effects.iter().filter(|effect| **effect == error_haptic()).count(),
        1
    );
    assert_eq!(screen.state.white().refusal_serial(), 1);

    // Те же факты доставлены повторно: второй вибрации и второй тряски нет.
    let repeated = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        4,
        WhiteTakeState::Refused,
    )));
    assert!(!repeated.effects.contains(&error_haptic()));
    assert_eq!(screen.state.white().refusal_serial(), 1);

    // Следующая команда с новым номером и тоже отказом — снова тряска.
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        5,
        WhiteTakeState::Refused,
    )));
    assert_eq!(screen.state.white().refusal_serial(), 2);
}

#[test]
fn a_refusal_right_after_creation_stays_silent_even_when_it_is_the_very_first_facts() {
    let mut screen = Screen::unprepared(ScreenConfig::default());
    let first = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        7,
        WhiteTakeState::Refused,
    )));
    assert!(first.effects.is_empty());
    assert_eq!(screen.state.white().refusal_serial(), 0);
}

#[test]
fn a_fast_refusal_the_screen_never_saw_waiting_for_still_counts() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    // Между двумя снимками фактов ожидание пришло и закрылось: экран видит сразу отказ.
    let refused = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        1,
        WhiteTakeState::Refused,
    )));
    assert!(refused.effects.contains(&error_haptic()));
    assert_eq!(screen.state.white().refusal_serial(), 1);
}

#[test]
fn a_refusal_after_a_seen_waiting_with_the_same_number_counts_once() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        2,
        WhiteTakeState::Waiting,
    )));
    assert_eq!(screen.state.white().refusal_serial(), 0);
    let refused = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        2,
        WhiteTakeState::Refused,
    )));
    assert!(refused.effects.contains(&error_haptic()));
    assert_eq!(screen.state.white().refusal_serial(), 1);
}

#[test]
fn a_refusal_with_a_smaller_number_from_a_new_engine_shakes_again() {
    // Номера команд начинаются заново у нового движка: «больше прежнего» здесь неверное правило.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        50,
        WhiteTakeState::Refused,
    )));
    assert_eq!(screen.state.white().refusal_serial(), 1);
    let restarted = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        3,
        WhiteTakeState::Refused,
    )));
    assert!(restarted.effects.contains(&error_haptic()));
    assert_eq!(screen.state.white().refusal_serial(), 2);
    // Те же факты повторно: тряски нет и теперь.
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        3,
        WhiteTakeState::Refused,
    )));
    assert_eq!(screen.state.white().refusal_serial(), 2);
}

#[test]
fn a_new_request_that_happens_to_reuse_the_old_number_still_shakes_after_its_waiting() {
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        1,
        WhiteTakeState::Refused,
    )));
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        1,
        WhiteTakeState::Waiting,
    )));
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        1,
        WhiteTakeState::Refused,
    )));
    assert_eq!(screen.state.white().refusal_serial(), 2);
}

#[test]
fn a_refusal_with_the_same_number_after_the_commands_vanished_shakes_again() {
    // Отказ №3, затем пустые факты (новый движок без команд), затем снова отказ №3: номера начались заново.
    let mut screen = Screen::ready();
    screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        3,
        WhiteTakeState::Refused,
    )));
    assert_eq!(screen.state.white().refusal_serial(), 1);
    screen.step(ScreenAction::CameraFactsChanged(live_facts()));
    assert_eq!(screen.state.white().refusal_serial(), 1, "пустые факты молчат");
    let again = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(
        3,
        WhiteTakeState::Refused,
    )));
    assert!(again.effects.contains(&error_haptic()));
    assert_eq!(screen.state.white().refusal_serial(), 2);
}

#[test]
fn taken_and_cancelled_outcomes_are_silent() {
    let mut screen = Screen::ready();
    for (serial, state) in [(1, WhiteTakeState::Taken), (2, WhiteTakeState::Cancelled)] {
        let transition = screen.step(ScreenAction::CameraFactsChanged(facts_with_take(serial, state)));
        assert!(!transition.effects.contains(&error_haptic()), "{state:?}");
    }
    assert_eq!(screen.state.white().refusal_serial(), 0);
}

#[test]
fn a_command_the_executor_could_not_deliver_shakes_and_vibrates_like_a_refusal() {
    let mut screen = Screen::ready();
    let failed = screen.step(ScreenAction::WhiteTakeNotDelivered);
    assert_eq!(failed.outcome, ActionOutcome::Applied);
    assert_eq!(failed.effects, [error_haptic()]);
    assert_eq!(screen.state.white().refusal_serial(), 1);
}

#[test]
fn facts_are_taken_as_is_and_origin_is_not_invented_by_the_screen() {
    let mut screen = Screen::ready();
    let held = HeldWhite {
        origin: WhiteOrigin::Manual,
        ..WhiteFacts::STARTING.held
    };
    screen.step(ScreenAction::CameraFactsChanged(CameraFacts {
        white: WhiteFacts {
            held,
            ..WhiteFacts::STARTING
        },
        ..live_facts()
    }));
    assert_eq!(screen.state.camera_facts().white.held, held);
}
