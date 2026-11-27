//! Заявка сценария на вид рендера (14o): что уходит рендеру, как держится заявка и что возвращает конец сценария.

use std::sync::Arc;

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};
use render_loop::config::RenderLoopConfig;
use render_loop::viewfinder::ViewfinderCommand;

use super::*;
use crate::gpu_services::UnavailableDiagnostics;
use crate::gpu_test_support::{FakeRenderStatus, RecordingRenderControl, presenting_status};
use crate::measurement_look::{MeasurementLook, MeasurementLookConfig};

fn gpu_over(control: Arc<dyn RenderControl>) -> GpuServices {
    GpuServices::new(
        FakeRenderStatus::live(presenting_status(), crate::gpu_services::ValidationState::Enabled),
        Arc::new(UnavailableDiagnostics::new("не нужны")),
        RenderLoopConfig::default(),
        control,
    )
}

/// Вид продукта: основа AgX, тон-образец (ручки тона не в нуле).
fn product() -> LookRecipe {
    let neutral = LookRecipe {
        base: ToneBaseModel::Agx,
        settings: LookSettings::NEUTRAL,
    };
    MeasurementLook::ToneSample.recipe_from(&neutral, &MeasurementLookConfig::default())
}

fn legacy_window() -> ScenarioLook {
    let product = product();
    ScenarioLook {
        shown: MeasurementLook::ToneBase(ToneBaseModel::Legacy)
            .recipe_from(&product, &MeasurementLookConfig::default()),
        restore_to: product,
    }
}

fn look_commands(control: &RecordingRenderControl) -> Vec<ViewfinderCommand> {
    control
        .recorded()
        .into_iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(command) => Some(command),
            _ => None,
        })
        .collect()
}

#[test]
fn showing_a_look_sends_base_then_settings_and_takes_the_render_claim() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_over(control.clone());
    let look = legacy_window();

    assert!(gpu.show_look_for_scenario(look.clone()).is_delivered());

    assert_eq!(
        look_commands(&control),
        vec![
            ViewfinderCommand::SetToneBase(ToneBaseModel::Legacy),
            ViewfinderCommand::SetLook(Arc::new(look.shown.settings)),
        ]
    );
    assert!(
        control.look_claim().is_held(),
        "заявка держится: исполнитель вида экрана молчит"
    );
}

#[test]
fn an_unavailable_render_is_reported_to_the_scenario_and_not_swallowed() {
    let gpu = gpu_over(Arc::new(UnavailableRenderControl::new("рендера нет")));
    let delivery = gpu.show_look_for_scenario(legacy_window());
    assert!(!delivery.is_delivered(), "{delivery:?}");
}

#[test]
fn scenario_end_gives_the_product_look_back_once_and_frees_the_claim() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_over(control.clone());
    let shell = super::tests::RateRecorder::default();
    let look = legacy_window();
    gpu.show_look_for_scenario(look.clone());

    let effects = gpu.render_scenario_ended(&shell);

    assert!(!control.look_claim().is_held());
    assert_eq!(control.look_claim().status().releases, 1);
    let commands = look_commands(&control);
    assert_eq!(
        commands[2..],
        [
            ViewfinderCommand::SetToneBase(ToneBaseModel::Agx),
            ViewfinderCommand::SetLook(Arc::new(look.restore_to.settings)),
        ],
        "после сценария рендер получил вид продукта"
    );
    assert!(effects.adjustments.iter().all(|(_, delivery)| delivery.is_delivered()));
    // Второй конец сценария ничего не шлёт: заявки нет.
    let before = control.recorded().len();
    assert!(gpu.render_scenario_ended(&shell).is_empty());
    assert_eq!(control.recorded().len(), before);
}

#[test]
fn releasing_in_the_middle_hands_the_look_back_and_the_end_sends_nothing_more() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_over(control.clone());
    let shell = super::tests::RateRecorder::default();
    assert_eq!(gpu.release_look_for_scenario(), None, "без заявки отдавать нечего");
    assert!(control.recorded().is_empty());

    gpu.show_look_for_scenario(legacy_window());
    let delivery = gpu.release_look_for_scenario().expect("заявка держалась");
    assert!(delivery.is_delivered());
    assert!(!control.look_claim().is_held());
    let after_release = control.recorded().len();

    gpu.render_scenario_ended(&shell);
    assert_eq!(
        control.recorded().len(),
        after_release,
        "конец сценария повторно вид не возвращает"
    );
}

#[test]
fn a_repeated_show_keeps_the_first_restore_recipe() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_over(control.clone());
    let first = legacy_window();
    gpu.show_look_for_scenario(first.clone());
    // Вторая заявка (следующее окно) несёт тот же продукт, но даже иной не должен затереть первый.
    let mut second = legacy_window();
    second.restore_to.base = ToneBaseModel::Filmic;
    gpu.show_look_for_scenario(second);

    gpu.release_look_for_scenario();
    let commands = look_commands(&control);
    assert_eq!(
        commands[commands.len() - 2],
        ViewfinderCommand::SetToneBase(first.restore_to.base)
    );
}
