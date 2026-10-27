//! Функциональные тесты выбора линии кадра через настоящий поток рендера (сессия 07):
//! линии кадра тика доходят до бэкенда как `FrameTiming::target` (выбранная линия, срок,
//! момент позднего захвата), замеры CPU бэкенда двигают оценку стоимости кадра, а показы
//! попадают в гистограмму опоздания относительно задуманного vsync.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameCpuTiming, FrameTiming, LatchWaitResult,
    PastPresentation, PresentOutcome, TargetChoice,
};
use render_loop::config::{
    DepthReductionMode, LateLatchMode, PresentTargetConfig, PresentTimelinePolicy, RenderLoopConfig,
};
use render_loop::mailbox::InboxDelivery;
use render_loop::thread::{
    FrameClock, FrameTick, FrameWait, MonotonicTimer, PresentTargetCommand, PresentTargetCommandOutcome,
    PresentTargetSelection, RenderLoop, RenderLoopHandle, RenderLoopParts, RenderStatus, SystemMonotonicTimer,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceSize};
use shell_contract::vsync::{FrameTimelines, VsyncTimeline};
use support::{FakeCommands, Journal, SIZE, fake_surface, wait_until};

const DEADLINE: Duration = Duration::from_secs(5);
/// Период линий кадра в тесте: 60 Гц.
const PERIOD_NANOS: i64 = 16_666_667;
/// Линий в тике и индекс предпочитаемой (как у Choreographer: платформа предпочитает не первую).
const TIMELINE_COUNT: usize = 4;
const PREFERRED_INDEX: usize = 2;
/// Замер CPU, который сообщает fake-бэкенд.
const REPORTED_ACQUIRE: Duration = Duration::from_micros(1_200);
const REPORTED_COST: Duration = Duration::from_micros(1_800);
const REPORTED_LATCH_WAIT: Duration = Duration::from_micros(700);

/// Что получил бэкенд: время каждого кадра.
#[derive(Default)]
struct Received {
    timings: Mutex<Vec<FrameTiming>>,
}

impl Received {
    fn timings(&self) -> Vec<FrameTiming> {
        self.timings.lock().expect("кадры").clone()
    }
}

/// Fake-бэкенд: запоминает `FrameTiming`, сообщает замер CPU и показ «на задуманном vsync + 1».
struct RecordingBackend {
    received: Arc<Received>,
    held_surface: Option<NativeSurface>,
    unreported: Vec<FrameTiming>,
    pending_cpu_timing: Option<FrameCpuTiming>,
}

impl DisplayBackend for RecordingBackend {
    fn attach(&mut self, surface: NativeSurface, _size: SurfaceSize) -> Result<(), AttachError> {
        self.held_surface = Some(surface);
        Ok(())
    }

    fn resize(&mut self, _size: SurfaceSize) -> Result<(), BackendError> {
        Ok(())
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        self.received.timings.lock().expect("кадры").push(timing);
        self.unreported.push(timing);
        self.pending_cpu_timing = Some(FrameCpuTiming {
            acquire_wait: REPORTED_ACQUIRE,
            latch_wait: REPORTED_LATCH_WAIT,
            latch_result: LatchWaitResult::Waited,
            latch_to_submit: Some(REPORTED_COST),
        });
        PresentOutcome::Presented
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        self.held_surface.take()
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        BackendDiagnostics::default()
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        // Каждый кадр «показан» ровно на один vsync позже задуманного.
        self.unreported
            .drain(..)
            .map(|timing| PastPresentation {
                present_id: u32::try_from(timing.frame_index).expect("номер"),
                frame_index: timing.frame_index,
                actual_present: MonotonicNanos::from_nanos(timing.expected_present().as_nanos() + PERIOD_NANOS),
            })
            .collect()
    }

    fn take_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        self.pending_cpu_timing.take()
    }
}

/// Часы кадров с несколькими линиями на тик, отсчитанными от НАСТОЯЩЕГО `CLOCK_MONOTONIC`:
/// линия `i` показывается через `i + 1` периодов, срок — за 4 мс до показа.
struct TimelineClock;

impl TimelineClock {
    fn timelines_from_now() -> FrameTimelines {
        let now = SystemMonotonicTimer.now().expect("часы").as_nanos();
        FrameTimelines::from_platform_list(TIMELINE_COUNT, PREFERRED_INDEX, |index| {
            let periods = i64::try_from(index).expect("индекс") + 1;
            VsyncTimeline {
                vsync_id: periods,
                expected_present: MonotonicNanos::from_nanos(now + periods * PERIOD_NANOS),
                deadline: MonotonicNanos::from_nanos(now + periods * PERIOD_NANOS - 4_000_000),
            }
        })
        .expect("линии корректны")
    }
}

impl FrameClock for TimelineClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        // Тики с реальным периодом: линии соседних тиков сдвинуты на vsync, как у Choreographer
        // (иначе защита «два кадра на один vsync» законно уводила бы выбор на следующую линию).
        thread::sleep(timeout.min(Duration::from_nanos(PERIOD_NANOS.unsigned_abs())));
        let timelines = Self::timelines_from_now();
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
    received: Arc<Received>,
    _journal: Arc<Journal>,
}

fn start(present_target: PresentTargetConfig) -> Rig {
    let journal = Arc::new(Journal::default());
    let received = Arc::new(Received::default());
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: RenderLoopConfig {
            preferred_frame_rate_hz: 60.0,
            // Частый опрос: снимок учёта линий попадает в статус при опросе диагностики.
            diagnostics_poll_interval_ms: 5,
            present_target,
            ..RenderLoopConfig::default()
        },
        backend: RecordingBackend {
            received: Arc::clone(&received),
            held_surface: None,
            unreported: Vec::new(),
            pending_cpu_timing: None,
        },
        clock: TimelineClock,
        commands: Arc::new(FakeCommands::default()),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
    })
    .expect("поток запускается");
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&journal, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
    Rig {
        handle,
        received,
        _journal: journal,
    }
}

impl Rig {
    fn wait_status(&self, what: &str, condition: impl Fn(&RenderStatus) -> bool) -> RenderStatus {
        assert!(
            wait_until(DEADLINE, || condition(&self.handle.status())),
            "{what}: {:?}",
            self.handle.status().present_target
        );
        self.handle.status()
    }
}

fn small_windows(policy: PresentTimelinePolicy) -> PresentTargetConfig {
    PresentTargetConfig {
        policy,
        initial_frame_cost_us: 3_000,
        // Слагаемое GPU до замеров выключено: здесь проверяется CPU-часть оценки (GPU — отдельные тесты).
        gpu_cost_fallback_us: 0,
        min_cost_samples: 5,
        cost_window_frames: 10,
        stats_window_frames: 20,
        latch_margin_us: 1_000,
        // Поведение до P14: без позднего захвата; продуктовый захват проверяет отдельный тест.
        late_latch: LateLatchMode::Disabled,
        ..PresentTargetConfig::default()
    }
}

/// Выбор линии как до P14 сессии 07 (профиль `baseline` A/B): `Preferred`, без захвата.
fn pre_p14_defaults() -> PresentTargetConfig {
    PresentTargetConfig {
        policy: PresentTimelinePolicy::Preferred,
        late_latch: LateLatchMode::Disabled,
        ..PresentTargetConfig::default()
    }
}

#[test]
fn preferred_policy_passes_the_preferred_timeline_and_latch_plan_to_the_backend() {
    let rig = start(pre_p14_defaults());
    rig.wait_status("кадры не пошли", |status| status.frames_presented >= 3);
    let timing = rig.received.timings()[0];
    assert_eq!(timing.target.choice, TargetChoice::PlatformPreferred);
    assert_eq!(timing.target.timeline_index, PREFERRED_INDEX);
    assert_eq!(timing.target.preferred_index, PREFERRED_INDEX);
    // Линия 2 — показ через 3 периода, срок за 4 мс до него.
    assert_eq!(
        timing.expected_present().as_nanos() - timing.target.deadline.as_nanos(),
        4_000_000
    );
    assert_eq!(timing.target.vsync_id, 3);
    // Первый кадр: замеров ещё нет, стоимость — начальная CPU 6 мс + оценка GPU 13 мс (бэкенд GPU
    // не меряет), запас 1 мс.
    assert_eq!(
        timing.target.deadline.as_nanos() - timing.target.latch.latch_at.as_nanos(),
        20_000_000
    );
    assert_eq!(timing.target.latch.spin_window, Duration::from_micros(500));
    assert_eq!(
        timing.target.latch.mode,
        LateLatchMode::Disabled,
        "захват выключен — не ждём"
    );
    drop(rig.handle.stop());
}

/// Продукт по умолчанию (P14): ближайшая выполнимая линия и поздний захват доходят до бэкенда.
#[test]
fn product_defaults_target_the_nearest_feasible_timeline_with_late_latch() {
    let rig = start(PresentTargetConfig::default());
    rig.wait_status("кадры не пошли", |status| status.frames_presented >= 3);
    let timing = rig.received.timings()[0];
    assert_ne!(timing.target.choice, TargetChoice::PlatformPreferred);
    assert_eq!(timing.target.latch.mode, LateLatchMode::BeforeCameraPoll);
    drop(rig.handle.stop());
}

/// Решение 54 через настоящий поток рендера: дорогой старт уводит кадры на дальнюю линию, после
/// серии «ближняя выполнима» поток один тик НЕ зовёт бэкенд (нет ни рисования, ни опроса камеры),
/// и дальше кадры метят на ближнюю линию. Номера кадров при пропуске не расходуются.
#[test]
fn depth_reduction_skips_one_tick_without_calling_the_backend_and_then_targets_nearer() {
    let rig = start(PresentTargetConfig {
        initial_frame_cost_us: 15_000,
        depth_reduction: DepthReductionMode::SkipOneVsync,
        depth_reduction_streak_frames: 5,
        depth_reduction_min_interval_ms: 0,
        ..small_windows(PresentTimelinePolicy::NearestFeasible { margin_us: 1_000 })
    });
    let status = rig.wait_status("сход не случился", |status| {
        status.present_target.depth_reductions >= 1 && status.frames_presented >= 20
    });
    assert_eq!(status.present_target.skipped_ticks_for_depth, 1, "ровно один пропуск");
    assert_eq!(status.present_target.depth_skips_without_reduction, 0);
    let timings = rig.received.timings();
    assert!(
        timings
            .iter()
            .enumerate()
            .all(|(index, timing)| timing.frame_index == index as u64),
        "пропуск не расходует номер кадра и не доходит до бэкенда"
    );
    assert_eq!(timings[0].target.timeline_index, 1, "дорогой старт — дальняя линия");
    let last = timings.last().expect("кадры");
    assert_eq!(last.target.timeline_index, 0, "после схода — ближняя линия");
    drop(rig.handle.stop());
}

#[test]
fn enabled_late_latch_mode_reaches_the_backend_in_the_plan() {
    let rig = start(PresentTargetConfig {
        late_latch: LateLatchMode::BeforeCameraPoll,
        ..PresentTargetConfig::default()
    });
    rig.wait_status("кадры не пошли", |status| status.frames_presented >= 1);
    assert_eq!(
        rig.received.timings()[0].target.latch.mode,
        LateLatchMode::BeforeCameraPoll
    );
    drop(rig.handle.stop());
}

#[test]
fn nearest_feasible_policy_reaches_the_backend_and_is_counted() {
    let rig = start(small_windows(PresentTimelinePolicy::NearestFeasible {
        margin_us: 2_000,
    }));
    let status = rig.wait_status("выбор линии не учтён", |status| {
        status.present_target.frames_planned >= 5 && status.frames_presented >= 5
    });
    // Линия 0: до срока 12,67 мс ≥ стоимость (3 мс начальная, затем 1,8 мс) + запас 2 мс.
    let first = rig.received.timings()[0];
    assert_eq!(first.target.choice, TargetChoice::NearestFeasible);
    assert_eq!(first.target.timeline_index, 0);
    assert_eq!(first.target.vsync_id, 1);
    let diagnostics = &status.present_target;
    assert_eq!(
        diagnostics.chosen_timeline[0], diagnostics.frames_planned,
        "всегда самая ранняя"
    );
    assert_eq!(diagnostics.chosen_earlier_than_preferred, diagnostics.frames_planned);
    assert_eq!(diagnostics.fallbacks_to_preferred, 0);
    drop(rig.handle.stop());
}

#[test]
fn impossible_margin_falls_back_to_preferred_through_the_thread() {
    let rig = start(small_windows(PresentTimelinePolicy::NearestFeasible {
        margin_us: 900_000,
    }));
    let status = rig.wait_status("откат не учтён", |status| {
        status.present_target.fallbacks_to_preferred >= 3
    });
    let first = rig.received.timings()[0];
    assert_eq!(first.target.choice, TargetChoice::FallbackToPreferred);
    assert_eq!(first.target.timeline_index, PREFERRED_INDEX);
    assert_eq!(
        status.present_target.chosen_timeline[PREFERRED_INDEX],
        status.present_target.frames_planned
    );
    drop(rig.handle.stop());
}

#[test]
fn backend_cpu_timings_drive_the_cost_estimate_and_the_latch_moment() {
    let rig = start(small_windows(PresentTimelinePolicy::Preferred));
    let status = rig.wait_status("оценка стоимости не обновилась", |status| {
        status.present_target.estimated_frame_cost == REPORTED_COST
    });
    let acquire = status.present_target.acquire_wait.expect("замеры acquire");
    assert_eq!(acquire.p50, REPORTED_ACQUIRE);
    assert_eq!(acquire.max, REPORTED_ACQUIRE);
    assert_eq!(
        status.present_target.latch_wait.expect("замеры ожидания").p95,
        REPORTED_LATCH_WAIT
    );
    let cost = status.present_target.latch_to_submit.expect("замеры стоимости");
    assert_eq!(cost.p95, REPORTED_COST);
    // После min_cost_samples кадров latch_at = deadline − 1,8 мс − 1 мс.
    assert!(
        wait_until(DEADLINE, || rig.received.timings().iter().any(|timing| {
            timing.target.deadline.as_nanos() - timing.target.latch.latch_at.as_nanos() == 2_800_000
        })),
        "момент захвата не учёл измеренную стоимость"
    );
    drop(rig.handle.stop());
}

#[test]
fn presentations_one_vsync_late_land_in_the_shift_histogram() {
    let rig = start(small_windows(PresentTimelinePolicy::Preferred));
    let status = rig.wait_status("гистограмма опоздания пуста", |status| {
        status.present_target.vsync_shift.late_by_periods[1] >= 5
    });
    let shift = status.present_target.vsync_shift;
    assert_eq!(
        shift.late_by_periods[0], 0,
        "fake показывает каждый кадр на vsync позже"
    );
    assert_eq!(shift.early, 0);
    assert_eq!(shift.unmatched, 0);
    drop(rig.handle.stop());
}

#[test]
fn runtime_command_switches_the_chosen_timeline_and_starts_a_fresh_phase() {
    let rig = start(small_windows(PresentTimelinePolicy::Preferred));
    let before = rig.wait_status("фаза A не набралась", |status| {
        status.present_target.chosen_timeline[PREFERRED_INDEX] >= 5
    });
    assert_eq!(
        before.present_target.selection,
        PresentTargetSelection {
            policy: PresentTimelinePolicy::Preferred,
            late_latch: LateLatchMode::Disabled,
            depth_reduction: render_loop::config::DepthReductionMode::default(),
        },
        "на старте — выбор из конфига"
    );
    let switch = PresentTargetCommand::Set {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 2_000 },
        late_latch: LateLatchMode::BeforeCameraPoll,
        depth_reduction: render_loop::config::DepthReductionMode::default(),
    };
    let frames_before_command = rig.received.timings().len();
    assert_eq!(
        rig.handle.inbox().present_target_command(switch),
        InboxDelivery::Delivered
    );

    let after = rig.wait_status("фаза B не набралась", |status| {
        status.present_target_commands.applied == 1 && status.present_target.frames_planned >= 5
    });
    assert_eq!(
        after
            .present_target_commands
            .last
            .as_ref()
            .map(|record| &record.outcome),
        Some(&PresentTargetCommandOutcome::Applied)
    );
    let phase = &after.present_target;
    assert_eq!(phase.selection, switch.selection(), "действующий выбор — из команды");
    assert_eq!(
        phase.chosen_timeline[0], phase.frames_planned,
        "после команды выбирается самая ранняя линия, и счётчики фазы A обнулены"
    );
    assert_eq!(phase.chosen_timeline[PREFERRED_INDEX], 0);
    assert_eq!(phase.vsync_shift.unmatched, 0, "показы фазы A в фазу B не попали");
    // Бэкенд получает новую линию и включённый поздний захват уже в планах кадров.
    let timings = rig.received.timings();
    let last = timings.last().expect("кадры после команды");
    assert!(timings.len() > frames_before_command);
    assert_eq!(last.target.choice, TargetChoice::NearestFeasible);
    assert_eq!(last.target.timeline_index, 0);
    assert_eq!(last.target.latch.mode, LateLatchMode::BeforeCameraPoll);

    // Повтор — «уже действует», счётчики не обнуляются.
    let planned = rig.handle.status().present_target.frames_planned;
    rig.handle.inbox().present_target_command(switch);
    let repeated = rig.wait_status("повтор не учтён", |status| {
        status.present_target_commands.already_in_effect == 1
    });
    assert!(repeated.present_target.frames_planned >= planned);
    drop(rig.handle.stop());
}

#[test]
fn invalid_runtime_command_is_rejected_and_the_config_choice_stays() {
    let rig = start(small_windows(PresentTimelinePolicy::Preferred));
    rig.wait_status("кадры не пошли", |status| status.frames_presented >= 2);
    rig.handle.inbox().present_target_command(PresentTargetCommand::Set {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 5_000_000 },
        late_latch: LateLatchMode::BeforeCameraPoll,
        depth_reduction: render_loop::config::DepthReductionMode::default(),
    });
    let status = rig.wait_status("отказ не учтён", |status| {
        status.present_target_commands.rejected == 1
    });
    let outcome = status
        .present_target_commands
        .last
        .as_ref()
        .map(|record| record.outcome.clone());
    assert!(
        matches!(&outcome, Some(PresentTargetCommandOutcome::Rejected { reason }) if reason.contains("margin_us")),
        "{outcome:?}"
    );
    assert_eq!(status.present_target_commands.applied, 0);
    assert_eq!(status.present_target.selection.policy, PresentTimelinePolicy::Preferred);
    let frames = rig.received.timings().len();
    rig.wait_status("кадры после отказа", |status| {
        status.frames_presented as usize > frames + 2
    });
    let last = *rig.received.timings().last().expect("кадры");
    assert_eq!(
        last.target.choice,
        TargetChoice::PlatformPreferred,
        "выбор не изменился"
    );
    assert_eq!(last.target.latch.mode, LateLatchMode::Disabled);
    drop(rig.handle.stop());
}
