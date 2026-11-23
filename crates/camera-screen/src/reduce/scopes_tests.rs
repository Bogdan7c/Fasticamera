//! Тесты намерений по приборам: действие экрана превращается ровно в один эффект с тем же намерением, а
//! состояние экрана и отказы не затрагиваются.

use scope_contract::{ScopeImageSize, ScopeIntent, ScopeKind, ScopeRequest, VectorscopeAxes};

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::outcome::ActionOutcome;
use crate::test_support::Screen;

fn request(kind: ScopeKind, axes: VectorscopeAxes) -> ScopeRequest {
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(256, 128).expect("размер в пределах"),
    }
}

#[test]
fn every_scope_intent_becomes_exactly_that_effect_in_order() {
    let mut screen = Screen::ready();
    let intents = [
        ScopeIntent::Show(request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709)),
        ScopeIntent::Show(request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb)),
        ScopeIntent::Hide,
    ];

    for intent in intents {
        let transition = screen.step(ScreenAction::Scopes(intent));

        assert_eq!(transition.outcome, ActionOutcome::Applied);
        assert_eq!(
            transition.effects,
            vec![Effect::Scopes(intent)],
            "ровно одно намерение наружу, значения без правок"
        );
    }
}

#[test]
fn a_scope_intent_leaves_the_screen_state_and_rejections_untouched() {
    let mut screen = Screen::ready();
    let before = screen.state.clone();

    screen.step(ScreenAction::Scopes(ScopeIntent::Show(request(
        ScopeKind::RgbParade,
        VectorscopeAxes::YCbCrBt709,
    ))));
    screen.step(ScreenAction::Scopes(ScopeIntent::Hide));

    assert_eq!(
        screen.state, before,
        "намерение по прибору не меняет состояние экрана и не считается отказом"
    );
}
