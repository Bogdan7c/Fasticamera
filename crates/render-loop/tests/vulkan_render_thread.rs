//! Сквозные тесты: НАСТОЯЩИЙ поток рендера + `VulkanDisplayBackend` на реальном драйвере RADV
//! (headless-стенд ПК) с полной validation (включая синхронизацию) и часами vsync, которые
//! публикует отдельный поток. Любая ошибка validation = провал теста.
//!
//! Ограничение стенда: у headless-WSI нет чтения показанных картинок, содержимое кадров
//! доказывают паритетные тесты `gpu-vulkan`; здесь доказывается, что кадры реально доходят
//! до показа через весь путь (ящик -> машина состояний -> бэкенд -> present), а surface
//! отпускается только после уничтожения GPU-объектов.

// Стенд headless-показа существует только на ПК.
#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod vulkan_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpu_vulkan::context::GpuContext;
use render_loop::config::RenderLoopConfig;
use render_loop::mailbox::InboxDelivery;
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState};
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff};
use support::{FakeCommands, Journal, fake_surface, wait_until};
use vulkan_support::{
    RESIZED, START_SIZE, assert_no_validation_errors, headless_backend_or_skip, vsync_clock_at_120_hz,
};

/// Предел ожидания: большой для медленного CI, но конечный.
const DEADLINE: Duration = Duration::from_secs(10);
/// Оболочка ждёт квитанцию в `surfaceDestroyed` до 500 мс; поток обязан уложиться раньше.
const RELEASE_BUDGET: Duration = Duration::from_millis(400);

struct GpuRig {
    handle: RenderLoopHandle,
    journal: Arc<Journal>,
    context: GpuContext,
    render_config: RenderLoopConfig,
}

/// Запускает поток с настоящим бэкендом; `None` — GPU нет и пропуск разрешён.
fn start_gpu_rig() -> Option<GpuRig> {
    let render_config = RenderLoopConfig {
        // Диагностику опрашиваем часто, чтобы статус быстро отражал живые объекты и показы.
        diagnostics_poll_interval_ms: 20,
        ..RenderLoopConfig::default()
    };
    let backend = headless_backend_or_skip(&render_config)?;
    let context = backend.context().clone();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: render_config.clone(),
        backend,
        clock: vsync_clock_at_120_hz(),
        commands: Arc::new(FakeCommands::default()),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");
    Some(GpuRig {
        handle,
        journal: Arc::new(Journal::default()),
        context,
        render_config,
    })
}

impl GpuRig {
    fn show_surface(&self, generation: u64, size: SurfaceSize) {
        let inbox = self.handle.inbox();
        assert_eq!(
            inbox.surface_created(fake_surface(&self.journal, generation)),
            InboxDelivery::Delivered
        );
        inbox.surface_changed(SurfaceGeneration::new(generation), size);
    }

    fn wait_for_frames_presented(&self, at_least: u64) {
        assert!(
            wait_until(DEADLINE, || self.handle.status().frames_presented >= at_least),
            "кадры не дошли до показа ({at_least}): {:?}",
            self.handle.status()
        );
    }

    fn destroy_surface(&self, generation: u64) -> (SurfaceReleaseOutcome, Duration) {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(generation));
        let started = Instant::now();
        self.handle.inbox().surface_destroyed(request);
        let outcome = waiter.wait(DEADLINE);
        (outcome, started.elapsed())
    }
}

#[test]
fn surface_is_presented_resized_and_released_with_clean_gpu_state() {
    let Some(rig) = start_gpu_rig() else { return };
    let baseline = rig.context.live_objects();

    rig.show_surface(1, START_SIZE);
    rig.wait_for_frames_presented(30);
    assert!(
        wait_until(DEADLINE, || rig.handle.status().display.is_some()),
        "диагностика не сообщила параметры цепочки"
    );
    let status = rig.handle.status();
    assert_eq!(status.state, RenderState::Presenting);
    assert_eq!(status.backend_errors, 0, "{status:?}");
    let display = status.display.expect("параметры цепочки");
    assert_eq!(display.extent, START_SIZE);
    assert_eq!(display.recreation_count, 0);
    assert!(display.image_count >= 2, "{display:?}");
    // На ПК display timing нет: реальный период неизвестен, пейсинг остаётся на запасном.
    assert_eq!(display.refresh_cycle, None);
    assert_eq!(
        status.pacing_refresh_period,
        rig.render_config.expected_refresh_period()
    );

    // Смена размера: цепочка пересоздана, кадры продолжают идти уже в новом размере.
    rig.handle.inbox().surface_changed(SurfaceGeneration::new(1), RESIZED);
    let frames_before_resize = rig.handle.status().frames_presented;
    assert!(
        wait_until(DEADLINE, || {
            let status = rig.handle.status();
            status.recreations >= 1
                && status.display.as_ref().is_some_and(|info| info.extent == RESIZED)
                && status.frames_presented >= frames_before_resize + 10
        }),
        "пересоздание не отразилось в статусе: {:?}",
        rig.handle.status()
    );
    let after_resize = rig.handle.status();
    assert_eq!(after_resize.recreations, 1);
    assert_eq!(after_resize.display.expect("цепочка").recreation_count, 1);

    // Уничтожение surface: квитанция вовремя, окно отпущено, GPU-объекты возвращены.
    let (outcome, waited) = rig.destroy_surface(1);
    assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed);
    assert!(waited < RELEASE_BUDGET, "квитанция заняла {waited:?}");
    assert_eq!(rig.journal.count("window_released(1)"), 1, "окно отпущено");
    assert!(
        wait_until(DEADLINE, || rig.handle.status().display.is_none()),
        "после detach цепочка должна исчезнуть из статуса"
    );
    assert_eq!(rig.context.live_objects(), baseline, "объекты GPU вернулись к исходным");
    let final_status = rig.handle.status();
    assert_eq!(final_status.state, RenderState::WaitingForSurface);
    assert_eq!(final_status.backend.detach_failures, 0);
    assert_eq!(final_status.backend.validation_errors, 0);
    assert_no_validation_errors(&rig.context);
}

#[test]
fn twenty_create_destroy_cycles_confirm_every_waiter_and_leak_nothing() {
    let Some(rig) = start_gpu_rig() else { return };
    let baseline = rig.context.live_objects();

    for cycle in 1..=20u64 {
        let frames_at_start = rig.handle.status().frames_presented;
        rig.show_surface(cycle, START_SIZE);
        rig.wait_for_frames_presented(frames_at_start + 3);
        let (outcome, waited) = rig.destroy_surface(cycle);
        assert_eq!(outcome, SurfaceReleaseOutcome::Confirmed, "цикл {cycle}");
        assert!(waited < RELEASE_BUDGET, "цикл {cycle}: квитанция заняла {waited:?}");
        assert_eq!(
            rig.journal.count(&format!("window_released({cycle})")),
            1,
            "цикл {cycle}: окно отпущено ровно раз"
        );
    }

    let status = rig.handle.status();
    assert_eq!(status.surfaces_attached, 20);
    assert_eq!(status.releases_confirmed, 20);
    assert_eq!(status.backend_errors, 0, "{status:?}");
    assert_eq!(
        rig.context.live_objects(),
        baseline,
        "объекты GPU не растут за 20 циклов"
    );
    assert_no_validation_errors(&rig.context);
}

#[test]
fn stopping_the_handle_while_presenting_detaches_and_releases_the_window() {
    let Some(rig) = start_gpu_rig() else { return };
    let baseline = rig.context.live_objects();
    rig.show_surface(1, START_SIZE);
    rig.wait_for_frames_presented(10);

    let GpuRig {
        handle,
        journal,
        context,
        ..
    } = rig;
    let final_status = handle.stop();

    assert_eq!(final_status.state, RenderState::Stopped);
    assert_eq!(journal.count("window_released(1)"), 1, "окно отпущено при остановке");
    assert_eq!(context.live_objects(), baseline, "после остановки утечек нет");
    assert_no_validation_errors(&context);
}

#[test]
fn destroy_request_racing_with_stop_is_never_left_waiting() {
    let Some(rig) = start_gpu_rig() else { return };
    let baseline = rig.context.live_objects();
    rig.show_surface(1, START_SIZE);
    rig.wait_for_frames_presented(10);

    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    rig.handle.inbox().surface_destroyed(request);
    let GpuRig {
        handle,
        journal,
        context,
        ..
    } = rig;
    let _final_status = handle.stop();

    // Квитанция либо подтверждена (поток успел обработать), либо уничтожена: но не «завис».
    let outcome = waiter.wait(RELEASE_BUDGET);
    assert!(
        matches!(
            outcome,
            SurfaceReleaseOutcome::Confirmed | SurfaceReleaseOutcome::DroppedWithoutConfirmation
        ),
        "{outcome:?}"
    );
    assert_eq!(journal.count("window_released(1)"), 1);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
