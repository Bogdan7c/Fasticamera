//! Пейсинг должен судить по РЕАЛЬНОМУ периоду экрана, который сообщает бэкенд
//! (`DisplayInfo::refresh_cycle`, у Vulkan-бэкенда на телефоне — из `VK_GOOGLE_display_timing`),
//! а не по `preferred_frame_rate_hz`. Здесь бэкенд-обёртка имитирует экран, держащий 90 Гц
//! при просьбе о 120 Гц: кадры показываются ровно каждые 13 мс.
//!
//! 13 мс — интервал, на котором периоды различаются по вердикту: относительно 120 Гц
//! (8,33 мс, порог пропуска 12,5 мс) каждый кадр был бы пропуском, относительно 90 Гц
//! (11,1 мс, порог 16,7 мс) пропусков нет.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, DisplayInfo, FrameTiming, PastPresentation,
    PresentOutcome,
};
use render_loop::config::RenderLoopConfig;
use render_loop::pacing::PacingVerdict;
use render_loop::thread::{RenderLoop, RenderLoopParts};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceSize};
use support::{FakeBackend, FakeClock, FakeCommands, Journal, Probe, SIZE, fake_surface, wait_until};

/// Реальный интервал между показами, нс (13 мс).
const REAL_INTERVAL_NANOS: i64 = 13_000_000;
/// Реальный период экрана: 90 Гц.
const REAL_REFRESH_CYCLE: Duration = Duration::from_nanos(11_111_111);

/// Обёртка над fake-бэкендом: времена показа переписаны на 13 мс, а экран «сообщает» 90 Гц.
struct NinetyHertzBackend {
    inner: FakeBackend,
    report_refresh_cycle: bool,
}

impl DisplayBackend for NinetyHertzBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        self.inner.attach(surface, size)
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        self.inner.resize(size)
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        self.inner.render_and_present(timing)
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.inner.detach()
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        let mut presentations = self.inner.drain_past_presentations();
        for presentation in &mut presentations {
            presentation.actual_present = MonotonicNanos::from_nanos(
                1_000_000_000 + i64::try_from(presentation.frame_index).expect("кадр") * REAL_INTERVAL_NANOS,
            );
        }
        presentations
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        let mut diagnostics = self.inner.diagnostics();
        diagnostics.display = Some(DisplayInfo {
            format: "B8G8R8A8_UNORM".to_owned(),
            color_space: "SRGB_NONLINEAR_KHR".to_owned(),
            extent: SIZE,
            transform: "Identity".to_owned(),
            image_count: 3,
            recreation_count: 0,
            refresh_cycle: self.report_refresh_cycle.then_some(REAL_REFRESH_CYCLE),
        });
        diagnostics
    }
}

/// Гоняет поток, пока окно статистики не наберётся, и возвращает итоговый статус.
fn run_until_pacing_verdict(report_refresh_cycle: bool) -> render_loop::thread::RenderStatus {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    let (clock, _clock_stop) = FakeClock::new();
    let config = RenderLoopConfig {
        preferred_frame_rate_hz: 120.0,
        pacing_window_s: 0.5,
        diagnostics_poll_interval_ms: 5,
        ..RenderLoopConfig::default()
    };
    let handle = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: NinetyHertzBackend {
            inner: FakeBackend::new(probe),
            report_refresh_cycle,
        },
        clock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
    // 200 кадров * 13 мс = 2,6 с синхронного времени показа: окно 0,5 с заведомо набрано.
    assert!(
        wait_until(Duration::from_secs(10), || {
            let status = handle.status();
            status.frames_presented >= 200 && status.pacing.verdict != PacingVerdict::NotEnoughData
        }),
        "статистика не набралась: {:?}",
        handle.status()
    );
    handle.stop()
}

#[test]
fn pacing_is_judged_against_reported_refresh_cycle_not_preferred_rate() {
    let status = run_until_pacing_verdict(true);

    assert_eq!(status.pacing_refresh_period, REAL_REFRESH_CYCLE);
    assert_eq!(
        status.pacing.misses, 0,
        "по 90 Гц интервалы в 13 мс — не пропуски: {:?}",
        status.pacing
    );
    assert_eq!(status.pacing.verdict, PacingVerdict::Passed);
    assert_eq!(
        status.display.as_ref().and_then(|info| info.refresh_cycle),
        Some(REAL_REFRESH_CYCLE),
        "параметры экрана видны в статусе"
    );
}

#[test]
fn pacing_falls_back_to_preferred_rate_while_refresh_cycle_is_unknown() {
    let status = run_until_pacing_verdict(false);

    assert_eq!(
        status.pacing_refresh_period,
        RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            ..RenderLoopConfig::default()
        }
        .expected_refresh_period()
    );
    assert_eq!(
        status.pacing.misses, status.pacing.intervals,
        "по запасным 120 Гц каждый интервал в 13 мс — пропуск"
    );
    assert!(matches!(status.pacing.verdict, PacingVerdict::Failed { .. }));
}
