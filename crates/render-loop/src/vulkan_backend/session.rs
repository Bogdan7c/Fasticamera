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
use std::time::Duration;

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::{
    AcquireOutcome, DetachError, DisplayTarget, PresentOutcome as GpuPresentOutcome, PresentRequest,
    PresentationSurface, RecreateOutcome, SwapchainError,
};
use gpu_vulkan::test_pattern::TestPattern;
use shell_contract::surface::{NativeSurface, SurfaceSize};

use super::config::VulkanBackendConfig;
use super::mapping::{FailureKind, classify_swapchain_error, display_info, gpu_backend_error};
use super::present_history::PresentHistory;
use super::present_time::PresentTimePolicy;
use crate::backend::{AttachError, BackendError, DisplayInfo, FrameTiming, PastPresentation, PresentOutcome};

const LOG_TARGET: &str = "fasticamera::render::vulkan";

/// Итог освобождения показа.
#[derive(Debug)]
pub(super) struct ReleaseReport {
    /// Окно платформы, которое можно отпускать (`None` — бэкенд не смог его вернуть).
    pub(super) surface: Option<NativeSurface>,
    /// Ошибка уничтожения цепочки: окно всё равно возвращено, но объекты GPU могли остаться.
    pub(super) error: Option<BackendError>,
}

/// Цель показа вместе с пайплайном, записанным в её кадры. Единственный способ их уничтожить —
/// [`DisplayChain::release`]: он знает, что пайплайн нельзя трогать, пока цель не освободила GPU.
struct DisplayChain {
    target: DisplayTarget,
    pattern: TestPattern,
}

/// Чем закончилось освобождение цепочки.
struct ChainRelease {
    /// Поверхность, возвращённая целью (при ошибке detach — тоже).
    surface: PresentationSurface,
    /// Ошибка `detach` цели, если GPU не освободился.
    detach_error: Option<GpuError>,
}

impl DisplayChain {
    /// Отсоединяет цель и решает судьбу пайплайна: уничтожить (GPU свободен) или намеренно оставить
    /// в утечке (`detach` не удался — кадры в полёте могут исполняться, VUID-vkDestroyPipeline-00765).
    fn release(self) -> ChainRelease {
        let Self { target, pattern } = self;
        match target.detach() {
            Ok(surface) => {
                // Только ПОСЛЕ detach: GPU больше не исполняет кадры с этим пайплайном.
                drop(pattern);
                ChainRelease {
                    surface,
                    detach_error: None,
                }
            }
            Err(DetachError { error, surface }) => {
                tracing::error!(
                    target: LOG_TARGET,
                    %error,
                    "detach не удался: пайплайн тестового паттерна намеренно НЕ уничтожается (утечка), кадры в полёте могут его исполнять"
                );
                std::mem::forget(pattern);
                ChainRelease {
                    surface,
                    detach_error: Some(error),
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
        let format = match target.info() {
            Ok(info) => info.format,
            Err(error) => {
                return Err(AttachError::SwapchainFailed(
                    classify_swapchain_error("display_target_info", &error).1,
                ));
            }
        };
        // Ошибка здесь: `target` уничтожится Drop-ом (с ожиданием GPU), паттерна ещё нет.
        let pattern = TestPattern::new(context, format)
            .map_err(|error| AttachError::SwapchainFailed(gpu_backend_error("create_test_pattern", &error)))?;
        Ok(Self {
            chain: Some(DisplayChain { target, pattern }),
            held_surface,
            context: context.clone(),
            size,
            recreate_pending: false,
            present_history: PresentHistory::new(config.presentation_history_capacity),
            present_time: config.present_time_policy(),
            measured_refresh_period: Cell::new(None),
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
    pub(super) fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        // Прошлый показ был SUBOPTIMAL: сначала пересоздание (его сделает поток через resize).
        if self.recreate_pending {
            return PresentOutcome::NeedsRecreate;
        }
        let Some(chain) = self.chain.as_mut() else {
            return PresentOutcome::Failed(chain_missing("render_and_present"));
        };
        let frame = match chain.target.acquire_frame() {
            Ok(AcquireOutcome::Ready(frame)) => frame,
            Ok(AcquireOutcome::TimedOut) => return PresentOutcome::TimedOut,
            Ok(AcquireOutcome::NeedsRecreate) => return PresentOutcome::NeedsRecreate,
            Err(error) => return failure_outcome("acquire_frame", &error),
        };
        // Паттерн показывает младшие 32 бита номера кадра: счётчик в метке циклический.
        let pattern_frame = u32::try_from(timing.frame_index & u64::from(u32::MAX)).unwrap_or(u32::MAX);
        if let Err(error) = chain.pattern.record(
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
        // Время показа: чуть раньше ожидаемого vsync, чтобы дрожание метки не переносило
        // кадр на следующий vsync (см. `present_time`).
        let request = PresentRequest {
            desired_present_time: Some(
                self.present_time
                    .desired_present_time(timing.expected_present, self.measured_refresh_period.get()),
            ),
        };
        match chain.target.present(frame, request) {
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
                    self.rebuild_pattern()?;
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

    /// Новый пайплайн под новый формат. Старый уничтожается, когда цель дождалась СВОИХ сабмитов
    /// (fence'ы кольца, с таймаутом): `vkDeviceWaitIdle` на весь GPU не нужен и не ограничен по
    /// времени. Показы на пайплайн не ссылаются, поэтому их ждать не требуется.
    fn rebuild_pattern(&mut self) -> Result<(), BackendError> {
        let chain = self.chain.as_mut().ok_or_else(|| chain_missing("rebuild_pattern"))?;
        let format = chain
            .target
            .info()
            .map_err(|error| classify_swapchain_error("display_target_info", &error).1)?
            .format;
        chain
            .target
            .wait_for_submitted_frames()
            .map_err(|error| classify_swapchain_error("wait_for_submitted_frames_before_pattern_rebuild", &error).1)?;
        chain.pattern = TestPattern::new(&self.context, format)
            .map_err(|error| gpu_backend_error("rebuild_test_pattern", &error))?;
        Ok(())
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
            };
        };
        let ChainRelease { surface, detach_error } = chain.release();
        ReleaseReport {
            surface: reclaim_window(surface, held_surface),
            error: detach_error.map(|error| gpu_backend_error("detach_display_target", &error)),
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
