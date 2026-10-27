use std::sync::Arc;
use std::time::Duration;

use render_loop::camera_diagnostics::{CameraStreamDiagnostics, PresentedCameraFrame};
use shell_contract::clock::MonotonicNanos;

use super::*;
use crate::gpu_services::{UnavailableDiagnostics, ValidationState};
use crate::gpu_test_support::{FakeRenderStatus, presenting_status};

fn gpu_over(render: Arc<FakeRenderStatus>) -> GpuServices {
    GpuServices::new(
        render,
        Arc::new(UnavailableDiagnostics::new("не нужны")),
        render_loop::config::RenderLoopConfig::default(),
        crate::gpu_test_support::RecordingRenderControl::new(),
    )
}

#[test]
fn unavailable_render_is_reported_with_its_reason_not_as_missing_camera() {
    let gpu = gpu_over(FakeRenderStatus::unavailable("GPU не поднялся"));
    assert_eq!(
        gpu.observe_camera_presentation(),
        CameraPresentationObservation::RenderUnavailable {
            reason: "GPU не поднялся".to_owned()
        }
    );
}

#[test]
fn render_without_a_camera_feed_is_a_separate_answer() {
    let gpu = gpu_over(FakeRenderStatus::live(presenting_status(), ValidationState::Enabled));
    assert_eq!(
        gpu.observe_camera_presentation(),
        CameraPresentationObservation::NotShowingCamera
    );
}

#[test]
fn live_camera_presentation_carries_counters_recent_times_display_rate_and_validation() {
    let mut status = presenting_status();
    status.backend.validation_errors = 3;
    status.backend.camera = Some(CameraStreamDiagnostics {
        camera_frames_presented: 42,
        last_presented_frame: Some(PresentedCameraFrame {
            sequence: 7,
            sensor_timestamp_nanos: 1_000,
            frame_number: Some(9),
            present_id: 5,
        }),
        ..CameraStreamDiagnostics::default()
    });
    status.recent_presentations = vec![render_loop::backend::PastPresentation {
        present_id: 5,
        frame_index: 1,
        actual_present: MonotonicNanos::from_nanos(2_000),
    }];
    status.pacing_refresh_period = Duration::from_secs_f64(1.0 / 90.0);
    let gpu = gpu_over(FakeRenderStatus::live(status, ValidationState::Enabled));

    let CameraPresentationObservation::Live(presentation) = gpu.observe_camera_presentation() else {
        panic!("рендер показывает камеру");
    };
    assert_eq!(presentation.diagnostics.camera_frames_presented, 42);
    assert_eq!(presentation.recent_presentations.len(), 1);
    assert!(
        (presentation.display_refresh_hz - 90.0).abs() < 1e-6,
        "{}",
        presentation.display_refresh_hz
    );
    assert_eq!(presentation.validation.errors, 3);
}
