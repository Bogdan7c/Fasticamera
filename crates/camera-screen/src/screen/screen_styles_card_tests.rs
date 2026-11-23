//! Карточка STYLES экрана PHOTO (решения 187, 201): ряд имён пресетов, подсветка активного, выбор тапом и «↻» —
//! вернуть правки к пресету. Касание → действие → эффект и рисунок в списке.

use look_contract::{BuiltinPreset, LookIntent, PresetId};
use ui_draw::icons::IconId;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::screen_calibration_tests::{center, edited};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::look_test_support::{SUNSET, look_facts, look_facts_with_active};
use crate::tools::ToolId;
use look_contract::LookFacts;

/// PHOTO с фактами о виде; меню Controls → пункт STYLES → карточка осела.
fn rig_with_styles_card(facts: LookFacts) -> Rig {
    let mut rig = Rig::new();
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(facts)));
    rig.run(100);
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
    let slot = rig
        .screen
        .state()
        .menu_tools()
        .iter()
        .position(|tool| *tool == ToolId::Styles)
        .expect("пункт STYLES в меню при наличии фактов о виде");
    let item = center(&rig, "controls_item", slot);
    rig.tap(item);
    rig.run(900);
    rig.effects();
    rig
}

fn look_intents_of(rig: &mut Rig) -> Vec<LookIntent> {
    crate::look_test_support::look_intents(&rig.effects())
        .into_iter()
        .cloned()
        .collect()
}

#[test]
fn the_styles_card_shows_every_preset_name_with_the_active_one_yellow_and_its_name_as_the_value() {
    let mut rig = rig_with_styles_card(look_facts());
    let list = rig.frame().clone();
    assert_eq!(
        text_count_of(&list, "STYLES", TextRoleKind::CardTitle),
        1,
        "заголовок карточки"
    );
    let yellow = UiKitTokens::default().colors.focus_yellow;
    for name in ["Honest", "As Eye", "Sunset", "Night"] {
        let drawn = texts_of(&list, name, TextRoleKind::CardOption);
        assert_eq!(drawn.len(), 1, "имя {name} в ряду");
        assert_eq!(
            drawn[0].color == yellow,
            name == "As Eye",
            "{name}: жёлтым только активный"
        );
    }
    assert_eq!(
        text_count_of(&list, "As Eye", TextRoleKind::CardValue),
        1,
        "крупное значение — имя активного пресета"
    );
}

#[test]
fn tapping_a_name_in_the_styles_card_selects_the_preset_and_keeps_the_card_open() {
    let mut rig = rig_with_styles_card(look_facts());
    let sunset = center(&rig, "styles_chip", 2);
    rig.tap(sunset);
    rig.run(100);
    assert_eq!(
        look_intents_of(&mut rig),
        vec![LookIntent::SelectPreset(PresetId::User(SUNSET))]
    );
    assert_eq!(
        rig.screen.state().panel(),
        crate::state::PanelState::Submenu(ToolId::Styles),
        "карточка осталась открытой (решение 103)"
    );
    // Библиотека применила выбор и прислала новые факты: подсветка и значение переехали на «Sunset».
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts_with_active(
            PresetId::User(SUNSET),
        ))));
    let list = rig.run(100).clone();
    let yellow = UiKitTokens::default().colors.focus_yellow;
    assert_eq!(texts_of(&list, "Sunset", TextRoleKind::CardOption)[0].color, yellow);
    assert_eq!(
        text_count_of(&list, "Sunset", TextRoleKind::CardValue),
        1,
        "значение сменилось вместе с пресетом"
    );
    assert_eq!(text_count_of(&list, "As Eye", TextRoleKind::CardValue), 0);
    // Повторный тап по активному пресету без правок ничего не шлёт.
    rig.tap(center(&rig, "styles_chip", 2));
    rig.run(100);
    assert!(look_intents_of(&mut rig).is_empty());
}

#[test]
fn the_styles_reset_arrow_discards_edits_and_is_dim_and_silent_when_there_are_none() {
    let mut rig = rig_with_styles_card(edited(|settings| settings));
    let list = rig.frame().clone();
    let reset = icons_of(&list, IconId::Reset);
    assert_eq!(reset.len(), 1);
    assert!(
        near(reset[0].opacity.get(), 1.0, 0.01),
        "с правками «↻» яркая: {}",
        reset[0].opacity.get()
    );
    let right = center(&rig, "card_button", 1);
    rig.tap(right);
    rig.run(100);
    assert_eq!(look_intents_of(&mut rig), vec![LookIntent::DiscardEdits]);

    let mut clean = rig_with_styles_card(look_facts_with_active(PresetId::Builtin(BuiltinPreset::AsEye)));
    let list = clean.frame().clone();
    let reset = icons_of(&list, IconId::Reset);
    assert_eq!(reset.len(), 1);
    let dim = crate::config::ScreenConfig::default().widgets.card.reset_dim_opacity;
    assert!(
        near(reset[0].opacity.get(), dim, 0.02),
        "без правок «↻» тусклая: {}",
        reset[0].opacity.get()
    );
    let right = center(&clean, "card_button", 1);
    clean.tap(right);
    clean.run(100);
    assert!(look_intents_of(&mut clean).is_empty(), "тусклая «↻» молчит");
}
