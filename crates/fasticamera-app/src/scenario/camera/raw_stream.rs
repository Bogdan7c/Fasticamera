//! Сценарии `raw-stream`, `raw-stream-endurance` и `raw-stream-cpu-probe` (сессия 05): вся цепочка
//! «камера -> лизинг кадров -> рендер -> экран» живьём, с честным вердиктом по числам.
//!
//! Ход прогона (всё по тикам маршрутизатора, ничего не блокируется):
//! 1. сценарий заявляет права на камеру, снимает базовые счётчики и открывает основную камеру
//!    (роль `wide`) в режиме из конфига; для пробы CPU заранее взводит пробу — читатель получает
//!    нужный usage только при создании;
//! 2. ждёт `Streaming` (стадия `streaming`), затем — пока рендер покажет первый кадр камеры
//!    (стадия `viewfinder_live`: на ней скрипт делает два скриншота и сравнивает их);
//! 3. выдерживает замер, раз в `progress_interval_ms` пишет стадию `progress` и на каждом тике
//!    снимает выборки (владение метаданными кадра, задержка «сенсор -> дисплей»);
//! 4. пишет подробности (`detail_stream`) и результат: вердикт — [`evaluate_raw_stream`].
//!
//! Камеру закрывает и пробу CPU выключает маршрутизатор при любом конце сценария
//! (`CameraServices::scenario_ended`): сценарий не может «забыть» об этом на каком-то из путей.

use std::time::{Duration, Instant};

use camera_android::reader_counters::{ArrivalAgeSnapshot, ReaderSnapshot};
use camera_core::backend::SessionGeneration;
use camera_core::{CameraEvent, CameraState, CameraStatus};
use raw_frame_stream::RawStreamCounters;
use render_loop::camera_diagnostics::CameraStreamDiagnostics;
use shell_contract::thermal::{HeadroomForecast, ThermalHeadroom};

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::{RunFailure, failure_of_event, fatal_failure, rejected_failure};
use super::progress::{Deadline, EventLog};
use super::raw_stream_deltas::{RenderCameraDelta, reader_delta, stream_delta};
use super::raw_stream_report::{DegradedNote, RawStreamObservation, RequestedMode, ThermalMoment, evaluate_raw_stream};
use super::raw_stream_samples::{LatencySampler, MetadataOwnershipTracker};
use super::stream_report::ConfiguredEntry;
use crate::camera_presentation::{CameraPresentation, CameraPresentationObservation};
use crate::camera_scenario_config::{ResolvedStep, SCENARIO_CAPTURE_INTENT};
use crate::camera_service::RunningCamera;
use crate::raw_stream_scenario_config::{RawStreamRunKind, RawStreamScenarioConfig};
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, judged, stage_fields,
};

/// Сколько байт JSON отдаём сводке в строке результата: запись logcat — 4000 байт, остальное уходит
/// на префикс, имя сценария и поле `error` с нарушениями.
const RESULT_SUMMARY_BUDGET_BYTES: usize = 2_900;

/// Снимки счётчиков на старте сценария: от них считаются приращения за прогон.
struct Baselines {
    stream: RawStreamCounters,
    reader: ReaderSnapshot,
    /// Счётчики показа рендером; нули, если рендер на старте кадры камеры не показывал.
    render: CameraStreamDiagnostics,
}

/// Фаза прогона.
enum Phase {
    /// Ждём `Streaming` новой сессии.
    Starting { deadline: Deadline },
    /// Поток идёт, ждём первый показанный кадр камеры.
    AwaitingRender { deadline: Deadline },
    /// Экран уже показывает камеру, ждём, пока сойдётся автоэкспозиция
    /// (`viewfinder_settle_ms`): замер и скриншоты — только по установившемуся потоку.
    Settling { until: Instant },
    /// Идёт замер.
    Measuring(Measuring),
}

/// Состояние замера.
struct Measuring {
    /// Когда замер начался (рендер показал первый кадр; для пробы CPU — когда пошёл поток).
    started: Instant,
    until: Instant,
    next_progress: Instant,
}

/// Стадия для скрипта, собранная на тике.
struct PendingStage {
    stage: &'static str,
    fields: serde_json::Map<String, serde_json::Value>,
}

/// Итог тика: стадии для вывода и признак конца прогона.
struct TickOutcome {
    stages: Vec<PendingStage>,
    finished: bool,
}

/// Состояние прогона.
struct Run {
    kind: RawStreamRunKind,
    config: RawStreamScenarioConfig,
    requested: RequestedMode,
    phase: Phase,
    events: EventLog,
    baselines: Baselines,
    /// Сессия до открытия: «наш» `Streaming` — уже с другой сессией.
    previous_session: Option<SessionGeneration>,
    failure: Option<RunFailure>,
    configured: Option<ConfiguredEntry>,
    degraded: Option<DegradedNote>,
    time_to_streaming_ms: Option<u64>,
    time_to_live_ms: Option<u64>,
    measure_started: Option<Instant>,
    presented_at_start: u64,
    thermal_start: ThermalMoment,
    ownership: MetadataOwnershipTracker,
    latency: LatencySampler,
    /// Последний снимок показа — из него берутся итоговые числа рендера.
    last_presentation: Option<CameraPresentation>,
}

/// Сценарий потока кадров. Один тип на три прогона: различаются длительностью и правилами вердикта.
pub struct CameraRawStreamScenario {
    kind: RawStreamRunKind,
    run: Option<Run>,
}

impl CameraRawStreamScenario {
    pub fn new(kind: RawStreamRunKind) -> Self {
        Self { kind, run: None }
    }
}

/// Имя роли для отчёта.
fn role_name(config: &RawStreamScenarioConfig) -> &'static str {
    use crate::camera_scenario_config::LensRoleSetting;
    match config.role {
        LensRoleSetting::UltraWide => "ultra_wide",
        LensRoleSetting::Wide => "wide",
        LensRoleSetting::Tele => "tele",
        LensRoleSetting::Front => "front",
    }
}

fn readout_name(config: &RawStreamScenarioConfig) -> &'static str {
    use crate::camera_scenario_config::ReadoutSetting;
    match config.readout {
        ReadoutSetting::FourByThree => "four_by_three",
        ReadoutSetting::SixteenByNine => "sixteen_by_nine",
    }
}

/// Команды сценария: чистое состояние, затем открыть, режим и поток.
fn send_commands(running: &RunningCamera, resolved: &ResolvedStep) -> Result<(), RunFailure> {
    let handle = running.handle();
    handle.close().map_err(|rejected| rejected_failure("close", rejected))?;
    handle
        .open(resolved.target.clone())
        .map_err(|rejected| rejected_failure("open", rejected))?;
    handle
        .configure(resolved.mode)
        .map_err(|rejected| rejected_failure("configure", rejected))?;
    handle
        .start_streaming(SCENARIO_CAPTURE_INTENT)
        .map_err(|rejected| rejected_failure("start_streaming", rejected))?;
    Ok(())
}

/// Тепловое состояние сейчас: статус системы и запас до троттлинга.
fn thermal_moment(context: &ScenarioContext<'_>) -> ThermalMoment {
    let thermal = &context.services.thermal;
    let (status, error) = match thermal.current_status() {
        Ok(status) => (Some(status.name()), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let headroom = match thermal.headroom(HeadroomForecast::NOW) {
        ThermalHeadroom::Measured(value) => Some(value),
        ThermalHeadroom::Unavailable => None,
    };
    ThermalMoment {
        status,
        headroom,
        error,
    }
}

/// Миллисекунды между двумя моментами.
fn millis_between(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX)
}

impl Run {
    fn new(
        kind: RawStreamRunKind,
        config: RawStreamScenarioConfig,
        context: &ScenarioContext<'_>,
        previous_session: Option<SessionGeneration>,
    ) -> Self {
        let now = context.now;
        let render = match context.gpu.observe_camera_presentation() {
            CameraPresentationObservation::Live(presentation) => presentation.diagnostics,
            _ => CameraStreamDiagnostics::default(),
        };
        let baselines = Baselines {
            stream: context.camera.raw_stream().counters(),
            reader: context
                .camera
                .running()
                .map(|running| running.diagnostics_snapshot().reader)
                .unwrap_or_else(|_| empty_reader()),
            render,
        };
        Self {
            kind,
            requested: RequestedMode {
                role: role_name(&config),
                raw_format: config.raw_format.name(),
                readout: readout_name(&config),
                frame_rate: config.frame_rate,
            },
            phase: Phase::Starting {
                deadline: Deadline::after_millis(now, config.streaming_timeout_ms),
            },
            events: EventLog::started_at(now),
            baselines,
            previous_session,
            failure: None,
            configured: None,
            degraded: None,
            time_to_streaming_ms: None,
            time_to_live_ms: None,
            measure_started: None,
            presented_at_start: 0,
            thermal_start: thermal_moment(context),
            ownership: MetadataOwnershipTracker::new(config.latency_samples_limit),
            latency: LatencySampler::new(config.latency_samples_limit, config.latency_plausible_max_ms),
            last_presentation: None,
            config,
        }
    }

    /// Один тик: продвигает фазу и сообщает стадии для вывода.
    fn tick(&mut self, context: &ScenarioContext<'_>, running: &RunningCamera) -> TickOutcome {
        let now = context.now;
        let status = running.handle().status();
        let events = running.handle().drain_events(usize::MAX);
        self.events.absorb(now, &events);
        self.note_degradation(&events);
        let mut stages = Vec::new();
        if let Some(failure) = events
            .iter()
            .find_map(failure_of_event)
            .or_else(|| fatal_failure(&status))
        {
            self.failure = Some(failure);
            return TickOutcome { stages, finished: true };
        }
        let finished = match &self.phase {
            Phase::Starting { deadline } => self.tick_starting(context, &status, *deadline, &mut stages),
            Phase::AwaitingRender { deadline } => self.tick_awaiting_render(context, *deadline),
            Phase::Settling { until } => self.tick_settling(context, *until, &mut stages),
            Phase::Measuring(_) => self.tick_measuring(context, &status, &mut stages),
        };
        TickOutcome { stages, finished }
    }

    /// Откат частоты — не отказ, но он обязан попасть в отчёт.
    fn note_degradation(&mut self, events: &[CameraEvent]) {
        for event in events {
            if let CameraEvent::StreamDegraded {
                requested,
                fallback,
                failure,
            } = event
            {
                self.degraded = Some(DegradedNote {
                    requested_fps: requested.frame_rate.frames_per_second(),
                    fallback_fps: fallback.frame_rate.frames_per_second(),
                    failure: failure.to_string(),
                });
            }
        }
    }

    fn tick_starting(
        &mut self,
        context: &ScenarioContext<'_>,
        status: &CameraStatus,
        deadline: Deadline,
        stages: &mut Vec<PendingStage>,
    ) -> bool {
        let now = context.now;
        if let CameraState::Streaming { stream } = &status.state
            && status.stream.session.is_some()
            && status.stream.session != self.previous_session
        {
            let after_ms = self.events.elapsed_ms(now);
            self.configured = Some(ConfiguredEntry::of(stream));
            self.time_to_streaming_ms = Some(after_ms);
            stages.push(PendingStage {
                stage: "streaming",
                fields: stage_fields([
                    ("after_ms", after_ms.into()),
                    ("camera", stream.stream.camera.to_string().into()),
                    ("frame_rate", stream.stream.frame_rate.frames_per_second().into()),
                ]),
            });
            if self.kind == RawStreamRunKind::CpuProbe {
                // Проба CPU смотрит на читатель камеры, экран ей не нужен.
                self.begin_measuring(context, 0, stages);
            } else {
                self.phase = Phase::AwaitingRender {
                    deadline: Deadline::after_millis(now, self.config.render_live_timeout_ms),
                };
            }
            return false;
        }
        if deadline.is_expired(now) {
            self.failure = Some(RunFailure::NeverStreaming {
                last_state: status.state.name(),
            });
            return true;
        }
        false
    }

    fn tick_awaiting_render(&mut self, context: &ScenarioContext<'_>, deadline: Deadline) -> bool {
        let now = context.now;
        let last_seen = match context.gpu.observe_camera_presentation() {
            CameraPresentationObservation::RenderUnavailable { reason } => {
                self.failure = Some(RunFailure::RenderUnavailable { reason });
                return true;
            }
            CameraPresentationObservation::Live(presentation) => {
                let presented_since_start = presentation
                    .diagnostics
                    .camera_frames_presented
                    .saturating_sub(self.baselines.render.camera_frames_presented);
                if presented_since_start > 0 {
                    self.time_to_live_ms = Some(self.events.elapsed_ms(now));
                    self.last_presentation = Some(*presentation);
                    self.phase = Phase::Settling {
                        until: now + Duration::from_millis(self.config.viewfinder_settle_ms),
                    };
                    return false;
                }
                "рендер жив, но кадров камеры ещё не показал"
            }
            CameraPresentationObservation::NotShowingCamera => "рендер не в режиме показа камеры",
        };
        if deadline.is_expired(now) {
            self.failure = Some(RunFailure::RenderNeverShowedCamera { last_seen });
            return true;
        }
        false
    }

    /// Ждёт установления автоэкспозиции после первого показанного кадра, затем начинает замер.
    /// Пока ждём, рендер обязан оставаться в режиме камеры: пропажа рендера — тот же отказ,
    /// что и при ожидании первого кадра.
    fn tick_settling(&mut self, context: &ScenarioContext<'_>, until: Instant, stages: &mut Vec<PendingStage>) -> bool {
        if context.now < until {
            return false;
        }
        match context.gpu.observe_camera_presentation() {
            CameraPresentationObservation::Live(presentation) => {
                let presented_since_start = presentation
                    .diagnostics
                    .camera_frames_presented
                    .saturating_sub(self.baselines.render.camera_frames_presented);
                self.last_presentation = Some(*presentation);
                self.begin_measuring(context, presented_since_start, stages);
                false
            }
            CameraPresentationObservation::RenderUnavailable { reason } => {
                self.failure = Some(RunFailure::RenderUnavailable { reason });
                true
            }
            CameraPresentationObservation::NotShowingCamera => {
                self.failure = Some(RunFailure::RenderNeverShowedCamera {
                    last_seen: "рендер ушёл из режима показа камеры во время установления",
                });
                true
            }
        }
    }

    /// Начинает замер: для обычного прогона вызывается, когда экран показал камеру (стадия
    /// `viewfinder_live` — сигнал скрипту делать скриншоты).
    fn begin_measuring(
        &mut self,
        context: &ScenarioContext<'_>,
        presented_at_start: u64,
        stages: &mut Vec<PendingStage>,
    ) {
        let now = context.now;
        let measure_ms = self.config.measure_ms(self.kind);
        self.measure_started = Some(now);
        self.presented_at_start = presented_at_start;
        if self.kind != RawStreamRunKind::CpuProbe {
            stages.push(PendingStage {
                stage: "viewfinder_live",
                fields: stage_fields([
                    ("after_ms", self.events.elapsed_ms(now).into()),
                    ("measure_ms", measure_ms.into()),
                    ("presented", presented_at_start.into()),
                ]),
            });
        }
        self.phase = Phase::Measuring(Measuring {
            started: now,
            until: now + Duration::from_millis(measure_ms),
            next_progress: now + Duration::from_millis(self.config.progress_interval_ms),
        });
    }

    fn tick_measuring(
        &mut self,
        context: &ScenarioContext<'_>,
        status: &CameraStatus,
        stages: &mut Vec<PendingStage>,
    ) -> bool {
        let now = context.now;
        self.sample_presentation(context);
        let Phase::Measuring(measuring) = &mut self.phase else {
            return false;
        };
        let (until, progress_due) = (measuring.until, now >= measuring.next_progress);
        if progress_due {
            measuring.next_progress = now + Duration::from_millis(self.config.progress_interval_ms);
        }
        let started = measuring.started;
        if progress_due {
            let fields = self.progress_fields(context, status, started);
            stages.push(PendingStage {
                stage: "progress",
                fields,
            });
        }
        let interrupted = !matches!(status.state, CameraState::Streaming { .. });
        now >= until || interrupted
    }

    /// Снимает выборки показа: владение метаданными и задержку. Для пробы CPU рендер не нужен.
    fn sample_presentation(&mut self, context: &ScenarioContext<'_>) {
        if self.kind == RawStreamRunKind::CpuProbe {
            return;
        }
        let CameraPresentationObservation::Live(presentation) = context.gpu.observe_camera_presentation() else {
            return;
        };
        self.ownership.observe(presentation.diagnostics.last_presented_frame);
        match context.camera.raw_stream().sample_clock_offset() {
            Ok(offset) => self.latency.collect(
                &presentation.diagnostics.present_history,
                &presentation.recent_presentations,
                offset,
            ),
            Err(error) => self.latency.note_clock_failure(&error),
        }
        self.last_presentation = Some(*presentation);
    }

    /// Поля стадии `progress`: по ним видно, что длинный прогон жив и куда ползут счётчики.
    fn progress_fields(
        &self,
        context: &ScenarioContext<'_>,
        status: &CameraStatus,
        started: Instant,
    ) -> serde_json::Map<String, serde_json::Value> {
        let elapsed_ms = millis_between(started, context.now);
        let stream = stream_delta(&self.baselines.stream, &context.camera.raw_stream().counters());
        let presented = self.last_presentation.as_ref().map_or(0, |presentation| {
            presentation
                .diagnostics
                .camera_frames_presented
                .saturating_sub(self.baselines.render.camera_frames_presented)
        });
        let thermal = thermal_moment(context);
        stage_fields([
            ("elapsed_ms", elapsed_ms.into()),
            (
                "sensor_fps",
                status
                    .stream
                    .measured_frames_per_second
                    .map_or(serde_json::Value::Null, |fps| ((fps * 100.0).round() / 100.0).into()),
            ),
            ("frames_offered", stream.frames_offered.into()),
            ("frames_presented", presented.into()),
            ("frames_outstanding", stream.frames_outstanding.into()),
            ("max_frames_outstanding", stream.max_frames_outstanding.into()),
            ("leaked_leases", stream.leaked_leases.into()),
            ("dropped_pool_exhausted", stream.dropped_pool_exhausted.into()),
            ("cpu_copies", stream.cpu_copies.into()),
            ("thermal", thermal.status.map_or(serde_json::Value::Null, Into::into)),
            ("headroom", thermal.headroom.map_or(serde_json::Value::Null, Into::into)),
        ])
    }

    /// Собирает наблюдения прогона на момент конца.
    fn into_observation(mut self, context: &ScenarioContext<'_>, running: &RunningCamera) -> RawStreamObservation {
        let status = running.handle().status();
        // Последний взгляд на показ: тик мог закончиться раньше, чем рендер дорисовал.
        self.sample_presentation(context);
        let reader_after = running.diagnostics_snapshot().reader;
        let stream = stream_delta(&self.baselines.stream, &context.camera.raw_stream().counters());
        let reader = reader_delta(&self.baselines.reader, &reader_after);
        let render_delta = self
            .last_presentation
            .as_ref()
            .map(|presentation| RenderCameraDelta::between(&self.baselines.render, &presentation.diagnostics));
        let presented_total = render_delta.map_or(0, |delta| delta.camera_frames_presented);
        let sensor_fps = status.stream.measured_frames_per_second;
        let frame_interval_nanos = sensor_fps.filter(|fps| *fps > 0.0).map(|fps| 1e9 / fps);
        let window_ms = self
            .measure_started
            .map_or(0, |started| millis_between(started, context.now));
        let notes = self.events.notes();
        let kept = notes.len().min(EVENTS_IN_REPORT_LIMIT);
        let pool_plan = context.camera.raw_stream().pool_plan_upper_bound();
        RawStreamObservation {
            kind: self.kind,
            failure: self.failure,
            requested: self.requested,
            configured: self.configured,
            degraded: self.degraded,
            time_to_streaming_ms: self.time_to_streaming_ms,
            time_to_live_ms: self.time_to_live_ms,
            window_ms,
            sensor_fps,
            stats: Some(status.stream.clone()),
            stream,
            reader,
            render: render_delta,
            presented_since_live: presented_total.saturating_sub(self.presented_at_start),
            display_refresh_hz: self
                .last_presentation
                .as_ref()
                .map(|presentation| presentation.display_refresh_hz),
            pool_plan,
            pool_max_images: pool_plan.reader_max_images(),
            validation: self
                .last_presentation
                .as_ref()
                .map(|presentation| presentation.validation.clone()),
            thermal_start: self.thermal_start,
            thermal_end: thermal_moment(context),
            ownership: self.ownership.verdict(frame_interval_nanos),
            latency: self.latency.report(),
            state_at_end: status.state.name(),
            cpu_probe_armed: self.kind == RawStreamRunKind::CpuProbe
                && context.camera.raw_stream().cpu_probe_mode() != camera_android::CpuProbeMode::Disarmed,
            events: notes[..kept].to_vec(),
            events_dropped: notes.len() - kept,
        }
    }
}

fn empty_reader() -> ReaderSnapshot {
    ReaderSnapshot {
        frames_acquired: 0,
        acquire_errors: 0,
        last_acquire_error: None,
        pool_exhausted: 0,
        fence_merge_failures: 0,
        images_returned: 0,
        images_leaked: 0,
        readers_retired: 0,
        readers_deleted: 0,
        readers_leaked: 0,
        cpu_access_probe_copies: 0,
        cpu_access_probe_failures: 0,
        arrival_age: ArrivalAgeSnapshot::default(),
    }
}

impl ObservingScenario for CameraRawStreamScenario {
    fn name(&self) -> &'static str {
        match self.kind {
            RawStreamRunKind::Standard => "raw-stream",
            RawStreamRunKind::Endurance => "raw-stream-endurance",
            RawStreamRunKind::CpuProbe => "raw-stream-cpu-probe",
        }
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let running = match context.camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = context.camera.scenarios().raw_stream.clone();
        let resolved = match config.resolve() {
            Ok(resolved) => resolved,
            Err(error) => {
                return ScenarioStep::Finished(failed(format!("настройки camera.scenarios.raw_stream: {error}")));
            }
        };
        // Заявка на камеру — ДО первой команды: камеру закроет и пробу выключит маршрутизатор при
        // любом конце сценария, даже если следующая строка вернёт ошибку.
        context.camera.claim_camera_for_scenario();
        if self.kind == RawStreamRunKind::CpuProbe {
            // До конфигурации сессии: usage читателя выбирается при его создании.
            context.camera.raw_stream().arm_cpu_probe();
        }
        let previous_session = running.handle().status().stream.session;
        // Хвост событий прошлых сценариев не относится к этому прогону.
        running.handle().drain_events(usize::MAX);
        let run = Run::new(self.kind, config.clone(), context, previous_session);
        if let Err(failure) = send_commands(running, &resolved) {
            return ScenarioStep::Finished(failed(failure.describe()));
        }
        context.emit_stage(
            "started",
            stage_fields([
                ("measure_ms", config.measure_ms(self.kind).into()),
                ("progress_interval_ms", config.progress_interval_ms.into()),
                ("frame_rate", config.frame_rate.into()),
                ("cpu_probe", (self.kind == RawStreamRunKind::CpuProbe).into()),
            ]),
        );
        self.run = Some(run);
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Ok(running) = context.camera.running() else {
            return ScenarioStep::Continue;
        };
        let Some(run) = &mut self.run else {
            return ScenarioStep::Continue;
        };
        let outcome = run.tick(context, running);
        for pending in outcome.stages {
            context.emit_stage(pending.stage, pending.fields);
        }
        if !outcome.finished {
            return ScenarioStep::Continue;
        }
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let config = finished.config.clone();
        let observation = finished.into_observation(context, running);
        // Полные счётчики — строками стадии до результата; в строку результата (4000 байт) идёт сводка.
        emit_details(context, "detail_stream", observation.detail_items());
        let violations = evaluate_raw_stream(&config, &observation);
        ScenarioStep::Finished(judged(
            observation.summary_within(RESULT_SUMMARY_BUDGET_BYTES),
            violations,
        ))
    }
}
