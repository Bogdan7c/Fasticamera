//! Тесты намерений по виду: действие экрана превращается ровно в один эффект с тем же намерением, а состояние
//! экрана и отказы не затрагиваются.

use look_contract::{BuiltinPreset, LookIntent, LookKnob, LookSettings, PresetId, PresetName};

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::outcome::ActionOutcome;
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
