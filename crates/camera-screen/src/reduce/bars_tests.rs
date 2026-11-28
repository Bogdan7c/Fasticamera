//! Полоски BARS в редьюсере (решение 343): точечное намерение `SetPrimary`, отказы и «ничего не менять».
//! Проверяются наблюдаемые итоги: итог действия и точные намерения наружу.

use look_contract::{LookIntent, LookSettings, PrimaryChannel, WheelZone};

use crate::action::ScreenAction;
use crate::calibration::ControlViewKind;
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts, look_facts_edited};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::test_support::Screen;

fn drag(zone: WheelZone, channel: PrimaryChannel, value: f32) -> ScreenAction {
    ScreenAction::PrimaryDragged {
        zone,
        channel,
        value: knob(value),
    }
}

#[test]
fn dragging_a_bar_sends_a_point_intent_for_that_zone_and_channel_only() {
    // Другие полоски уже правлены: намерение про Gain.G их не упоминает.
    let mut settings = LookSettings::NEUTRAL;
    settings.primaries = settings
        .primaries
        .with_value(WheelZone::Lift, PrimaryChannel::Red, knob(0.5));
    let mut screen = Screen::calibrating(look_facts_edited(settings));

    let transition = screen.step(drag(WheelZone::Gain, PrimaryChannel::Green, -0.25));

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(
        transition.effects,
        [Effect::Look(LookIntent::SetPrimary {
            zone: WheelZone::Gain,
            channel: PrimaryChannel::Green,
            value: knob(-0.25),
        })]
    );
}

#[test]
fn every_channel_and_zone_keeps_its_own_address_in_the_intent() {
    // Перестановка R ↔ B или зон ловится: каждая из 12 полосок шлёт СВОЙ адрес.
    let mut screen = Screen::calibrating(look_facts());
    for zone in WheelZone::ALL {
        for channel in PrimaryChannel::ALL {
            let transition = screen.step(drag(zone, channel, 0.3));
            assert_eq!(
                transition.effects,
                [Effect::Look(LookIntent::SetPrimary {
                    zone,
                    channel,
                    value: knob(0.3),
                })],
                "{zone:?} / {channel:?}"
            );
        }
    }
}

#[test]
fn the_same_value_as_in_the_facts_is_a_no_change_without_effects() {
    let mut settings = LookSettings::NEUTRAL;
    settings.primaries = settings
        .primaries
        .with_value(WheelZone::Offset, PrimaryChannel::Blue, knob(0.4));
    let mut screen = Screen::calibrating(look_facts_edited(settings));

    let same = screen.step(drag(WheelZone::Offset, PrimaryChannel::Blue, 0.4));
    let other_channel = screen.step(drag(WheelZone::Offset, PrimaryChannel::Red, 0.4));
    let other_zone = screen.step(drag(WheelZone::Gain, PrimaryChannel::Blue, 0.4));

    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
    assert!(same.effects.is_empty());
    assert_eq!(
        other_channel.outcome,
        ActionOutcome::Applied,
        "у R другое значение (нейтраль)"
    );
    assert_eq!(
        other_zone.outcome,
        ActionOutcome::Applied,
        "у Gain.B другое значение (нейтраль)"
    );
}

#[test]
fn bar_drags_are_rejected_outside_calibration_and_without_facts_with_distinct_reasons() {
    let mut photo = Screen::with_look(look_facts());
    let in_photo = photo.step(drag(WheelZone::Lift, PrimaryChannel::Red, 0.1));
    assert_eq!(
        in_photo.outcome,
        ActionOutcome::Rejected(Rejection::NotInCalibrationMode)
    );
    assert!(in_photo.effects.is_empty());

    // Раздел есть, а фактов о виде ещё нет: другая причина отказа.
    let mut bare = Screen::ready();
    bare.step(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    let without_facts = bare.step(drag(WheelZone::Lift, PrimaryChannel::Red, 0.1));
    assert_eq!(
        without_facts.outcome,
        ActionOutcome::Rejected(Rejection::LookFactsMissing)
    );
    assert!(without_facts.effects.is_empty());
}

#[test]
fn choosing_bars_and_a_tab_changes_only_screen_state_and_the_tab_is_shared_with_wheels() {
    let mut screen = Screen::calibrating(look_facts());

    // Вкладку выбрали в колёсах — в BARS открыта та же зона: у вида своего выбора нет.
    screen.step(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    screen.step(ScreenAction::WheelSelected(WheelZone::Gain));
    let to_bars = screen.step(ScreenAction::ControlViewSelected(ControlViewKind::Bars));

    assert_eq!(to_bars.outcome, ActionOutcome::Applied);
    assert!(
        to_bars.effects.is_empty(),
        "выбор вида — состояние экрана, а не намерение"
    );
    assert_eq!(screen.state.calibration().control_view(), ControlViewKind::Bars);
    assert_eq!(screen.state.calibration().selected_wheel(), WheelZone::Gain);
    let again = screen.step(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    assert_eq!(again.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
}
