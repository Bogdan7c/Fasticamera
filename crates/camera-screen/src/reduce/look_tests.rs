//! Тесты намерений по виду: действие экрана превращается ровно в один эффект с тем же намерением, а состояние
//! экрана и отказы не затрагиваются.

use look_contract::{
    BuiltinPreset, KnobStop, LookIntent, LookKnob, LookSettings, LookWhiteStops, PresetId, PresetName, StopFraction,
};

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::outcome::{ActionOutcome, NoChangeReason};
use crate::test_support::Screen;

fn warm_look() -> LookSettings {
    LookSettings {
        warmth: LookKnob::new(0.5).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    }
}

#[test]
fn every_look_intent_becomes_exactly_that_effect_in_order() {
    let mut screen = Screen::ready();
    let name = PresetName::new("Golden hour").expect("имя");
    let intents = [
        LookIntent::SetLook(warm_look()),
        LookIntent::SelectPreset(PresetId::Builtin(BuiltinPreset::Honest)),
        LookIntent::SaveAsNewPreset(name),
        LookIntent::DiscardEdits,
    ];

    for intent in intents {
        let transition = screen.step(ScreenAction::Look(intent.clone()));

        assert_eq!(transition.outcome, ActionOutcome::Applied);
        assert_eq!(
            transition.effects,
            vec![Effect::Look(intent)],
            "ровно одно намерение наружу, значения без правок"
        );
    }
}

#[test]
fn a_look_intent_leaves_the_screen_state_and_rejections_untouched() {
    let mut screen = Screen::ready();
    let before = screen.state.clone();

    screen.step(ScreenAction::Look(LookIntent::SetLook(warm_look())));

    assert_eq!(
        screen.state, before,
        "намерение по виду не меняет состояние экрана и не считается отказом"
    );
}

#[test]
fn white_stops_are_stored_once_and_a_repeat_is_no_change_without_effects() {
    let mut screen = Screen::ready();
    assert_eq!(
        screen.state.look().knob_stops(look_contract::GlobalKnob::Tint).plus,
        KnobStop::None,
        "до первого сообщения упоров нет"
    );
    let stops = LookWhiteStops::Known(look_contract::KnobTravelStops {
        warmth: look_contract::KnobSideStops {
            minus: KnobStop::None,
            plus: KnobStop::None,
        },
        tint: look_contract::KnobSideStops {
            minus: KnobStop::None,
            plus: KnobStop::At(StopFraction::new(0.46).expect("доля")),
        },
    });

    let first = screen.step(ScreenAction::LookWhiteStopsChanged(stops));
    assert_eq!(first.outcome, ActionOutcome::Applied);
    assert!(
        first.effects.is_empty(),
        "упоры эффектов не рождают: это только рисунок"
    );
    let tint = screen.state.look().knob_stops(look_contract::GlobalKnob::Tint);
    assert_eq!(tint.plus, KnobStop::At(StopFraction::new(0.46).expect("доля")));
    assert_eq!(
        screen.state.look().knob_stops(look_contract::GlobalKnob::Warmth).plus,
        KnobStop::None,
        "теплота своих упоров не получила"
    );
    assert_eq!(
        screen.state.look().knob_stops(look_contract::GlobalKnob::Contrast).plus,
        KnobStop::None,
        "у прочих ручек упоров нет"
    );

    let repeat = screen.step(ScreenAction::LookWhiteStopsChanged(stops));
    assert_eq!(repeat.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));

    screen.step(ScreenAction::LookWhiteStopsChanged(LookWhiteStops::NotApplicable));
    assert_eq!(
        screen.state.look().knob_stops(look_contract::GlobalKnob::Tint).plus,
        KnobStop::None,
        "«не применимо» гасит прежние отметки"
    );
}
