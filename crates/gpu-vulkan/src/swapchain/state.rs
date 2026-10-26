//! Состояние ЖИВОЙ цели показа: surface, swapchain, кольцо кадров и счётчики.
//!
//! Отделено от фасада [`super::target::DisplayTarget`], потому что у фасада есть `Drop`, а нам нужно
//! уметь разобрать состояние по частям (вернуть окно владельцу, а при зависшем GPU — намеренно
//! «утечь» Vulkan-объекты, а не уничтожать занятое). Тип без `Drop` это позволяет.
//!
//! Порядок полей = порядок уничтожения при обычном `drop`: кольцо → ресурсы swapchain → `VkSurfaceKHR` →
//! окно платформы. Для `detach`/`Drop` фасада порядок и ожидание GPU выполняет [`TargetState::release`].

use std::time::{Duration, Instant};

use ash::vk;
use shell_contract::surface::SurfaceSize;

use super::SwapchainError;
use super::config::DisplayTargetConfig;
use super::display_timing;
use super::fault_injection::{FaultPlan, InjectedFault};
use super::frame::{AcquireOutcome, DisplayFrame, LayoutTransition, PresentOutcome, PresentRequest};
use super::policy::{PlanOutcome, PlanRequest, SwapchainPlan, plan_swapchain};
use super::present_fences::{PresentTracking, PresentsDone};
use super::resources::SwapchainResources;
use super::surface::{PresentationSurface, query_surface};
use super::target::{DisplayTargetInfo, RecreateOutcome, ZeroExtentReason};
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::frame_ring::{FrameRing, SlotIndex, SlotWait, timeout_nanos};
use crate::live_objects::OwnedSurface;

/// Как классифицирован результат `vkAcquireNextImageKHR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AcquireResult {
    /// Картинка получена (`suboptimal` — swapchain уже не идеален, но рисовать можно).
    Image { index: u32, suboptimal: bool },
    /// Картинка не освободилась за таймаут (`TIMEOUT`, а при нулевом таймауте — `NOT_READY`).
    TimedOut,
    /// Swapchain устарел.
    OutOfDate,
    /// Прочая ошибка (в том числе `SURFACE_LOST`).
    Failed(vk::Result),
}

/// Чистая классификация результата acquire (отдельно от GPU, чтобы тестировать все коды).
pub(super) fn classify_acquire(result: Result<(u32, bool), vk::Result>) -> AcquireResult {
    match result {
        Ok((index, suboptimal)) => AcquireResult::Image { index, suboptimal },
        Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => AcquireResult::TimedOut,
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => AcquireResult::OutOfDate,
        Err(other) => AcquireResult::Failed(other),
    }
}

/// Состояние живой цели показа.
pub(super) struct TargetState {
    ring: FrameRing,
    resources: SwapchainResources,
    surface: OwnedSurface,
    presentation: PresentationSurface,
    context: GpuContext,
    config: DisplayTargetConfig,
    /// Номер следующего кадра (циклически по `u32`); он же `present_id`.
    next_frame_index: u32,
    /// Номер кадра, который выдан и ещё не завершён (`present`/`abandon_frame`).
    outstanding_frame: Option<u32>,
    /// Swapchain надо пересоздать (устарел, `SUBOPTIMAL`, неудачное пересоздание).
    needs_recreate: bool,
    /// Поколение swapchain: растёт при каждом пересоздании; кадр старого поколения принять нельзя.
    generation: u64,
    recreation_count: u64,
    /// Как узнаём, что показы закончены (fence'ы показа или запасное ожидание простоя устройства).
    present_tracking: PresentTracking,
    /// Семафор «картинка получена» какого-то слота остался сигнальным без ожидающего сабмита
    /// (кадр сорвался сразу после acquire): при ближайшем пересоздании семафоры заменяются.
    acquire_semaphores_tainted: bool,
    /// Взведённые тестовые швы отказов (в боевых сборках пусты).
    faults: FaultPlan,
}

/// Результат создания частей до сборки состояния.
pub(super) struct BuiltParts {
    pub(super) surface: OwnedSurface,
    pub(super) resources: SwapchainResources,
    pub(super) ring: FrameRing,
}

impl TargetState {
    /// Создаёт все части: `VkSurfaceKHR` → swapchain → кольцо кадров. Окно не трогает (оно у вызывающего).
    pub(super) fn build_parts(
        context: &GpuContext,
        presentation: &PresentationSurface,
        config: &DisplayTargetConfig,
    ) -> Result<BuiltParts, SwapchainError> {
        let surface = presentation.create_vulkan_surface(context)?;
        let query = query_surface(context, &surface)?;
        let support = super::policy::SurfaceSupport::from_vk(&query.capabilities);
        let request = PlanRequest {
            color_depth: config.color_depth,
            requested_size: config.requested_size,
        };
        let plan = match plan_swapchain(&support, &query.formats, &request)? {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::ZeroExtent => return Err(SwapchainError::ZeroExtent),
        };
        let resources = SwapchainResources::create(context, &surface, &plan, None)?;
        let ring = FrameRing::new(context, config.frames_in_flight, resources.image_count())?;
        Ok(BuiltParts {
            surface,
            resources,
            ring,
        })
    }

    /// Собирает состояние из готовых частей.
    pub(super) fn assemble(
        context: &GpuContext,
        presentation: PresentationSurface,
        config: DisplayTargetConfig,
        parts: BuiltParts,
    ) -> Self {
        let present_tracking = PresentTracking::for_context(context, config.present_fence_capacity);
        Self {
            ring: parts.ring,
            resources: parts.resources,
            surface: parts.surface,
            presentation,
            context: context.clone(),
            config,
            next_frame_index: 0,
            outstanding_frame: None,
            needs_recreate: false,
            generation: 0,
            recreation_count: 0,
            present_tracking,
            acquire_semaphores_tainted: false,
            faults: FaultPlan::default(),
        }
    }

    /// Взводит тестовый шов отказа.
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn inject_fault(&mut self, fault: InjectedFault) {
        if fault == InjectedFault::WithoutPresentFences {
            self.present_tracking = PresentTracking::DeviceWideWait;
        } else {
            self.faults.arm(fault);
        }
    }

    /// Отслеживаются ли показы fence'ами (иначе освобождение упирается в `vkDeviceWaitIdle` без таймаута).
    pub(super) fn tracks_presents_with_fences(&self) -> bool {
        self.present_tracking.is_fence_based()
    }

    /// Сколько показов ждут сигнала fence'а (тесты учёта).
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn pending_present_count(&self) -> usize {
        self.present_tracking.pending_count()
    }

    /// Был ли текущий swapchain уже retired (тесты порядка пересоздания).
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn current_swapchain_is_retired(&self) -> bool {
        self.resources.is_retired()
    }

    /// Снимок параметров цели.
    pub(super) fn info(&self) -> DisplayTargetInfo {
        let plan = self.resources.plan();
        DisplayTargetInfo {
            format: plan.surface_format.format,
            color_space: plan.surface_format.color_space,
            extent: plan.extent,
            logical_extent: plan.logical_extent(),
            transform: plan.transform,
            image_count: self.resources.image_count(),
            composite_alpha: plan.composite_alpha,
            present_mode: plan.present_mode,
            frames_in_flight: self.ring.frames_in_flight(),
            recreation_count: self.recreation_count,
            display_timing_supported: display_timing::is_supported(&self.context),
        }
    }

    fn plan(&self) -> &SwapchainPlan {
        self.resources.plan()
    }

    pub(super) fn refresh_cycle(&self) -> Result<Option<Duration>, SwapchainError> {
        display_timing::refresh_cycle(&self.context, self.resources.swapchain_handle())
    }

    pub(super) fn drain_past_presentations(&self) -> Result<Vec<display_timing::PastPresentation>, SwapchainError> {
        display_timing::drain_past_presentations(&self.context, self.resources.swapchain_handle())
    }

    /// Выдаёт следующий кадр: ждёт слот, получает картинку, начинает запись и переводит картинку в раскладку рисования.
    pub(super) fn acquire_frame(&mut self) -> Result<AcquireOutcome, SwapchainError> {
        if let Some(frame_index) = self.outstanding_frame {
            return Err(GpuError::InvalidState {
                what: format!("кадр {frame_index} выдан и не завершён (present/abandon_frame)"),
            }
            .into());
        }
        if self.needs_recreate {
            return Ok(AcquireOutcome::NeedsRecreate);
        }

        // Единый бюджет ожидания: слот (fence) + acquire не должны вместе превысить acquire_timeout.
        let timeout = self.config.acquire_timeout;
        let started = Instant::now();
        let slot = match self.ring.wait_for_next_slot(timeout)? {
            SlotWait::Ready(slot) => slot,
            SlotWait::TimedOut => return Ok(AcquireOutcome::TimedOut),
        };
        let remaining = timeout.saturating_sub(started.elapsed());

        let loader = self.context.swapchain_loader()?;
        // SAFETY: swapchain и семафор слота живы; семафор не ждёт никто другой (fence слота сигнален,
        // значит прошлый сабмит, ждавший его, завершён); fence acquire не используется.
        let acquired = unsafe {
            loader.acquire_next_image(
                self.resources.swapchain_handle(),
                timeout_nanos(remaining),
                self.ring.image_acquired_semaphore(slot),
                vk::Fence::null(),
            )
        };
        let (image_index, acquired_suboptimal) = match classify_acquire(acquired) {
            AcquireResult::Image { index, suboptimal } => (index, suboptimal),
            AcquireResult::TimedOut => return Ok(AcquireOutcome::TimedOut),
            AcquireResult::OutOfDate => {
                self.needs_recreate = true;
                return Ok(AcquireOutcome::NeedsRecreate);
            }
            AcquireResult::Failed(code) => return Err(SwapchainError::from_vulkan(code, "vkAcquireNextImageKHR")),
        };

        // С этого момента семафор слота сигнален и картинка занята.
        let frame_index = self.next_frame_index;
        self.next_frame_index = self.next_frame_index.wrapping_add(1);
        self.outstanding_frame = Some(frame_index);

        let (image, view, command_buffer) = match self.begin_acquired_frame(slot, image_index) {
            Ok(parts) => parts,
            Err(error) => {
                self.abort_frame_after_acquire();
                return Err(error.into());
            }
        };
        LayoutTransition::AcquiredToColorAttachment.record(self.context.device(), command_buffer, image);

        let plan = self.plan();
        Ok(AcquireOutcome::Ready(DisplayFrame {
            slot,
            image_index,
            image,
            command_buffer,
            pattern_target: crate::test_pattern::PatternTarget {
                view,
                extent: plan.extent,
            },
            format: plan.surface_format.format,
            transform: plan.transform,
            frame_index,
            swapchain_generation: self.generation,
            acquired_suboptimal,
            finished: false,
        }))
    }

    /// Всё, что нужно сделать с уже полученной картинкой до первой записи: найти её и начать командный буфер.
    fn begin_acquired_frame(
        &mut self,
        slot: SlotIndex,
        image_index: u32,
    ) -> Result<(vk::Image, vk::ImageView, vk::CommandBuffer), GpuError> {
        if self.faults.trigger(InjectedFault::AcquireFailsAfterImageAcquired) {
            return Err(GpuError::InvalidState {
                what: "внедрённый отказ после acquire".to_owned(),
            });
        }
        let (image, view) = self.resources.image(image_index)?;
        let command_buffer = self.ring.begin_recording(slot)?;
        Ok((image, view, command_buffer))
    }

    /// Приводит состояние в порядок, когда кадр сорвался ПОСЛЕ успешного `vkAcquireNextImageKHR`.
    ///
    /// Что осталось «висеть»: картинка занята (acquired), а семафор слота сигнален и его никто не
    /// ждёт. Показать такую картинку нельзя (содержимое и раскладка не определены, а сабмит,
    /// который бы ждал семафор, не состоялся), вернуть её без `VK_EXT_swapchain_maintenance1` тоже
    /// нельзя. Спецификация-корректный выход — отказаться от этого swapchain: картинка, выданная
    /// и не показанная, не мешает его уничтожению. Поэтому: кадр больше не «выдан» (иначе каждый
    /// следующий acquire и recreate вечно получали бы `InvalidState`), swapchain помечен
    /// устаревшим, а сигнальные семафоры acquire будут заменены при ближайшем `recreate`.
    fn abort_frame_after_acquire(&mut self) {
        tracing::error!(
            target: "fasticamera::gpu::swapchain",
            "кадр сорвался после acquire: swapchain помечен устаревшим, семафоры acquire будут заменены при пересоздании"
        );
        self.outstanding_frame = None;
        self.needs_recreate = true;
        self.acquire_semaphores_tainted = true;
    }

    /// Завершает кадр: переводит картинку в раскладку показа, отправляет команды и ставит картинку в очередь показа.
    pub(super) fn present(
        &mut self,
        mut frame: DisplayFrame,
        request: PresentRequest,
    ) -> Result<PresentOutcome, SwapchainError> {
        // Кадр считается завершённым в любом случае: дальше ответственность — у состояния цели.
        frame.finished = true;
        if frame.swapchain_generation != self.generation || self.outstanding_frame != Some(frame.frame_index) {
            return Err(GpuError::InvalidState {
                what: format!(
                    "кадр {} не принадлежит текущему поколению swapchain или уже завершён",
                    frame.frame_index
                ),
            }
            .into());
        }

        LayoutTransition::ColorAttachmentToPresent.record(self.context.device(), frame.command_buffer, frame.image);
        self.ring.end_and_submit(frame.slot, frame.image_index)?;
        // Сабмит принят: кадр больше не «висит», даже если сам показ ниже вернёт ошибку.
        self.outstanding_frame = None;

        let outcome = self.queue_present(&frame, request)?;
        if frame.acquired_suboptimal {
            self.needs_recreate = true;
        }
        Ok(match outcome {
            PresentOutcome::Presented { present_id } if frame.acquired_suboptimal => {
                PresentOutcome::Suboptimal { present_id }
            }
            other => other,
        })
    }

    /// `vkQueuePresentKHR` с данными display timing (если расширение включено).
    fn queue_present(
        &mut self,
        frame: &DisplayFrame,
        request: PresentRequest,
    ) -> Result<PresentOutcome, SwapchainError> {
        let loader = self.context.swapchain_loader()?;
        let wait_semaphores = [self.ring.render_finished_semaphore(frame.image_index)?];
        let swapchains = [self.resources.swapchain_handle()];
        let image_indices = [frame.image_index];
        let present_times = [display_timing::present_time_entry(
            frame.frame_index,
            request.desired_present_time,
        )];
        let mut times_info = vk::PresentTimesInfoGOOGLE::default().times(&present_times);
        let present_fence = self.present_tracking.begin_present()?;
        let present_fences = present_fence.map(|fence| [fence]);
        let mut fence_info = vk::SwapchainPresentFenceInfoEXT::default();
        if let Some(fences) = &present_fences {
            fence_info = fence_info.fences(fences);
        }
        let mut present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        if present_fences.is_some() {
            present_info = present_info.push_next(&mut fence_info);
        }
        if display_timing::is_supported(&self.context) {
            present_info = present_info.push_next(&mut times_info);
        }

        // SAFETY: очередь под замком контекста; swapchain/семафор живы; семафор будет сигнален сабмитом,
        // который мы только что отправили; fence несигнальный и свободен (пул); все массивы живут до конца вызова.
        let result = self
            .context
            .with_queue(|queue| unsafe { loader.queue_present(queue, &present_info) });
        match result {
            Ok(false) => Ok(PresentOutcome::Presented {
                present_id: frame.frame_index,
            }),
            Ok(true) => {
                self.needs_recreate = true;
                Ok(PresentOutcome::Suboptimal {
                    present_id: frame.frame_index,
                })
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.needs_recreate = true;
                self.present_tracking.present_failed(present_fence);
                Ok(PresentOutcome::NeedsRecreate)
            }
            Err(code) => {
                self.present_tracking.present_failed(present_fence);
                Err(SwapchainError::from_vulkan(code, "vkQueuePresentKHR"))
            }
        }
    }

    /// Ждёт, пока GPU и показ закончат со всем, что принадлежит цели.
    ///
    /// 1. fence'ы кольца кадров (сабмиты) — с таймаутом;
    /// 2. fence'ы показа (`VK_EXT_swapchain_maintenance1`) — с остатком того же бюджета;
    /// 3. ТОЛЬКО если п. 2 недоступен (расширения нет или исход какого-то показа неизвестен) —
    ///    `vkDeviceWaitIdle`. Он без таймаута: это известный пробел ПК-стенда без расширения,
    ///    на телефоне расширение есть и поток рендера не блокируется надолго.
    ///
    /// Общий бюджет — `teardown_timeout`, а не по бюджету на каждый шаг.
    fn wait_gpu_done(&mut self) -> Result<(), GpuError> {
        if self.faults.trigger(InjectedFault::StuckGpu) {
            return Err(GpuError::Timeout {
                step: "внедрённый отказ: GPU не освобождается",
            });
        }
        let budget = self.config.teardown_timeout;
        let started = Instant::now();
        self.ring.wait_for_idle(budget)?;
        let remaining = budget.saturating_sub(started.elapsed());
        match self.present_tracking.wait_presents_done(remaining)? {
            PresentsDone::Confirmed => Ok(()),
            PresentsDone::DeviceWideWaitRequired => {
                self.context.wait_idle()?;
                self.present_tracking.settle_after_device_idle()
            }
        }
    }

    /// Ждёт только сабмиты цели (fence'ы кольца), не весь GPU и не показы. Нужно перед
    /// уничтожением того, на что ссылаются записанные командные буферы (пайплайн рисования):
    /// показы на пайплайн не ссылаются. Ограничено `teardown_timeout`.
    pub(super) fn wait_for_submitted_frames(&self) -> Result<(), GpuError> {
        self.ring.wait_for_idle(self.config.teardown_timeout)
    }

    /// Пересоздаёт swapchain (`oldSwapchain`) под новый размер/поворот.
    pub(super) fn recreate(&mut self, size: Option<SurfaceSize>) -> Result<RecreateOutcome, SwapchainError> {
        if let Some(frame_index) = self.outstanding_frame {
            return Err(GpuError::InvalidState {
                what: format!("нельзя пересоздать swapchain, пока кадр {frame_index} не завершён"),
            }
            .into());
        }
        if size.is_some() {
            self.config.requested_size = size;
        }
        self.wait_gpu_done()?;
        // Дальше старый swapchain может стать «retired» даже при неудаче создания нового (спецификация):
        // до успеха считаем, что пересоздание ещё нужно.
        self.needs_recreate = true;

        let query = query_surface(&self.context, &self.surface)?;
        let support = super::policy::SurfaceSupport::from_vk(&query.capabilities);
        let request = PlanRequest {
            color_depth: self.config.color_depth,
            requested_size: self.config.requested_size,
        };
        let plan = match plan_swapchain(&support, &query.formats, &request)? {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::ZeroExtent => return Ok(RecreateOutcome::Deferred(ZeroExtentReason::ZeroExtent)),
        };

        let previous_format = self.plan().surface_format.format;
        // `create` помечает старый swapchain retired (он становится таким при любом исходе). Если
        // что-то ниже сорвётся, новый swapchain будет отброшен, а retired старый останется текущим:
        // в следующую попытку он уже не пойдёт как `oldSwapchain` (передастся null).
        let new_resources = SwapchainResources::create(&self.context, &self.surface, &plan, Some(&mut self.resources))?;
        if self.faults.trigger(InjectedFault::RecreateFailsAfterSwapchainCreated) {
            return Err(GpuError::InvalidState {
                what: "внедрённый отказ после создания swapchain".to_owned(),
            }
            .into());
        }
        self.ring.recreate_render_finished(new_resources.image_count())?;
        // Заменяем и роняем старое поколение: представления, затем swapchain.
        drop(std::mem::replace(&mut self.resources, new_resources));
        if self.acquire_semaphores_tainted {
            // Старый swapchain (источник сигнала acquire) уничтожен: выбрасываем сигнальные семафоры.
            self.ring.replace_image_acquired_semaphores()?;
            self.acquire_semaphores_tainted = false;
        }

        self.generation += 1;
        self.recreation_count += 1;
        self.needs_recreate = false;
        Ok(RecreateOutcome::Recreated {
            format_changed: previous_format != plan.surface_format.format,
        })
    }

    /// Освобождает цель: ждёт GPU, уничтожает swapchain, затем `VkSurfaceKHR`, возвращает окно.
    ///
    /// Если дождаться GPU не удалось (таймаут, потеря устройства), Vulkan-объекты НЕ уничтожаются
    /// (уничтожать занятое — UB), а намеренно утекают; окно возвращается в ошибке.
    pub(super) fn release(mut self) -> Result<PresentationSurface, (GpuError, PresentationSurface)> {
        let wait = self.wait_gpu_done();
        let Self {
            ring,
            resources,
            surface,
            presentation,
            ..
        } = self;
        match wait {
            Ok(()) => {
                // Порядок: swapchain (с представлениями) → surface → кольцо; окно возвращаем последним.
                drop(resources);
                drop(surface);
                drop(ring);
                Ok(presentation)
            }
            Err(error) => {
                tracing::error!(
                    target: "fasticamera::gpu::swapchain",
                    %error,
                    "GPU не освободился: swapchain, surface и кольцо кадров намеренно НЕ уничтожаются (утечка)"
                );
                std::mem::forget(resources);
                std::mem::forget(surface);
                std::mem::forget(ring);
                Err((error, presentation))
            }
        }
    }

    /// Нужен ли `recreate` (для диагностики и тестов).
    pub(super) fn needs_recreate(&self) -> bool {
        self.needs_recreate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_codes_are_classified_without_merging_timeout_and_failure() {
        assert_eq!(
            classify_acquire(Ok((2, false))),
            AcquireResult::Image {
                index: 2,
                suboptimal: false
            }
        );
        assert_eq!(
            classify_acquire(Ok((0, true))),
            AcquireResult::Image {
                index: 0,
                suboptimal: true
            }
        );
        assert_eq!(classify_acquire(Err(vk::Result::TIMEOUT)), AcquireResult::TimedOut);
        assert_eq!(classify_acquire(Err(vk::Result::NOT_READY)), AcquireResult::TimedOut);
        assert_eq!(
            classify_acquire(Err(vk::Result::ERROR_OUT_OF_DATE_KHR)),
            AcquireResult::OutOfDate
        );
        assert_eq!(
            classify_acquire(Err(vk::Result::ERROR_SURFACE_LOST_KHR)),
            AcquireResult::Failed(vk::Result::ERROR_SURFACE_LOST_KHR)
        );
    }
}
