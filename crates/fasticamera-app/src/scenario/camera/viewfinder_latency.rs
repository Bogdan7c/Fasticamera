//! Сценарий `viewfinder-latency` (сессия 07, P13): A/B двух ПРОФИЛЕЙ настроек рендера (политика
//! линии кадра + поздний захват + сход на ближнюю линию + ADPF + подача кадров видоискателя) в чередующихся окнах ABBA (`latency.pairs` пар по
//! `latency.window_ms`). Меряются частота ISP, задержка «сенсор → экран», опоздания vsync, время
//! кадра на GPU и работа ADPF.
//!
//! Ход: общий запуск продуктового видоискателя ([`ViewfinderSession`]), затем окна подряд. В начале
//! окна сценарий ставит профиль окна регулировками рендера (`set_present_target_for_scenario` и
//! `set_adpf_for_scenario`) и политикой подачи потока кадров (`set_viewfinder_pacing_for_scenario`); первые `warmup_ms` не меряются (оценка стоимости кадра перестраивается),
//! дальше копятся задержки и приращения счётчиков. В конце окна проверяется, что поток рендера
//! работал именно с настройками профиля. Штатные настройки рендера и политику подачи кадров по концу сценария
//! возвращает маршрутизатор (`GpuServices::render_scenario_ended`, `CameraServices::scenario_ended`).

use std::time::{Duration, Instant};

use raw_frame_stream::RawStreamCounters;
use render_loop::thread::AdpfDiagnostics;

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details, to_value};
use super::engine_watch::RunFailure;
use super::raw_stream_deltas::stream_delta;
use super::raw_stream_samples::LatencySampler;
use super::result_budget::finish_within_budget;
use super::viewfinder_latency_report::{LatencyObservation, LatencyWindow, ProfileSlot, evaluate_latency};
use super::viewfinder_latency_verdict::compare_profiles;
use super::viewfinder_metrics::{GpuTimeMs, WindowBaseline, WindowDelta, collect_latency, ratio, round2};
use super::viewfinder_pacing_report::PacingWindowReport;
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use super::viewfinder_tuning_report::{AdpfWindowDelta, AdpfWindowReport, adpf_session_name};
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::raw_stream_services::RawStreamServices;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_latency_profile::{LatencyProfileConfig, validate_latency_profiles};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Работают ли поток рендера и поток кадров с настройками профиля (действующий выбор из статуса
/// рендера и политика подачи из потока): линия кадра, поздний захват, сход на ближнюю линию, режим
/// ADPF и подача кадров — всё вместе.
fn selection_matches(
    presentation: &CameraPresentation,
    raw_stream: &RawStreamServices,
    profile: &LatencyProfileConfig,
) -> bool {
    let setting = profile.present_target_setting();
    let selection = &presentation.present_target.selection;
    selection.policy == setting.policy
        && selection.late_latch == setting.late_latch
        && selection.depth_reduction == setting.depth_reduction
        && presentation.adpf.mode == profile.adpf
        && raw_stream.viewfinder_pacing() == profile.viewfinder_pacing
}

/// Счётчики на начало измеряемой части окна (кроме учёта ISP/линии кадра — он в [`WindowBaseline`]).
struct ProfileCounters {
    window: WindowBaseline,
    stream: RawStreamCounters,
    adpf: AdpfDiagnostics,
    /// Видимых повторов кадра на экране (`visible_repeats` рендера: нарисованные и пропущенные нарочно) на начало измеряемой части.
    display_redraws: u64,
}

/// Кадров в секунду по приращению за `window_ms`; `None` — окно пустое.
fn per_second(frames: u64, window_ms: u64) -> Option<f64> {
    ratio(frames as f64 * 1_000.0, window_ms as f64).map(round2)
}

/// Открытое окно.
struct OpenWindow {
    index: usize,
    profile: ProfileSlot,
    measure_from: Instant,
    ends: Instant,
    /// Счётчики на начало измеряемой части и выборка задержки (после разогрева).
    measuring: Option<(ProfileCounters, LatencySampler)>,
}

struct LatencyRun {
    config: ViewfinderScenarioConfig,
    session: ViewfinderSession,
    open: Option<OpenWindow>,
    windows: Vec<LatencyWindow>,
    undelivered: Vec<String>,
}

/// Сценарий `viewfinder-latency`.
#[derive(Default)]
pub struct ViewfinderLatencyScenario {
    run: Option<LatencyRun>,
}

impl ViewfinderLatencyScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

enum RunStep {
    Continue,
    Finish(Option<RunFailure>),
}

impl LatencyRun {
    fn total_windows(&self) -> usize {
        usize::try_from(self.config.latency.pairs).unwrap_or(usize::MAX) * 2
    }

    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        for stage in self.session.take_stages() {
            context.emit_stage(stage.stage, stage.fields);
        }
        match tick {
            SessionTick::Preparing => RunStep::Continue,
            SessionTick::Failed(failure) => RunStep::Finish(Some(failure)),
            SessionTick::BecameLive(_) => {
                self.open_window(context, 0);
                RunStep::Continue
            }
            SessionTick::Live(presentation) => self.advance(context, &presentation),
        }
    }

    /// Профиль роли `slot` из конфига (список проверен при старте: ровно два профиля).
    fn profile(&self, slot: ProfileSlot) -> &LatencyProfileConfig {
        &self.config.latency.profiles[slot.index()]
    }

    /// Открывает окно `index`: ставит настройки его профиля (линия кадра + поздний захват + сход на
    /// ближнюю линию, затем ADPF, затем подача кадров в потоке RAW).
    fn open_window(&mut self, context: &mut ScenarioContext<'_>, index: usize) {
        let now = context.now;
        let slot = ProfileSlot::of_window(index);
        let profile = self.profile(slot).clone();
        let present_target = context
            .gpu
            .set_present_target_for_scenario(profile.present_target_setting());
        let adpf = context.gpu.set_adpf_for_scenario(profile.adpf);
        // Поток кадров отвечает сразу и синхронно (Switched/AlreadyActive — обе нормы), поэтому
        // «недоставленного» тут не бывает; итог проверяет `selection_matches` в конце окна.
        let pacing = context
            .camera
            .raw_stream()
            .set_viewfinder_pacing_for_scenario(profile.viewfinder_pacing);
        tracing::debug!(target: "fasticamera::scenario", window = index, ?pacing, "подача кадров видоискателя");
        let active_pacing = context.camera.raw_stream().viewfinder_pacing();
        for (what, delivery) in [("линия кадра", present_target), ("ADPF", adpf)] {
            if !delivery.is_delivered() {
                self.undelivered
                    .push(format!("окно {index} ({}, {what}): {delivery:?}", profile.name));
            }
        }
        context.emit_stage(
            "window",
            stage_fields([
                ("index", index.into()),
                ("profile", profile.name.as_str().into()),
                ("slot", to_value(&slot)),
                // Политика подачи, действующая в потоке кадров после переключения (не желаемая).
                ("viewfinder_pacing", to_value(&active_pacing)),
            ]),
        );
        self.open = Some(OpenWindow {
            index,
            profile: slot,
            measure_from: now + Duration::from_millis(self.config.latency.warmup_ms),
            ends: now + Duration::from_millis(self.config.latency.window_ms),
            measuring: None,
        });
    }

    fn advance(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        let now = context.now;
        let Some(open) = &mut self.open else {
            return RunStep::Continue;
        };
        match &mut open.measuring {
            None if now >= open.measure_from => {
                let stream = context.camera.raw_stream().counters();
                let baseline = ProfileCounters {
                    window: WindowBaseline::capture(now, presentation, stream),
                    stream,
                    adpf: presentation.adpf.clone(),
                    display_redraws: presentation.diagnostics.visible_repeats(),
                };
                let mut sampler =
                    LatencySampler::new(self.config.latency_samples_limit, self.config.latency_plausible_max_ms);
                // Кадры, поставленные на показ до начала измеряемой части (при прежней политике), не в счёт.
                sampler.skip_already_presented(&presentation.diagnostics.present_history);
                open.measuring = Some((baseline, sampler));
            }
            Some((_, sampler)) => collect_latency(sampler, context.camera.raw_stream(), presentation),
            None => {}
        }
        if now < open.ends {
            return RunStep::Continue;
        }
        let Some(open) = self.open.take() else {
            return RunStep::Continue;
        };
        let window = self.close_window(context, &open, presentation);
        self.windows.push(window);
        if open.index + 1 < self.total_windows() {
            self.open_window(context, open.index + 1);
            RunStep::Continue
        } else {
            RunStep::Finish(None)
        }
    }

    /// Собирает отчёт окна из счётчиков измеряемой части (нет измеряемой части — пустые числа).
    fn close_window(
        &self,
        context: &ScenarioContext<'_>,
        open: &OpenWindow,
        presentation: &CameraPresentation,
    ) -> LatencyWindow {
        let stream_now = context.camera.raw_stream().counters();
        let (delta, latency, stream, adpf, display_repeats) = match &open.measuring {
            Some((counters, sampler)) => (
                WindowDelta::since(&counters.window, context.now, presentation, &stream_now),
                sampler.report(),
                stream_delta(&counters.stream, &stream_now),
                AdpfWindowDelta::between(&counters.adpf, &presentation.adpf),
                presentation
                    .diagnostics
                    .visible_repeats()
                    .saturating_sub(counters.display_redraws),
            ),
            None => (
                WindowDelta::default(),
                LatencySampler::new(1, self.config.latency_plausible_max_ms).report(),
                stream_delta(&stream_now, &stream_now),
                AdpfWindowDelta::default(),
                0,
            ),
        };
        LatencyWindow {
            index: open.index,
            pair: open.index / 2,
            profile: open.profile,
            measured_ms: delta.window_ms,
            processed_fps: delta.isp_frames_per_second().map(round2),
            sensor_fps: per_second(stream.frames_offered, delta.window_ms),
            superseded: stream.dropped_superseded,
            superseded_per_s: per_second(stream.dropped_superseded, delta.window_ms),
            pacing: PacingWindowReport::of_window(&stream, display_repeats, delta.window_ms),
            latency,
            present_target: delta.present_target,
            adpf: AdpfWindowReport {
                mode: presentation.adpf.mode,
                session: adpf_session_name(&presentation.adpf.session),
                delta: adpf,
            },
            frame_gpu_time: GpuTimeMs::of_optional(presentation.present_target.frame_gpu_time.as_ref()),
            selection_matched: selection_matches(presentation, context.camera.raw_stream(), self.profile(open.profile)),
        }
    }

    fn into_observation(self, running: &RunningCamera, failure: Option<RunFailure>) -> LatencyObservation {
        let comparison = compare_profiles(&self.config.latency, &self.windows);
        LatencyObservation {
            failure,
            session: self.session.summary(running, EVENTS_IN_REPORT_LIMIT),
            profiles: self.config.latency.profiles.clone(),
            expected_windows: self.total_windows(),
            windows: self.windows,
            undelivered: self.undelivered,
            comparison,
        }
    }
}

impl ObservingScenario for ViewfinderLatencyScenario {
    fn name(&self) -> &'static str {
        "viewfinder-latency"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = camera.scenarios().viewfinder.clone();
        let step = match config.product_step() {
            Ok(step) => step,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder: {error}"))),
        };
        if let Err(error) = validate_latency_profiles(&config.latency.profiles) {
            return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder.latency: {error}")));
        }
        let session = match ViewfinderSession::start(context, running, &step, SessionTiming::of(&config)) {
            Ok(session) => session,
            Err(failure) => return ScenarioStep::Finished(failed(failure.describe())),
        };
        context.emit_stage(
            "started",
            stage_fields([
                ("window_ms", config.latency.window_ms.into()),
                ("pairs", config.latency.pairs.into()),
                ("baseline", config.latency.profiles[0].name.as_str().into()),
                ("tuned", config.latency.profiles[1].name.as_str().into()),
            ]),
        );
        self.run = Some(LatencyRun {
            config,
            session,
            open: None,
            windows: Vec::new(),
            undelivered: Vec::new(),
        });
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let (Ok(running), Some(run)) = (camera.running(), self.run.as_mut()) else {
            return ScenarioStep::Continue;
        };
        let RunStep::Finish(failure) = run.tick(context, running) else {
            return ScenarioStep::Continue;
        };
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let latency_config = finished.config.latency.clone();
        let observation = finished.into_observation(running, failure);
        emit_details(context, "detail_latency", observation.detail_items());
        let violations = evaluate_latency(&latency_config, &observation);
        ScenarioStep::Finished(finish_within_budget(context, observation.summary(), violations))
    }
}
