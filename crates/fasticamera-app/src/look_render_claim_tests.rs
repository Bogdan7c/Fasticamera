//! Тесты заявки на вид рендера: учёт держания, снятия и клонов.

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};

use super::LookRenderClaim;

fn recipe(base: ToneBaseModel) -> LookRecipe {
    LookRecipe {
        base,
        settings: LookSettings::NEUTRAL,
    }
}

#[test]
fn a_fresh_claim_is_free_and_releasing_it_changes_nothing() {
    let claim = LookRenderClaim::default();
    assert!(!claim.is_held());
    assert_eq!(claim.release(), None, "снимать нечего");
    assert_eq!(claim.status().releases, 0, "пустое снятие счётчик не двигает");
}

#[test]
fn hold_then_release_returns_the_recipe_to_restore_and_counts_once() {
    let claim = LookRenderClaim::default();
    claim.hold(recipe(ToneBaseModel::Aces));
    assert!(claim.is_held());
    assert_eq!(claim.release(), Some(recipe(ToneBaseModel::Aces)));
    let status = claim.status();
    assert!(!status.held);
    assert_eq!(status.releases, 1);
    assert_eq!(claim.release(), None, "второе снятие пустое");
    assert_eq!(claim.status().releases, 1);
}

#[test]
fn a_repeated_hold_keeps_the_first_restore_recipe() {
    let claim = LookRenderClaim::default();
    claim.hold(recipe(ToneBaseModel::Agx));
    claim.hold(recipe(ToneBaseModel::Legacy));
    assert_eq!(claim.release(), Some(recipe(ToneBaseModel::Agx)));
}

#[test]
fn clones_share_one_state() {
    let scenario_side = LookRenderClaim::default();
    let screen_side = scenario_side.clone();
    scenario_side.hold(recipe(ToneBaseModel::Filmic));
    assert!(screen_side.is_held(), "экран видит заявку сценария");
    scenario_side.release();
    assert_eq!(screen_side.status().releases, 1);
}
