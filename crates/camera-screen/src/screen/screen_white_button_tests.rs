//! Число белого на кнопке у затвора через настоящие кадры экрана: Кельвины над «K», жёлтые при ручном белом,
//! белые у телефона и запасного света. Вид доводит пакет виджетов панели; здесь закреплено, что число доходит до кадра.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId};
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::facts::CameraFacts;
use crate::test_support::live_facts;
use crate::white_facts::WhiteFacts;

fn facts_with_white(kelvin: f64, origin: WhiteOrigin) -> CameraFacts {
    CameraFacts {
        white: WhiteFacts {
            held: HeldWhite {
                setting: WhiteBalanceSetting {
                    temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
                    tint: Tint::NEUTRAL,
                },
                origin,
            },
            ..WhiteFacts::STARTING
        },
        ..live_facts()
    }
}

#[test]
fn the_button_shows_the_kelvin_number_over_a_k_and_follows_the_held_white() {
    let mut rig = Rig::new();
    let tokens = UiKitTokens::default();
    rig.act(ScreenAction::CameraFactsChanged(facts_with_white(
        3_146.0,
        WhiteOrigin::Source(WhiteSourceId::PHONE),
    )));
    rig.run(300);
    // Число округлено до 10 К и белое (белое, взятое у телефона).
    let number = texts_of(&rig.last, "3150", TextRoleKind::StripValue);
    let unit_count = text_count_of(&rig.last, "K", TextRoleKind::StripUnit);
    assert_eq!(number.len(), 1);
    assert_eq!(unit_count, 1);
    assert_eq!(number[0].color, tokens.colors.text_primary);

    // Рука поставила другое белое: число сменилось и стало жёлтым.
    rig.act(ScreenAction::CameraFactsChanged(facts_with_white(
        3_200.0,
        WhiteOrigin::Manual,
    )));
    rig.run(300);
    assert_eq!(text_count_of(&rig.last, "3150", TextRoleKind::StripValue), 0);
    let manual = texts_of(&rig.last, "3200", TextRoleKind::StripValue);
    assert_eq!(manual.len(), 1);
    assert_eq!(manual[0].color, tokens.colors.focus_yellow);
    assert_eq!(
        texts_of(&rig.last, "K", TextRoleKind::StripUnit)[0].color,
        tokens.colors.focus_yellow
    );
}

#[test]
fn the_starting_white_shows_the_fallback_daylight_in_white() {
    let mut rig = Rig::new();
    let tokens = UiKitTokens::default();
    rig.frame();
    let number = texts_of(&rig.last, "5600", TextRoleKind::StripValue);
    assert_eq!(number.len(), 1);
    assert_eq!(number[0].color, tokens.colors.text_primary);
}
