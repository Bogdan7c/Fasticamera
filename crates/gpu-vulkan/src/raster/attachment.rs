//! Цель растрового прохода и режим загрузки её содержимого.

use ash::vk;

use crate::test_pattern::PatternTarget;

/// Что делать с содержимым цели в начале прохода.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttachmentLoad {
    /// Оставить как есть (проход UI поверх уже нарисованного композитором). Цель обязана уже
    /// содержать нужное и быть синхронизирована (`record_between_color_passes` /
    /// `ColorTarget::record_prepare_for_render` с `Preserve`).
    Preserve,
    /// Залить цветом (RGBA, значения 0..1; для fp16-цели допустимы и вне диапазона).
    Clear([f32; 4]),
}

/// Цель рисования: представление цветовой картинки в `COLOR_ATTACHMENT_OPTIMAL`, её размер и формат.
///
/// Хэндл «голый»: владелец картинки обязан пережить сабмит. Получать цель стоит у владельцев:
/// [`crate::color_target::ColorTarget::attachment`] или [`Self::from_display_target`].
#[derive(Debug, Clone, Copy)]
pub struct ColorAttachment {
    view: vk::ImageView,
    extent: vk::Extent2D,
    format: vk::Format,
}

impl ColorAttachment {
    /// Цель из кадра показа (`DisplayFrame::pattern_target()` + `DisplayFrame::format()`): формат
    /// swapchain выбирается политикой показа и не выводится из `PatternTarget`, поэтому передаётся явно.
    pub fn from_display_target(target: PatternTarget, format: vk::Format) -> Self {
        Self::from_parts(target.view, target.extent, format)
    }

    /// Сборка владельцем картинки (`ColorTarget`).
    pub(crate) fn from_parts(view: vk::ImageView, extent: vk::Extent2D, format: vk::Format) -> Self {
        Self { view, extent, format }
    }

    /// Размер цели, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Формат цели (должен совпасть с `RasterPipelineDesc::color_format` конвейеров прохода).
    pub fn format(&self) -> vk::Format {
        self.format
    }

    /// Представление картинки (для `RenderingAttachmentInfo`).
    pub(crate) fn view(&self) -> vk::ImageView {
        self.view
    }
}
