//! Сценарий `gpu-saturation-bench` (сессия 16a, P4b): цена работ GPU при насыщении на телефоне.
//!
//! На Adreno 740 миллисекунды GPU в темпе vsync — не цена работы (DVFS растягивает). Честная цена — пропускная
//! способность при подряд идущих кадрах без показа. Сценарий прогоняет набор работ каталога
//! (`saturation_bench::catalog`) одну за другой и отдаёт таблицу «работа → к/с → мс/кадр → проходы → шкала → тепло».
//! Продуктового поведения не меняет.
//!
//! # Тишина продукта на время замера
//! Иначе видоискатель делит GPU со стендом, и цифра не про работу.
//! * экран камеры снят (политика по умолчанию `SuspendForScenario`): UI и приборы не рисуются;
//! * камера закрыта: сценарий заявляет её (`claim_camera_for_scenario`) и шлёт `close`, ждёт `Closed`; по концу
//!   сценария маршрутизатор закрывает заявленную камеру и возвращает видоискатель продукта на штатное место —
//!   отдельного рычага в `render-loop`/`camera-core` для этого не заводилось;
//! * доказательство в результате: счётчики потока рендера (`frames_presented`, кадры ISP, перекомпоновки, кадры UI)
//!   до и после замера — `render.delta`; прирост продуктовой работы GPU сверх допуска конфига — провал.
//!
//! # Поток
//! Прогон блокирующий и долгий: он идёт в фоновом потоке ([`BackgroundJob`]) на СВОЁМ контексте GPU
//! (`GpuServices::saturation`), не мешая потоку маршрутизатора. Отмена — [`CancelToken`]: уход приложения в фон,
//! срок прогона, конец сценария (`Drop`). События раннера (начало и конец каждой работы) приходят каналом и
//! пишутся стадиями из `on_tick`: оркестратор по ним совмещает журнал тепла с ПК по времени.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use camera_core::CameraState;
use camera_core::status::CameraStatus;
use gpu_vulkan::saturation::CancelToken;
use render_loop::thread::RenderStatus;
use serde_json::Value;
use shell_contract::event::ShellEventKind;
use shell_contract::thermal::ThermalStatus;

use super::background_job::{BackgroundJob, JobPoll};
use super::camera::detail::emit_details;
use super::camera::result_budget::finish_within_budget;
use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields};
use crate::camera_screen_service::CameraScreenPolicy;
use crate::gpu_services::{RenderObservation, ValidationState};
use crate::power_measurement::ThermalSnapshot;
use crate::saturation_bench::catalog::CatalogEntry;
use crate::saturation_bench::catalog_wait::{CatalogPoll, CatalogWait, catalog_published};
use crate::saturation_bench::inputs::{InputSources, LookSummary, ProductInputs, SensorSource, gather_inputs};
use crate::saturation_bench::report::{RenderActivity, RenderSilence, RunSummary, violations};
use crate::saturation_bench::runner::{BenchRunner, PauseOutcome, RunClock, RunEvent, ThermalProbe};
use crate::saturation_bench::screen_scene::SurfaceSizePx;
use crate::saturation_bench::services::ExecutorSetup;
use crate::saturation_bench::summary::{CameraSilence, SummaryHeader, build_summary, detail_item};
use crate::saturation_scenario_config::SaturationScenarioConfig;

/// Шаг проверки отмены внутри паузы между работами.
const PAUSE_SLICE: Duration = Duration::from_millis(50);

/// Настоящие часы прогона: `Instant` и пауза кусками с проверкой отмены.
struct WallClock {
    started: Instant,
}

impl RunClock for WallClock {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn pause(&self, duration: Duration, cancel: &CancelToken) -> PauseOutcome {
        let end = Instant::now() + duration;
        loop {
            if cancel.is_cancelled() {
                return PauseOutcome::Cancelled;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return PauseOutcome::Elapsed;
            }
            std::thread::sleep(left.min(PAUSE_SLICE));
        }
    }
}

/// Тепло из сервиса оболочки.
struct ServiceThermal(Arc<dyn shell_contract::thermal::ThermalSource>);

impl ThermalProbe for ServiceThermal {
    fn snapshot(&self) -> ThermalSnapshot {
        ThermalSnapshot::read(self.0.as_ref())
    }
}

/// Что в сценарии выполняется сейчас.
enum Phase {
    /// Ждём, пока движок камеры опубликует каталог камер: без него сенсор для стенда был бы запасным.
    AwaitingCatalog { wait: CatalogWait },
    /// `close` отправлена, ждём `Closed`.
    AwaitingCameraClosed { deadline: Instant },
    /// Камера тиха; даём потокам доиграть хвост и берём снимок рендера «до».
    Settling { until: Instant },
    /// Прогон идёт в фоновом потоке.
    Running(RunningBench),
    /// Итог выдан.
    Done,
}

struct RunningBench {
    job: BackgroundJob<Result<RunSummary, String>>,
    events: Receiver<RunEvent>,
    render_before: Option<RenderActivity>,
    paused_by_app: bool,
}

/// Сценарий `gpu-saturation-bench`.
pub struct GpuSaturationScenario {
    phase: Phase,
    cancel: CancelToken,
    config: SaturationScenarioConfig,
    entries: Vec<&'static CatalogEntry>,
    inputs: Option<ProductInputs>,
    camera: CameraSilence,
    /// Откуда сенсор и какой вид действовали (для сводки; сами входы уходят в поток).
    header_sensor: Option<SensorSource>,
    header_look: Option<LookSummary>,
}

impl GpuSaturationScenario {
    pub fn new() -> Self {
        Self {
            phase: Phase::Done,
            cancel: CancelToken::new(),
            config: SaturationScenarioConfig::default(),
            entries: Vec::new(),
            inputs: None,
            camera: CameraSilence::EngineUnavailable,
            header_sensor: None,
            header_look: None,
        }
    }

    /// Закрывает камеру (если движок есть) и назначает фазу ожидания.
    fn silence_camera(&mut self, context: &mut ScenarioContext<'_>) -> Result<(), String> {
        let timeout = Duration::from_millis(self.config.camera_close_timeout_ms);
        let Ok(running) = context.camera.running() else {
            self.camera = CameraSilence::EngineUnavailable;
            self.phase = Phase::Settling {
                until: context.now + Duration::from_millis(self.config.quiet_settle_ms),
            };
            return Ok(());
        };
        // Заявка — ДО команды: камеру по концу сценария закроет и вернёт на место маршрутизатор при любом исходе.
        context.camera.claim_camera_for_scenario();
        running
            .handle()
            .close()
            .map_err(|rejected| format!("команда закрытия камеры не принята движком: {rejected}"))?;
        self.camera = CameraSilence::ClosedByScenario;
        self.phase = Phase::AwaitingCameraClosed {
            deadline: context.now + timeout,
        };
        Ok(())
    }

    /// Состояние камеры тихое: закрыта либо в ошибке (не стримит).
    fn camera_is_quiet(context: &ScenarioContext<'_>) -> bool {
        match context.camera.running() {
            Ok(running) => matches!(
                running.handle().status().state,
                CameraState::Closed { .. } | CameraState::Error { .. }
            ),
            Err(_) => true,
        }
    }

    /// Снимок статуса движка (`None` — движка нет).
    fn engine_status(context: &ScenarioContext<'_>) -> Option<CameraStatus> {
        context.camera.running().ok().map(|running| running.handle().status())
    }

    /// Собирает входы стенда по снимку статуса движка, закрывает камеру и объявляет старт.
    fn begin(&mut self, context: &mut ScenarioContext<'_>, engine_status: Option<CameraStatus>) -> ScenarioStep {
        let inputs = gather_inputs(&InputSources {
            engine_status: engine_status.as_ref(),
            lens_shading: &context.gpu.saturation.lens_shading,
            render_config: &context.gpu.render_config,
            look: context.camera_screen.look_handle(),
            screen: context.camera_screen.config(),
        })
        // Образцы вида для работ цены и паритета вида (`look_cost`, `look_parity`): общий конфиг замеров.
        .with_measurement_look(context.camera.scenarios().measurement_look.clone());
        self.header_sensor = Some(inputs.sensor_source.clone());
        self.header_look = Some(inputs.look.clone());
        self.inputs = Some(inputs);
        if let Err(reason) = self.silence_camera(context) {
            return ScenarioStep::Finished(failed(reason));
        }
        context.emit_stage(
            "started",
            stage_fields([
                ("workloads", self.entries.len().into()),
                ("camera", serde_json::to_value(self.camera).unwrap_or(Value::Null)),
            ]),
        );
        ScenarioStep::Continue
    }

    fn launch(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Some(inputs) = self.inputs.take() else {
            return ScenarioStep::Finished(failed("входы стенда потеряны до запуска (внутренняя ошибка сценария)"));
        };
        let bench = match self.config.bench_config() {
            Ok(bench) => bench,
            Err(error) => return ScenarioStep::Finished(failed(error.to_string())),
        };
        let render_before = observe_render(context).activity();
        let (sender, events) = channel();
        let entries = self.entries.clone();
        let cooldown = Duration::from_millis(self.config.cooldown_between_ms);
        let gpu = Arc::clone(&context.gpu.saturation.gpu);
        let thermal = Arc::clone(&context.services.thermal);
        let cancel = self.cancel.clone();
        let deadline = self.config.job_deadline(entries.len());
        let spawned = BackgroundJob::spawn("fasticamera-saturation", deadline, move || {
            run_on_gpu(&*gpu, inputs, bench, &entries, cooldown, thermal, sender, &cancel)
        });
        match spawned {
            Ok(job) => {
                context.emit_stage(
                    "bench_started",
                    stage_fields([
                        ("workloads", self.entries.len().into()),
                        (
                            "deadline_ms",
                            u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX).into(),
                        ),
                        ("camera", serde_json::to_value(self.camera).unwrap_or(Value::Null)),
                    ]),
                );
                self.phase = Phase::Running(RunningBench {
                    job,
                    events,
                    render_before,
                    paused_by_app: false,
                });
                ScenarioStep::Continue
            }
            Err(reason) => ScenarioStep::Finished(failed(reason)),
        }
    }

    /// Пишет стадии по событиям раннера, накопившимся с прошлого тика.
    fn emit_run_events(running: &RunningBench, context: &mut ScenarioContext<'_>) {
        loop {
            match running.events.try_recv() {
                Ok(RunEvent::WorkloadStarted {
                    index,
                    total,
                    name,
                    offset,
                    thermal,
                }) => context.emit_stage(
                    "workload_started",
                    stage_fields([
                        ("name", name.into()),
                        ("index", index.into()),
                        ("total", total.into()),
                        ("offset_ms", millis(offset).into()),
                        (
                            "thermal_status",
                            thermal.status.map_or("unknown", ThermalStatus::name).into(),
                        ),
                        ("thermal_headroom", thermal.headroom.map_or(Value::Null, Value::from)),
                    ]),
                ),
                Ok(RunEvent::WorkloadFinished {
                    index,
                    total,
                    name,
                    offset,
                    verdict_word,
                }) => context.emit_stage(
                    "workload_finished",
                    stage_fields([
                        ("name", name.into()),
                        ("index", index.into()),
                        ("total", total.into()),
                        ("offset_ms", millis(offset).into()),
                        ("verdict", verdict_word.into()),
                    ]),
                ),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    fn finish(&mut self, running: RunningBench, run: RunSummary, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let silence = render_silence(running.render_before, &observe_render(context));
        let tolerance = self.config.render_gpu_work_tolerance_frames;
        let mut found = violations(&run, &silence, tolerance);
        if running.paused_by_app {
            found.push("приложение ушло в фон во время замера: прогон отменён".to_owned());
        }
        let header = SummaryHeader {
            validation: validation_of(context),
            sensor: self.header_sensor.clone().unwrap_or(SensorSource::Fallback {
                reason: "входы не сохранены".to_owned(),
            }),
            look: self.header_look.clone().unwrap_or(LookSummary {
                preset: "unknown".to_owned(),
                tone_base: look_contract::ToneBaseModel::Agx,
            }),
            camera: self.camera,
            requested: self.entries.iter().map(|entry| entry.name).collect(),
            warmup_ms: self.config.warmup_ms,
            measure_ms: self.config.measure_ms,
            frames_in_flight: self.config.frames_in_flight,
            render_tolerance_frames: tolerance,
        };
        emit_details(context, "detail_rows", run.rows.iter().map(detail_item).collect());
        let summary = build_summary(&header, &run, &silence);
        self.phase = Phase::Done;
        ScenarioStep::Finished(finish_within_budget(context, summary, found))
    }
}

impl Default for GpuSaturationScenario {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GpuSaturationScenario {
    /// Сценарий снят (вытеснен, конец): работа в фоновом потоке не должна продолжать насыщать GPU.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Прогон в фоновом потоке: контекст GPU, исполнитель, раннер.
#[allow(
    clippy::too_many_arguments,
    reason = "всё это переезжает в поток как есть, своей структуры не требует"
)]
fn run_on_gpu(
    gpu: &dyn crate::saturation_bench::services::SaturationGpu,
    inputs: ProductInputs,
    bench: gpu_vulkan::saturation::SaturationConfig,
    entries: &[&'static CatalogEntry],
    cooldown: Duration,
    thermal: Arc<dyn shell_contract::thermal::ThermalSource>,
    events: Sender<RunEvent>,
    cancel: &CancelToken,
) -> Result<RunSummary, String> {
    let mut executor = gpu.executor(ExecutorSetup {
        inputs,
        bench,
        surface: SurfaceSizePx::ONEPLUS_11,
    })?;
    let clock = WallClock {
        started: Instant::now(),
    };
    let thermal = ServiceThermal(thermal);
    let mut observer = move |event: RunEvent| {
        // Получатель исчез (сценарий снят) — прогон всё равно доводится до отмены токеном.
        if events.send(event).is_err() {
            tracing::debug!(target: "fasticamera::saturation", "наблюдатель прогона ушёл");
        }
    };
    let mut runner = BenchRunner {
        executor: executor.as_mut(),
        clock: &clock,
        thermal: &thermal,
        observer: &mut observer,
        cooldown_between: cooldown,
    };
    Ok(runner.run(entries, cancel))
}

/// Что видно у рендера сейчас.
enum RenderView {
    Live(RenderActivity),
    Unavailable(String),
}

impl RenderView {
    fn activity(&self) -> Option<RenderActivity> {
        match self {
            Self::Live(activity) => Some(*activity),
            Self::Unavailable(_) => None,
        }
    }
}

fn observe_render(context: &ScenarioContext<'_>) -> RenderView {
    match context.gpu.render.observe() {
        RenderObservation::Live { status, .. } => RenderView::Live(activity_of(&status)),
        RenderObservation::Unavailable { reason } => RenderView::Unavailable(reason),
    }
}

/// Счётчики потока рендера из существующей диагностики (`RenderStatus`).
fn activity_of(status: &RenderStatus) -> RenderActivity {
    let viewfinder = status.backend.viewfinder.as_ref();
    RenderActivity {
        frames_presented: status.frames_presented,
        isp_frames: viewfinder.map_or(0, |view| view.counters.frames_processed),
        recomposed_frames: viewfinder.map_or(0, |view| view.counters.recomposed),
        ui_frames_drawn: viewfinder.map_or(0, |view| view.ui_overlay.frames_drawn),
    }
}

fn validation_of(context: &ScenarioContext<'_>) -> ValidationState {
    context.gpu.saturation.gpu.validation()
}

/// Итог наблюдения за рендером: «нет рендера» и «рендер молчал» — разные ответы.
fn render_silence(before: Option<RenderActivity>, after: &RenderView) -> RenderSilence {
    match (before, after) {
        (Some(before), RenderView::Live(after)) => RenderSilence::Observed {
            before,
            after: *after,
            delta: after.since(&before),
        },
        (_, RenderView::Unavailable(reason)) => RenderSilence::RenderUnavailable { reason: reason.clone() },
        (None, RenderView::Live(_)) => RenderSilence::RenderUnavailable {
            reason: "снимок рендера «до» не получен: рендер появился во время замера".to_owned(),
        },
    }
}

impl ObservingScenario for GpuSaturationScenario {
    fn name(&self) -> &'static str {
        "gpu-saturation-bench"
    }

    /// Экран снимается на время замера (умолчание), но объявляем это явно: от этого зависит честность цифры.
    fn camera_screen_policy(&self) -> CameraScreenPolicy {
        CameraScreenPolicy::SuspendForScenario
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.config = context.camera.scenarios().saturation.clone();
        self.entries = match self.config.validate().and_then(|()| self.config.resolve_workloads()) {
            Ok(entries) => entries,
            Err(error) => return ScenarioStep::Finished(failed(format!("конфиг стенда насыщения: {error}"))),
        };
        // Входы собираются ДО закрытия камеры: каталог и факты экрана берутся у живого движка. Каталог движок
        // публикует чуть позже старта: пока его нет, ждём (иначе сенсор был бы запасным, а не настоящим).
        let engine_status = Self::engine_status(context);
        if let Some(status) = &engine_status
            && !catalog_published(status)
        {
            let limit = Duration::from_millis(self.config.catalog_wait_ms);
            self.phase = Phase::AwaitingCatalog {
                wait: CatalogWait::starting_at(context.now, limit),
            };
            return ScenarioStep::Continue;
        }
        self.begin(context, engine_status)
    }

    fn on_event(&mut self, event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if event.kind != ShellEventKind::Paused {
            return ScenarioStep::Continue;
        }
        // Уход в фон: насыщать GPU приложением, которого не видно, незачем. До замера — отказ, во время — отмена
        // (итог досчитается частичным: работа вернёт то, что успела).
        self.cancel.cancel();
        match &mut self.phase {
            Phase::Running(running) => {
                running.paused_by_app = true;
                ScenarioStep::Continue
            }
            Phase::AwaitingCatalog { .. } | Phase::AwaitingCameraClosed { .. } | Phase::Settling { .. } => {
                self.phase = Phase::Done;
                ScenarioStep::Finished(failed("приложение ушло в фон до начала замера: прогон не начинался"))
            }
            Phase::Done => ScenarioStep::Continue,
        }
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        match &mut self.phase {
            Phase::AwaitingCatalog { wait } => {
                let wait = *wait;
                let Some(status) = Self::engine_status(context) else {
                    // Движок пропал, пока ждали: идём дальше без него (сенсор запасной, причина в результате).
                    return self.begin(context, None);
                };
                match wait.poll(status, context.now) {
                    CatalogPoll::Waiting => ScenarioStep::Continue,
                    CatalogPoll::Ready(status) | CatalogPoll::TimedOut(status) => self.begin(context, Some(status)),
                }
            }
            Phase::AwaitingCameraClosed { deadline } => {
                let deadline = *deadline;
                if Self::camera_is_quiet(context) {
                    self.phase = Phase::Settling {
                        until: context.now + Duration::from_millis(self.config.quiet_settle_ms),
                    };
                } else if context.now >= deadline {
                    self.phase = Phase::Done;
                    return ScenarioStep::Finished(failed(format!(
                        "камера не закрылась за {} мс: видоискатель продукта делил бы GPU со стендом",
                        self.config.camera_close_timeout_ms
                    )));
                }
                ScenarioStep::Continue
            }
            Phase::Settling { until } => {
                if context.now >= *until {
                    return self.launch(context);
                }
                ScenarioStep::Continue
            }
            Phase::Running(running) => {
                Self::emit_run_events(running, context);
                match running.job.poll() {
                    JobPoll::Pending => ScenarioStep::Continue,
                    JobPoll::Done(Ok(run)) => {
                        let Phase::Running(running) = std::mem::replace(&mut self.phase, Phase::Done) else {
                            return ScenarioStep::Continue;
                        };
                        Self::emit_run_events(&running, context);
                        self.finish(running, run, context)
                    }
                    JobPoll::Done(Err(reason)) => {
                        self.phase = Phase::Done;
                        ScenarioStep::Finished(failed(format!("стенд не запущен: {reason}")))
                    }
                    JobPoll::TimedOut { after } => {
                        self.cancel.cancel();
                        self.phase = Phase::Done;
                        ScenarioStep::Finished(failed(format!(
                            "прогон не закончился за {} мс и отменён",
                            after.as_millis()
                        )))
                    }
                    JobPoll::Lost(reason) => {
                        self.phase = Phase::Done;
                        ScenarioStep::Finished(failed(reason))
                    }
                }
            }
            Phase::Done => ScenarioStep::Continue,
        }
    }
}

#[cfg(test)]
#[path = "gpu_saturation_tests.rs"]
mod tests;
