//! [`EncoderInputTarget`] — одноразовая цель «Vulkan рисует готовый снимок прямо во входную поверхность
//! кодера» (сессия 14, пакет P4a, решение 142: HEIF через `HeifWriter`).
//!
//! # Идея
//! У кодера (`HeifWriter`/`MediaCodec`) есть входная `Surface` (`ANativeWindow`). Vulkan создаёт на ней
//! swapchain, копирует готовый `Rgba8StorageImage` в картинку swapchain и показывает её ровно один раз —
//! кодер получает кадр как «буфер от продюсера». Копирования через CPU нет.
//!
//! ```text
//! attach(surface, extent) ─▶ present_once(submitter, image) ─▶ poll → Completed ─▶ detach(proof) ─▶ surface
//!   swapchain + 2 семафора    acquire → blit → present        (OneShotSubmitter)      вернули окно
//! ```
//!
//! # Решения (по зонду 00 `tools/device-probe/native/src/codec_surface/`)
//! * usage swapchain — `TRANSFER_DST` (обязателен: в него пишет blit) `+ COLOR_ATTACHMENT`, если surface его
//!   объявила (как в зонде: часть драйверов кодера требует именно эту пару); проверяется `supportedUsageFlags`;
//! * формат — 8-бит `R8G8B8A8_UNORM`/`B8G8R8A8_UNORM` + `SRGB_NONLINEAR`: sRGB-кодирование делает наш шейдер,
//!   аппаратное перекодирование не нужно (UNORM). Копирует `vkCmdBlitImage2` (а не copy): он пересобирает каналы
//!   при разном порядке RGBA/BGRA, copy переставил бы R и B;
//! * present mode — FIFO (обязателен по спецификации); число картинок — `minImageCount` (кадр один);
//! * `preTransform = currentTransform`: поворот НЕ применяется (кодер не вращает), значение отдаётся в
//!   [`EncoderInputInfo::transform`] — потребитель решает, нужен ли поворот самой картинки.
//!
//! # Инварианты
//! * цель владеет поверхностью от `attach` до `detach`; `VkSurfaceKHR` уничтожается после swapchain;
//! * показ ровно один (`AlreadyPresented`); ошибка ДО отправки оставляет цель пригодной для повтора;
//! * swapchain не уничтожается, пока не доказано завершение работы ([`CompletedWork`] именно этой работы) и
//!   не закончен показ (fence показа, `VK_EXT_swapchain_maintenance1`; без него — единственный известный
//!   пробел: `vkDeviceWaitIdle` без таймаута, как у `DisplayTarget`);
//! * ни одного бесконечного ожидания (acquire и разбор ограничены таймаутами конфигурации).

mod blit;
mod plan;
mod target;

#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod tests;

use std::time::Duration;

use ash::vk;

use crate::error::GpuError;
use crate::one_shot_queue::{SubmitError, SubmittedWork};
use crate::swapchain::SwapchainError;

pub use self::plan::{EncoderSwapchainPlan, plan_encoder_swapchain};
pub use self::target::{
    DetachProgress, EncoderAttachError, EncoderDetachError, EncoderInputInfo, EncoderInputTarget, PresentOnceError,
};

/// Настройки цели кодера.
#[derive(Debug, Clone)]
pub struct EncoderTargetConfig {
    /// Бюджет `vkAcquireNextImageKHR` в `present_once` (на свежем swapchain картинка свободна сразу).
    pub acquire_timeout: Duration,
    /// Бюджет ожидания окончания показа при разборе (`detach`). Истёк — объекты НЕ уничтожаются (утечка).
    pub teardown_timeout: Duration,
    /// Сколько fence'ов показа пул держит максимум; показ один, поэтому достаточно малого числа.
    pub present_fence_capacity: usize,
}

impl EncoderTargetConfig {
    /// 2 с: на свежем swapchain acquire мгновенный; запас на медленный драйвер/CI, но не вечное ожидание.
    pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
    /// 5 с — как у `DisplayTargetConfig`: запас для простоя GPU при разборе.
    pub const DEFAULT_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);
    /// 2: показ один, второй fence — запас на повтор после сорванного показа.
    pub const DEFAULT_PRESENT_FENCE_CAPACITY: usize = 2;
}

impl Default for EncoderTargetConfig {
    fn default() -> Self {
        Self {
            acquire_timeout: Self::DEFAULT_ACQUIRE_TIMEOUT,
            teardown_timeout: Self::DEFAULT_TEARDOWN_TIMEOUT,
            present_fence_capacity: Self::DEFAULT_PRESENT_FENCE_CAPACITY,
        }
    }
}

/// Ошибка цели кодера. Разные причины не сливаются: «не подходит surface» ≠ «таймаут» ≠ «ошибка GPU».
#[derive(Debug, thiserror::Error)]
pub enum EncoderTargetError {
    /// Нулевой размер снимка.
    #[error("размер снимка нулевой")]
    ZeroExtent,
    /// Surface сама диктует размер (`currentExtent`), и он не совпал с размером снимка.
    #[error("surface требует размер {required:?}, а снимок {requested:?}")]
    ExtentRejectedBySurface {
        /// Размер снимка.
        requested: vk::Extent2D,
        /// Размер, который требует surface.
        required: vk::Extent2D,
    },
    /// Размер снимка вне `[minImageExtent, maxImageExtent]` surface.
    #[error("размер снимка {requested:?} вне допустимого диапазона surface {min:?}..={max:?}")]
    ExtentOutOfRange {
        /// Размер снимка.
        requested: vk::Extent2D,
        /// `minImageExtent`.
        min: vk::Extent2D,
        /// `maxImageExtent`.
        max: vk::Extent2D,
    },
    /// Surface не поддерживает `TRANSFER_DST` для картинок swapchain: blit невозможен.
    #[error("surface не поддерживает TRANSFER_DST (есть {supported:?})")]
    TransferDstUnsupported {
        /// `supportedUsageFlags` surface.
        supported: vk::ImageUsageFlags,
    },
    /// Нет 8-бит UNORM-формата `RGBA`/`BGRA` с `SRGB_NONLINEAR`.
    #[error("surface не предлагает 8-бит UNORM с SRGB_NONLINEAR (есть {available:?})")]
    NoSuitableFormat {
        /// Что предложила surface.
        available: Vec<vk::SurfaceFormatKHR>,
    },
    /// Ни один режим composite alpha не объявлен.
    #[error("ни один режим composite alpha не поддержан ({supported:?})")]
    NoCompositeAlpha {
        /// `supportedCompositeAlpha` surface.
        supported: vk::CompositeAlphaFlagsKHR,
    },
    /// Формат картинок swapchain не поддерживает blit-запись на этом устройстве.
    #[error("формат {format:?} не поддерживает BLIT_DST на этом устройстве")]
    BlitUnsupported {
        /// Выбранный формат.
        format: vk::Format,
    },
    /// Размер картинки не равен размеру swapchain (масштабировать при показе кодеру нельзя).
    #[error("картинка {image:?} не совпадает с размером цели {target:?}")]
    ImageExtentMismatch {
        /// Размер цели.
        target: vk::Extent2D,
        /// Размер переданной картинки.
        image: vk::Extent2D,
    },
    /// Показ уже выполнен (или цель непригодна для повтора): цель одноразовая.
    #[error("цель кодера уже использована: показ ровно один")]
    AlreadyPresented,
    /// `vkAcquireNextImageKHR` не уложился в бюджет.
    #[error("acquire картинки не уложился в бюджет")]
    AcquireTimedOut,
    /// `ERROR_OUT_OF_DATE_KHR`: поверхность кодера изменилась (например, закрыта).
    #[error("swapchain устарел (VK_ERROR_OUT_OF_DATE_KHR)")]
    SurfaceOutOfDate,
    /// Отправка копирования отказала (ёмкость, запись, Vulkan).
    #[error("отправка копирования: {0}")]
    Submit(#[from] SubmitError<GpuError>),
    /// Ошибка swapchain (включая потерю surface).
    #[error(transparent)]
    Swapchain(#[from] SwapchainError),
    /// Прочая ошибка GPU.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

/// Итог неудачного `present_once` с отправленной работой: копирование ушло на GPU, а показ сорвался.
///
/// Работу НЕЛЬЗЯ терять: без её токена нельзя доказать завершение и безопасно разобрать цель.
#[derive(Debug)]
pub struct SubmittedButNotPresented {
    /// Токен отправленной работы (опрашивается как обычно).
    pub work: SubmittedWork,
    /// Что сорвалось при показе.
    pub error: EncoderTargetError,
}
