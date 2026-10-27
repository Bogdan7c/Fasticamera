//! Хук синхронизации сабмита кадра показа (`present_with_sync`) на реальном драйвере (headless-показ,
//! RADV, полная validation): экспорт release-fence после сабмита, ожидание импортированного fence
//! камеры и неизменность старого пути `present`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

use std::time::Duration;

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::swapchain::{
    AcquireOutcome, DisplayTarget, DisplayTargetConfig, PresentOutcome, PresentRequest, PresentSync,
    PresentationSurface, SubmitProgress,
};
use gpu_vulkan::sync::{ExportableSemaphore, FenceWait, import_wait_fence, wait_release_fence_on_cpu};
use gpu_vulkan::test_pattern::TestPattern;
use gpu_vulkan::validation::ValidationMode;
use shell_contract::surface::SurfaceSize;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
const SIZE: SurfaceSize = SurfaceSize {
    width_px: 64,
    height_px: 48,
};
const FENCE_TIMEOUT: Duration = Duration::from_secs(5);

fn presentation_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::HeadlessPresentation, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) if std::env::var(SKIP_ENV).as_deref() == Ok("1") => {
            eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
            None
        }
        Err(error) => panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно"),
    }
}

struct Rig {
    context: GpuContext,
    target: DisplayTarget,
    pattern: TestPattern,
}

impl Rig {
    fn new(context: GpuContext) -> Self {
        let target = DisplayTarget::attach(
            &context,
            PresentationSurface::Headless,
            DisplayTargetConfig {
                requested_size: Some(SIZE),
                ..DisplayTargetConfig::default()
            },
        )
        .map_err(|error| error.error)
        .expect("attach");
        let format = target.info().expect("info").format;
        let pattern = TestPattern::new(&context, format).expect("паттерн");
        Self {
            context,
            target,
            pattern,
        }
    }

    /// Берёт кадр и рисует в него паттерн; кадр ещё надо показать.
    fn draw(&mut self) -> gpu_vulkan::swapchain::DisplayFrame {
        let frame = match self.target.acquire_frame().expect("acquire") {
            AcquireOutcome::Ready(frame) => frame,
            other => panic!("ожидали Ready: {other:?}"),
        };
        self.pattern
            .record(
                frame.command_buffer(),
                frame.pattern_target(),
                frame.frame_index(),
                frame.transform(),
            )
            .expect("запись паттерна");
        frame
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Показ ПК-стенда: перед уничтожением паттерна дожидаемся GPU.
        self.context.wait_idle().expect("wait_idle");
    }
}

#[test]
fn present_without_sync_behaves_as_before_and_exports_nothing() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut rig = Rig::new(context);
    let first = rig.draw();
    let plain = rig
        .target
        .present(first, PresentRequest::default())
        .expect("старый present");
    let second = rig.draw();
    let synced = rig
        .target
        .present_with_sync(second, PresentRequest::default(), &PresentSync::none());

    assert_eq!(plain, PresentOutcome::Presented { present_id: 0 });
    assert_eq!(synced.submit, SubmitProgress::Submitted);
    assert!(synced.release_signal.is_none(), "без запроса экспорта токена нет");
    assert_eq!(
        synced.outcome.expect("показ"),
        PresentOutcome::Presented { present_id: 1 }
    );
    let report = rig.context.validation_report();
    assert_eq!(report.errors, 0, "{report:?}");
}

#[test]
fn exported_release_fence_signals_after_submit_and_semaphore_is_reusable() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut rig = Rig::new(context);
    let export = ExportableSemaphore::new(&rig.context).expect("экспортируемый семафор");

    for round in 0..3 {
        let frame = rig.draw();
        let synced = rig.target.present_with_sync(
            frame,
            PresentRequest::default(),
            &PresentSync::none().export_release_after_submit(&export),
        );
        assert_eq!(synced.submit, SubmitProgress::Submitted, "круг {round}");
        assert!(export.is_awaiting_export(), "до экспорта семафор занят");
        synced.outcome.as_ref().expect("показ");
        let fence = synced
            .release_signal
            .expect("токен экспорта после сабмита")
            .export()
            .expect("экспорт fence");
        assert!(!export.is_awaiting_export(), "после экспорта семафор свободен");
        assert_eq!(
            wait_release_fence_on_cpu(&rig.context, &fence, FENCE_TIMEOUT).expect("ожидание fence"),
            FenceWait::Signaled,
            "круг {round}: fence сабмита должен сработать"
        );
    }
    let report = rig.context.validation_report();
    assert_eq!(report.errors, 0, "{report:?}");
}

#[test]
fn fence_of_a_later_submit_covers_earlier_unexported_submits() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut rig = Rig::new(context);
    let export = ExportableSemaphore::new(&rig.context).expect("семафор");
    // Три сабмита подряд; fence берём только у последнего.
    for _ in 0..2 {
        let frame = rig.draw();
        rig.target
            .present(frame, PresentRequest::default())
            .expect("показ без экспорта");
    }
    let frame = rig.draw();
    let synced = rig.target.present_with_sync(
        frame,
        PresentRequest::default(),
        &PresentSync::none().export_release_after_submit(&export),
    );
    let fence = synced.release_signal.expect("токен").export().expect("экспорт");
    assert_eq!(
        wait_release_fence_on_cpu(&rig.context, &fence, FENCE_TIMEOUT).unwrap(),
        FenceWait::Signaled
    );
    // Раз fence позднего сабмита сработал, ранние сабмиты кольца уже закончены: ожидание не блокируется.
    rig.target
        .wait_for_submitted_frames()
        .expect("ранние сабмиты завершены");
    let report = rig.context.validation_report();
    assert_eq!(report.errors, 0, "{report:?}");
}

#[test]
fn imported_camera_fence_is_waited_by_the_next_submit() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut rig = Rig::new(context);
    let export = ExportableSemaphore::new(&rig.context).expect("семафор");

    // Первый сабмит играет роль «камеры»: его release-fence импортируем как acquire-fence кадра.
    let frame = rig.draw();
    let first = rig.target.present_with_sync(
        frame,
        PresentRequest::default(),
        &PresentSync::none().export_release_after_submit(&export),
    );
    let camera_fence = first.release_signal.expect("токен").export().expect("экспорт");
    let fence_for_gpu = camera_fence_as_wait(camera_fence);
    let wait = import_wait_fence(&rig.context, fence_for_gpu)
        .map_err(|error| error.reason)
        .expect("импорт fence");

    let frame = rig.draw();
    let second = rig.target.present_with_sync(
        frame,
        PresentRequest::default(),
        &PresentSync::none()
            .wait_for_camera_acquire(&wait)
            .export_release_after_submit(&export),
    );
    assert_eq!(second.submit, SubmitProgress::Submitted);
    let fence = second.release_signal.expect("токен").export().expect("экспорт");
    assert_eq!(
        wait_release_fence_on_cpu(&rig.context, &fence, FENCE_TIMEOUT).unwrap(),
        FenceWait::Signaled,
        "сабмит с ожиданием импортированного fence дошёл до конца"
    );
    let report = rig.context.validation_report();
    assert_eq!(report.errors, 0, "{report:?}");
}

#[test]
fn busy_export_semaphore_rejects_the_submit_without_side_effects() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut rig = Rig::new(context);
    let export = ExportableSemaphore::new(&rig.context).expect("семафор");

    let frame = rig.draw();
    let first = rig.target.present_with_sync(
        frame,
        PresentRequest::default(),
        &PresentSync::none().export_release_after_submit(&export),
    );
    // Токен первого сабмита НЕ экспортируем: семафор остаётся занятым.
    let pending = first.release_signal.expect("токен");

    let frame = rig.draw();
    let refused = rig.target.present_with_sync(
        frame,
        PresentRequest::default(),
        &PresentSync::none().export_release_after_submit(&export),
    );
    assert_eq!(
        refused.submit,
        SubmitProgress::NotSubmitted,
        "занятый семафор — сабмита нет"
    );
    assert!(refused.release_signal.is_none());
    assert!(refused.outcome.is_err());
    assert!(export.is_awaiting_export(), "отказ не снимает чужую пометку «занят»");
    // Экспорт первого токена по-прежнему работает.
    pending.export().expect("первый экспорт");
    assert!(!export.is_awaiting_export());
    // Отказ до сабмита не должен был сбросить fence слота: GPU простаивает без таймаута.
    rig.context.wait_idle().expect("wait_idle");
    let report = rig.context.validation_report();
    assert_eq!(report.errors, 0, "{report:?}");
}

/// fence освобождения одного сабмита как fence ожидания другого (тот же sync_fd).
fn camera_fence_as_wait(release: gpu_vulkan::sync::GpuReleaseFence) -> gpu_vulkan::sync::GpuWaitFence {
    match release.into_owned_fd() {
        Some(fd) => gpu_vulkan::sync::GpuWaitFence::from_sync_fd(fd),
        None => gpu_vulkan::sync::GpuWaitFence::already_signaled(),
    }
}
