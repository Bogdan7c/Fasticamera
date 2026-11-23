//! Тесты пресетов: быстрый выбор (карточка Styles в PHOTO и ряд имён раздела), сброс правок, сохранение,
//! ввод имени системной клавиатурой, двойное подтверждение удаления (решения 187, 196, 197, 201).

use look_contract::{
    BuiltinPreset, GlobalKnob, LookIntent, LookSettings, PresetId, PresetName, PresetNameError, UserPresetId,
};
use shell_contract::text_input::{TextInputOutcome, TextInputResult, TextPurpose, TextRequestId};

use crate::action::ScreenAction;
use crate::calibration::{DeleteConfirmation, Naming, PendingText, TextFlow};
use crate::effect::Effect;
use crate::look_test_support::{
    NIGHT, SUNSET, band, knob, look_facts, look_facts_edited, look_facts_with_active, look_intents,
};
use crate::look_view::ProblemKind;
use crate::modes::CaptureMode;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::state::PanelState;
use crate::test_support::{Screen, at_ms};
use crate::tools::{ToolId, ToolOption};

fn edited_settings() -> LookSettings {
    LookSettings::NEUTRAL.with_knob(GlobalKnob::Contrast, knob(0.4))
}

fn finished(id: u64, outcome: TextInputOutcome) -> ScreenAction {
    ScreenAction::TextInputFinished(TextInputResult {
        id: TextRequestId(id),
        outcome,
    })
}

fn name(text: &str) -> PresetName {
    PresetName::new(text).expect("имя теста допустимо")
}

// ---------------------------------------------------------------- Styles в PHOTO

#[test]
fn styles_appears_in_the_photo_menu_only_with_look_facts() {
    let without = Screen::ready();
    assert!(!without.state.menu_tools().contains(&ToolId::Styles));

    let with = Screen::with_look(look_facts());
    let menu = with.state.menu_tools();
    assert_eq!(
        menu.last(),
        Some(&ToolId::Styles),
        "Styles стоит последним в меню Controls: {menu:?}"
    );
    assert_eq!(with.state.preset_entries().len(), 4);
    assert!(without.state.preset_entries().is_empty());
}

#[test]
fn choosing_a_style_in_the_card_selects_the_preset_and_keeps_the_card_open() {
    let mut screen = Screen::with_look(look_facts());
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Styles));
    assert_eq!(screen.state.panel(), PanelState::Submenu(ToolId::Styles));

    let chosen = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Style(PresetId::User(
        SUNSET,
    ))));

    assert_eq!(chosen.outcome, ActionOutcome::Applied);
    assert_eq!(
        chosen.effects,
        [Effect::Look(LookIntent::SelectPreset(PresetId::User(SUNSET)))]
    );
    assert_eq!(
        screen.state.panel(),
        PanelState::Submenu(ToolId::Styles),
        "карточка остаётся открытой"
    );
}

#[test]
fn a_style_option_outside_its_card_is_refused() {
    let mut screen = Screen::with_look(look_facts());

    let transition = screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Style(PresetId::User(
        SUNSET,
    ))));

    assert_eq!(
        transition.outcome,
        ActionOutcome::Rejected(Rejection::OptionNotInSubmenu)
    );
    assert!(transition.effects.is_empty());
}

#[test]
fn the_styles_card_reset_discards_edits_and_is_dim_when_clean() {
    let mut clean = Screen::with_look(look_facts());
    let dim = clean.step(ScreenAction::DiscardEditsTapped);
    assert_eq!(dim.outcome, ActionOutcome::NoChange(NoChangeReason::NoEditsOnTop));
    assert!(dim.effects.is_empty());

    let mut edited = Screen::with_look(look_facts_edited(edited_settings()));
    let reset = edited.step(ScreenAction::DiscardEditsTapped);
    assert_eq!(reset.effects, [Effect::Look(LookIntent::DiscardEdits)]);
}

// ---------------------------------------------------------------- выбор пресета

#[test]
fn selecting_a_preset_sends_the_intent_in_any_mode() {
    for mode in [CaptureMode::Photo, CaptureMode::Calibrate] {
        let mut screen = Screen::with_look(look_facts());
        screen.step(ScreenAction::ModeSelected(mode));

        let transition = screen.step(ScreenAction::PresetSelected(PresetId::Builtin(BuiltinPreset::Honest)));

        assert_eq!(
            transition.effects,
            [Effect::Look(LookIntent::SelectPreset(PresetId::Builtin(
                BuiltinPreset::Honest
            )))],
            "{mode:?}"
        );
    }
}

#[test]
fn selecting_the_active_clean_preset_changes_nothing_but_with_edits_it_reverts_them() {
    let active = PresetId::Builtin(BuiltinPreset::AsEye);
    let mut clean = Screen::with_look(look_facts());
    let same = clean.step(ScreenAction::PresetSelected(active));
    assert_eq!(same.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));

    let mut edited = Screen::with_look(look_facts_edited(edited_settings()));
    let revert = edited.step(ScreenAction::PresetSelected(active));
    assert_eq!(revert.effects, [Effect::Look(LookIntent::SelectPreset(active))]);
}

#[test]
fn selecting_an_unlisted_preset_or_selecting_without_facts_is_rejected() {
    let mut screen = Screen::with_look(look_facts());
    let unlisted = screen.step(ScreenAction::PresetSelected(PresetId::User(UserPresetId(999))));
    assert_eq!(unlisted.outcome, ActionOutcome::Rejected(Rejection::UnknownPreset));

    let mut no_facts = Screen::ready();
    let missing = no_facts.step(ScreenAction::PresetSelected(PresetId::Builtin(BuiltinPreset::Honest)));
    assert_eq!(missing.outcome, ActionOutcome::Rejected(Rejection::LookFactsMissing));
}

// ---------------------------------------------------------------- перезапись

#[test]
fn saving_over_the_active_preset_works_for_user_presets_only() {
    let mut builtin = Screen::calibrating(look_facts_edited(edited_settings()));
    let refused = builtin.step(ScreenAction::OverwritePresetTapped);
    assert_eq!(
        refused.outcome,
        ActionOutcome::Rejected(Rejection::BuiltinPresetReadOnly)
    );
    assert!(refused.effects.is_empty());

    let mut user_facts = look_facts_edited(edited_settings());
    user_facts.active = PresetId::User(SUNSET);
    let mut user = Screen::calibrating(user_facts);
    let saved = user.step(ScreenAction::OverwritePresetTapped);
    assert_eq!(saved.effects, [Effect::Look(LookIntent::OverwritePreset(SUNSET))]);

    let mut clean = Screen::calibrating(look_facts_with_active(PresetId::User(SUNSET)));
    let nothing = clean.step(ScreenAction::OverwritePresetTapped);
    assert_eq!(nothing.outcome, ActionOutcome::NoChange(NoChangeReason::NoEditsOnTop));
}

#[test]
fn preset_management_is_not_available_in_photo() {
    let mut screen = Screen::with_look(look_facts_with_active(PresetId::User(SUNSET)));

    for action in [
        ScreenAction::OverwritePresetTapped,
        ScreenAction::CreatePresetTapped,
        ScreenAction::RenamePresetTapped(SUNSET),
        ScreenAction::DeletePresetTapped(SUNSET),
    ] {
        let transition = screen.step(action.clone());
        assert_eq!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotInCalibrationMode),
            "{action:?}"
        );
        assert!(transition.effects.is_empty());
    }
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
}

// ---------------------------------------------------------------- ввод имени

#[test]
fn creating_a_preset_asks_for_a_name_then_saves_it_under_the_entered_name() {
    let mut screen = Screen::calibrating(look_facts());

    let asked = screen.step(ScreenAction::CreatePresetTapped);
    assert_eq!(
        asked.effects,
        [Effect::RequestText {
            purpose: TextPurpose::NewPresetName,
            initial_text: String::new(),
        }]
    );
    assert_eq!(
        screen.state.calibration().text_flow(),
        TextFlow::AwaitingId {
            naming: Naming::NewPreset,
            previous_dialog: None
        }
    );

    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(5) });
    assert_eq!(
        screen.state.calibration().text_flow(),
        TextFlow::Pending(PendingText {
            id: TextRequestId(5),
            naming: Naming::NewPreset
        })
    );

    let entered = screen.step(finished(5, TextInputOutcome::Entered("  Warm film ".to_owned())));
    assert_eq!(
        entered.effects,
        [Effect::Look(LookIntent::SaveAsNewPreset(name("Warm film")))]
    );
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
    assert!(screen.state.look().banner().is_none());
}

#[test]
fn renaming_starts_with_the_current_name_and_sends_the_rename() {
    let mut screen = Screen::calibrating(look_facts());

    let asked = screen.step(ScreenAction::RenamePresetTapped(SUNSET));
    assert_eq!(
        asked.effects,
        [Effect::RequestText {
            purpose: TextPurpose::RenamePreset,
            initial_text: "Sunset".to_owned(),
        }]
    );
    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(1) });

    let entered = screen.step(finished(1, TextInputOutcome::Entered("Dusk".to_owned())));
    assert_eq!(
        entered.effects,
        [Effect::Look(LookIntent::RenamePreset {
            preset: SUNSET,
            name: name("Dusk")
        })]
    );
}

#[test]
fn renaming_an_unknown_preset_is_rejected_without_asking_anything() {
    let mut screen = Screen::calibrating(look_facts());

    let transition = screen.step(ScreenAction::RenamePresetTapped(UserPresetId(404)));

    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::UnknownPreset));
    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
}

#[test]
fn cancelled_and_superseded_answers_clear_the_wait_without_a_problem_or_an_intent() {
    for outcome in [TextInputOutcome::Cancelled, TextInputOutcome::Superseded] {
        let mut screen = Screen::calibrating(look_facts());
        screen.step(ScreenAction::CreatePresetTapped);
        screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(2) });

        let transition = screen.step(finished(2, outcome.clone()));

        assert_eq!(transition.outcome, ActionOutcome::Applied, "{outcome:?}");
        assert!(transition.effects.is_empty(), "{outcome:?}");
        assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
        assert!(screen.state.look().banner().is_none(), "{outcome:?}");
    }
}

#[test]
fn an_unavailable_keyboard_clears_the_wait_and_shows_a_problem() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::CreatePresetTapped);
    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(2) });

    let transition = screen.step(finished(2, TextInputOutcome::Unavailable));

    assert!(transition.effects.is_empty());
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
    assert_eq!(
        screen.state.look().banner().map(|banner| banner.kind.clone()),
        Some(ProblemKind::KeyboardUnavailable)
    );
}

#[test]
fn a_request_that_could_not_be_sent_shows_the_keyboard_problem_and_stale_feedback_is_ignored() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::CreatePresetTapped);

    let failed = screen.step(ScreenAction::TextRequestNotSent);
    assert_eq!(failed.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
    assert_eq!(
        screen.state.look().banner().map(|banner| banner.kind.clone()),
        Some(ProblemKind::KeyboardUnavailable)
    );

    let again = screen.step(ScreenAction::TextRequestNotSent);
    assert_eq!(again.outcome, ActionOutcome::NoChange(NoChangeReason::StaleTextResult));
    let orphan = screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(9) });
    assert_eq!(orphan.outcome, ActionOutcome::NoChange(NoChangeReason::StaleTextResult));
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
}

#[test]
fn an_invalid_name_becomes_a_local_problem_and_sends_no_intent() {
    let cases = [
        ("   ".to_owned(), PresetNameError::Empty),
        ("я".repeat(41), PresetNameError::TooLong { max: 40, actual: 41 }),
        ("a\nb".to_owned(), PresetNameError::ControlCharacter { found: '\n' }),
    ];
    for (text, expected) in cases {
        let mut screen = Screen::calibrating(look_facts());
        screen.step(ScreenAction::CreatePresetTapped);
        screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(3) });

        let transition = screen.step(finished(3, TextInputOutcome::Entered(text)));

        assert!(look_intents(&transition.effects).is_empty(), "{expected:?}");
        assert_eq!(
            screen.state.look().banner().map(|banner| banner.kind.clone()),
            Some(ProblemKind::InvalidPresetName(expected.clone())),
            "{expected:?}"
        );
        assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
    }
}

#[test]
fn an_answer_with_another_id_or_without_a_request_is_stale_and_changes_nothing() {
    let mut screen = Screen::calibrating(look_facts());

    let orphan = screen.step(finished(1, TextInputOutcome::Entered("Warm".to_owned())));
    assert_eq!(orphan.outcome, ActionOutcome::NoChange(NoChangeReason::StaleTextResult));
    assert!(orphan.effects.is_empty());

    screen.step(ScreenAction::CreatePresetTapped);
    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(7) });
    let before = screen.state.clone();
    let foreign = screen.step(finished(6, TextInputOutcome::Entered("Warm".to_owned())));

    assert_eq!(
        foreign.outcome,
        ActionOutcome::NoChange(NoChangeReason::StaleTextResult)
    );
    assert!(foreign.effects.is_empty());
    assert_eq!(
        screen.state.calibration(),
        before.calibration(),
        "ожидание не сбито чужим ответом"
    );
}

#[test]
fn a_newer_request_supersedes_the_older_one_and_the_late_answers_of_the_old_one_are_stale() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::CreatePresetTapped);
    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(1) });
    screen.step(ScreenAction::RenamePresetTapped(NIGHT));

    // Брокер вытеснил первый запрос: его `Superseded` приходит до номера второго и закрывает старый диалог.
    let superseded = screen.step(finished(1, TextInputOutcome::Superseded));
    assert_eq!(superseded.outcome, ActionOutcome::Applied);
    assert!(superseded.effects.is_empty());
    assert_eq!(
        screen.state.calibration().text_flow(),
        TextFlow::AwaitingId {
            naming: Naming::Rename(NIGHT),
            previous_dialog: None
        }
    );

    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(2) });
    let late = screen.step(finished(1, TextInputOutcome::Entered("Old".to_owned())));
    assert_eq!(late.outcome, ActionOutcome::NoChange(NoChangeReason::StaleTextResult));

    let current = screen.step(finished(2, TextInputOutcome::Entered("Moon".to_owned())));
    assert_eq!(
        current.effects,
        [Effect::Look(LookIntent::RenamePreset {
            preset: NIGHT,
            name: name("Moon")
        })]
    );
}

// ---------------------------------------------------------------- удаление

#[test]
fn delete_needs_two_taps_on_the_same_preset() {
    let mut screen = Screen::calibrating(look_facts());

    let first = screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    assert_eq!(first.outcome, ActionOutcome::Applied);
    assert!(first.effects.is_empty(), "первый тап ничего не удаляет");
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Armed(SUNSET)
    );

    let second = screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    assert_eq!(second.effects, [Effect::Look(LookIntent::DeletePreset(SUNSET))]);
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );

    // Третий тап начинает заново: взведение, а не второе удаление.
    let third = screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    assert!(third.effects.is_empty());
}

#[test]
fn a_tap_on_another_preset_moves_the_armed_state_instead_of_deleting() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::DeletePresetTapped(SUNSET));

    let other = screen.step(ScreenAction::DeletePresetTapped(NIGHT));

    assert!(other.effects.is_empty());
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Armed(NIGHT)
    );
}

#[test]
fn background_actions_keep_the_armed_state_but_any_other_user_action_drops_it() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    screen.step(ScreenAction::Tick(at_ms(500)));
    screen.push_look(look_facts());
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Armed(SUNSET)
    );

    screen.step(ScreenAction::BandSelected(band("sky")));
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );
    let after = screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    assert!(after.effects.is_empty(), "после чужого действия нужен снова первый тап");
}

#[test]
fn even_a_rejected_or_no_change_action_in_between_cancels_the_armed_delete() {
    // Отказ откатывает рабочую копию состояния, но отмена намерения «удалить» обязана пережить откат.
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    let rejected = screen.step(ScreenAction::ShutterTapped { at: at_ms(10) });
    assert_eq!(
        rejected.outcome,
        ActionOutcome::Rejected(Rejection::ShutterAbsentInMode)
    );
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );
    let confirm = screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    assert!(confirm.effects.is_empty(), "второй тап после отказа не должен удалить");

    screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );
    screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    let no_change = screen.step(ScreenAction::CleanViewReleased);
    assert_eq!(
        no_change.outcome,
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    );
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );
}

#[test]
fn delete_of_an_unknown_preset_is_rejected_and_a_vanished_preset_disarms() {
    let mut screen = Screen::calibrating(look_facts());
    let unknown = screen.step(ScreenAction::DeletePresetTapped(UserPresetId(404)));
    assert_eq!(unknown.outcome, ActionOutcome::Rejected(Rejection::UnknownPreset));

    screen.step(ScreenAction::DeletePresetTapped(SUNSET));
    let mut without_sunset = look_facts();
    without_sunset
        .presets
        .retain(|entry| entry.id != PresetId::User(SUNSET));
    screen.push_look(without_sunset);
    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );
}

#[test]
fn switching_the_mode_drops_the_armed_delete() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::DeletePresetTapped(SUNSET));

    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));

    assert_eq!(
        screen.state.calibration().delete_confirmation(),
        DeleteConfirmation::Idle
    );
}

#[test]
fn a_refused_second_request_keeps_the_first_dialog_alive_and_its_name_is_not_lost() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::CreatePresetTapped);
    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(1) });
    // Второй запрос ушёл, но брокер его не принял: первый диалог остаётся открытым.
    screen.step(ScreenAction::RenamePresetTapped(NIGHT));

    let failed = screen.step(ScreenAction::TextRequestNotSent);
    assert_eq!(failed.outcome, ActionOutcome::Applied);
    assert_eq!(
        screen.state.calibration().text_flow(),
        TextFlow::Pending(PendingText {
            id: TextRequestId(1),
            naming: Naming::NewPreset
        }),
        "первый диалог восстановлен"
    );

    let entered = screen.step(finished(1, TextInputOutcome::Entered("Warm film".to_owned())));
    assert_eq!(
        entered.effects,
        [Effect::Look(LookIntent::SaveAsNewPreset(name("Warm film")))],
        "имя из первого диалога дошло до библиотеки"
    );
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
}

#[test]
fn the_first_dialog_answered_while_the_second_waits_for_its_id_is_applied_and_not_restored_later() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::CreatePresetTapped);
    screen.step(ScreenAction::TextRequestIssued { id: TextRequestId(1) });
    screen.step(ScreenAction::RenamePresetTapped(NIGHT));

    let entered = screen.step(finished(1, TextInputOutcome::Entered("Warm film".to_owned())));
    assert_eq!(
        entered.effects,
        [Effect::Look(LookIntent::SaveAsNewPreset(name("Warm film")))]
    );

    // Второй запрос потом не ушёл: воскрешать уже отвеченный диалог нельзя.
    screen.step(ScreenAction::TextRequestNotSent);
    assert_eq!(screen.state.calibration().text_flow(), TextFlow::Idle);
}
