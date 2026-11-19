//! Источник кадра UI, который поставил сценарий, снимает КОНЕЦ СЦЕНАРИЯ при любом исходе (сессия 11):
//! настоящий маршрутизатор, подставной сценарий и запись регулировок рендера (`RecordingRenderControl`).
//! Каждый тест доходит до строки результата: снятие делается ДО неё, поэтому скрипт, увидев результат,
//! застаёт рендер без стекла.

use super::*;
use crate::gpu_test_support::RecordedUiOverlay;
use crate::scenario::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, passed};

/// Источник кадра UI, который ничего не рисует.
struct IdleUiSource;

impl ui_draw::frame_source::UiFrameSource for IdleUiSource {
    fn name(&self) -> &'static str {
        "idle-ui"
    }

    fn produce_frame(
        &mut self,
        _request: &ui_draw::frame_source::UiFrameRequest,
    ) -> Result<ui_draw::draw_list::DrawList, ui_draw::frame_source::UiFrameError> {
        Ok(ui_draw::draw_list::DrawList::new(
            ui_draw::light::LightDirection::from_angle_degrees(0.0),
        ))
    }
}

/// Что делает подставной сценарий со стеклом.
#[derive(Clone, Copy)]
enum UiBehavior {
    /// Не трогает UI вообще.
    LeavesUiAlone,
    /// Ставит источник и кончается успехом, не снимая его.
    InstallsAndFinishes,
    /// Ставит источник, сам снимает и кончается успехом.
    InstallsRemovesAndFinishes,
    /// Ставит источник и паникует.
    InstallsAndPanics,
}

/// Подставной сценарий: всё делает в `start`, итог пишет на первом тике.
struct UiProbeScenario {
    behavior: UiBehavior,
}

impl ObservingScenario for UiProbeScenario {
    fn name(&self) -> &'static str {
        "ui-probe"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        match self.behavior {
            UiBehavior::LeavesUiAlone => {}
            UiBehavior::InstallsAndFinishes => {
                assert!(
                    context
                        .gpu
                        .install_ui_overlay_for_scenario(Box::new(IdleUiSource))
                        .is_delivered()
                );
            }
            UiBehavior::InstallsRemovesAndFinishes => {
                assert!(
                    context
                        .gpu
                        .install_ui_overlay_for_scenario(Box::new(IdleUiSource))
                        .is_delivered()
                );
                assert!(context.gpu.remove_ui_overlay_for_scenario().is_delivered());
            }
            UiBehavior::InstallsAndPanics => {
                assert!(
                    context
                        .gpu
                        .install_ui_overlay_for_scenario(Box::new(IdleUiSource))
                        .is_delivered()
                );
                panic!("сценарий упал со стеклом на экране");
            }
        }
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Finished(passed(serde_json::json!({ "ok": true })))
    }
}

fn run(behavior: UiBehavior) -> Harness {
    let mut harness = harness(ScenarioConfig::default());
    harness
        .starter
        .start(Box::new(UiProbeScenario { behavior }))
        .expect("старт");
    harness.run_until_result();
    assert_eq!(
        harness.output.results.lock().expect("замок").len(),
        1,
        "сценарий дошёл до итога"
    );
    harness
}

#[test]
fn scenario_that_installed_the_source_and_finished_has_it_removed_by_the_router() {
    let harness = run(UiBehavior::InstallsAndFinishes);
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        [RecordedUiOverlay::Install("idle-ui"), RecordedUiOverlay::Remove]
    );
    assert_eq!(harness.output.single_result()["status"], "passed");
}

#[test]
fn panicking_scenario_still_has_its_source_removed() {
    let harness = run(UiBehavior::InstallsAndPanics);
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        [RecordedUiOverlay::Install("idle-ui"), RecordedUiOverlay::Remove],
        "паника сценария — тоже конец: стекло не остаётся на экране"
    );
    assert_eq!(harness.output.single_result()["status"], "panicked");
}

#[test]
fn scenario_that_never_touched_the_ui_makes_the_router_send_nothing() {
    let harness = run(UiBehavior::LeavesUiAlone);
    assert!(harness.control.recorded_ui_overlay().is_empty());
    assert!(harness.control.recorded().is_empty(), "рендер сценарий не трогал вовсе");
}

#[test]
fn scenario_that_removed_the_source_itself_is_not_removed_twice() {
    let harness = run(UiBehavior::InstallsRemovesAndFinishes);
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        [RecordedUiOverlay::Install("idle-ui"), RecordedUiOverlay::Remove]
    );
}
