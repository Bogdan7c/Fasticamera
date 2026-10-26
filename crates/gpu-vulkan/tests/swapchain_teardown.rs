//! Тесты завершения и восстановления цели показа на ПК (реальный драйвер, полная validation):
//! ожидание показов fence'ами, порядок пересоздания с `oldSwapchain`, срыв кадра после acquire
//! и намеренная утечка при зависшем GPU. Пути ошибок включаются тестовыми швами (`InjectedFault`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::swapchain::{
    AcquireOutcome, DisplayTarget, DisplayTargetConfig, InjectedFault, PresentOutcome, PresentRequest,
    PresentationSurface, RecreateOutcome, SwapchainError,
};
use gpu_vulkan::test_pattern::TestPattern;
use gpu_vulkan::validation::ValidationMode;
use shell_contract::surface::SurfaceSize;

const SIZE: SurfaceSize = SurfaceSize {
    width_px: 129,
    height_px: 65,
};
const OTHER_SIZE: SurfaceSize = SurfaceSize {
    width_px: 97,
    height_px: 81,
};

fn context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::HeadlessPresentation, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error}");
                None
            } else {
                panic!("GPU недоступен: {error}");
            }
        }
    }
}

fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

fn attach(context: &GpuContext) -> DisplayTarget {
    let config = DisplayTargetConfig {
        requested_size: Some(SIZE),
        ..DisplayTargetConfig::default()
    };
    DisplayTarget::attach(context, PresentationSurface::Headless, config)
        .map_err(|error| error.error)
        .expect("attach на headless-surface")
}

/// Рисует и показывает один кадр, требуя штатного успеха.
fn present_one_frame(target: &mut DisplayTarget, pattern: &TestPattern) {
    let AcquireOutcome::Ready(frame) = target.acquire_frame().expect("acquire") else {
        panic!("ожидали Ready");
    };
    pattern
        .record(
            frame.command_buffer(),
            frame.pattern_target(),
            frame.frame_index(),
            frame.transform(),
        )
        .expect("запись паттерна");
    let outcome = target.present(frame, PresentRequest::default()).expect("present");
    assert!(matches!(outcome, PresentOutcome::Presented { .. }), "{outcome:?}");
}

#[test]
fn presents_are_tracked_by_fences_and_detach_releases_everything() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut target = attach(&context);
    assert!(
        target.tracks_presents_with_fences().unwrap(),
        "стенд ПК обязан включать VK_EXT_swapchain_maintenance1 (иначе путь present-fence'ов не проверяется)"
    );
    let info = target.info().unwrap();
    let pattern = TestPattern::new(&context, info.format).unwrap();
    for _ in 0..6 {
        present_one_frame(&mut target, &pattern);
    }
    // Кроме fence'ов слотов кольца появились fence'ы показов: путь реально задействован.
    let fences = context.live_objects().count(LiveObjectKind::Fence);
    assert!(
        fences > info.frames_in_flight,
        "fence'ов {fences}, показы не отслеживаются"
    );

    target.detach().map_err(|failure| failure.error).expect("detach");
    drop(pattern);
    assert_eq!(context.live_objects(), baseline, "после detach ничего не течёт");
    assert_no_validation_errors(&context);
}

#[test]
fn detach_without_present_fences_falls_back_to_device_wait_and_still_releases_everything() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut target = attach(&context);
    target.inject_fault(InjectedFault::WithoutPresentFences).unwrap();
    assert!(!target.tracks_presents_with_fences().unwrap());
    let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
    for _ in 0..4 {
        present_one_frame(&mut target, &pattern);
    }
    assert_eq!(target.pending_present_count().unwrap(), 0);
    target.detach().map_err(|failure| failure.error).expect("detach");
    drop(pattern);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn failed_recreate_after_new_swapchain_does_not_pass_retired_old_swapchain_again() {
    let Some(context) = context_or_skip() else { return };
    let mut target = attach(&context);
    let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
    present_one_frame(&mut target, &pattern);

    target
        .inject_fault(InjectedFault::RecreateFailsAfterSwapchainCreated)
        .unwrap();
    let failure = target
        .recreate(Some(OTHER_SIZE))
        .expect_err("шов должен сорвать пересоздание");
    assert!(matches!(failure, SwapchainError::Gpu(_)), "{failure:?}");
    assert!(
        target.needs_recreate().unwrap(),
        "после срыва пересоздание остаётся нужным"
    );
    assert!(
        target.current_swapchain_is_retired().unwrap(),
        "старый swapchain стал retired, хотя остался текущим"
    );

    // Повтор: oldSwapchain обязан быть null (retired передавать нельзя), иначе validation
    // сообщит VUID-VkSwapchainCreateInfoKHR-oldSwapchain-01933.
    let outcome = target.recreate(Some(OTHER_SIZE)).expect("повторное пересоздание");
    assert!(matches!(outcome, RecreateOutcome::Recreated { .. }), "{outcome:?}");
    assert!(!target.current_swapchain_is_retired().unwrap());
    present_one_frame(&mut target, &pattern);
    target.detach().map_err(|failure| failure.error).expect("detach");
    drop(pattern);
    assert_no_validation_errors(&context);
}

#[test]
fn frame_failing_after_acquire_does_not_wedge_the_target() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut target = attach(&context);
    let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
    present_one_frame(&mut target, &pattern);

    target
        .inject_fault(InjectedFault::AcquireFailsAfterImageAcquired)
        .unwrap();
    assert!(target.acquire_frame().is_err(), "шов срывает кадр после acquire");

    // Цель не застряла в «кадр выдан»: просит пересоздание, а не отвечает InvalidState навсегда.
    assert!(matches!(
        target.acquire_frame().expect("acquire после срыва"),
        AcquireOutcome::NeedsRecreate
    ));
    let outcome = target.recreate(None).expect("recreate после срыва");
    assert!(matches!(outcome, RecreateOutcome::Recreated { .. }), "{outcome:?}");
    // Семафор слота заменён: кадры снова идут, validation молчит.
    for _ in 0..4 {
        present_one_frame(&mut target, &pattern);
    }
    target.detach().map_err(|failure| failure.error).expect("detach");
    drop(pattern);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn stuck_gpu_makes_detach_fail_returns_the_surface_and_leaks_instead_of_destroying() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut target = attach(&context);
    let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
    present_one_frame(&mut target, &pattern);
    target.inject_fault(InjectedFault::StuckGpu).unwrap();

    let failure = target
        .detach()
        .expect_err("зависший GPU: detach обязан сообщить ошибку");
    assert!(matches!(failure.surface, PresentationSurface::Headless));
    let live = context.live_objects();
    assert!(
        live.count(LiveObjectKind::Swapchain) >= 1 && live.count(LiveObjectKind::Surface) >= 1,
        "Vulkan-объекты намеренно оставлены (утечка), а не уничтожены: {live:?} против {baseline:?}"
    );
    drop(pattern);
}
