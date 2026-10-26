//! Пейсинг не должен терять записи времён показа между опросами.
//!
//! Драйвер Android хранит лишь короткую историю времён показа (`kMaxTimingInfos` в
//! `libvulkan/swapchain.cpp`): записи старше последних нескольких перезаписываются.
//! Здесь бэкенд-обёртка воспроизводит это: «кольцо драйвера» на 3 записи и экран, идущий
//! идеально ровно на 120 Гц, без единого настоящего пропуска.
//!
//! * поток вычерпывает записи на КАЖДОМ кадре -> ничего не теряется: дыр нет, вердикт Passed;
//! * если вычерпывать редко (обёртка отдаёт записи лишь на каждый 10-м вызове) -> появляются
//!   дыры измерения (`timing_gaps`), но они НЕ превращаются в пропуски (`misses` = 0). Зато такая
//!   разреженная статистика (мало интервалов на кадры окна) не может быть «Passed»: вердикт
//!   `SparseData` (порог `pacing_min_coverage_ratio`).
//!   Раньше потерянные записи давали интервалы через несколько кадров, которые считались
//!   пропусками (на телефоне: 40 «пропусков» при 616 показанных кадрах).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, DisplayInfo, FrameTiming, PastPresentation,
    PresentOutcome,
};
use render_loop::config::RenderLoopConfig;
use render_loop::pacing::PacingVerdict;
use render_loop::thread::{RenderLoop, RenderLoopParts, RenderStatus};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::{NativeSurface, SurfaceSize};
use support::{FakeBackend, FakeClock, FakeCommands, Journal, Probe, SIZE, fake_surface, wait_until};

/// Идеальные 120 Гц.
const PERIOD_NANOS: i64 = 8_333_333;
/// Сколько записей помнит «драйвер».
const DRIVER_RING_CAPACITY: usize = 3;

/// Обёртка: кольцо драйвера на `DRIVER_RING_CAPACITY` записей; вычерпывание срабатывает
/// только на каждом `serve_every_nth_drain`-м вызове (остальные возвращают пусто и записи
/// остаются в кольце, вытесняясь новыми).
struct DriverRingBackend {
    inner: FakeBackend,
    ring: VecDeque<PastPresentation>,
    drain_calls: u64,
    serve_every_nth_drain: u64,
    /// Следующий номер показа цепочки (растёт на каждый показанный кадр).
    next_present_id: u32,
    /// Сколько номеров «съедает» пересоздание цепочки (счётчик продолжается, записи старой
    /// цепочки пропали вместе с ней).
    ids_lost_on_resize: u32,
    /// Период экрана, который «сообщает» бэкенд, нс. Тест меняет его на ходу (60 -> 120 Гц).
    reported_period_nanos: Arc<AtomicU64>,
}

impl DisplayBackend for DriverRingBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        self.inner.attach(surface, size)
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        self.ring.clear();
        self.next_present_id += self.ids_lost_on_resize;
        self.inner.resize(size)
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        let outcome = self.inner.render_and_present(timing);
        if outcome == PresentOutcome::Presented {
            if self.ring.len() == DRIVER_RING_CAPACITY {
                self.ring.pop_front();
            }
            let present_id = self.next_present_id;
            self.next_present_id += 1;
            self.ring.push_back(PastPresentation {
                present_id,
                frame_index: timing.frame_index,
                actual_present: MonotonicNanos::from_nanos(
                    1_000_000_000 + i64::try_from(timing.frame_index).expect("кадр") * PERIOD_NANOS,
                ),
            });
        }
        outcome
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.inner.detach()
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
            refresh_cycle: Some(Duration::from_nanos(self.reported_period_nanos.load(Ordering::SeqCst))),
        });
        diagnostics
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        self.drain_calls += 1;
        if !self.drain_calls.is_multiple_of(self.serve_every_nth_drain) {
            return Vec::new();
        }
        self.ring.drain(..).collect()
    }
}

fn run_with_drain_period(serve_every_nth_drain: u64) -> RenderStatus {
    run_scenario(serve_every_nth_drain, 0, false)
}

/// `recreate_once` — на одном из кадров бэкенд просит пересоздать цепочку; пересоздание
/// «съедает» `ids_lost_on_resize` номеров показа.
fn run_scenario(serve_every_nth_drain: u64, ids_lost_on_resize: u32, recreate_once: bool) -> RenderStatus {
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    if recreate_once {
        // 190 кадров нормальных (окно успевает набраться ДО пересоздания, и оно остаётся в окне), затем один с просьбой пересоздать цепочку.
        let mut outcomes = probe.scripted_outcomes.lock().expect("исходы");
        outcomes.extend(std::iter::repeat_n(PresentOutcome::Presented, 190));
        outcomes.push_back(PresentOutcome::NeedsRecreate);
    }
    let (clock, _clock_stop) = FakeClock::new();
    let config = RenderLoopConfig {
        // Fake показывает ровно по 120 Гц, поэтому запасной период — 120 Гц (по умолчанию 60).
        preferred_frame_rate_hz: 120.0,
        pacing_window_s: 0.5,
        // Опрос диагностики заведомо реже кадра: на нём одном записи терялись бы.
        diagnostics_poll_interval_ms: 20,
        ..RenderLoopConfig::default()
    };
    let handle = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: DriverRingBackend {
            inner: FakeBackend::new(probe),
            ring: VecDeque::new(),
            drain_calls: 0,
            serve_every_nth_drain,
            next_present_id: 0,
            ids_lost_on_resize,
            reported_period_nanos: Arc::new(AtomicU64::new(PERIOD_NANOS as u64)),
        },
        clock,
        commands: Arc::new(FakeCommands::default()),
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(shell_contract::surface::SurfaceGeneration::new(1), SIZE);
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
fn draining_every_frame_loses_no_records_even_with_tiny_driver_history() {
    let status = run_with_drain_period(1);

    assert_eq!(status.pacing.timing_gaps, 0, "ничего не потеряно: {:?}", status.pacing);
    assert_eq!(status.pacing.misses, 0);
    assert!(status.pacing.intervals > 0);
    assert_eq!(
        status.pacing.frames_covered,
        status.pacing.intervals + 1,
        "окно целиком из соседних кадров"
    );
    assert_eq!(status.pacing.verdict, PacingVerdict::Passed);
}

#[test]
fn rare_draining_produces_gaps_but_never_turns_them_into_misses() {
    let status = run_with_drain_period(10);

    assert!(status.pacing.timing_gaps > 0, "записи потеряны: {:?}", status.pacing);
    assert_eq!(status.pacing.misses, 0, "дыры не пропуски: {:?}", status.pacing);
    // Прошлый вердикт тут был Passed по горстке интервалов; теперь разреженность видна явно.
    match status.pacing.verdict {
        PacingVerdict::SparseData {
            coverage_ratio,
            min_coverage_ratio,
            intervals,
            frames_covered,
        } => {
            assert_eq!(
                min_coverage_ratio,
                RenderLoopConfig::default().pacing_min_coverage_ratio
            );
            assert_eq!(
                (intervals, frames_covered),
                (status.pacing.intervals, status.pacing.frames_covered)
            );
            assert!(coverage_ratio < min_coverage_ratio, "{coverage_ratio}");
        }
        other => panic!("ожидалось SparseData, получено {other:?}: {:?}", status.pacing),
    }
}

/// Пересоздание цепочки теряет записи старой и «съедает» номера: это разрыв непрерывности,
/// а не дыра измерения и тем более не пропуск кадра.
#[test]
fn swapchain_recreation_is_a_continuity_break_not_gaps_or_misses() {
    let status = run_scenario(1, 5, true);

    assert_eq!(status.recreations, 1, "цепочка пересоздана ровно раз");
    assert_eq!(status.pacing.timing_gaps, 0, "{:?}", status.pacing);
    assert_eq!(status.pacing.misses, 0, "{:?}", status.pacing);
    assert_eq!(status.pacing.verdict, PacingVerdict::Passed);
}

/// Экран переключился 60 -> 120 Гц на ходу: пейсинг должен перейти на новый период и не
/// мешать интервалы старого периода с новыми (окно очищается, набирается заново).
#[test]
fn refresh_period_change_at_runtime_switches_pacing_and_restarts_window() {
    const OLD_PERIOD_NANOS: u64 = 16_666_667;
    let journal = Arc::new(Journal::default());
    let probe = Probe::new(Arc::clone(&journal));
    let (clock, _clock_stop) = FakeClock::new();
    let reported_period_nanos = Arc::new(AtomicU64::new(OLD_PERIOD_NANOS));
    let config = RenderLoopConfig {
        // Fake показывает ровно по 120 Гц, поэтому запасной период — 120 Гц (по умолчанию 60).
        preferred_frame_rate_hz: 120.0,
        pacing_window_s: 0.5,
        diagnostics_poll_interval_ms: 20,
        ..RenderLoopConfig::default()
    };
    let handle = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: DriverRingBackend {
            inner: FakeBackend::new(probe),
            ring: VecDeque::new(),
            drain_calls: 0,
            serve_every_nth_drain: 1,
            next_present_id: 0,
            ids_lost_on_resize: 0,
            reported_period_nanos: Arc::clone(&reported_period_nanos),
        },
        clock,
        commands: Arc::new(FakeCommands::default()),
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(shell_contract::surface::SurfaceGeneration::new(1), SIZE);
    // Окно на старом периоде набрано полностью.
    assert!(
        wait_until(Duration::from_secs(10), || {
            handle.status().pacing.verdict != PacingVerdict::NotEnoughData
        }),
        "окно на старом периоде не набралось: {:?}",
        handle.status()
    );
    assert_eq!(
        handle.status().pacing_refresh_period,
        Duration::from_nanos(OLD_PERIOD_NANOS)
    );

    reported_period_nanos.store(PERIOD_NANOS as u64, Ordering::SeqCst);
    let new_period = Duration::from_nanos(PERIOD_NANOS as u64);
    assert!(
        wait_until(Duration::from_secs(10), || handle.status().pacing_refresh_period
            == new_period),
        "пейсинг не перешёл на новый период: {:?}",
        handle.status()
    );
    // Статус публикуется целиком: период и снимок пейсинга относятся к одному опросу.
    let status = handle.status();
    assert!(
        status.pacing.intervals < 30,
        "окно старого периода должно быть очищено, а не смешано с новым: {:?}",
        status.pacing
    );
    handle.stop();
}
