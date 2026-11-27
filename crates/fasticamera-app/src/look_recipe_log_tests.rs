//! Тесты строк лога рецепта: формат, отпечаток и согласованность двух строк.

use look_contract::{CleanView, GlobalKnob, LookKnob, LookRecipe, LookSettings, ToneBaseModel};

use super::{
    EDIT_SETTLE_FRAMES, EditSettleTracker, RecipeOrigin, ScenarioClaimAtPress, ViewfinderAtPress, recipe_line,
    still_press_line,
};

fn recipe() -> LookRecipe {
    LookRecipe {
        base: ToneBaseModel::Filmic,
        settings: LookSettings::NEUTRAL.with_knob(GlobalKnob::ALL[0], LookKnob::new(0.25).unwrap()),
    }
}

#[test]
fn recipe_line_carries_origin_fingerprint_base_and_parsable_settings() {
    let recipe = recipe();
    let line = recipe_line(RecipeOrigin::PresetChanged, &recipe);
    assert!(line.starts_with("look_recipe origin=preset_changed "), "{line}");
    assert!(line.contains(&format!("fp={}", recipe.fingerprint_hex())), "{line}");
    assert!(line.contains("base=filmic"), "{line}");
    let json = line.split("settings=").nth(1).expect("в строке есть settings=");
    let parsed: LookSettings = serde_json::from_str(json).expect("settings читается обратно как LookSettings");
    assert_eq!(parsed, recipe.settings, "строка лога несёт ровно выставленный вид");
    assert!(!line.contains('\n'), "одна строка лога — одна запись");
}

#[test]
fn every_origin_has_a_distinct_name() {
    let origins = [
        RecipeOrigin::Startup,
        RecipeOrigin::PresetChanged,
        RecipeOrigin::EditSettled,
        RecipeOrigin::EditBeforeStill,
        RecipeOrigin::CleanViewShowBase,
        RecipeOrigin::CleanViewShowEdited,
        RecipeOrigin::ScenarioClaim,
        RecipeOrigin::ScenarioRelease,
    ];
    let names: std::collections::HashSet<_> = origins.iter().map(|origin| origin.name()).collect();
    assert_eq!(names.len(), origins.len());
}

#[test]
fn still_press_line_uses_the_same_fingerprint_for_still_and_viewfinder() {
    let recipe = recipe();
    let line = still_press_line(
        &recipe,
        &ViewfinderAtPress::Confirmed(recipe.clone()),
        CleanView::ShowEdited,
        ScenarioClaimAtPress::Free,
    );
    let fp = recipe.fingerprint_hex();
    assert_eq!(
        line,
        format!("look_still_press fp={fp} view_fp={fp} clean_view=show_edited scenario_claim=free")
    );
}

#[test]
fn still_press_line_reports_unknown_view_and_held_claim() {
    let line = still_press_line(
        &recipe(),
        &ViewfinderAtPress::Unknown,
        CleanView::ShowBase,
        ScenarioClaimAtPress::Held,
    );
    assert!(line.contains("view_fp=unknown"), "{line}");
    assert!(line.contains("clean_view=show_base"), "{line}");
    assert!(line.ends_with("scenario_claim=held"), "{line}");
}

#[test]
fn settle_tracker_fires_once_after_the_quiet_period_and_resets_on_new_edit() {
    let mut tracker = EditSettleTracker::default();
    assert!(!tracker.tick(), "без правок нечего записывать");
    tracker.note_edit();
    for _ in 0..EDIT_SETTLE_FRAMES - 1 {
        assert!(!tracker.tick());
    }
    tracker.note_edit(); // палец ещё двигается: отсчёт с нуля
    for _ in 0..EDIT_SETTLE_FRAMES - 1 {
        assert!(!tracker.tick(), "новая правка отодвинула запись");
    }
    assert!(tracker.tick(), "покой выдержан: запись");
    assert!(!tracker.tick(), "жест записывается один раз");
    assert!(!tracker.has_pending());
}

#[test]
fn settle_tracker_take_pending_flushes_early_and_only_once() {
    let mut tracker = EditSettleTracker::default();
    assert!(!tracker.take_pending());
    tracker.note_edit();
    assert!(tracker.has_pending());
    assert!(tracker.take_pending());
    assert!(!tracker.take_pending());
    assert!(!tracker.tick(), "после досрочной записи покой ничего не пишет");
}
