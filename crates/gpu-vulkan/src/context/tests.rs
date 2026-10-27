//! Тесты контекста на реальном GPU.

use super::test_support::gpu_context_or_skip;
use crate::error::GpuError;
use crate::live_objects::LiveObjectKind;

#[test]
fn headless_context_reports_real_device_with_required_features() {
    let Some(context) = gpu_context_or_skip() else { return };
    let capabilities = context.capabilities();
    assert!(!capabilities.device_name.is_empty());
    assert!(!capabilities.driver_name.is_empty(), "драйвер должен назвать себя");
    assert!(capabilities.api_version.major == 1 && capabilities.api_version.minor >= 3);
    assert!(capabilities.subgroup_size > 0);
    if capabilities.driver_name.to_lowercase().contains("radv") {
        // RADV (ПК-тесты) поддерживает арифметику subgroup в compute — на ней проверяется быстрый путь AWB.
        assert!(capabilities.subgroup_arithmetic_in_compute, "RADV: {capabilities:?}");
    }
    assert!(
        capabilities
            .enabled_device_extensions
            .iter()
            .any(|name| name == "VK_KHR_external_semaphore_fd")
    );
    assert!(
        capabilities
            .enabled_instance_extensions
            .iter()
            .any(|name| name == "VK_EXT_debug_utils"),
        "validation включена, messenger обязан быть"
    );
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn headless_profile_does_not_expose_wsi_loaders() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Профиль Headless не включает swapchain: доступ обязан дать типизированную ошибку, а не панику/None.
    assert!(matches!(
        context.swapchain_loader(),
        Err(GpuError::ExtensionNotEnabled { .. })
    ));
    assert!(matches!(
        context.android_surface_loader(),
        Err(GpuError::ExtensionNotEnabled { .. })
    ));
    assert!(context.external_semaphore_fd_loader().is_ok());
}

#[test]
fn one_shot_runs_recorded_commands_and_cleans_up() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut recorded = false;
    context
        .run_one_shot(|_command_buffer| {
            recorded = true;
            Ok(())
        })
        .unwrap();
    assert!(recorded);
    assert_eq!(context.live_objects(), baseline, "пул и fence обязаны освободиться");
    assert_eq!(context.live_objects().count(LiveObjectKind::CommandPool), 0);
    context.wait_idle().unwrap();
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn one_shot_propagates_recording_error_without_leaking() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let error = context
        .run_one_shot(|_command_buffer| {
            Err(GpuError::InvalidState {
                what: "тест".to_owned(),
            })
        })
        .unwrap_err();
    assert_eq!(
        error,
        GpuError::InvalidState {
            what: "тест".to_owned()
        }
    );
    assert_eq!(context.live_objects(), baseline);
}
