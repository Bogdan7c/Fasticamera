//! Заявка сценария замера на вид рендера (14o) и исполнитель вида экрана: пока заявка держится, рендеру от исполнителя
//! ничего не уходит, намерения и режим показа ведутся дальше, а после снятия рендер получает то, что должно быть
//! показано. Библиотека пресетов и файл при этом не трогаются сценарием. Проверяются записанные команды рендеру.

use look_contract::{CleanView, LookIntent, LookRecipe, LookSettings, ToneBaseModel};
use render_loop::viewfinder::ViewfinderCommand;

use super::*;
use crate::gpu_test_support::RecordingRenderControl;
use crate::look_test_support::{StubPersistence, contrast_look, handle_with};
use crate::viewfinder_control::RenderAdjustment;

fn commands(render: &RecordingRenderControl) -> Vec<ViewfinderCommand> {
    render
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(command) => Some(command),
            _ => None,
        })
        .collect()
}

fn effects(stub: &StubPersistence) -> LookEffects {
    LookEffects::new(Some(handle_with(stub.clone())))
}

fn scenario_recipe() -> LookRecipe {
    LookRecipe {
        base: ToneBaseModel::Legacy,
        settings: LookSettings::NEUTRAL,
    }
}

#[test]
fn while_the_scenario_holds_the_look_intents_are_applied_but_nothing_reaches_the_render() {
    let stub = StubPersistence::accepting();
    let mut effects = effects(&stub);
    let render = RecordingRenderControl::new();
    render.look_claim().hold(effects.current_recipe());

    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    effects.set_clean_view(CleanView::ShowBase, render.as_ref());
    effects.retry_if_render_unsure(render.as_ref());

    assert!(
        commands(&render).is_empty(),
        "рендер принадлежит сценарию: {:?}",
        commands(&render)
    );
    assert_eq!(
        effects.current_recipe().settings,
        contrast_look(0.5),
        "намерение библиотекой принято: вид продукта ведётся дальше"
    );
}

#[test]
fn after_the_release_the_executor_sends_the_wanted_look_and_only_once() {
    let stub = StubPersistence::accepting();
    let mut effects = effects(&stub);
    let render = RecordingRenderControl::new();
    let claim = render.look_claim();
    claim.hold(effects.current_recipe());
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    effects.retry_if_render_unsure(render.as_ref());
    assert!(commands(&render).is_empty());

    claim.release();
    effects.retry_if_render_unsure(render.as_ref());

    assert_eq!(
        commands(&render),
        vec![
            ViewfinderCommand::SetToneBase(effects.current_recipe().base),
            ViewfinderCommand::SetLook(Arc::new(contrast_look(0.5))),
        ],
        "основа и значения продукта дошли после снятия заявки (сценарий мог оставить свои)"
    );
    effects.retry_if_render_unsure(render.as_ref());
    assert_eq!(commands(&render).len(), 2, "повторных команд без причины нет");
}

#[test]
fn a_release_with_the_clean_view_pressed_resends_neutral_values_on_the_active_base() {
    let stub = StubPersistence::accepting();
    let mut effects = effects(&stub);
    let render = RecordingRenderControl::new();
    let claim = render.look_claim();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    effects.set_clean_view(CleanView::ShowBase, render.as_ref());
    effects.retry_if_render_unsure(render.as_ref());
    claim.hold(effects.current_recipe());
    let before = commands(&render).len();

    claim.release();
    effects.retry_if_render_unsure(render.as_ref());

    let resent = &commands(&render)[before..];
    assert!(
        resent.contains(&ViewfinderCommand::SetLook(Arc::new(LookSettings::NEUTRAL))),
        "чистый вид зажат: после сценария рендер получает нейтраль, а не правки: {resent:?}"
    );
    assert!(!resent.contains(&ViewfinderCommand::SetLook(Arc::new(contrast_look(0.5)))));
}

#[test]
fn a_scenario_claim_never_touches_the_library_or_the_preset_file() {
    let stub = StubPersistence::accepting();
    let mut effects = effects(&stub);
    let render = RecordingRenderControl::new();
    let claim = render.look_claim();
    let recipe_before = effects.current_recipe();
    let saves_before = stub.requested().len();

    claim.hold(scenario_recipe());
    effects.retry_if_render_unsure(render.as_ref());
    claim.release();
    effects.retry_if_render_unsure(render.as_ref());

    assert_eq!(effects.current_recipe(), recipe_before, "рецепт библиотеки тот же");
    assert_eq!(stub.requested().len(), saves_before, "файл пресетов не писали");
    assert_eq!(effects.counters().save_requests, LookSaveRequestCounters::default());
}

#[test]
fn the_still_recipe_stays_the_product_recipe_even_while_the_scenario_holds_the_render() {
    let stub = StubPersistence::accepting();
    let mut effects = effects(&stub);
    let render = RecordingRenderControl::new();
    effects.run_intent(LookIntent::SetLook(contrast_look(0.5)), render.as_ref());
    let product = effects.current_recipe();
    render.look_claim().hold(scenario_recipe());

    assert_eq!(effects.recipe_for_still(render.as_ref()), product);
}

#[test]
fn a_fresh_executor_that_never_saw_a_claim_does_not_resend_on_its_first_frames() {
    let stub = StubPersistence::accepting();
    let mut effects = effects(&stub);
    let render = RecordingRenderControl::new();
    // Заявку ставили и снимали ДО этого исполнителя: его `shown` построен по рендеру при сборке, лишнего слать не надо.
    render.look_claim().hold(scenario_recipe());
    render.look_claim().release();

    effects.retry_if_render_unsure(render.as_ref());
    effects.retry_if_render_unsure(render.as_ref());

    assert!(commands(&render).is_empty());
}
