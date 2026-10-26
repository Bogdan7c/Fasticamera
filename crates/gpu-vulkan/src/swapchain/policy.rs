//! ЧИСТАЯ политика swapchain: что выбрать по возможностям surface. Ни одного вызова Vulkan —
//! входные данные — снимок [`SurfaceSupport`] и список форматов, поэтому всё тестируется на ПК
//! с синтетическими возможностями (в том числе «как на телефоне»: поворот 90°, `max = 0`).
//!
//! Решения (все ссылки — на спецификацию Vulkan, раздел `VkSwapchainCreateInfoKHR`):
//! * формат — 10-бит (если просили и есть) → 8-бит; всегда UNORM + `SRGB_NONLINEAR`. sRGB-кодированные
//!   форматы НЕ выбираем: `TestPattern` целочисленно точен только на UNORM (см. `ColorDepth::for_format`);
//! * число картинок — `minImageCount + 1`, но не больше `maxImageCount` (0 = предела нет);
//! * размер — если `currentExtent` определён, обязан совпасть с ним (VUID `imageExtent-01274`);
//!   иначе берём запрошенный размер, при повороте на 90/270 меняем стороны местами и зажимаем в
//!   `[minImageExtent, maxImageExtent]`;
//! * `preTransform = currentTransform` — композитор не делает лишнего поворота, поворот картинки
//!   отдаём наружу как [`SurfaceTransform`] (рисовать надо уже повёрнутой);
//! * composite alpha — OPAQUE → INHERIT → PRE_MULTIPLIED → POST_MULTIPLIED;
//! * present mode — FIFO (есть у любой реализации по спецификации).

use ash::vk;
use shell_contract::surface::SurfaceSize;

use super::config::ColorDepthPreference;
use crate::error::GpuError;
use crate::test_pattern::SurfaceTransform;

/// Снимок `VkSurfaceCapabilitiesKHR` в виде, удобном для чистой логики.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceSupport {
    /// `minImageCount`.
    pub min_image_count: u32,
    /// `maxImageCount`; `0` — без ограничения.
    pub max_image_count: u32,
    /// `currentExtent`; `None`, если surface оставила размер на выбор приложения (`0xFFFFFFFF`).
    pub current_extent: Option<vk::Extent2D>,
    /// `minImageExtent`.
    pub min_image_extent: vk::Extent2D,
    /// `maxImageExtent`.
    pub max_image_extent: vk::Extent2D,
    /// `currentTransform` — как дисплей сейчас повёрнут относительно «прямого» вида.
    pub current_transform: vk::SurfaceTransformFlagsKHR,
    /// `supportedCompositeAlpha`.
    pub supported_composite_alpha: vk::CompositeAlphaFlagsKHR,
    /// `supportedUsageFlags`.
    pub supported_usage: vk::ImageUsageFlags,
}

impl SurfaceSupport {
    /// Значение `currentExtent`, означающее «размер выбирает приложение» (спецификация: `0xFFFFFFFF`).
    const UNDEFINED_EXTENT: u32 = 0xFFFF_FFFF;

    /// Из ответа драйвера.
    pub fn from_vk(capabilities: &vk::SurfaceCapabilitiesKHR) -> Self {
        let current = capabilities.current_extent;
        let defined = current.width != Self::UNDEFINED_EXTENT && current.height != Self::UNDEFINED_EXTENT;
        Self {
            min_image_count: capabilities.min_image_count,
            max_image_count: capabilities.max_image_count,
            current_extent: defined.then_some(current),
            min_image_extent: capabilities.min_image_extent,
            max_image_extent: capabilities.max_image_extent,
            current_transform: capabilities.current_transform,
            supported_composite_alpha: capabilities.supported_composite_alpha,
            supported_usage: capabilities.supported_usage_flags,
        }
    }
}

/// Что приложение просит сверх возможностей surface.
#[derive(Debug, Clone, Copy)]
pub struct PlanRequest {
    /// Предпочтение глубины цвета.
    pub color_depth: ColorDepthPreference,
    /// Желаемый размер в обычной ориентации (используется, только если surface не диктует размер).
    pub requested_size: Option<SurfaceSize>,
}

/// Итог выбора для создания swapchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapchainPlan {
    /// Формат и цветовое пространство.
    pub surface_format: vk::SurfaceFormatKHR,
    /// `minImageCount` для создания.
    pub image_count: u32,
    /// Физический размер картинок (с учётом поворота).
    pub extent: vk::Extent2D,
    /// Поворот, который применит дисплей (`preTransform = currentTransform`).
    pub transform: SurfaceTransform,
    /// Режим альфы композитора.
    pub composite_alpha: vk::CompositeAlphaFlagsKHR,
    /// Режим показа (всегда FIFO).
    pub present_mode: vk::PresentModeKHR,
    /// Использование картинок.
    pub usage: vk::ImageUsageFlags,
}

impl SwapchainPlan {
    /// Размер в «прямой» ориентации (после того как дисплей применит поворот): стороны меняются
    /// местами при 90/270. Это размер, в котором пользователь видит картинку.
    pub fn logical_extent(&self) -> vk::Extent2D {
        if self.transform.swaps_extent() {
            vk::Extent2D {
                width: self.extent.height,
                height: self.extent.width,
            }
        } else {
            self.extent
        }
    }
}

/// Результат планирования: либо план, либо «размер нулевой, ждём».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOutcome {
    /// Swapchain можно создавать по этому плану.
    Ready(SwapchainPlan),
    /// Размер нулевой (окно свёрнуто): создавать нечего, не ошибка.
    ZeroExtent,
}

/// Предпочтительные форматы по порядку; цветовое пространство всегда `SRGB_NONLINEAR`.
const TEN_BIT_FORMATS: [vk::Format; 2] = [
    vk::Format::A2B10G10R10_UNORM_PACK32,
    vk::Format::A2R10G10B10_UNORM_PACK32,
];
const EIGHT_BIT_FORMATS: [vk::Format; 2] = [vk::Format::R8G8B8A8_UNORM, vk::Format::B8G8R8A8_UNORM];

/// Выбирает формат: нет подходящего UNORM + `SRGB_NONLINEAR` — честная ошибка `Unsupported`
/// (sRGB-кодированные форматы тестовый паттерн не принимает, поэтому «хоть какой-нибудь» не берём).
pub fn choose_surface_format(
    available: &[vk::SurfaceFormatKHR],
    preference: ColorDepthPreference,
) -> Result<vk::SurfaceFormatKHR, GpuError> {
    let ten_bit = (preference == ColorDepthPreference::PreferTenBit).then_some(TEN_BIT_FORMATS.as_slice());
    let candidates = ten_bit.into_iter().flatten().chain(EIGHT_BIT_FORMATS.iter()).copied();
    for wanted in candidates {
        let found = available
            .iter()
            .find(|entry| entry.format == wanted && entry.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR);
        if let Some(entry) = found {
            return Ok(*entry);
        }
    }
    Err(GpuError::Unsupported {
        what: format!("surface не предлагает UNORM-формат с SRGB_NONLINEAR (есть {available:?})"),
    })
}

/// Число картинок: `min + 1` (одна запас под показ), но не больше `max`; `max == 0` — без предела.
pub fn choose_image_count(min_image_count: u32, max_image_count: u32) -> u32 {
    let wanted = min_image_count.saturating_add(1);
    if max_image_count == 0 {
        wanted
    } else {
        wanted.min(max_image_count)
    }
}

/// Режим альфы по предпочтению; ни один не объявлен — `Unsupported` (по спецификации хотя бы один есть).
pub fn choose_composite_alpha(supported: vk::CompositeAlphaFlagsKHR) -> Result<vk::CompositeAlphaFlagsKHR, GpuError> {
    [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ]
    .into_iter()
    .find(|flag| supported.contains(*flag))
    .ok_or_else(|| GpuError::Unsupported {
        what: format!("ни один режим composite alpha не поддержан ({supported:?})"),
    })
}

/// Размер картинок. `None` — нулевой размер (ждём).
fn choose_extent(
    support: &SurfaceSupport,
    transform: SurfaceTransform,
    requested: Option<SurfaceSize>,
) -> Result<Option<vk::Extent2D>, GpuError> {
    let extent = match (support.current_extent, requested) {
        // Surface диктует размер: обязаны совпасть с ним (в её собственной системе координат).
        (Some(current), _) => current,
        (None, Some(size)) => {
            let (width, height) = if transform.swaps_extent() {
                (size.height_px, size.width_px)
            } else {
                (size.width_px, size.height_px)
            };
            vk::Extent2D {
                width: width.clamp(support.min_image_extent.width, support.max_image_extent.width.max(1)),
                height: height.clamp(support.min_image_extent.height, support.max_image_extent.height.max(1)),
            }
        }
        (None, None) => {
            return Err(GpuError::InvalidArgument {
                what: "surface не задаёт размер (currentExtent не определён), а requested_size не указан".to_owned(),
            });
        }
    };
    // Нулевой запрошенный размер: зажатие в min дало бы ненулевой, поэтому проверяем исходные числа.
    let requested_is_zero = requested.is_some_and(|size| size.width_px == 0 || size.height_px == 0);
    let current_is_zero = support.current_extent.is_some() && (extent.width == 0 || extent.height == 0);
    if current_is_zero || (support.current_extent.is_none() && requested_is_zero) {
        return Ok(None);
    }
    Ok(Some(extent))
}

/// Собирает план swapchain из возможностей surface.
pub fn plan_swapchain(
    support: &SurfaceSupport,
    formats: &[vk::SurfaceFormatKHR],
    request: &PlanRequest,
) -> Result<PlanOutcome, GpuError> {
    if !support.supported_usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
        return Err(GpuError::Unsupported {
            what: format!(
                "surface не поддерживает COLOR_ATTACHMENT (есть {:?})",
                support.supported_usage
            ),
        });
    }
    let transform = SurfaceTransform::from_vk(support.current_transform).ok_or_else(|| GpuError::Unsupported {
        what: format!(
            "currentTransform {:?} не одиночный известный флаг",
            support.current_transform
        ),
    })?;
    // Зеркальные повороты тестовый паттерн и шов показа не поддерживают: говорим об этом сразу,
    // а не рисуем зеркально (`quarter_turns` вернёт ту же ошибку).
    transform.quarter_turns()?;

    let surface_format = choose_surface_format(formats, request.color_depth)?;
    let composite_alpha = choose_composite_alpha(support.supported_composite_alpha)?;
    let Some(extent) = choose_extent(support, transform, request.requested_size)? else {
        return Ok(PlanOutcome::ZeroExtent);
    };
    Ok(PlanOutcome::Ready(SwapchainPlan {
        surface_format,
        image_count: choose_image_count(support.min_image_count, support.max_image_count),
        extent,
        transform,
        composite_alpha,
        // FIFO гарантирован спецификацией для любой реализации — запрашивать список режимов незачем.
        present_mode: vk::PresentModeKHR::FIFO,
        usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
    }))
}

#[cfg(test)]
mod tests;
