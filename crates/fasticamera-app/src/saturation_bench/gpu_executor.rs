//! Исполнитель работ стенда насыщения на настоящем GPU.
//!
//! На каждую работу каталога: снять базу живых объектов Vulkan → создать работу → проверить число слотов →
//! прогнать [`SaturationBench`] → забрать результаты (времена проходов, первая ошибка работы, счётчики UI) →
//! дождаться GPU и ПОЛНОСТЬЮ освободить работу → сверить живые объекты с базой. Следующая работа создаётся только
//! после возврата этой (V4 ≈ 360 МиБ, снимок ≈ 400 МиБ: две сразу на телефоне не нужны и не честны).
//! Ошибка любого шага — типизированный провал строки ([`WorkloadFailure`]), остальные работы идут дальше.
//!
//! Исполнитель принимает `&GpuContext` и не создаёт его: на телефоне это долгоживущий контекст диагностик,
//! в тестах — контекст теста.

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::saturation::{
    CancelToken, PassTimeSummary, SaturationBench, SaturationConfig, SaturationError, SaturationReport,
    SaturationWorkload, WorkloadDigest,
};
use isp_vulkan::saturation::{
    DEFAULT_GRAY_CODE, HeadroomSaturation, HeadroomSaturationConfig, SaturationWorkError, SceneContent,
    ScopesSaturation, ScopesSaturationConfig, StillSaturation, StillSaturationConfig, ViewfinderSaturation,
    ViewfinderSaturationConfig,
};
use scope_contract::{ScopeKind, ScopeRequest, VectorscopeAxes};
use ui_draw::draw_list::LiveImage;
use ui_render_vulkan::LiveImageInput;
use ui_render_vulkan::saturation::{
    BackdropCadence, FrameComposition, SaturationScene, UiFrameSaturation, UiFrameSaturationConfig,
};

use super::backdrop::{BackdropFailures, IspBackdrop};
use super::catalog::{
    BackdropRhythm, CatalogEntry, DisplayScreen, ScopeInputKind, ScopeShape, ViewfinderVariant, WorkloadKind,
};
use super::digest_report::DigestOutcome;
use super::draw_filter::DrawFilter;
use super::inputs::{PipelineKind, ProductInputs};
use super::report::{FailureKind, Measurement, SceneSummary, UiCountersSummary, WorkloadFailure};
use super::runner::{ExecutedWorkload, WorkloadExecutor};
use super::screen_scene::{ScreenPlan, ScreenPlanError, SurfaceSizePx, summarize};
use super::viewfinder_variant;

/// Зерно «натурной» сцены (один и тот же кадр от прогона к прогону).
const NATURAL_SCENE_SEED: u32 = 0x5EED_16A0;
/// Окно короткого прогона, которым приборы один раз считают картинку для панели экрана калибровки.
const SCOPE_PICTURE_BENCH_MEASURE: std::time::Duration = std::time::Duration::from_millis(50);

/// Работа, собранная для стенда, со всем, что нужно забрать после прогона.
trait BenchWork {
    /// Работа как цель стенда.
    fn workload(&mut self) -> &mut dyn SaturationWorkload;
    /// Сколько слотов у работы (обязано равняться `frames_in_flight` стенда).
    fn slots(&self) -> usize;
    fn pass_times(&self) -> PassTimeSummary;
    /// Первая ошибка, случившаяся внутри работы за прогон.
    fn take_failure(&mut self) -> Option<String>;
    /// Дозавершение после прогона (проявки снимка и т. п.).
    fn finish(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn scene(&self) -> Option<SceneSummary> {
        None
    }
    fn ui_counters(&self) -> Option<UiCountersSummary> {
        None
    }
    /// Дайджест выхода последнего кадра. Звать ПОСЛЕ прогона, `finish()` и `wait_idle`, до освобождения работы.
    /// Отказ чтения — текст причины (строка замера из-за него не срывается).
    fn output_digest(&mut self) -> Result<WorkloadDigest, String>;
    /// Факт о прочитанном кадре для результата (см. `DigestOutcome::Computed::frame_note`).
    fn digest_note(&self) -> Option<String> {
        None
    }
}

fn failure_text(error: Option<SaturationWorkError>) -> Option<String> {
    error.map(|error| error.to_string())
}

struct ViewfinderWork(ViewfinderSaturation);

impl BenchWork for ViewfinderWork {
    fn workload(&mut self) -> &mut dyn SaturationWorkload {
        &mut self.0
    }
    fn slots(&self) -> usize {
        self.0.slots()
    }
    fn pass_times(&self) -> PassTimeSummary {
        self.0.pass_times()
    }
    fn take_failure(&mut self) -> Option<String> {
        failure_text(self.0.take_failure())
    }
    fn output_digest(&mut self) -> Result<WorkloadDigest, String> {
        self.0.output_digest().map_err(|error| error.to_string())
    }
    fn digest_note(&self) -> Option<String> {
        self.0.counters().last_frame_had_statistics.map(|had| {
            if had {
                "last_frame=awb_statistics".to_owned()
            } else {
                "last_frame=no_awb_statistics".to_owned()
            }
        })
    }
}

struct ScopesWork(ScopesSaturation);

impl BenchWork for ScopesWork {
    fn workload(&mut self) -> &mut dyn SaturationWorkload {
        &mut self.0
    }
    fn slots(&self) -> usize {
        self.0.slots()
    }
    fn pass_times(&self) -> PassTimeSummary {
        self.0.pass_times()
    }
    fn take_failure(&mut self) -> Option<String> {
        failure_text(self.0.take_failure())
    }
    fn output_digest(&mut self) -> Result<WorkloadDigest, String> {
        self.0.output_digest().map_err(|error| error.to_string())
    }
}

struct HeadroomWork(HeadroomSaturation);

impl BenchWork for HeadroomWork {
    fn workload(&mut self) -> &mut dyn SaturationWorkload {
        &mut self.0
    }
    fn slots(&self) -> usize {
        self.0.slots()
    }
    fn pass_times(&self) -> PassTimeSummary {
        self.0.pass_times()
    }
    fn take_failure(&mut self) -> Option<String> {
        failure_text(self.0.take_failure())
    }
    fn output_digest(&mut self) -> Result<WorkloadDigest, String> {
        self.0.output_digest().map_err(|error| error.to_string())
    }
}

struct StillWork(StillSaturation);

impl BenchWork for StillWork {
    fn workload(&mut self) -> &mut dyn SaturationWorkload {
        &mut self.0
    }
    fn slots(&self) -> usize {
        self.0.slots()
    }
    fn pass_times(&self) -> PassTimeSummary {
        self.0.pass_times()
    }
    fn take_failure(&mut self) -> Option<String> {
        failure_text(self.0.take_failure())
    }
    fn finish(&mut self) -> Result<(), String> {
        self.0.finish_pending_blocking().map_err(|error| error.to_string())
    }
    fn output_digest(&mut self) -> Result<WorkloadDigest, String> {
        self.0.output_digest().map_err(|error| error.to_string())
    }
}

/// Кадр показа: адаптер UI + всё, что должно жить, пока GPU читает его сцену.
struct DisplayWork {
    /// Первым в списке полей: уничтожается раньше картинки приборов (порядок полей = порядок уничтожения).
    ui: UiFrameSaturation,
    /// Приборы, чья картинка лежит в таблице живых картинок сцены (только CALIBRATE).
    _scopes: Option<ScopesSaturation>,
    backdrop_failures: BackdropFailures,
    scene: SceneSummary,
}

impl BenchWork for DisplayWork {
    fn workload(&mut self) -> &mut dyn SaturationWorkload {
        &mut self.ui
    }
    fn slots(&self) -> usize {
        self.ui.config().frames_in_flight.get()
    }
    fn pass_times(&self) -> PassTimeSummary {
        self.ui.pass_times()
    }
    fn take_failure(&mut self) -> Option<String> {
        if self.backdrop_failures.count() > 0 {
            return Some(format!(
                "фон кадра не опубликован {} раз",
                self.backdrop_failures.count()
            ));
        }
        (self.ui.release_failures() > 0)
            .then(|| format!("освобождение слотов UI: {} ошибок", self.ui.release_failures()))
    }
    fn output_digest(&mut self) -> Result<WorkloadDigest, String> {
        self.ui.output_digest().map_err(|error| error.to_string())
    }
    fn scene(&self) -> Option<SceneSummary> {
        Some(self.scene)
    }
    fn ui_counters(&self) -> Option<UiCountersSummary> {
        let counters = self.ui.renderer_counters();
        Some(UiCountersSummary {
            frames_recorded: counters.frames_recorded,
            glass_groups: counters.glass_groups,
            shape_instances: counters.shape_instances,
            glyph_instances: counters.glyph_instances,
            images_drawn: counters.images_drawn,
            live_images_drawn: counters.live_images_drawn,
            live_images_missing: counters.live_images_missing,
            pyramid_rebuilds: counters.pyramid_rebuilds,
            pyramid_reuses: counters.pyramid_reuses,
        })
    }
}

/// Как собрать работу не удалось.
struct BuildError(WorkloadFailure);

impl From<SaturationWorkError> for BuildError {
    fn from(error: SaturationWorkError) -> Self {
        Self(WorkloadFailure::new(FailureKind::BuildFailed, error.to_string()))
    }
}

impl From<ScreenPlanError> for BuildError {
    fn from(error: ScreenPlanError) -> Self {
        Self(WorkloadFailure::new(FailureKind::BuildFailed, error.to_string()))
    }
}

impl From<ui_render_vulkan::saturation::UiSaturationError> for BuildError {
    fn from(error: ui_render_vulkan::saturation::UiSaturationError) -> Self {
        Self(WorkloadFailure::new(FailureKind::BuildFailed, error.to_string()))
    }
}

/// Исполнитель на настоящем GPU.
pub struct GpuWorkloadExecutor {
    context: GpuContext,
    inputs: ProductInputs,
    bench: SaturationConfig,
    surface: SurfaceSizePx,
    /// Списки рисования экранов: строятся один раз при первой работе показа (или приборов).
    plan: Option<Result<ScreenPlan, ScreenPlanError>>,
}

impl GpuWorkloadExecutor {
    pub fn new(context: GpuContext, inputs: ProductInputs, bench: SaturationConfig, surface: SurfaceSizePx) -> Self {
        Self {
            context,
            inputs,
            bench,
            surface,
            plan: None,
        }
    }

    fn slots(&self) -> usize {
        self.bench.frames_in_flight.get()
    }

    /// Списки рисования экранов (строятся один раз).
    fn plan(&mut self) -> Result<&ScreenPlan, ScreenPlanError> {
        if self.plan.is_none() {
            self.plan = Some(ScreenPlan::build(&self.inputs, self.surface));
        }
        match self.plan.as_ref() {
            Some(Ok(plan)) => Ok(plan),
            Some(Err(error)) => Err(error.clone()),
            None => unreachable!("план записан строкой выше"),
        }
    }

    fn build(&mut self, entry: &CatalogEntry) -> Result<Box<dyn BenchWork>, BuildError> {
        match entry.kind {
            WorkloadKind::Viewfinder { variant } => self.build_viewfinder(variant),
            WorkloadKind::Scopes { shape, input } => self.build_scopes(shape, input),
            WorkloadKind::Compositor => self.build_display(
                FrameComposition::CompositorOnly,
                DisplayScreen::Photo,
                BackdropRhythm::NewFrameEveryFrame,
                DrawFilter::All,
            ),
            WorkloadKind::Display { screen, rhythm, filter } => {
                self.build_display(FrameComposition::CompositorThenUi, screen, rhythm, filter)
            }
            WorkloadKind::HeadroomV4 => self.build_headroom(),
            WorkloadKind::Still => self.build_still(),
        }
    }

    fn viewfinder_config(&self) -> ViewfinderSaturationConfig {
        let mut config = ViewfinderSaturationConfig::default();
        config.isp.frame_slots = self.slots();
        config
    }

    fn build_viewfinder(&self, variant: ViewfinderVariant) -> Result<Box<dyn BenchWork>, BuildError> {
        let params = self.inputs.params(PipelineKind::Viewfinder)?;
        let config = viewfinder_variant::configure(variant, self.viewfinder_config());
        let work = ViewfinderSaturation::new_synthetic(&self.context, config, params)?;
        Ok(Box::new(ViewfinderWork(work)))
    }

    fn build_scopes(&mut self, shape: ScopeShape, input: ScopeInputKind) -> Result<Box<dyn BenchWork>, BuildError> {
        // Размер картинки и положение глаза — из настоящей раскладки экрана калибровки.
        let layout_request = self.calibrate_scope_request()?;
        let (kind, axes) = match shape {
            ScopeShape::Waveform => (ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709),
            ScopeShape::Parade => (ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709),
            ScopeShape::VectorscopeYcbcr => (ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709),
            ScopeShape::VectorscopeOklab => (ScopeKind::Vectorscope, VectorscopeAxes::OklabAb),
        };
        let request = ScopeRequest {
            kind,
            vectorscope_axes: axes,
            ..layout_request
        };
        let work = self.scopes_for(&request, scope_content(input))?;
        Ok(Box::new(ScopesWork(work)))
    }

    /// Запрос прибора раздела CALIBRATE: размер картинки и положение глаза из раскладки экрана.
    fn calibrate_scope_request(&mut self) -> Result<ScopeRequest, BuildError> {
        let plan = self.plan()?;
        plan.scene(DisplayScreen::Calibrate).scope_request.ok_or_else(|| {
            BuildError(WorkloadFailure::new(
                FailureKind::BuildFailed,
                "экран CALIBRATE не попросил прибор: размер картинки взять неоткуда",
            ))
        })
    }

    fn scopes_for(&self, request: &ScopeRequest, content: SceneContent) -> Result<ScopesSaturation, BuildError> {
        let config = ScopesSaturationConfig {
            viewfinder: self.viewfinder_config(),
            input_content: content,
            kind: request.kind,
            vectorscope_axes: request.vectorscope_axes,
            scan: request.scan,
            picture_size: request.image_size,
            frame_slots: self.slots(),
            ..ScopesSaturationConfig::default()
        };
        let params = self.inputs.params(PipelineKind::Viewfinder)?;
        Ok(ScopesSaturation::new_synthetic(&self.context, config, params)?)
    }

    fn build_headroom(&self) -> Result<Box<dyn BenchWork>, BuildError> {
        let mut config = HeadroomSaturationConfig::default();
        config.headroom.frame_slots = self.slots();
        let params = self.inputs.params(PipelineKind::Quality)?;
        let work = HeadroomSaturation::new_synthetic(&self.context, config, params)?;
        Ok(Box::new(HeadroomWork(work)))
    }

    fn build_still(&self) -> Result<Box<dyn BenchWork>, BuildError> {
        let config = StillSaturationConfig {
            frame_slots: self.slots(),
            ..StillSaturationConfig::default()
        };
        let params = self.inputs.params(PipelineKind::Quality)?;
        let work = StillSaturation::new_synthetic(&self.context, config, params)?;
        Ok(Box::new(StillWork(work)))
    }

    fn build_display(
        &mut self,
        composition: FrameComposition,
        screen: DisplayScreen,
        rhythm: BackdropRhythm,
        filter: DrawFilter,
    ) -> Result<Box<dyn BenchWork>, BuildError> {
        let slots = self.slots();
        let (draw_list, viewfinder, scope_request) = {
            let scene = self.plan()?.scene(screen);
            (filter.apply(&scene.draw_list), scene.viewfinder, scene.scope_request)
        };
        // Состав сцены в результате — фактический, после фильтра (а не весь экран).
        let summary = summarize(&draw_list, self.surface);
        // Приборы: настоящая картинка панели считается один раз, дальше только читается кадрами показа.
        let (scopes, live_images) = match (screen, scope_request) {
            (DisplayScreen::Calibrate, Some(request)) => {
                let mut scopes = self.scopes_for(
                    &request,
                    SceneContent::Natural {
                        seed: NATURAL_SCENE_SEED,
                    },
                )?;
                self.compute_scope_picture_once(&mut scopes)?;
                let view = scopes.scopes().output_view().ok_or_else(|| {
                    BuildError(WorkloadFailure::new(
                        FailureKind::BuildFailed,
                        "приборы не записали картинку для панели экрана",
                    ))
                })?;
                (Some(scopes), vec![(LiveImage::ColorScope, LiveImageInput { view })])
            }
            _ => (None, Vec::new()),
        };
        let failures = BackdropFailures::default();
        let backdrop = IspBackdrop::new(&self.context, &self.inputs, slots, failures.clone())?;
        let config = UiFrameSaturationConfig {
            target_extent: ash::vk::Extent2D {
                width: self.surface.width,
                height: self.surface.height,
            },
            frames_in_flight: self.bench.frames_in_flight,
            composition,
            backdrop_cadence: match rhythm {
                BackdropRhythm::NewFrameEveryFrame => BackdropCadence::NewFrameEveryFrame,
                BackdropRhythm::SameFrame => BackdropCadence::SameFrame,
            },
            ..UiFrameSaturationConfig::default()
        };
        let scene = SaturationScene {
            draw_list,
            live_images,
            viewfinder,
            backdrop: Box::new(backdrop),
        };
        let ui = UiFrameSaturation::new(&self.context, config, scene)?;
        Ok(Box::new(DisplayWork {
            ui,
            _scopes: scopes,
            backdrop_failures: failures,
            scene: summary,
        }))
    }

    /// Один короткий прогон приборов: после него картинка прибора записана и лежит в `SHADER_READ_ONLY_OPTIMAL`.
    fn compute_scope_picture_once(&self, scopes: &mut ScopesSaturation) -> Result<(), BuildError> {
        let config = SaturationConfig {
            warmup: std::time::Duration::ZERO,
            measure: SCOPE_PICTURE_BENCH_MEASURE,
            timeline_step: SCOPE_PICTURE_BENCH_MEASURE,
            ..self.bench
        };
        let fail = |what: String| BuildError(WorkloadFailure::new(FailureKind::BuildFailed, what));
        let mut bench = SaturationBench::new(&self.context, config)
            .map_err(|error| fail(format!("стенд картинки прибора: {error}")))?;
        let report = bench
            .run(scopes, &CancelToken::new())
            .map_err(|error| fail(format!("прогон картинки прибора: {error}")))?;
        if report.total_completed_frames() == 0 {
            return Err(fail("приборы не завершили ни одного кадра картинки панели".to_owned()));
        }
        if let Some(error) = scopes.take_failure() {
            return Err(fail(format!("приборы картинки панели: {error}")));
        }
        scopes.reset_statistics();
        Ok(())
    }

    /// Прогон стенда по собранной работе; освобождение — снаружи.
    fn run_bench(&self, work: &mut dyn BenchWork, cancel: &CancelToken) -> Result<SaturationReport, WorkloadFailure> {
        let mut bench = SaturationBench::new(&self.context, self.bench)
            .map_err(|error| WorkloadFailure::new(FailureKind::BenchFailed, error.to_string()))?;
        let result = bench.run(work.workload(), cancel);
        drop(bench);
        result.map_err(|error| bench_failure(&error, work.take_failure()))
    }
}

fn scope_content(input: ScopeInputKind) -> SceneContent {
    match input {
        ScopeInputKind::Natural => SceneContent::Natural {
            seed: NATURAL_SCENE_SEED,
        },
        ScopeInputKind::UniformGray => SceneContent::UniformGray {
            code: DEFAULT_GRAY_CODE,
        },
    }
}

/// Ошибка стенда с типизированной причиной самой работы, если она её запомнила.
fn bench_failure(error: &SaturationError, work_failure: Option<String>) -> WorkloadFailure {
    match work_failure {
        Some(cause) => WorkloadFailure::new(FailureKind::WorkloadReportedFailure, format!("{cause}; стенд: {error}")),
        None => WorkloadFailure::new(FailureKind::BenchFailed, error.to_string()),
    }
}

impl WorkloadExecutor for GpuWorkloadExecutor {
    fn execute(&mut self, entry: &CatalogEntry, cancel: &CancelToken) -> Result<ExecutedWorkload, WorkloadFailure> {
        let baseline = self.context.live_objects();
        let outcome = self.execute_and_release(entry, cancel);
        // Работа уже уничтожена (область видимости `execute_and_release` закрыта): живые объекты вернулись к базе.
        if let Err(error) = self.context.wait_idle() {
            return Err(WorkloadFailure::new(
                FailureKind::ReleaseFailed,
                format!("ожидание GPU: {error}"),
            ));
        }
        let differences = self.context.live_objects().differences_from(&baseline);
        let executed = outcome?;
        if !differences.is_empty() {
            let text: Vec<String> = differences
                .iter()
                .map(|(kind, before, after)| format!("{kind:?} {before}->{after}"))
                .collect();
            return Err(WorkloadFailure::new(
                FailureKind::ObjectsNotReleased,
                format!(
                    "после освобождения работы живых объектов Vulkan стало иначе: {}",
                    text.join(", ")
                ),
            ));
        }
        Ok(executed)
    }
}

impl GpuWorkloadExecutor {
    /// Создаёт, гонит и уничтожает работу. Всё, что держит GPU-объекты работы, живёт ТОЛЬКО внутри этого метода.
    fn execute_and_release(
        &mut self,
        entry: &CatalogEntry,
        cancel: &CancelToken,
    ) -> Result<ExecutedWorkload, WorkloadFailure> {
        let mut work = self.build(entry).map_err(|error| error.0)?;
        let expected_slots = self.slots();
        if work.slots() != expected_slots {
            let actual = work.slots();
            self.release(work);
            return Err(WorkloadFailure::new(
                FailureKind::SlotsMismatch,
                format!("у работы {actual} слотов, у стенда {expected_slots}"),
            ));
        }
        let run = self.run_bench(work.as_mut(), cancel);
        let finish = work.finish();
        let reported = work.take_failure();
        let executed = run.and_then(|report| {
            if let Err(error) = &finish {
                return Err(WorkloadFailure::new(FailureKind::ReleaseFailed, error.clone()));
            }
            if let Some(cause) = reported {
                return Err(WorkloadFailure::new(FailureKind::WorkloadReportedFailure, cause));
            }
            let measurement = Measurement::of(&report, &work.pass_times());
            // Дайджест строго ПОСЛЕ окна замера, `finish()` и `wait_idle`: чтение блокирующее и цену не искажает.
            let digest = self.digest_of(work.as_mut());
            Ok(ExecutedWorkload {
                measurement,
                scene: work.scene(),
                ui: work.ui_counters(),
                digest,
            })
        });
        self.release(work);
        executed
    }

    /// Дайджест выхода работы после прогона. Отказ не срывает строку: он становится `Unavailable`.
    fn digest_of(&self, work: &mut dyn BenchWork) -> DigestOutcome {
        let result = match self.context.wait_idle() {
            Ok(()) => work.output_digest(),
            Err(error) => Err(format!("ожидание GPU перед дайджестом: {error}")),
        };
        DigestOutcome::from_result(result, work.digest_note())
    }

    /// Освобождение работы: GPU дочитал всё, потом уничтожение.
    fn release(&self, work: Box<dyn BenchWork>) {
        if let Err(error) = self.context.wait_idle() {
            tracing::error!(target: "fasticamera::saturation", %error, "GPU не дождались перед освобождением работы");
        }
        drop(work);
    }
}
