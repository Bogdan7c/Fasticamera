//! Функциональные тесты P13 сессии 07 через НАСТОЯЩИЙ поток рендера на fake-бэкенде:
//! время кадра на GPU (приходит от бэкенда через два кадра) входит в оценку стоимости кадра —
//! `latch_at` и выбор линии сдвигаются ровно на него; ADPF: сессию создаёт сам поток рендера один
//! раз, каждый показанный кадр уходит в отчёт с CPU и поздним GPU, команда выключает и включает
//! сессию, неподдерживаемая платформа — честное состояние без вреда показу.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameCpuTiming, FrameGpuTiming, FrameTiming,
    LatchWaitResult, PastPresentation, PresentOutcome,
};
use render_loop::config::{AdpfConfig, AdpfMode, PresentTargetConfig, PresentTimelinePolicy, RenderLoopConfig};
use render_loop::mailbox::InboxDelivery;
use render_loop::thread::{
    AdpfCommand, AdpfCommandOutcome, AdpfSessionState, FrameClock, FrameTick, FrameWait, GpuCostSource, MonotonicTimer,
    RENDER_THREAD_NAME, RenderLoop, RenderLoopHandle, RenderLoopParts, RenderStatus, SystemMonotonicTimer,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::performance_hint::fake::{FakeHintBehavior, FakePerformanceHints};
use shell_contract::performance_hint::{HintThreadId, PerformanceHintError, PerformanceHintService};
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceSize};
use shell_contract::vsync::{FrameTimelines, VsyncTimeline};
use support::{FakeCommands, Journal, SIZE, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);
const PERIOD_NANOS: i64 = 16_666_667;
const TIMELINE_COUNT: usize = 4;
const PREFERRED_INDEX: usize = 2;
/// CPU «захват → сабмит», который сообщает fake-бэкенд.
const REPORTED_CPU: Duration = Duration::from_micros(1_800);
/// На сколько кадров позже кадра бэкенд выдаёт его время GPU (как слот кольца после fence).
const GPU_DELAY_FRAMES: usize = 2;
/// «tid», который подделка ADPF отдаёт потоку.
const FAKE_TID: i32 = 777;

/// Fake-бэкенд: CPU-замер на каждый кадр, время GPU (задаётся тестом на ходу) — через два кадра.
struct GpuReportingBackend {
    held_surface: Option<NativeSurface>,
    gpu_nanos: Arc<AtomicU64>,
    frames: Arc<Mutex<Vec<FrameTiming>>>,
    pending_cpu: Option<FrameCpuTiming>,
    /// Кадры, чьё время GPU ещё «исполняется».
    in_flight: VecDeque<u64>,
    ready_gpu: Vec<FrameGpuTiming>,
}

impl DisplayBackend for GpuReportingBackend {
    fn attach(&mut self, surface: NativeSurface, _size: SurfaceSize) -> Result<(), AttachError> {
        self.held_surface = Some(surface);
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> Result<(), BackendError> {
        Ok(())
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        self.frames.lock().expect("кадры").push(timing);
        self.pending_cpu = Some(FrameCpuTiming {
            acquire_wait: Duration::ZERO,
            latch_wait: Duration::ZERO,
            latch_result: LatchWaitResult::NotRequested,
            latch_to_submit: Some(REPORTED_CPU),
        });
        self.in_flight.push_back(timing.frame_index);
        while self.in_flight.len() > GPU_DELAY_FRAMES {
            let frame_index = self.in_flight.pop_front().expect("есть кадр");
            self.ready_gpu.push(FrameGpuTiming {
                frame_index,
                gpu_duration: Duration::from_nanos(self.gpu_nanos.load(Ordering::SeqCst)),
            });
        }
        PresentOutcome::Presented
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.held_surface.take()
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        BackendDiagnostics::default()
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        Vec::new()
    }

    fn take_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        self.pending_cpu.take()
    }

    fn drain_frame_gpu_timings(&mut self) -> Vec<FrameGpuTiming> {
        std::mem::take(&mut self.ready_gpu)
    }
}

/// Тики с несколькими линиями от настоящего `CLOCK_MONOTONIC` (линия `i` — через `i + 1` периодов,
/// срок за 4 мс до показа), с реальным периодом.
struct TimelineClock;

impl FrameClock for TimelineClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        thread::sleep(timeout.min(Duration::from_nanos(PERIOD_NANOS.unsigned_abs())));
        let now = SystemMonotonicTimer.now().expect("часы").as_nanos();
        let timelines = FrameTimelines::from_platform_list(TIMELINE_COUNT, PREFERRED_INDEX, |index| {
            let periods = i64::try_from(index).expect("индекс") + 1;
            VsyncTimeline {
                vsync_id: periods,
                expected_present: MonotonicNanos::from_nanos(now + periods * PERIOD_NANOS),
                deadline: MonotonicNanos::from_nanos(now + periods * PERIOD_NANOS - 4_000_000),
            }
        })
        .expect("линии корректны");
        let preferred = timelines.preferred();
        FrameWait::Tick(FrameTick {
            sequence: 1,
            expected_present: preferred.expected_present,
            deadline: preferred.deadline,
            skipped_ticks: 0,
            timelines,
        })
    }
}

struct Rig {
    handle: RenderLoopHandle,
    gpu_nanos: Arc<AtomicU64>,
    frames: Arc<Mutex<Vec<FrameTiming>>>,
    _journal: Arc<Journal>,
}

fn start(
    present_target: PresentTargetConfig,
    adpf: AdpfConfig,
    hints: Arc<dyn PerformanceHintService>,
    gpu: Duration,
) -> Rig {
    let journal = Arc::new(Journal::default());
    let gpu_nanos = Arc::new(AtomicU64::new(u64::try_from(gpu.as_nanos()).expect("нс")));
    let frames = Arc::new(Mutex::new(Vec::new()));
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig {
            preferred_frame_rate_hz: 60.0,
            diagnostics_poll_interval_ms: 5,
            present_target,
            adpf,
            ..RenderLoopConfig::default()
        },
        backend: GpuReportingBackend {
            held_surface: None,
            gpu_nanos: Arc::clone(&gpu_nanos),
            frames: Arc::clone(&frames),
            pending_cpu: None,
            in_flight: VecDeque::new(),
            ready_gpu: Vec::new(),
        },
        clock: TimelineClock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: hints,
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
    Rig {
        handle,
        gpu_nanos,
        frames,
        _journal: journal,
    }
}

impl Rig {
    fn wait_status(&self, what: &str, condition: impl Fn(&RenderStatus) -> bool) -> RenderStatus {
        assert!(
            wait_until(DEADLINE, || condition(&self.handle.status())),
            "{what}: {:?} / {:?}",
            self.handle.status().present_target,
            self.handle.status().adpf
        );
        self.handle.status()
    }

    fn last_frame(&self) -> FrameTiming {
        *self.frames.lock().expect("кадры").last().expect("кадры есть")
    }
}

fn cost_config(policy: PresentTimelinePolicy) -> PresentTargetConfig {
    PresentTargetConfig {
        policy,
        initial_frame_cost_us: 3_000,
        // До замеров GPU — ноль: так видно, что сдвиг дал именно замер GPU, а не оценка конфига.
        gpu_cost_fallback_us: 0,
        min_cost_samples: 5,
        cost_window_frames: 10,
        latch_margin_us: 1_000,
        ..PresentTargetConfig::default()
    }
}

fn no_hints() -> Arc<dyn PerformanceHintService> {
    Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
        "тест без ADPF",
    ))
}

#[test]
fn measured_gpu_time_moves_latch_at_earlier_by_exactly_that_time() {
    let gpu = Duration::from_micros(6_400);
    let rig = start(
        cost_config(PresentTimelinePolicy::Preferred),
        AdpfConfig::default(),
        no_hints(),
        gpu,
    );
    let status = rig.wait_status("время GPU не дошло до оценки", |status| {
        status.present_target.estimated_cost_parts.gpu_source == GpuCostSource::Measured
            && status.present_target.estimated_cost_parts.cpu == REPORTED_CPU
    });
    assert_eq!(status.present_target.estimated_cost_parts.gpu, gpu);
    assert_eq!(status.present_target.estimated_frame_cost, REPORTED_CPU + gpu);
    let window = status.present_target.frame_gpu_time.expect("окно GPU в диагностике");
    assert_eq!(window.p50, gpu);
    // Следующий кадр планируется по полной оценке: срок − latch_at = CPU 1,8 + GPU 6,4 + запас 1 мс.
    let frames_before = rig.frames.lock().expect("кадры").len();
    assert!(wait_until(DEADLINE, || rig.frames.lock().expect("кадры").len() > frames_before + 1));
    let timing = rig.last_frame();
    assert_eq!(
        timing.target.deadline.as_nanos() - timing.target.latch.latch_at.as_nanos(),
        9_200_000
    );
    drop(rig.handle.stop());
}

#[test]
fn slow_gpu_moves_nearest_feasible_to_a_later_timeline() {
    // Линия 0: срок через ~12,67 мс от тика. GPU 1 мс: 1,8 + 1 + 1 = 3,8 мс — успеваем к линии 0.
    let rig = start(
        cost_config(PresentTimelinePolicy::NearestFeasible { margin_us: 1_000 }),
        AdpfConfig::default(),
        no_hints(),
        Duration::from_millis(1),
    );
    rig.wait_status("оценка GPU не измерена", |status| {
        status.present_target.estimated_cost_parts.gpu_source == GpuCostSource::Measured
    });
    assert!(
        wait_until(DEADLINE, || rig.last_frame().target.timeline_index == 0),
        "быстрый GPU: ожидалась линия 0: {:?} / {:?}",
        rig.last_frame().target,
        rig.handle.status().present_target
    );
    // GPU 11 мс: 1,8 + 11 + 1 = 13,8 > 12,67 — линия 0 невыполнима, выбор уходит на линию 1.
    rig.gpu_nanos.store(11_000_000, Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || rig.last_frame().target.timeline_index == 1),
        "медленный GPU не сдвинул выбор: {:?}",
        rig.handle.status().present_target
    );
    // Обратно к линии 0 планировщик сам НЕ возвращается, даже когда GPU снова быстрый: защита
    // «строго позже линии прошлого кадра» не даёт двум кадрам метить на один vsync, а шаг на линию
    // раньше и есть такой кадр. Это свойство планировщика (не P13): вернуть глубину очереди можно
    // только новой фазой (команда выбора линии). Закрепляем, чтобы изменение было осознанным.
    rig.gpu_nanos.store(1_000_000, Ordering::SeqCst);
    let frames_now = rig.frames.lock().expect("кадры").len();
    assert!(wait_until(DEADLINE, || rig.frames.lock().expect("кадры").len() > frames_now + 20));
    assert_eq!(rig.last_frame().target.timeline_index, 1);
    drop(rig.handle.stop());
}

#[test]
fn adpf_session_is_created_once_by_the_render_thread_and_gets_every_frame_with_late_gpu() {
    let fake = FakePerformanceHints::new(FAKE_TID);
    let gpu = Duration::from_micros(9_500);
    let rig = start(
        cost_config(PresentTimelinePolicy::Preferred),
        AdpfConfig {
            mode: AdpfMode::Enabled,
            ..AdpfConfig::default()
        },
        Arc::new(fake.clone()),
        gpu,
    );
    let status = rig.wait_status("отчёты ADPF не идут", |status| {
        status.adpf.reports_with_gpu >= 20
    });
    assert_eq!(status.adpf.session, AdpfSessionState::Active);
    let journal = fake.journal();
    assert_eq!(journal.sessions_created, vec![vec![HintThreadId::from_raw(FAKE_TID)]]);
    assert_eq!(
        journal.creating_threads,
        vec![Some(RENDER_THREAD_NAME.to_owned())],
        "сессию создаёт сам поток рендера"
    );
    // Цель — 0,75 периода (пейсинг по умолчанию 60 Гц).
    let target = journal.initial_targets[0];
    assert!((target.as_secs_f64() - 0.75 / 60.0).abs() < 1e-4, "цель {target:?}");
    let with_gpu: Vec<_> = journal.reports.iter().filter(|report| report.gpu().is_some()).collect();
    assert!(with_gpu.len() >= 20);
    for report in &with_gpu {
        assert_eq!(report.gpu(), Some(gpu));
        assert!(report.cpu() > Duration::ZERO, "CPU кадра измерен часами потока");
        assert_eq!(report.total(), report.cpu() + gpu);
    }
    // Начала периодов растут: отчёты идут по порядку кадров, хоть GPU и приходит позже.
    assert!(
        journal
            .reports
            .windows(2)
            .all(|pair| pair[0].period_start() < pair[1].period_start())
    );
    assert_eq!(status.adpf.report_failures, 0);
    drop(rig.handle.stop());
    assert_eq!(fake.journal().sessions_closed, 1, "остановка потока закрывает сессию");
}

#[test]
fn adpf_command_disables_and_reenables_the_session() {
    let fake = FakePerformanceHints::new(FAKE_TID);
    let rig = start(
        cost_config(PresentTimelinePolicy::Preferred),
        AdpfConfig {
            mode: AdpfMode::Enabled,
            ..AdpfConfig::default()
        },
        Arc::new(fake.clone()),
        Duration::from_millis(5),
    );
    rig.wait_status("сессия не создана", |status| {
        status.adpf.reports_sent >= 5
    });
    let disable = AdpfCommand::Set {
        mode: AdpfMode::Disabled,
    };
    assert_eq!(rig.handle.inbox().adpf_command(disable), InboxDelivery::Delivered);
    let disabled = rig.wait_status("команда не применена", |status| {
        status.adpf_commands.applied == 1
    });
    assert_eq!(disabled.adpf.session, AdpfSessionState::Disabled);
    assert_eq!(fake.journal().sessions_closed, 1);
    let reports_after_disable = fake.journal().reports.len();
    let frames_now = rig.handle.status().frames_presented;
    rig.wait_status("показ остановился", |status| {
        status.frames_presented >= frames_now + 10
    });
    assert_eq!(
        fake.journal().reports.len(),
        reports_after_disable,
        "выключенный ADPF молчит"
    );

    assert_eq!(
        rig.handle.inbox().adpf_command(AdpfCommand::Set {
            mode: AdpfMode::Enabled
        }),
        InboxDelivery::Delivered
    );
    rig.wait_status("новая сессия не создана", |status| {
        status.adpf.sessions_created == 2 && status.adpf.session == AdpfSessionState::Active
    });
    assert_eq!(
        rig.handle.status().adpf_commands.last,
        Some((
            AdpfCommand::Set {
                mode: AdpfMode::Enabled
            },
            AdpfCommandOutcome::Applied
        ))
    );
    drop(rig.handle.stop());
}

#[test]
fn unsupported_adpf_is_an_honest_state_and_presentation_goes_on() {
    let fake = FakePerformanceHints::new(FAKE_TID);
    fake.set_behavior(FakeHintBehavior {
        create_error: Some(PerformanceHintError::Unsupported {
            reason: "createSession вернул null".to_owned(),
        }),
        ..FakeHintBehavior::default()
    });
    let rig = start(
        cost_config(PresentTimelinePolicy::Preferred),
        AdpfConfig {
            mode: AdpfMode::Enabled,
            ..AdpfConfig::default()
        },
        Arc::new(fake.clone()),
        Duration::from_millis(5),
    );
    let status = rig.wait_status("показ не идёт", |status| status.frames_presented >= 30);
    assert_eq!(status.adpf.session, AdpfSessionState::Unsupported);
    assert_eq!(fake.journal().create_attempts, 1, "без повторов на каждом кадре");
    assert!(
        status
            .adpf
            .last_error
            .is_some_and(|error| error.contains("createSession"))
    );
    assert_eq!(status.adpf.reports_sent, 0);
    drop(rig.handle.stop());
}

#[test]
fn disabled_adpf_never_touches_the_service() {
    let fake = FakePerformanceHints::new(FAKE_TID);
    let rig = start(
        cost_config(PresentTimelinePolicy::Preferred),
        AdpfConfig::default(),
        Arc::new(fake.clone()),
        Duration::from_millis(5),
    );
    let status = rig.wait_status("показ не идёт", |status| status.frames_presented >= 20);
    assert_eq!(status.adpf.session, AdpfSessionState::Disabled);
    drop(rig.handle.stop());
    assert_eq!(fake.journal().create_attempts, 0);
}
