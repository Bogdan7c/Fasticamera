//! Присоединённый показ: цель `DisplayTarget` + пайплайн тестового паттерна + учёт кадров.
//!
//! Владение и порядок уничтожения (ЗДЕСЬ ЖИВЁТ ЛОВУШКА A3, VUID-vkDestroyPipeline-00765):
//! пайплайн паттерна нельзя уничтожать, пока GPU ещё исполняет кадры, записанные с ним.
//! Кадры в полёте принадлежат `DisplayTarget`, а `detach`/`Drop` цели дожидаются GPU.
//! Поэтому:
//! * цель и паттерн живут в [`DisplayChain`], которая уничтожает их ТОЛЬКО через [`DisplayChain::release`]:
//!   сначала `detach` цели, и лишь после успеха — паттерн;
//! * если `detach` не удался (GPU завис, `gpu-vulkan` намеренно оставил swapchain/кольцо кадров
//!   в утечке), паттерн тоже намеренно утекает: кадры в полёте всё ещё могут ссылаться на пайплайн;
//! * то же правило действует в `Drop` (`AttachedDisplay` уничтожен без `release`);
//! * при смене формата пайплайн пересоздаётся после ожидания собственных сабмитов цели
//!   (не всего устройства).

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::{
    AcquireOutcome, DetachError, DisplayFrame, DisplayTarget, PresentOutcome as GpuPresentOutcome, PresentRequest,
    PresentationSurface, RecreateOutcome, SwapchainError,
};
use gpu_vulkan::test_pattern::TestPattern;
use shell_contract::surface::{NativeSurface, SurfaceSize};

#[cfg(feature = "test-support")]
use super::ViewfinderReadback;
use super::camera::{
    CameraDrawError, CameraFeed, CameraPipelines, CameraSession, CameraSessionFailure, CameraSessionParts,
    HeadroomBenchParts, ViewfinderSettings, ViewfinderStats, WhiteBalanceParts,
};
use super::config::VulkanBackendConfig;
use super::mapping::{FailureKind, classify_swapchain_error, display_info, gpu_backend_error};
use super::present_history::PresentHistory;
use super::present_time::PresentTimePolicy;
use crate::backend::{
    AttachError, BackendError, DisplayInfo, FrameCpuTiming, FrameGpuTiming, FrameLatchPlan, FrameTiming,
    LatchWaitResult, PastPresentation, PresentOutcome,
};
use crate::camera_diagnostics::CameraStreamDiagnostics;
use crate::config::LateLatchMode;
use crate::thread::{MonotonicTimer, SleepUntilOutcome, SystemMonotonicTimer};
use crate::viewfinder::{AwbStatisticsFeed, ViewfinderDiagnostics};

const LOG_TARGET: &str = "fasticamera::render::vulkan";

/// Итог освобождения показа.
#[derive(Debug)]
pub(super) struct ReleaseReport {
    /// Окно платформы, которое можно отпускать (`None` — бэкенд не смог его вернуть).
    pub(super) surface: Option<NativeSurface>,
    /// Ошибка уничтожения цепочки: окно всё равно возвращено, но объекты GPU могли остаться.
    pub(super) error: Option<BackendError>,
    /// Итоговые счётчики показа кадров камеры этой surface (`None` — показывался тестовый паттерн).
    pub(super) camera: Option<CameraStreamDiagnostics>,
    /// Статистика видоискателя, которую сессия возвращает владельцу (`None` — камеры не было).
    pub(super) viewfinder: Option<ViewfinderStats>,
}

/// Что нужно показу камеры при присоединении surface.
pub(super) struct CameraAttach<'a> {
    /// Источник кадров и фабрика импортёра.
    pub(super) feed: &'a CameraFeed,
    /// Действующие настройки видоискателя.
    pub(super) settings: ViewfinderSettings,
    /// Статистика видоискателя: забирается сессией и возвращается при освобождении (или при отказе).
    pub(super) stats: &'a mut Option<ViewfinderStats>,
    /// Окно статистики времени GPU (если статистики ещё нет).
    pub(super) gpu_time_window_frames: usize,
    /// Лента статистики AWB.
    pub(super) awb_feed: &'a AwbStatisticsFeed,
    /// Наш AWB: настройки, указания 3A, память белой сцены.
    pub(super) white_balance: WhiteBalanceParts,
}

/// Чем рисуется содержимое кадра. Пайплайн всегда записан в кадры цели, поэтому уничтожается только
/// после того, как цель освободила GPU (см. [`DisplayChain::release`]).
enum Painter {
    /// Тестовый паттерн (градиент + метка кадра).
    TestPattern(TestPattern),
    /// Кадры камеры: объекты GPU (ISP, компоновщик, мозаика) и состояние показа (лизинги, серии сабмитов).
    Camera {
        pipelines: Box<CameraPipelines>,
        session: Box<CameraSession>,
    },
}

/// Цель показа вместе с рисовальщиком, записанным в её кадры. Единственный способ их уничтожить —
/// [`DisplayChain::release`]: он знает, что пайплайн нельзя трогать, пока цель не освободила GPU.
struct DisplayChain {
    target: DisplayTarget,
    painter: Painter,
}

/// Чем закончилось освобождение цепочки.
struct ChainRelease {
    /// Поверхность, возвращённая целью (при ошибке detach — тоже).
    surface: PresentationSurface,
    /// Ошибка `detach` цели, если GPU не освободился.
    detach_error: Option<GpuError>,
    /// Итоговые счётчики показа камеры.
    camera: Option<CameraStreamDiagnostics>,
    /// Статистика видоискателя для владельца.
    viewfinder: Option<ViewfinderStats>,
}

impl DisplayChain {
    /// Отсоединяет цель и решает судьбу пайплайна: уничтожить (GPU свободен) или намеренно оставить
    /// в утечке (`detach` не удался — кадры в полёте могут исполняться, VUID-vkDestroyPipeline-00765).
    ///
    /// Для камеры сначала освобождается сессия показа: дождаться своих сабмитов, вернуть лизинги камере
    /// (или выбросить, если GPU завис), и только потом отсоединять цель.
    fn release(self) -> ChainRelease {
        let Self { target, painter } = self;
        // Пайплайн хранится как «владелец, которого надо дожить до конца работы цели»: уничтожается
        // обычным drop после успешного detach или забывается (утечка) при зависшем GPU.
        let (pipeline_owner, camera, viewfinder): (Box<dyn std::any::Any>, _, _) = match painter {
            Painter::TestPattern(pattern) => (Box::new(pattern), None, None),
            Painter::Camera { pipelines, session } => {
                let shutdown = session.shutdown(&target);
                (
                    Box::new(pipelines),
                    Some(shutdown.diagnostics),
                    Some(shutdown.viewfinder),
                )
            }
        };
        match target.detach() {
            Ok(surface) => {
                // Только ПОСЛЕ detach: GPU больше не исполняет кадры с этим пайплайном.
                drop(pipeline_owner);
                ChainRelease {
                    surface,
                    detach_error: None,
                    camera,
                    viewfinder,
                }
            }
            Err(DetachError { error, surface }) => {
                tracing::error!(
                    target: LOG_TARGET,
                    %error,
                    "detach не удался: пайплайн показа намеренно НЕ уничтожается (утечка), кадры в полёте могут его исполнять"
                );
                std::mem::forget(pipeline_owner);
                ChainRelease {
                    surface,
                    detach_error: Some(error),
                    camera,
                    viewfinder,
                }
            }
        }
    }
}

/// Живой показ одной surface.
pub(super) struct AttachedDisplay {
    /// `None` только внутри `release`/`Drop` (после снятия цепочки).
    chain: Option<DisplayChain>,
    /// Окно, которое держит бэкенд сам (только ПК-стенд: цель работает на `Headless`,
    /// а `NativeSurface` надо вернуть при `detach`). На Android окно внутри цели.
    held_surface: Option<NativeSurface>,
    /// Контекст GPU (для пересоздания пайплайна).
    context: GpuContext,
    /// Последний известный размер surface (для повторного пересоздания).
    size: SurfaceSize,
    /// Прошлый показ вернул `SUBOPTIMAL`: на следующем кадре просим поток пересоздать цепочку.
    recreate_pending: bool,
    present_history: PresentHistory,
    /// Как ставить желаемое время показа (упреждение от ожидаемого vsync).
    present_time: PresentTimePolicy,
    /// Последний период экрана от драйвера (`refresh_cycle`), обновляется на каждом опросе
    /// диагностики (`display_info`, раз в `diagnostics_poll_interval_ms`), чтобы не
    /// спрашивать драйвер на каждом кадре. `Cell`: опрос идёт через `&self`.
    measured_refresh_period: Cell<Option<Duration>>,
    /// Замер CPU последнего кадра; поток забирает его сразу после `render_and_present`.
    last_frame_cpu_timing: Option<FrameCpuTiming>,
}

impl AttachedDisplay {
    /// Поднимает цепочку и пайплайн. При любой ошибке ничего не остаётся живым: окно
    /// (и `presentation`, и `held_surface`) уничтожено, как требует контракт `DisplayBackend`.
    pub(super) fn attach(
        context: &GpuContext,
        config: &VulkanBackendConfig,
        presentation: PresentationSurface,
        held_surface: Option<NativeSurface>,
        size: SurfaceSize,
        camera: Option<CameraAttach<'_>>,
    ) -> Result<Self, AttachError> {
        let target = match DisplayTarget::attach(context, presentation, config.display_target_config(size)) {
            Ok(target) => target,
            Err(failure) => {
                // Окно из ошибки отпускаем сразу (drop на выходе), в бэкенде ничего не остаётся.
                let (kind, error) = classify_swapchain_error("create_display_target", &failure.error);
                return Err(match kind {
                    FailureKind::SurfaceLost => AttachError::SurfaceRejected(error),
                    FailureKind::Other => AttachError::SwapchainFailed(error),
                });
            }
        };
        let info = match target.info() {
            Ok(info) => info,
            Err(error) => {
                return Err(AttachError::SwapchainFailed(
                    classify_swapchain_error("display_target_info", &error).1,
                ));
            }
        };
        // Ошибка здесь: `target` уничтожится Drop-ом (с ожиданием GPU), рисовальщика ещё нет.
        let painter = match camera {
            None => Painter::TestPattern(
                TestPattern::new(context, info.format)
                    .map_err(|error| AttachError::SwapchainFailed(gpu_backend_error("create_test_pattern", &error)))?,
            ),
            Some(camera) => create_camera_painter(context, config, &info, camera)?,
        };
        Ok(Self {
            chain: Some(DisplayChain { target, painter }),
            held_surface,
            context: context.clone(),
            size,
            recreate_pending: false,
            present_history: PresentHistory::new(config.presentation_history_capacity),
            present_time: config.present_time_policy(),
            measured_refresh_period: Cell::new(None),
            last_frame_cpu_timing: None,
        })
    }

    /// Взводит тестовый шов отказа цели (только для тестов с feature `fault-injection`).
    #[cfg(feature = "fault-injection")]
    pub(super) fn inject_fault(&mut self, fault: gpu_vulkan::swapchain::InjectedFault) -> Result<(), BackendError> {
        let chain = self.chain.as_mut().ok_or_else(|| chain_missing("inject_fault"))?;
        chain
            .target
            .inject_fault(fault)
            .map_err(|error| classify_swapchain_error("inject_fault", &error).1)
    }

    /// Один кадр: acquire -> паттерн -> present. Исходы разведены по смыслу (см. `PresentOutcome`).
    ///
    /// Попутно меряет CPU кадра ([`FrameCpuTiming`]): время в `acquire_frame` и путь от захвата
    /// кадра камеры до возврата из сабмита. Замер — только чтение монотонных часов, без ожиданий.
    pub(super) fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        self.last_frame_cpu_timing = None;
        // Прошлый показ был SUBOPTIMAL: сначала пересоздание (его сделает поток через resize).
        if self.recreate_pending {
            return PresentOutcome::NeedsRecreate;
        }
        let Some(chain) = self.chain.as_mut() else {
            return PresentOutcome::Failed(chain_missing("render_and_present"));
        };
        let acquire_started = read_monotonic_clock();
        let acquired = chain.target.acquire_frame();
        let acquire_finished = read_monotonic_clock();
        let acquire_wait = elapsed_between(acquire_started, acquire_finished);
        // Кадр, не дошедший до показа, тоже даёт замер acquire (таймаут — самое интересное в нём).
        // Нечитаемые часы дают `None`: ноль вместо замера исказил бы статистику.
        self.last_frame_cpu_timing = acquire_wait.map(|acquire_wait| FrameCpuTiming {
            acquire_wait,
            latch_wait: Duration::ZERO,
            latch_result: LatchWaitResult::NotRequested,
            latch_to_submit: None,
        });
        let frame = match acquired {
            Ok(AcquireOutcome::Ready(frame)) => frame,
            Ok(AcquireOutcome::TimedOut) => return PresentOutcome::TimedOut,
            Ok(AcquireOutcome::NeedsRecreate) => return PresentOutcome::NeedsRecreate,
            Err(error) => return failure_outcome("acquire_frame", &error),
        };
        // Поздний захват: ждём `latch_at` (если включён и показывается камера), и стоимость кадра
        // считаем от конца ожидания — само ожидание в неё не входит.
        let latch_result = self.wait_for_camera_latch(&timing.target.latch);
        let latch_moment = match latch_result {
            LatchWaitResult::NotRequested => acquire_finished,
            LatchWaitResult::Waited | LatchWaitResult::RefusedBeyondBudget | LatchWaitResult::ClockFailed => {
                read_monotonic_clock()
            }
        };
        let outcome = self.draw_and_present_acquired(frame, timing);
        let submitted = read_monotonic_clock();
        if let Some(cpu_timing) = self.last_frame_cpu_timing.as_mut() {
            cpu_timing.latch_wait = elapsed_between(acquire_finished, latch_moment).unwrap_or(Duration::ZERO);
            cpu_timing.latch_result = latch_result;
            cpu_timing.latch_to_submit = elapsed_between(latch_moment, submitted);
        }
        outcome
    }

    /// Ждёт момента позднего захвата перед опросом камеры, если это включено в плане и рисуется
    /// камера (тестовому паттерну свежесть кадра камеры не нужна). Только ожидание CPU, не GPU.
    fn wait_for_camera_latch(&self, plan: &FrameLatchPlan) -> LatchWaitResult {
        let draws_camera = matches!(
            self.chain.as_ref().map(|chain| &chain.painter),
            Some(Painter::Camera { .. })
        );
        if plan.mode != LateLatchMode::BeforeCameraPoll || !draws_camera {
            return LatchWaitResult::NotRequested;
        }
        match plan.wait_until_latch() {
            SleepUntilOutcome::Reached { .. } | SleepUntilOutcome::AlreadyPassed { .. } => LatchWaitResult::Waited,
            SleepUntilOutcome::RefusedTooFar { remaining } => {
                // Не warn на каждый кадр: при `Preferred` с далёкой линией это штатно, отказы
                // считает диагностика (`latch_waits_refused`).
                tracing::debug!(
                    target: LOG_TARGET,
                    remaining_us = remaining.as_micros(),
                    max_wait_us = plan.max_wait.as_micros(),
                    "момент позднего захвата дальше предела ожидания: камера опрошена сразу"
                );
                LatchWaitResult::RefusedBeyondBudget
            }
            SleepUntilOutcome::ClockFailed(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "поздний захват без ожидания: часы не прочитались");
                LatchWaitResult::ClockFailed
            }
        }
    }

    /// Замер CPU последнего кадра (выдаётся один раз).
    pub(super) fn take_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        self.last_frame_cpu_timing.take()
    }

    /// Времена кадров на GPU, собранные сессией камеры (тестовый паттерн GPU не меряет — пусто).
    pub(super) fn drain_frame_gpu_timings(&mut self) -> Vec<FrameGpuTiming> {
        match self.chain.as_mut().map(|chain| &mut chain.painter) {
            Some(Painter::Camera { session, .. }) => session.drain_frame_gpu_timings(),
            _ => Vec::new(),
        }
    }

    /// Рисует содержимое в полученную картинку swapchain и ставит её на показ.
    fn draw_and_present_acquired(&mut self, frame: DisplayFrame, timing: FrameTiming) -> PresentOutcome {
        let Some(chain) = self.chain.as_mut() else {
            return PresentOutcome::Failed(chain_missing("draw_and_present"));
        };
        // Время показа: чуть раньше ожидаемого vsync ВЫБРАННОЙ линии кадра, чтобы дрожание метки
        // не переносило кадр на следующий vsync (см. `present_time`).
        let request = PresentRequest {
            desired_present_time: Some(
                self.present_time
                    .desired_present_time(timing.expected_present(), self.measured_refresh_period.get()),
            ),
        };
        // К этому моменту ожидание позднего захвата (если включено) уже прошло: сессия камеры
        // забирает кадр этого vsync — ровно один опрос потока на кадр показа (политика потока решает,
        // самый свежий это кадр или старейший из буфера видоискателя, решение 53). Весь план (`timing.target`: линия, срок, `latch_at`)
        // в области видимости — его можно передать сессии камеры параметром.
        let presented = match &mut chain.painter {
            Painter::TestPattern(pattern) => {
                // Паттерн показывает младшие 32 бита номера кадра: счётчик в метке циклический.
                let pattern_frame = u32::try_from(timing.frame_index & u64::from(u32::MAX)).unwrap_or(u32::MAX);
                if let Err(error) = pattern.record(
                    frame.command_buffer(),
                    frame.pattern_target(),
                    pattern_frame,
                    frame.transform(),
                ) {
                    // Кадр нельзя бросить: возвращаем картинку цели, иначе она потеряет слот.
                    if let Err(abandon_error) = chain.target.abandon_frame(frame) {
                        tracing::warn!(target: LOG_TARGET, %abandon_error, "не удалось вернуть неотрисованный кадр");
                    }
                    return PresentOutcome::Failed(gpu_backend_error("record_test_pattern", &error));
                }
                chain.target.present(frame, request)
            }
            Painter::Camera { pipelines, session } => {
                match session.draw_and_present(&mut chain.target, pipelines, frame, request, timing.frame_index) {
                    Ok(outcome) => Ok(outcome),
                    Err(CameraDrawError::Swapchain(error)) => Err(error),
                    Err(CameraDrawError::Record(error)) => {
                        return PresentOutcome::Failed(gpu_backend_error("record_camera_preview", &error));
                    }
                }
            }
        };
        match presented {
            Ok(GpuPresentOutcome::Presented { present_id }) => {
                self.present_history.record(present_id, timing.frame_index);
                PresentOutcome::Presented
            }
            Ok(GpuPresentOutcome::Suboptimal { present_id }) => {
                // Кадр показан; цепочку пересоздадим на следующем тике.
                self.present_history.record(present_id, timing.frame_index);
                self.recreate_pending = true;
                PresentOutcome::Presented
            }
            Ok(GpuPresentOutcome::NeedsRecreate) => PresentOutcome::NeedsRecreate,
            Err(error) => failure_outcome("queue_present", &error),
        }
    }

    /// Пересоздаёт цепочку под размер; при смене формата заново создаёт пайплайн.
    pub(super) fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        let chain = self.chain.as_mut().ok_or_else(|| chain_missing("resize"))?;
        match chain.target.recreate(Some(size)) {
            Ok(RecreateOutcome::Recreated { format_changed }) => {
                self.size = size;
                self.recreate_pending = false;
                // Показы старой цепочки отчитаться уже не смогут: поток вычерпал их до resize.
                self.present_history.clear();
                if format_changed {
                    self.rebuild_painter()?;
                }
                Ok(())
            }
            // Размер нулевой (окно свёрнуто): цепочка остаётся устаревшей, acquire снова
            // потребует пересоздания, когда размер появится.
            Ok(RecreateOutcome::Deferred(_)) => {
                self.size = size;
                Ok(())
            }
            Err(error) => Err(classify_swapchain_error("recreate_swapchain", &error).1),
        }
    }

    /// Сколько показов ждут записи display timing (тестовый шов).
    #[cfg(feature = "fault-injection")]
    pub(super) fn present_history_len(&self) -> usize {
        self.present_history.len()
    }

    /// Новый пайплайн под новый формат (для камеры пересоздаётся только предпросмотр, сессия живёт дальше). Старый уничтожается, когда цель дождалась СВОИХ сабмитов
    /// (fence'ы кольца, с таймаутом): `vkDeviceWaitIdle` на весь GPU не нужен и не ограничен по
    /// времени. Показы на пайплайн не ссылаются, поэтому их ждать не требуется.
    fn rebuild_painter(&mut self) -> Result<(), BackendError> {
        let chain = self.chain.as_mut().ok_or_else(|| chain_missing("rebuild_painter"))?;
        let info = chain
            .target
            .info()
            .map_err(|error| classify_swapchain_error("display_target_info", &error).1)?;
        chain
            .target
            .wait_for_submitted_frames()
            .map_err(|error| classify_swapchain_error("wait_for_submitted_frames_before_pattern_rebuild", &error).1)?;
        match &mut chain.painter {
            Painter::TestPattern(pattern) => {
                *pattern = TestPattern::new(&self.context, info.format)
                    .map_err(|error| gpu_backend_error("rebuild_test_pattern", &error))?;
            }
            Painter::Camera { pipelines, .. } => {
                pipelines
                    .rebuild_for_format(&info)
                    .map_err(|error| gpu_backend_error("rebuild_camera_pipelines", &error))?;
            }
        }
        Ok(())
    }

    /// Счётчики показа кадров камеры (`None` — показывается тестовый паттерн).
    pub(super) fn camera_diagnostics(&self) -> Option<CameraStreamDiagnostics> {
        match &self.chain.as_ref()?.painter {
            Painter::Camera { session, .. } => Some(session.diagnostics()),
            Painter::TestPattern(_) => None,
        }
    }

    /// Диагностика видоискателя (`None` — показывается тестовый паттерн).
    pub(super) fn viewfinder_diagnostics(&mut self) -> Option<ViewfinderDiagnostics> {
        match &mut self.chain.as_mut()?.painter {
            Painter::Camera { session, .. } => Some(session.viewfinder_diagnostics()),
            Painter::TestPattern(_) => None,
        }
    }

    /// Новые настройки видоискателя для живой сессии показа.
    pub(super) fn apply_viewfinder_settings(&mut self, settings: ViewfinderSettings) {
        if let Some(Painter::Camera { session, .. }) = self.chain.as_mut().map(|chain| &mut chain.painter) {
            session.apply_settings(settings);
        }
    }

    /// Читает изображение видоискателя (тестовый шов): размер и пиксели `[r, g, b, a]` построчно.
    /// `Ok(None)` — камеры нет или ISP ещё не записал ни одного кадра.
    #[cfg(feature = "test-support")]
    pub(super) fn read_viewfinder_image(&self) -> Result<Option<ViewfinderReadback>, GpuError> {
        let Some(Painter::Camera { pipelines, .. }) = self.chain.as_ref().map(|chain| &chain.painter) else {
            return Ok(None);
        };
        let Some(image) = pipelines.isp().image() else {
            return Ok(None);
        };
        let extent = image.extent();
        let pixels = gpu_vulkan::kit_test_support::read_storage_image_rgba(image.storage_image(), image.current_use())?;
        Ok(Some(ViewfinderReadback {
            width: extent.width,
            height: extent.height,
            pixels,
        }))
    }

    /// Параметры цепочки в нейтральном виде; ошибка запроса логируется, а не глотается.
    pub(super) fn display_info(&self) -> Option<DisplayInfo> {
        let target = &self.chain.as_ref()?.target;
        let info = target
            .info()
            .inspect_err(|error| tracing::warn!(target: LOG_TARGET, %error, "info цели недоступна"))
            .ok()?;
        let refresh_cycle = target
            .refresh_cycle()
            .inspect_err(|error| tracing::warn!(target: LOG_TARGET, %error, "refresh_cycle недоступен"))
            .ok()
            .flatten();
        self.measured_refresh_period.set(refresh_cycle);
        Some(display_info(&info, refresh_cycle))
    }

    /// Вычерпывает реальные времена показа и привязывает их к кадрам потока.
    pub(super) fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        let Some(chain) = self.chain.as_ref() else {
            return Vec::new();
        };
        let drained = match chain.target.drain_past_presentations() {
            Ok(drained) => drained,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "не удалось вычерпать времена показа");
                return Vec::new();
            }
        };
        drained
            .into_iter()
            .filter_map(|past| {
                let frame_index = self.present_history.resolve(past.present_id)?;
                Some(PastPresentation {
                    present_id: past.present_id,
                    frame_index,
                    actual_present: past.actual,
                })
            })
            .collect()
    }

    /// Полное освобождение: дождаться GPU, уничтожить цепочку и вернуть окно, затем паттерн
    /// (если GPU освободился).
    pub(super) fn release(mut self) -> ReleaseReport {
        let held_surface = self.held_surface.take();
        let Some(chain) = self.chain.take() else {
            return ReleaseReport {
                surface: held_surface,
                error: Some(chain_missing("release")),
                camera: None,
                viewfinder: None,
            };
        };
        let ChainRelease {
            surface,
            detach_error,
            camera,
            viewfinder,
        } = chain.release();
        ReleaseReport {
            surface: reclaim_window(surface, held_surface),
            error: detach_error.map(|error| gpu_backend_error("detach_display_target", &error)),
            camera,
            viewfinder,
        }
    }
}

impl Drop for AttachedDisplay {
    /// Показ уничтожен без `release` (например, бэкенд выброшен при паническом выходе): тот же
    /// порядок и то же правило утечки при зависшем GPU. Окно ПК-стенда отпускается вместе с полями.
    fn drop(&mut self) {
        if let Some(chain) = self.chain.take() {
            let ChainRelease { detach_error, .. } = chain.release();
            if let Some(error) = detach_error {
                tracing::error!(target: LOG_TARGET, %error, "показ уничтожен без release, GPU не освободился");
            }
        }
    }
}

/// Рисовальщик камеры: объекты GPU под формат цели и сессия показа. При отказе статистика
/// видоискателя возвращается владельцу (`camera.stats`), а цель уничтожит вызывающий.
fn create_camera_painter(
    context: &GpuContext,
    config: &VulkanBackendConfig,
    info: &gpu_vulkan::swapchain::DisplayTargetInfo,
    camera: CameraAttach<'_>,
) -> Result<Painter, AttachError> {
    let pipelines = CameraPipelines::new(context, info, &config.viewfinder.isp).map_err(|error| {
        AttachError::SwapchainFailed(BackendError {
            operation: "create_camera_pipelines".to_owned(),
            reason: error.to_string(),
        })
    })?;
    let stats = camera
        .stats
        .take()
        .unwrap_or_else(|| ViewfinderStats::new(camera.gpu_time_window_frames));
    let session = CameraSession::new(
        context,
        CameraSessionParts {
            frames: Arc::clone(&camera.feed.frames),
            make_importer: &camera.feed.make_importer,
            history_capacity: config.camera_present_history_capacity,
            settings: camera.settings,
            viewfinder: stats,
            awb_feed: camera.awb_feed.clone(),
            white_balance: camera.white_balance.clone(),
            headroom: HeadroomBenchParts {
                config: config.viewfinder.headroom_bench,
                frame_slots: info.frames_in_flight,
                mode: camera.settings.headroom,
            },
        },
    )
    .map_err(|failure| {
        let CameraSessionFailure { error, viewfinder } = *failure;
        *camera.stats = Some(viewfinder);
        AttachError::SwapchainFailed(BackendError {
            operation: "create_camera_session".to_owned(),
            reason: error.to_string(),
        })
    })?;
    Ok(Painter::Camera {
        pipelines: Box::new(pipelines),
        session: Box::new(session),
    })
}

/// Ошибка «цепочки нет»: показ уже освобождён (недостижимо через `VulkanDisplayBackend`, но не паникуем).
fn chain_missing(operation: &str) -> BackendError {
    BackendError {
        operation: operation.to_owned(),
        reason: "цепочка показа уже освобождена".to_owned(),
    }
}

/// Ошибка кадра -> исход потока: потеря surface отделена от прочих ошибок.
fn failure_outcome(operation: &str, error: &SwapchainError) -> PresentOutcome {
    match classify_swapchain_error(operation, error) {
        (FailureKind::SurfaceLost, _) => PresentOutcome::SurfaceLost,
        (FailureKind::Other, backend_error) => PresentOutcome::Failed(backend_error),
    }
}

/// Достаёт окно платформы: на Android оно лежит внутри возвращённой `PresentationSurface`,
/// на ПК-стенде цель работала на `Headless`, а окно хранил сам бэкенд.
fn reclaim_window(returned: PresentationSurface, held_surface: Option<NativeSurface>) -> Option<NativeSurface> {
    match returned {
        #[cfg(target_os = "android")]
        PresentationSurface::Android(window) => Some(window),
        PresentationSurface::Headless => held_surface,
    }
}

/// «Сейчас» по монотонным часам; сбой чтения логируется и даёт `None` (замер пропускается).
fn read_monotonic_clock() -> Option<shell_contract::clock::MonotonicNanos> {
    SystemMonotonicTimer
        .now()
        .inspect_err(|error| tracing::warn!(target: LOG_TARGET, %error, "замер CPU кадра пропущен"))
        .ok()
}

/// Длительность между двумя замерами (ноль при обратном порядке); `None`, если замера нет.
fn elapsed_between(
    earlier: Option<shell_contract::clock::MonotonicNanos>,
    later: Option<shell_contract::clock::MonotonicNanos>,
) -> Option<Duration> {
    let difference = later?.as_nanos().saturating_sub(earlier?.as_nanos());
    Some(Duration::from_nanos(u64::try_from(difference).unwrap_or(0)))
}
