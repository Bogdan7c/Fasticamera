use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_test_support::FakeCameraRig;
use crate::scenario::observing::ObservingScenario;

#[test]
fn bare_viewfinder_suspends_the_screen_and_the_screen_surfaces_keep_it_with_fake_capture() {
    assert_eq!(
        screen_policy_for(StageBudgetSurface::BareViewfinder),
        CameraScreenPolicy::SuspendForScenario
    );
    for surface in [StageBudgetSurface::PhotoScreen, StageBudgetSurface::CalibrateScreen] {
        let policy = screen_policy_for(surface);
        assert_eq!(policy, CameraScreenPolicy::KeepRunningWithFakeCapture, "{surface:?}");
        assert!(policy.keeps_screen() && policy.wants_fake_capture());
    }
}

/// Роутер спрашивает политику ДО `start`: ответ обязан зависеть от конфига, который приходит через сервисы камеры.
#[test]
fn the_scenario_reads_the_screen_policy_from_the_configured_surface() {
    let scenario = ViewfinderStageBudgetScenario::new();
    for (surface, keeps) in [
        (StageBudgetSurface::BareViewfinder, false),
        (StageBudgetSurface::PhotoScreen, true),
        (StageBudgetSurface::CalibrateScreen, true),
    ] {
        let mut config = CameraConfig::default();
        config.scenarios.viewfinder.stage_budget.surface = surface;
        let rig = FakeCameraRig::with_config(config);
        assert_eq!(
            scenario.camera_screen_policy_for(&rig.services).keeps_screen(),
            keeps,
            "{surface:?}"
        );
    }
}
