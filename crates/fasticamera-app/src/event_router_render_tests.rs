//! Функциональные тесты решения 31: маршрутизатор отдаёт surface получателю событий экрана
//! (поток рендера или запасной маршрут), а сценарии `render`, `render-lifecycle`, `ahb-import`
//! и `gpu-fence` доходят до итога через настоящий маршрутизатор на подставных рендере и GPU.

use render_loop::thread::RenderState;
use shell_contract::surface::SurfaceGeneration;
use std::collections::BTreeMap;

use super::*;
use crate::gpu_services::{AhbCheckResult, AhbCheckSummary, AhbFormat, FenceCheckResult, FenceCheckSummary};
use crate::render_route::HoldAndConfirmRoute;

// ---------- маршрутизация surface ----------

#[test]
fn surface_events_reach_the_route_in_order_and_router_keeps_only_facts() {
    let mut harness = harness(ScenarioConfig::default());
    harness.surface_created(5);
    harness.surface_changed(5);
    harness.deliver(ShellEvent::Resumed);
    harness.deliver(ShellEvent::Paused);
    assert_eq!(harness.surface_destroyed(5), SurfaceReleaseOutcome::Confirmed);

    let size = SurfaceSize {
        width_px: 1440,
        height_px: 3216,
    };
    assert_eq!(
        *harness.route_log.lock().expect("замок"),
        [
            RoutedEvent::SurfaceCreated(5),
            RoutedEvent::SurfaceChanged(5, size),
            RoutedEvent::Resumed,
            RoutedEvent::Paused,
            RoutedEvent::SurfaceDestroyed(5),
        ],
        "события экрана доходят получателю по порядку"
    );
    assert!(
        harness.route_surfaces.lock().expect("замок").is_empty(),
        "после destroyed surface у получателя нет"
    );
    assert_eq!(harness.router.display().surface_generation(), None);
}

#[test]
fn surface_lives_in_the_route_while_router_tracks_generation_and_size() {
    let mut harness = harness(ScenarioConfig::default());
    harness.surface_created(9);
    harness.surface_changed(9);
    harness.deliver(ShellEvent::Resumed);
    let held: Vec<u64> = harness
        .route_surfaces
        .lock()
        .expect("замок")
        .iter()
        .map(|surface| surface.generation().value())
        .collect();
    assert_eq!(held, [9], "surface принадлежит получателю");
    assert_eq!(
        harness.router.display().surface_generation(),
        Some(SurfaceGeneration::new(9))
    );
    assert!(harness.router.display().is_ready_to_draw());
}

#[test]
fn router_does_not_confirm_release_itself_the_route_does() {
    let mut harness = harness_with_render(
        ScenarioConfig::default(),
        RenderLoopConfig::default(),
        ReceiptPolicy::Hold,
    );
    harness.surface_created(1);
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    harness.deliver(ShellEvent::SurfaceDestroyed(request));
    assert_eq!(
        waiter.wait(Duration::from_millis(50)),
        SurfaceReleaseOutcome::TimedOut,
        "пока получатель не подтвердил, оболочка ждёт"
    );
}

#[test]
fn dead_render_loop_drops_the_receipt_and_router_keeps_working() {
    let (inbox, receiver) = render_loop::mailbox::render_mailbox();
    drop(receiver);
    let render = FakeRenderStatus::live(presenting_status(), ValidationState::Enabled);
    let diagnostics = FakeGpuDiagnostics::new(Vec::new(), None);
    let mut harness = build_harness(
        ScenarioConfig::default(),
        Box::new(crate::render_route::InboxRoute::new(inbox)),
        render,
        diagnostics,
        RenderLoopConfig::default(),
    );
    harness.surface_created(1);
    assert_eq!(
        harness.surface_destroyed(1),
        SurfaceReleaseOutcome::DroppedWithoutConfirmation,
        "оболочка сразу узнаёт, что подтверждения не будет"
    );
    harness.deliver(ShellEvent::Resumed);
    assert!(harness.router.display().surface_generation().is_none());
}

#[test]
fn unavailable_render_falls_back_to_immediate_confirmation_and_scenarios_see_the_reason() {
    let render = FakeRenderStatus::unavailable("GPU не поднялся: нет Vulkan");
    let diagnostics = FakeGpuDiagnostics::new(Vec::new(), None);
    let mut harness = build_harness(
        ScenarioConfig::default(),
        Box::new(HoldAndConfirmRoute::new()),
        render,
        diagnostics,
        RenderLoopConfig::default(),
    );
    harness.surface_created(3);
    harness.surface_changed(3);
    harness.deliver(ShellEvent::Resumed);
    assert_eq!(
        harness.surface_destroyed(3),
        SurfaceReleaseOutcome::Confirmed,
        "surfaceDestroyed не зависает, даже если рендера нет"
    );

    harness.start(Scenario::Render);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("нет Vulkan"),
        "{result}"
    );
}

// ---------- render ----------

/// Рендер, который начинает показ не сразу.
fn starting_render() -> Arc<FakeRenderStatus> {
    let mut status = presenting_status();
    status.state = RenderState::WaitingForSurface;
    status.frames_presented = 0;
    FakeRenderStatus::live(status, ValidationState::Enabled)
}

/// Конфиг потока рендера с коротким окном пейсинга, чтобы тест не ждал 10 секунд.
fn quick_window() -> RenderLoopConfig {
    RenderLoopConfig {
        // Статусы подставных тестов — на 120 Гц; по умолчанию приложение целится в 60.
        preferred_frame_rate_hz: 120.0,
        pacing_window_s: 0.15,
        ..RenderLoopConfig::default()
    }
}

fn harness_with_status(
    config: ScenarioConfig,
    render_config: RenderLoopConfig,
    render: Arc<FakeRenderStatus>,
    diagnostics: Arc<FakeGpuDiagnostics>,
) -> Harness {
    build_harness(
        config,
        Box::new(RecordingRoute::new(ReceiptPolicy::Confirm)),
        render,
        diagnostics,
        render_config,
    )
}

#[test]
fn render_scenario_waits_for_presenting_then_observes_window_and_passes() {
    let render = starting_render();
    let mut harness = harness_with_status(
        ScenarioConfig::default(),
        quick_window(),
        Arc::clone(&render),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    harness.router.step(Duration::from_millis(5));
    assert!(harness.output.stage_names().is_empty(), "показа нет — стадии нет");

    render.update(|status| status.state = RenderState::Presenting);
    // Кадры ещё не пошли: наблюдение не начинается.
    harness.router.step(Duration::from_millis(5));
    assert!(harness.output.stage_names().is_empty());
    render.update(|status| status.frames_presented = 10);
    harness.router.step(Duration::from_millis(5));
    assert_eq!(harness.output.stage_names(), ["presenting"]);
    assert!(
        harness.output.results.lock().expect("замок").is_empty(),
        "окно ещё не прошло"
    );

    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["scenario"], "render");
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["display"]["width_px"], 1440);
    assert_eq!(result["report"]["pacing"]["misses"], 0);
}

#[test]
fn render_scenario_failure_carries_report_with_the_failed_numbers() {
    let render = FakeRenderStatus::live(presenting_status(), ValidationState::Enabled);
    render.update(|status| {
        status.pacing.verdict = render_loop::pacing::PacingVerdict::Failed {
            miss_ratio: 0.1,
            max_miss_ratio: 0.005,
            misses: 120,
            intervals: 1200,
        };
        status.pacing.misses = 120;
    });
    let mut harness = harness_with_status(
        ScenarioConfig::default(),
        quick_window(),
        render,
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("120 из 1200"),
        "{result}"
    );
    assert_eq!(result["report"]["pacing"]["misses"], 120, "отчёт виден и при провале");
}

#[test]
fn render_scenario_times_out_when_presentation_never_starts() {
    let config = ScenarioConfig {
        render_stage_timeout_ms: 30,
        ..ScenarioConfig::default()
    };
    let mut harness = harness_with_status(
        config,
        quick_window(),
        starting_render(),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("показ не начался"),
        "{result}"
    );
}

#[path = "event_router_refresh_tests.rs"]
mod refresh_tests;

// ---------- render-lifecycle ----------

fn lifecycle_config() -> ScenarioConfig {
    ScenarioConfig {
        render_lifecycle_cycles: 2,
        render_lifecycle_screen_cycles: 1,
        render_settle_ms: 5,
        render_stage_timeout_ms: 300,
        ..ScenarioConfig::default()
    }
}

/// Поведение «телефона» для тестов: отвечает на стадии сценария так, как это делают скрипт и рендер.
struct PhoneSimulation {
    seen_stages: usize,
    /// Сколько лишних `Fence` добавляется в покое после каждого освобождения, начиная со второго
    /// (имитация утечки; первое освобождение задаёт базу).
    leak_per_release: u64,
    /// Сколько раз surface освобождена.
    releases_done: u64,
    /// Не освобождать surface на стадии `presenting_*` (имитация зависшего рендера).
    never_release: bool,
}

impl PhoneSimulation {
    fn healthy() -> Self {
        Self {
            seen_stages: 0,
            leak_per_release: 0,
            releases_done: 0,
            never_release: false,
        }
    }

    fn react(&mut self, harness: &Harness) {
        let names = harness.output.stage_names();
        for name in &names[self.seen_stages..] {
            match name.as_str() {
                "presenting_home" | "presenting_screen" if !self.never_release => {
                    // В покое поток рендера снимает перепись сразу после detach.
                    let leaked_fences = self.leak_per_release * self.releases_done;
                    self.releases_done += 1;
                    harness.render.update(|status| {
                        status.state = RenderState::WaitingForSurface;
                        status.generation = None;
                        status.releases_confirmed += 1;
                        status.last_release_latency = Some(Duration::from_millis(7));
                        status.backend.live_objects_by_kind =
                            BTreeMap::from([("Fence".to_owned(), 3 + leaked_fences), ("Image".to_owned(), 5)]);
                    });
                }
                "backgrounded_home" | "backgrounded_screen" => {
                    let returns = self.releases_done;
                    harness.render.update(|status| {
                        status.state = RenderState::Presenting;
                        let next = status.surfaces_attached + 1;
                        status.surfaces_attached = next;
                        status.generation = Some(SurfaceGeneration::new(next + 1));
                        // При показе число объектов шумит (20/21 на телефоне) — на вердикт не влияет.
                        status.backend.live_objects = 20 + returns % 2;
                    });
                }
                _ => {}
            }
        }
        self.seen_stages = names.len();
    }
}

fn run_lifecycle(mut phone: PhoneSimulation, config: ScenarioConfig) -> (serde_json::Value, Vec<String>) {
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::RenderLifecycle);
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        phone.react(&harness);
    }
    (harness.output.single_result(), harness.output.stage_names())
}

#[test]
fn render_lifecycle_runs_all_cycles_and_passes_on_a_healthy_phone() {
    let (result, stages) = run_lifecycle(PhoneSimulation::healthy(), lifecycle_config());
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        stages,
        [
            "presenting_home",
            "backgrounded_home",
            "presenting_home",
            "backgrounded_home",
            "presenting_screen",
            "backgrounded_screen",
        ],
        "скрипт получает действие на каждый цикл: сначала HOME, затем экран"
    );
    assert_eq!(result["report"]["releases_confirmed"], 3);
    assert_eq!(result["report"]["cycles"].as_array().expect("циклы").len(), 3);
    assert_eq!(result["report"]["cycles"][2]["kind"], "screen");
    assert_eq!(result["report"]["release_latency"]["max_ms"], 7.0);
    assert_eq!(result["report"]["activity_recreated"], false);
    assert_eq!(result["report"]["idle_baseline_live_objects"]["Fence"], 3);
    let presenting: Vec<_> = result["report"]["cycles"]
        .as_array()
        .expect("циклы")
        .iter()
        .map(|cycle| cycle["presenting_live_objects"].clone())
        .collect();
    assert!(
        presenting.windows(2).any(|pair| pair[0] != pair[1]),
        "счёт при показе шумит, но вердикт проходит: {presenting:?}"
    );
}

#[test]
fn render_lifecycle_detects_gpu_object_growth() {
    let phone = PhoneSimulation {
        leak_per_release: 1,
        ..PhoneSimulation::healthy()
    };
    let (result, _) = run_lifecycle(phone, lifecycle_config());
    assert_eq!(result["status"], "failed");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("Fence: было 3, стало 4"), "{result}");
}

#[test]
fn render_lifecycle_fails_when_render_never_releases_the_surface() {
    let phone = PhoneSimulation {
        never_release: true,
        ..PhoneSimulation::healthy()
    };
    let (result, stages) = run_lifecycle(phone, lifecycle_config());
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"]
            .as_str()
            .expect("error")
            .contains("не освободил surface"),
        "{result}"
    );
    assert_eq!(stages, ["presenting_home"], "дальше первого цикла дело не пошло");
}

#[test]
fn render_lifecycle_counts_shell_release_timeouts_and_activity_recreation() {
    let mut harness = harness_with_status(
        lifecycle_config(),
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::RenderLifecycle);
    let mut phone = PhoneSimulation::healthy();
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        phone.react(&harness);
        // Оболочка не дождалась подтверждения и Activity пересоздалась — после первого цикла.
        if harness.output.stage_names().len() == 2 {
            let mut counters = harness.shell.counters.lock().expect("замок");
            counters.surface_release_timeouts = 1;
            counters.activity_attachments = 1;
        }
    }
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("таймауты освобождения"), "{error}");
    assert!(error.contains("Activity пересоздавалась"), "{error}");
}

/// Прогон `render-lifecycle` с заданным начальным значением счётчика таймаутов оболочки.
fn run_lifecycle_with_preexisting_timeouts(preexisting: u64, new_timeouts_after_first_cycle: u64) -> serde_json::Value {
    let mut harness = harness_with_status(
        lifecycle_config(),
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    // Таймауты, случившиеся ДО сценария (счётчик оболочки считается от запуска процесса).
    harness.shell.counters.lock().expect("замок").surface_release_timeouts = preexisting;
    harness.start(Scenario::RenderLifecycle);
    let mut phone = PhoneSimulation::healthy();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut injected = false;
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        phone.react(&harness);
        if !injected && harness.output.stage_names().len() == 2 {
            injected = true;
            harness.shell.counters.lock().expect("замок").surface_release_timeouts += new_timeouts_after_first_cycle;
        }
    }
    harness.output.single_result()
}

#[test]
fn render_lifecycle_ignores_release_timeouts_that_happened_before_it_started() {
    let result = run_lifecycle_with_preexisting_timeouts(7, 0);
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["surface_release_timeouts"], 0, "{result}");
}

#[test]
fn render_lifecycle_fails_on_a_new_timeout_on_top_of_preexisting_ones() {
    let result = run_lifecycle_with_preexisting_timeouts(7, 1);
    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("таймауты освобождения surface в оболочке: 1"), "{error}");
}

// ---------- ahb-import и gpu-fence ----------

fn ahb_summary() -> AhbCheckSummary {
    AhbCheckSummary {
        matched: true,
        second_import_hit: true,
        live_objects_restored: true,
        validation_errors: 0,
        known_deviations: Default::default(),
        validation: ValidationState::Enabled,
        details: "ok".to_owned(),
    }
}

#[test]
fn ahb_import_scenario_runs_both_formats_on_a_worker_and_reports() {
    let diagnostics = FakeGpuDiagnostics::new(
        vec![
            AhbCheckResult::Completed(ahb_summary()),
            AhbCheckResult::AllocationUnsupported {
                status: -22,
                requested: "RAW10".to_owned(),
            },
        ],
        None,
    );
    let config = ScenarioConfig {
        ahb_width: 64,
        ahb_height: 32,
        ..ScenarioConfig::default()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        Arc::clone(&diagnostics),
    );
    harness.start(Scenario::AhbImport);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["scenario"], "ahb-import");
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["raw16"]["outcome"], "matched");
    assert_eq!(result["report"]["raw10"]["outcome"], "unsupported");
    let requests = diagnostics.ahb_requests.lock().expect("замок").clone();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.format, request.width, request.height))
            .collect::<Vec<_>>(),
        [(AhbFormat::Raw16, 64, 32), (AhbFormat::Raw10, 64, 32)],
        "размеры берутся из конфига, формат RAW16 идёт первым"
    );
    // Оба формата — одной пачкой, то есть на одном GPU-контексте: пересоздание instance под
    // validation layer на телефоне убивало поток (сессия 03).
    assert_eq!(diagnostics.ahb_sessions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(harness.output.stage_names(), ["started"]);
}

#[test]
fn ahb_import_scenario_fails_on_mismatch() {
    let mut broken = ahb_summary();
    broken.matched = false;
    let diagnostics = FakeGpuDiagnostics::new(
        vec![
            AhbCheckResult::Completed(ahb_summary()),
            AhbCheckResult::Completed(broken),
        ],
        None,
    );
    let mut harness = harness_with_status(
        ScenarioConfig::default(),
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        diagnostics,
    );
    harness.start(Scenario::AhbImport);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(result["error"].as_str().expect("error").contains("Raw10"), "{result}");
    assert_eq!(result["report"]["raw10"]["outcome"], "mismatch");
}

#[test]
fn gpu_fence_scenario_passes_busy_iterations_from_config_and_reports() {
    let summary = FenceCheckSummary {
        expected_value: 7,
        work_a_value: 7,
        work_b_value: 7,
        b_started_after_a_finished: true,
        work_a_duration_ns: 1_000_000,
        work_b_duration_ns: 10_000,
        gap_between_a_end_and_b_begin_ns: 500,
        release_fence_pending_at_export: Some(true),
        exported_fence_was_already_signaled: false,
        already_signaled_wait_semaphore_created: false,
        already_signaled_work_completed: true,
        already_signaled_result_value: 7,
        validation_errors: 0,
        known_deviations: Default::default(),
        validation: ValidationState::Enabled,
    };
    let diagnostics = FakeGpuDiagnostics::new(Vec::new(), Some(FenceCheckResult::Completed(summary)));
    let config = ScenarioConfig {
        fence_busy_iterations: 1234,
        ..ScenarioConfig::default()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        Arc::clone(&diagnostics),
    );
    harness.start(Scenario::GpuFence);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["scenario"], "gpu-fence");
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["gap_between_a_end_and_b_begin_ns"], 500);
    assert_eq!(
        diagnostics.fence_requests.lock().expect("замок")[0].busy_iterations,
        1234
    );
}

#[test]
fn gpu_fence_scenario_without_gpu_fails_with_reason() {
    let mut harness = harness_with_status(
        ScenarioConfig::default(),
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        FakeGpuDiagnostics::new(
            Vec::new(),
            Some(FenceCheckResult::Failed {
                reason: "нет Vulkan".to_owned(),
            }),
        ),
    );
    harness.start(Scenario::GpuFence);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("нет Vulkan"),
        "{result}"
    );
}

// ---------- зависшая GPU-работа ----------

#[test]
fn hung_ahb_import_gets_a_timeout_verdict_instead_of_hanging_forever() {
    let (diagnostics, _keep_hanging) = FakeGpuDiagnostics::hanging();
    let config = ScenarioConfig {
        gpu_job_timeout_ms: 80,
        ..ScenarioConfig::default()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        diagnostics,
    );
    harness.start(Scenario::AhbImport);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("работа GPU не завершилась за 80 мс"), "{error}");
    assert!(
        error.contains("завис RAW16"),
        "частичные сведения о том, где застряло: {error}"
    );
}

#[test]
fn hung_gpu_fence_gets_a_timeout_verdict_instead_of_hanging_forever() {
    let (diagnostics, _keep_hanging) = FakeGpuDiagnostics::hanging();
    let config = ScenarioConfig {
        gpu_job_timeout_ms: 80,
        fence_busy_iterations: 4321,
        ..ScenarioConfig::default()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig::default(),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        diagnostics,
    );
    harness.start(Scenario::GpuFence);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("работа GPU не завершилась за 80 мс"), "{error}");
    assert!(error.contains("4321"), "{error}");
}
