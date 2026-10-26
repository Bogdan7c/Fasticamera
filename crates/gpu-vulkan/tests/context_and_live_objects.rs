//! Тесты контекста и учёта живых объектов на реальном GPU.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::instance::SyncValidationBackend;
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::offscreen::OffscreenTarget;
use gpu_vulkan::test_pattern::{SurfaceTransform, TestPattern};
use gpu_vulkan::validation::ValidationMode;

#[test]
fn context_reports_real_radv_device_with_sync_validation_active() {
    let Some(context) = gpu_context_or_skip() else { return };
    let capabilities = context.capabilities();
    // Стенд разработки — AMD с драйвером RADV; на другом железе (llvmpipe, NVIDIA) утверждение
    // про имя драйвера не проверяем, но проверяем общий контракт.
    if std::env::var("FASTICAMERA_EXPECT_RADV").as_deref() == Ok("1") {
        assert!(
            capabilities.driver_name.to_lowercase().contains("radv"),
            "{capabilities:?}"
        );
    }
    assert!(capabilities.device_name.len() > 2);
    assert_eq!(
        capabilities.sync_validation,
        SyncValidationBackend::LayerSettings,
        "sync-валидация не включилась"
    );
    assert!(
        capabilities
            .timestamps
            .is_some_and(|support| support.valid_bits > 0 && support.period_ns > 0.0)
    );
    assert_no_validation_errors(&context);
}

#[test]
fn live_objects_return_to_baseline_after_twenty_cycles() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    assert_eq!(baseline.total(), 0, "свежий контекст не должен владеть объектами");

    for cycle in 0..20u32 {
        let pattern = TestPattern::new(&context, OffscreenTarget::FORMAT).unwrap();
        let mut target = OffscreenTarget::new(&context, 33, 17).unwrap();
        target
            .render(|command_buffer, attachment| {
                pattern.record(command_buffer, attachment, cycle, SurfaceTransform::Identity)
            })
            .unwrap();
        let _ = target.readback().unwrap();
        // Внутри цикла объекты живы и учтены.
        assert!(context.live_objects().count(LiveObjectKind::Image) >= 1);
        assert!(context.live_objects().count(LiveObjectKind::Pipeline) >= 1);
    }
    assert_eq!(
        context.live_objects(),
        baseline,
        "разница: {:?}",
        context.live_objects().differences_from(&baseline)
    );
    assert_no_validation_errors(&context);
}

#[test]
fn leaked_object_is_visible_in_snapshot() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let target = OffscreenTarget::new(&context, 8, 8).unwrap();
    assert_eq!(context.live_objects().count(LiveObjectKind::Image), 1);
    // Намеренная утечка: деструктор не выполнится. Счётчик обязан это показать.
    std::mem::forget(target);
    let after_leak = context.live_objects();
    assert_ne!(after_leak, baseline);
    let differences = after_leak.differences_from(&baseline);
    assert!(differences.contains(&(LiveObjectKind::Image, 0, 1)), "{differences:?}");
    assert!(
        differences.contains(&(LiveObjectKind::ImageView, 0, 1)),
        "{differences:?}"
    );
    assert!(
        differences.contains(&(LiveObjectKind::DeviceMemory, 0, 1)),
        "{differences:?}"
    );
}

#[test]
fn clones_share_one_device_and_one_set_of_counters() {
    let Some(context) = gpu_context_or_skip() else { return };
    let clone: GpuContext = context.clone();
    let target = OffscreenTarget::new(&clone, 8, 8).unwrap();
    // Объект, созданный через клон, виден в исходном контексте.
    assert_eq!(context.live_objects().count(LiveObjectKind::Image), 1);
    drop(clone);
    // Устройство живёт, пока жив объект: после drop обоих клонов цель всё ещё можно использовать
    // (она держит собственный клон контекста), а уничтожение пройдёт без ошибок validation.
    let observer_counters = context.clone();
    drop(context);
    assert_eq!(target.extent().width, 8);
    drop(target);
    assert_eq!(observer_counters.live_objects().total(), 0);
}

#[test]
fn unavailable_validation_layer_request_is_not_silently_ignored() {
    // Если слой есть (на стенде он есть), режим Disabled даёт контекст БЕЗ debug_utils.
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::Disabled);
    let Ok(context) = GpuContext::new(&config) else {
        // Нет GPU: тест про политику GPU покрывают остальные тесты.
        if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") {
            eprintln!("ПРОПУСК GPU-теста: нет Vulkan (FASTICAMERA_SKIP_GPU_TESTS=1)");
            return;
        }
        panic!("GPU недоступен; задайте FASTICAMERA_SKIP_GPU_TESTS=1 для осознанного пропуска");
    };
    let capabilities = context.capabilities();
    assert!(
        !capabilities
            .enabled_instance_extensions
            .iter()
            .any(|name| name == "VK_EXT_debug_utils")
    );
    assert_eq!(capabilities.sync_validation, SyncValidationBackend::NotRequested);
    assert_eq!(context.validation_report().errors, 0);
}
