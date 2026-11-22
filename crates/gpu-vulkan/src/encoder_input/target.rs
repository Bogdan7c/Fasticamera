//! Фасад [`EncoderInputTarget`]: владелец поверхности кодера, swapchain и двух семафоров показа.
//!
//! Состояния ([`Phase`]) — единственный источник правды о том, что можно с целью делать:
//!
//! ```text
//! Fresh ──acquire+submit ok──▶ InFlight{work} ──detach(proof)──▶ (поверхность возвращена)
//!   │  ▲                            ▲
//!   │  └─ ошибка ДО acquire/отправки (повтор допустим)
//!   └──acquire ok, submit сорвался──▶ AcquiredUnsubmitted (повтор нельзя; разбор — с ожиданием простоя)
//! ```

use ash::vk;

use super::blit::record_present_blit;
use super::plan::{EncoderSwapchainPlan, plan_encoder_swapchain};
use super::{EncoderTargetConfig, EncoderTargetError, SubmittedButNotPresented};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::frame_ring::timeout_nanos;
use crate::live_objects::{OwnedSemaphore, OwnedSurface, OwnedSwapchain};
use crate::one_shot_queue::{CompletedWork, OneShotSubmitter, SemaphoreSync, SubmittedWork, WorkId};
use crate::rgba8_image::Rgba8StorageImage;
use crate::swapchain::present_fences::{PresentTracking, PresentsDone, PresentsPoll};
use crate::swapchain::surface::{PresentationSurface, query_surface};
use crate::swapchain::{SurfaceSupport, SwapchainError, swapchain_step};

/// Параметры созданного swapchain (для диагностики и решений потребителя).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderInputInfo {
    /// Формат картинок swapchain.
    pub format: vk::Format,
    /// Цветовое пространство (всегда `SRGB_NONLINEAR`).
    pub color_space: vk::ColorSpaceKHR,
    /// Размер картинок = размер снимка.
    pub extent: vk::Extent2D,
    /// Фактический usage картинок.
    pub usage: vk::ImageUsageFlags,
    /// Сколько картинок создал драйвер.
    pub image_count: usize,
    /// `preTransform` (поворот НЕ применён; кодер сам не вращает).
    pub transform: vk::SurfaceTransformFlagsKHR,
}

/// Фаза цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Ничего не отправлялось, картинка не занята: разбор безопасен без ожиданий.
    Fresh,
    /// Картинка получена (acquire), но отправка сорвалась: семафор acquire «висит»; показ невозможен.
    AcquiredUnsubmitted,
    /// Копирование отправлено (и, возможно, показано); ждём доказательство завершения этой работы.
    InFlight { work: WorkId },
}

/// Итог опроса «GPU и показ закончили с ресурсами цели» (внутренний).
enum GpuDone {
    Done,
    Pending,
    DeviceWideWaitRequired,
}

/// Ресурсы Vulkan цели. Порядок полей = порядок уничтожения: fence'ы показа → swapchain → семафоры → surface.
struct Resources {
    present_tracking: PresentTracking,
    swapchain: OwnedSwapchain,
    acquire_semaphore: OwnedSemaphore,
    render_finished: OwnedSemaphore,
    /// Держим ради порядка уничтожения (после swapchain); напрямую не читается.
    _surface: OwnedSurface,
    images: Vec<vk::Image>,
}

/// Всё, что цель держит до разбора.
struct Inner {
    resources: Resources,
    // Окно живёт дольше `VkSurfaceKHR` (поле после `resources`), возвращается вызывающему последним.
    presentation: PresentationSurface,
    info: EncoderInputInfo,
    config: EncoderTargetConfig,
    phase: Phase,
}

/// Одноразовая цель кодера (см. модуль `encoder_input`).
pub struct EncoderInputTarget {
    context: GpuContext,
    inner: Option<Inner>,
}

impl std::fmt::Debug for EncoderInputTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Some(inner) => write!(
                formatter,
                "EncoderInputTarget({}x{}, {:?})",
                inner.info.extent.width, inner.info.extent.height, inner.phase
            ),
            None => write!(formatter, "EncoderInputTarget(разобрана)"),
        }
    }
}

/// `attach` не удался: поверхность ВОЗВРАЩАЕТСЯ (на Android окно надо вернуть оболочке).
#[derive(Debug)]
pub struct EncoderAttachError {
    /// Что пошло не так.
    pub error: EncoderTargetError,
    /// Поверхность, которую вызывающий отдавал.
    pub surface: PresentationSurface,
}

impl std::fmt::Display for EncoderAttachError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "цель кодера не создана: {}", self.error)
    }
}

impl std::error::Error for EncoderAttachError {}

/// Ошибка `present_once`: различаем «ничего не ушло на GPU» и «копирование ушло, показ сорвался».
#[derive(Debug, thiserror::Error)]
pub enum PresentOnceError {
    /// Отправки не было: работы нет. Если фаза осталась `Fresh`, `present_once` можно повторить.
    #[error("показ не начат: {0}")]
    NotSubmitted(EncoderTargetError),
    /// Копирование отправлено, показ сорвался: токен работы нужен для безопасного разбора.
    #[error("копирование отправлено, но показ сорвался: {}", .0.error)]
    SubmittedButNotPresented(Box<SubmittedButNotPresented>),
}

/// Итог НЕБЛОКИРУЮЩЕГО разбора цели ([`EncoderInputTarget::try_detach`]).
///
/// Три исхода не сливаются в один `bool`: вызывающий по-разному живёт с «ещё не готово» (спросить на следующем
/// кадре) и «опросом не дождаться» (нужна политика владельца: отложить, оставить в утечку или остановить
/// устройство в безопасном месте).
#[derive(Debug)]
pub enum DetachProgress {
    /// Показы закончены: цель разобрана, поверхность кодера возвращена.
    Detached(PresentationSurface),
    /// Fence'ы показа ещё не просигналили (`VK_EXT_swapchain_maintenance1`): цель возвращена нетронутой,
    /// спросите позже.
    PresentsPending(Box<EncoderInputTarget>),
    /// Окончание показа опросом узнать нельзя (нет `VK_EXT_swapchain_maintenance1`, пробел покрытия fence'ов или
    /// сорванная отправка с висящим семафором acquire): безопасно разобрать цель можно только остановкой
    /// устройства ([`EncoderInputTarget::detach`]). Цель возвращена нетронутой; решение (когда и где ждать, либо
    /// оставить в учтённой утечке) за владельцем.
    DeviceWideWaitRequired(Box<EncoderInputTarget>),
}

/// Разбор цели не состоялся.
#[derive(Debug, thiserror::Error)]
pub enum EncoderDetachError {
    /// Доказательство относится к другой работе. Цель ВОЗВРАЩЕНА нетронутой — повторите с верным.
    #[error("доказательство завершения относится к другой работе")]
    ProofMismatch {
        /// Цель, не тронутая разбором.
        target: Box<EncoderInputTarget>,
    },
    /// Копирование в полёте, а вызван разбор без доказательства. Цель возвращена нетронутой.
    #[error("работа в полёте: нужен `detach` с доказательством завершения")]
    ProofRequired {
        /// Цель, не тронутая разбором.
        target: Box<EncoderInputTarget>,
    },
    /// GPU не освободился (таймаут/потеря устройства): Vulkan-объекты НАМЕРЕННО не уничтожены (утечка),
    /// поверхность возвращена.
    #[error("GPU не освободился, объекты оставлены (утечка): {error}")]
    GpuNotIdle {
        /// Причина.
        error: GpuError,
        /// Поверхность кодера (окно вернуть оболочке).
        surface: PresentationSurface,
    },
    /// Цель уже разобрана (защитный вариант: `detach` принимает `self`, повторить его нельзя).
    #[error("цель уже разобрана")]
    AlreadyReleased,
}

impl EncoderInputTarget {
    /// Создаёт swapchain на поверхности кодера для снимка `extent` с настройками по умолчанию.
    ///
    /// При ошибке поверхность возвращается внутри [`EncoderAttachError`].
    pub fn attach(
        context: &GpuContext,
        surface: PresentationSurface,
        extent: vk::Extent2D,
    ) -> Result<Self, EncoderAttachError> {
        Self::attach_with_config(context, surface, extent, EncoderTargetConfig::default())
    }

    /// То же, с явными настройками.
    pub fn attach_with_config(
        context: &GpuContext,
        surface: PresentationSurface,
        extent: vk::Extent2D,
        config: EncoderTargetConfig,
    ) -> Result<Self, EncoderAttachError> {
        match build_resources(context, &surface, extent, &config) {
            Ok((resources, info)) => Ok(Self {
                context: context.clone(),
                inner: Some(Inner {
                    resources,
                    presentation: surface,
                    info,
                    config,
                    phase: Phase::Fresh,
                }),
            }),
            Err(error) => Err(EncoderAttachError { error, surface }),
        }
    }

    /// Параметры swapchain; `None` — цель уже разобрана.
    pub fn info(&self) -> Option<EncoderInputInfo> {
        self.inner.as_ref().map(|inner| inner.info)
    }

    /// Копирует `image` в картинку swapchain и показывает её ровно один раз (CPU не ждёт GPU).
    ///
    /// Контракты вызывающего: `image` того же размера, что цель, и сейчас в
    /// [`Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT`] (раскладка не меняется); барьер «запись → чтение копией»
    /// уже записан раньше в той же очереди. Возвращает токен копирования: `submitter.poll(&work)` →
    /// `Completed` — и можно звать [`Self::detach`].
    pub fn present_once(
        &mut self,
        submitter: &mut OneShotSubmitter,
        image: &Rgba8StorageImage,
    ) -> Result<SubmittedWork, PresentOnceError> {
        let context = self.context.clone();
        let Some(inner) = self.inner.as_mut() else {
            return Err(PresentOnceError::NotSubmitted(EncoderTargetError::AlreadyPresented));
        };
        inner.present_once(&context, submitter, image)
    }

    /// Разбирает цель после завершения копирования и показа: возвращает поверхность кодера.
    ///
    /// `proof` обязан относиться к работе, возвращённой `present_once` ЭТОЙ цели (иначе `ProofMismatch` и цель
    /// возвращается нетронутой). Дальше ждём окончания показа с таймаутом `teardown_timeout`.
    pub fn detach(self, proof: &CompletedWork) -> Result<PresentationSurface, EncoderDetachError> {
        let expected = self.inner.as_ref().map(|inner| inner.phase);
        match expected {
            Some(Phase::InFlight { work }) if work != proof.id() => {
                Err(EncoderDetachError::ProofMismatch { target: Box::new(self) })
            }
            _ => self.release(),
        }
    }

    /// Разбирает цель, в которую ничего не отправляли (или отправка сорвалась до GPU). Если копирование
    /// в полёте — `ProofRequired` (цель возвращается).
    pub fn detach_unpresented(self) -> Result<PresentationSurface, EncoderDetachError> {
        match self.inner.as_ref().map(|inner| inner.phase) {
            Some(Phase::InFlight { .. }) => Err(EncoderDetachError::ProofRequired { target: Box::new(self) }),
            _ => self.release(),
        }
    }

    /// Неблокирующий вариант [`Self::detach`]: НЕ ждёт GPU и показ.
    ///
    /// Проверка доказательства — как у `detach` (`ProofMismatch` возвращает цель). Дальше опрашиваются fence'ы
    /// показа: закончены — цель разбирается без ожидания, не закончены — [`DetachProgress::PresentsPending`],
    /// опросом не узнать — [`DetachProgress::DeviceWideWaitRequired`]. Поток рендера зовёт этот метод на каждом
    /// кадре, пока показ не закончится.
    pub fn try_detach(self, proof: &CompletedWork) -> Result<DetachProgress, EncoderDetachError> {
        let expected = self.inner.as_ref().map(|inner| inner.phase);
        match expected {
            Some(Phase::InFlight { work }) if work != proof.id() => {
                Err(EncoderDetachError::ProofMismatch { target: Box::new(self) })
            }
            _ => self.poll_release(),
        }
    }

    /// Неблокирующий вариант [`Self::detach_unpresented`]: цель, в которую ничего не отправляли (или отправка
    /// сорвалась до GPU). Копирование в полёте — `ProofRequired`. Цель после сорванной отправки (картинка
    /// получена, семафор acquire висит) — [`DetachProgress::DeviceWideWaitRequired`].
    pub fn try_detach_unpresented(self) -> Result<DetachProgress, EncoderDetachError> {
        match self.inner.as_ref().map(|inner| inner.phase) {
            Some(Phase::InFlight { .. }) => Err(EncoderDetachError::ProofRequired { target: Box::new(self) }),
            _ => self.poll_release(),
        }
    }

    /// Тесты и швы отказов (`fault-injection`): имитирует устройство без `VK_EXT_swapchain_maintenance1` (показы
    /// fence'ами не отследить). Звать до `present_once`.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn force_device_wide_wait_for_test(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            inner.resources.present_tracking = PresentTracking::DeviceWideWait;
        }
    }

    fn poll_release(mut self) -> Result<DetachProgress, EncoderDetachError> {
        let Some(mut inner) = self.inner.take() else {
            return Err(EncoderDetachError::AlreadyReleased);
        };
        match inner.poll_gpu_done() {
            Ok(GpuDone::Done) => {
                let Inner {
                    resources,
                    presentation,
                    ..
                } = inner;
                // Порядок уничтожения — как в `release`: fence'ы показа → swapchain → семафоры → `VkSurfaceKHR`.
                drop(resources);
                Ok(DetachProgress::Detached(presentation))
            }
            Ok(GpuDone::Pending) => {
                self.inner = Some(inner);
                Ok(DetachProgress::PresentsPending(Box::new(self)))
            }
            Ok(GpuDone::DeviceWideWaitRequired) => {
                self.inner = Some(inner);
                Ok(DetachProgress::DeviceWideWaitRequired(Box::new(self)))
            }
            Err(error) => {
                tracing::error!(
                    target: "fasticamera::gpu::encoder_input",
                    %error,
                    "опрос показа не удался: swapchain, семафоры и surface кодера намеренно НЕ уничтожаются (утечка)"
                );
                let Inner {
                    resources,
                    presentation,
                    ..
                } = inner;
                std::mem::forget(resources);
                Err(EncoderDetachError::GpuNotIdle {
                    error,
                    surface: presentation,
                })
            }
        }
    }

    fn release(mut self) -> Result<PresentationSurface, EncoderDetachError> {
        let Some(mut inner) = self.inner.take() else {
            return Err(EncoderDetachError::AlreadyReleased);
        };
        match inner.wait_gpu_done(&self.context) {
            Ok(()) => {
                let Inner {
                    resources,
                    presentation,
                    ..
                } = inner;
                // Порядок: fence'ы показа → swapchain → семафоры → `VkSurfaceKHR`; окно — последним.
                drop(resources);
                Ok(presentation)
            }
            Err(error) => {
                tracing::error!(
                    target: "fasticamera::gpu::encoder_input",
                    %error,
                    "GPU не освободился: swapchain, семафоры и surface кодера намеренно НЕ уничтожаются (утечка)"
                );
                let Inner {
                    resources,
                    presentation,
                    ..
                } = inner;
                std::mem::forget(resources);
                Err(EncoderDetachError::GpuNotIdle {
                    error,
                    surface: presentation,
                })
            }
        }
    }
}

impl Drop for EncoderInputTarget {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else { return };
        if inner.phase == Phase::Fresh {
            // Ничего не отправлялось и картинка не занята: обычное уничтожение безопасно.
            return;
        }
        tracing::error!(
            target: "fasticamera::gpu::encoder_input",
            "цель кодера выброшена без detach после начала показа: Vulkan-объекты намеренно НЕ уничтожаются (утечка)"
        );
        // Окно (остальные поля `inner`) освобождается обычным порядком в конце функции: `VkSurfaceKHR` держит
        // собственную ссылку на него.
        let Inner { resources, .. } = inner;
        std::mem::forget(resources);
    }
}

impl Inner {
    fn present_once(
        &mut self,
        context: &GpuContext,
        submitter: &mut OneShotSubmitter,
        image: &Rgba8StorageImage,
    ) -> Result<SubmittedWork, PresentOnceError> {
        if self.phase != Phase::Fresh {
            return Err(PresentOnceError::NotSubmitted(EncoderTargetError::AlreadyPresented));
        }
        if image.extent().width != self.info.extent.width || image.extent().height != self.info.extent.height {
            return Err(PresentOnceError::NotSubmitted(
                EncoderTargetError::ImageExtentMismatch {
                    target: self.info.extent,
                    image: image.extent(),
                },
            ));
        }
        let image_index = self.acquire(context).map_err(PresentOnceError::NotSubmitted)?;
        // С этого момента семафор acquire сигнален/в процессе: без отправки цель не повторить.
        self.phase = Phase::AcquiredUnsubmitted;

        let Some(destination) = self.resources.images.get(image_index as usize).copied() else {
            return Err(PresentOnceError::NotSubmitted(EncoderTargetError::Gpu(
                GpuError::InvalidState {
                    what: format!(
                        "драйвер вернул индекс картинки {image_index}, а в swapchain их {}",
                        self.resources.images.len()
                    ),
                },
            )));
        };
        let waits = [self.resources.acquire_semaphore.handle()];
        let signals = [self.resources.render_finished.handle()];
        let extent = self.info.extent;
        let work = submitter
            .submit_with_sync(
                Vec::new(),
                &SemaphoreSync {
                    waits: &waits,
                    signals: &signals,
                    ..SemaphoreSync::default()
                },
                |command_buffer| {
                    record_present_blit(context, command_buffer, image, destination, extent);
                    Ok::<(), GpuError>(())
                },
            )
            .map_err(|error| PresentOnceError::NotSubmitted(error.into()))?;
        self.phase = Phase::InFlight { work: work.id() };

        match self.queue_present(context, image_index) {
            Ok(()) => Ok(work),
            Err(error) => Err(PresentOnceError::SubmittedButNotPresented(Box::new(
                SubmittedButNotPresented { work, error },
            ))),
        }
    }

    /// `vkAcquireNextImageKHR` с семафором acquire; бюджет — `acquire_timeout`.
    fn acquire(&self, context: &GpuContext) -> Result<u32, EncoderTargetError> {
        let loader = context.swapchain_loader()?;
        // SAFETY: swapchain и семафор живы; семафор никто не ждёт (цель одноразовая, acquire ещё не было);
        // fence не используется.
        let acquired = unsafe {
            loader.acquire_next_image(
                self.resources.swapchain.handle(),
                timeout_nanos(self.config.acquire_timeout),
                self.resources.acquire_semaphore.handle(),
                vk::Fence::null(),
            )
        };
        match acquired {
            // Suboptimal для одного кадра не важен: снимок показывается как есть.
            Ok((index, _suboptimal)) => Ok(index),
            Err(vk::Result::TIMEOUT | vk::Result::NOT_READY) => Err(EncoderTargetError::AcquireTimedOut),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Err(EncoderTargetError::SurfaceOutOfDate),
            Err(code) => Err(SwapchainError::from_vulkan(code, "vkAcquireNextImageKHR").into()),
        }
    }

    /// `vkQueuePresentKHR` с ожиданием семафора «рендер готов» и (если есть расширение) fence'ом показа.
    fn queue_present(&mut self, context: &GpuContext, image_index: u32) -> Result<(), EncoderTargetError> {
        let loader = context.swapchain_loader()?;
        let wait_semaphores = [self.resources.render_finished.handle()];
        let swapchains = [self.resources.swapchain.handle()];
        let image_indices = [image_index];
        let present_fence = self.resources.present_tracking.begin_present()?;
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
        // SAFETY: очередь под замком контекста; swapchain/семафор живы; семафор сигналит отправленная копия;
        // fence несигнальный и свободен (пул); массивы живут до конца вызова.
        let result = context.with_queue(|queue| unsafe { loader.queue_present(queue, &present_info) });
        match result {
            Ok(_suboptimal) => Ok(()),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.resources.present_tracking.present_failed(present_fence);
                Err(EncoderTargetError::SurfaceOutOfDate)
            }
            Err(code) => {
                self.resources.present_tracking.present_failed(present_fence);
                Err(SwapchainError::from_vulkan(code, "vkQueuePresentKHR").into())
            }
        }
    }

    /// То же, что [`Self::wait_gpu_done`], но без ожидания: спрашивает fence'ы показа.
    fn poll_gpu_done(&mut self) -> Result<GpuDone, GpuError> {
        match self.phase {
            Phase::Fresh => Ok(GpuDone::Done),
            // Семафор acquire ждёт неизвестно что, а доказательства нет: опросом не узнать.
            Phase::AcquiredUnsubmitted => Ok(GpuDone::DeviceWideWaitRequired),
            Phase::InFlight { .. } => Ok(match self.resources.present_tracking.poll_presents_done()? {
                PresentsPoll::Done => GpuDone::Done,
                PresentsPoll::Pending => GpuDone::Pending,
                PresentsPoll::DeviceWideWaitRequired => GpuDone::DeviceWideWaitRequired,
            }),
        }
    }

    /// Ждёт, пока GPU и показ закончат с ресурсами цели (порядок и пробелы — как у `DisplayTarget`).
    fn wait_gpu_done(&mut self, context: &GpuContext) -> Result<(), GpuError> {
        match self.phase {
            // Ничего не занято.
            Phase::Fresh => Ok(()),
            // Семафор acquire ждёт неизвестно что, а доказательства нет: только простой устройства
            // (без таймаута — известный пробел редкого пути сорванной отправки).
            Phase::AcquiredUnsubmitted => context.wait_idle(),
            Phase::InFlight { .. } => {
                match self
                    .resources
                    .present_tracking
                    .wait_presents_done(self.config.teardown_timeout)?
                {
                    PresentsDone::Confirmed => Ok(()),
                    PresentsDone::DeviceWideWaitRequired => {
                        context.wait_idle()?;
                        self.resources.present_tracking.settle_after_device_idle()
                    }
                }
            }
        }
    }
}

/// Создаёт `VkSurfaceKHR`, swapchain, семафоры и трекер показов; при ошибке всё созданное уничтожается RAII.
fn build_resources(
    context: &GpuContext,
    presentation: &PresentationSurface,
    extent: vk::Extent2D,
    config: &EncoderTargetConfig,
) -> Result<(Resources, EncoderInputInfo), EncoderTargetError> {
    let surface = presentation.create_vulkan_surface(context)?;
    let query = query_surface(context, &surface)?;
    let plan = plan_encoder_swapchain(&SurfaceSupport::from_vk(&query.capabilities), &query.formats, extent)?;
    ensure_blit_is_supported(context, &plan)?;

    let loader = context.swapchain_loader()?;
    let create_info = vk::SwapchainCreateInfoKHR::default()
        .surface(surface.handle())
        .min_image_count(plan.image_count)
        .image_format(plan.surface_format.format)
        .image_color_space(plan.surface_format.color_space)
        .image_extent(plan.extent)
        .image_array_layers(1)
        .image_usage(plan.usage)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(plan.transform)
        .composite_alpha(plan.composite_alpha)
        // FIFO есть у любой реализации (спецификация): запрашивать список режимов незачем.
        .present_mode(vk::PresentModeKHR::FIFO)
        .clipped(true);
    // SAFETY: surface принадлежит этому instance и жива; create_info ссылается на живые локальные данные.
    let raw_swapchain = swapchain_step(
        unsafe { loader.create_swapchain(&create_info, None) },
        "vkCreateSwapchainKHR(encoder_input)",
    )?;
    // Сразу берём во владение: любая ошибка ниже уничтожит swapchain RAII-ом.
    let swapchain = OwnedSwapchain::from_raw(context, raw_swapchain)?;
    // SAFETY: swapchain только что создан этим загрузчиком.
    let images = swapchain_step(
        unsafe { loader.get_swapchain_images(swapchain.handle()) },
        "vkGetSwapchainImagesKHR(encoder_input)",
    )?;
    let acquire_semaphore = create_binary_semaphore(context, "vkCreateSemaphore(encoder acquire)")?;
    let render_finished = create_binary_semaphore(context, "vkCreateSemaphore(encoder render_finished)")?;
    let info = EncoderInputInfo {
        format: plan.surface_format.format,
        color_space: plan.surface_format.color_space,
        extent: plan.extent,
        usage: plan.usage,
        image_count: images.len(),
        transform: plan.transform,
    };
    let resources = Resources {
        present_tracking: PresentTracking::for_context(context, config.present_fence_capacity),
        swapchain,
        acquire_semaphore,
        render_finished,
        _surface: surface,
        images,
    };
    Ok((resources, info))
}

/// blit пишет в картинку swapchain и читает RGBA8: у обоих форматов должны быть соответствующие возможности.
fn ensure_blit_is_supported(context: &GpuContext, plan: &EncoderSwapchainPlan) -> Result<(), EncoderTargetError> {
    let destination_ok = context
        .optimal_tiling_features(plan.surface_format.format)
        .contains(vk::FormatFeatureFlags::BLIT_DST);
    let source_ok = context
        .optimal_tiling_features(vk::Format::R8G8B8A8_UNORM)
        .contains(vk::FormatFeatureFlags::BLIT_SRC);
    if destination_ok && source_ok {
        Ok(())
    } else {
        Err(EncoderTargetError::BlitUnsupported {
            format: plan.surface_format.format,
        })
    }
}

fn create_binary_semaphore(context: &GpuContext, step: &'static str) -> Result<OwnedSemaphore, GpuError> {
    // SAFETY: устройство валидно; create_info — локальная структура.
    let handle = run_step(
        unsafe {
            context
                .device()
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        },
        step,
    )?;
    Ok(OwnedSemaphore::from_raw(context, handle))
}
