use isp_vulkan::viewfinder::IspStage;

use super::*;
use crate::power_measurement::AbPowerExperiment;
use crate::viewfinder_scenario_config::IspStageSetting;

fn suppressing(works: &[SuppressibleGpuWork]) -> GpuWorkSuppression {
    works
        .iter()
        .fold(GpuWorkSuppression::NONE, |mask, work| mask.suppressing(*work))
}

#[test]
fn a_stage_row_turns_off_exactly_that_stage_and_touches_no_other_lever() {
    let states = RowStates::of(StageBudgetRowSetting::Stage(IspStageSetting::Tone));
    assert_eq!(states.on, WorkState::PRODUCT);
    assert_eq!(states.off.painter, ViewfinderPainter::Isp);
    assert_eq!(states.off.stages, IspStageSet::ALL.without(IspStage::Tone));
    assert_eq!(states.off.suppression, GpuWorkSuppression::NONE);
    assert_eq!(states.off.display, DisplayDemand::Untouched);
    assert_eq!(states.cost, CostMetric::IspDispatch);
    assert!(!states.drives_suppression(), "старая строка команду подавления не шлёт");
}

#[test]
fn whole_viewfinder_swaps_the_painter_for_the_debug_mosaic_and_nothing_else() {
    let states = RowStates::of(StageBudgetRowSetting::WholeViewfinder);
    assert_eq!(states.on, WorkState::PRODUCT);
    assert_eq!(
        states.off,
        WorkState {
            painter: ViewfinderPainter::DebugMosaic,
            ..WorkState::PRODUCT
        }
    );
    assert_eq!(states.cost, CostMetric::FrameTotal);
    assert!(!states.drives_suppression());
}

#[test]
fn color_scopes_suppresses_only_the_scopes_in_window_b() {
    let states = RowStates::of(StageBudgetRowSetting::ColorScopes);
    assert_eq!(states.on, WorkState::PRODUCT);
    assert_eq!(states.off.suppression, suppressing(&[SuppressibleGpuWork::ColorScopes]));
    assert_eq!(states.off.painter, ViewfinderPainter::Isp);
    assert_eq!(states.off.stages, IspStageSet::ALL);
    assert_eq!(states.cost, CostMetric::ColorScopes);
    assert!(states.drives_suppression());
}

#[test]
fn ui_overlay_suppresses_only_the_ui_drawing_in_window_b() {
    let states = RowStates::of(StageBudgetRowSetting::UiOverlay);
    assert_eq!(states.on, WorkState::PRODUCT);
    assert_eq!(
        states.off.suppression,
        suppressing(&[SuppressibleGpuWork::UiOverlayDrawing])
    );
    assert_eq!(states.off.painter, ViewfinderPainter::Isp);
    assert_eq!(states.cost, CostMetric::UiOverlay);
}

#[test]
fn whole_product_is_the_camera_without_our_processing_and_without_ui() {
    let states = RowStates::of(StageBudgetRowSetting::WholeProduct);
    assert_eq!(states.on, WorkState::PRODUCT);
    assert_eq!(states.off.painter, ViewfinderPainter::DebugMosaic);
    assert_eq!(
        states.off.suppression,
        suppressing(&[SuppressibleGpuWork::ColorScopes, SuppressibleGpuWork::UiOverlayDrawing])
    );
    assert_eq!(states.cost, CostMetric::FrameTotal);
}

#[test]
fn display_row_asks_120_with_a_held_touch_in_a_and_60_without_touches_in_b() {
    let states = RowStates::of(StageBudgetRowSetting::Display120Hz);
    assert_eq!(states.on.display, DisplayDemand::Hz120WithHeldTouch);
    assert_eq!(states.on.display.target_hz(), Some(120.0));
    assert!(states.on.display.holds_touch());
    assert_eq!(states.off.display, DisplayDemand::Hz60);
    assert_eq!(states.off.display.target_hz(), Some(60.0));
    assert!(!states.off.display.holds_touch());
    // Остальное — продукт в обоих окнах: меряем только частоту.
    assert_eq!(
        (states.on.painter, states.on.stages),
        (states.off.painter, states.off.stages)
    );
    assert_eq!(states.on.suppression, GpuWorkSuppression::NONE);
    assert!(!states.drives_suppression());
}

#[test]
fn abba_order_is_unchanged_for_the_new_rows_a_is_the_first_window_of_even_pairs() {
    // Расписание живёт в эксперименте; строка лишь говорит, что такое A и B. Окна: A B | B A | A B.
    let stages: Vec<StageSetting> = (0..6).map(AbPowerExperiment::stage_of).collect();
    assert_eq!(
        stages,
        [
            StageSetting::On,
            StageSetting::Off,
            StageSetting::Off,
            StageSetting::On,
            StageSetting::On,
            StageSetting::Off
        ]
    );
    let states = RowStates::of(StageBudgetRowSetting::UiOverlay);
    let applied: Vec<bool> = stages
        .iter()
        .map(|stage| states.state_for(*stage).suppression.is_none())
        .collect();
    assert_eq!(applied, [true, false, false, true, true, false]);
}
