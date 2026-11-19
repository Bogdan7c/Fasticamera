//! [`FrameTarget`] — то, что записи кадра видоискателя нужно знать о цели кадра.
//!
//! Раньше запись принимала `&DisplayFrame` swapchain. Но `DisplayFrame` собирает только сама цепочка
//! показа, а нам из него нужны шесть значений. Узкий тип делает запись компоновки и UI проверяемой на
//! offscreen-цели, которую можно прочитать (swapchain headless-стенда читать нельзя): тест строит
//! `FrameTarget` из `OffscreenTarget`, а бой — из `DisplayFrame`. Логики владения здесь нет.

use ash::vk;
use gpu_vulkan::swapchain::{DisplayColorSpace, DisplayFrame};
use gpu_vulkan::test_pattern::{PatternTarget, SurfaceTransform};

/// Цель одного кадра: куда и с каким поворотом дисплея записывать команды.
#[derive(Debug, Clone, Copy)]
pub struct FrameTarget {
    /// Командный буфер, в который идёт запись (уже в состоянии записи).
    pub command_buffer: vk::CommandBuffer,
    /// Представление цели и её физический размер.
    pub pattern_target: PatternTarget,
    /// Формат цели.
    pub format: vk::Format,
    /// Пространство цвета дисплея.
    pub display_color_space: DisplayColorSpace,
    /// Поворот дисплея (`preTransform`).
    pub transform: SurfaceTransform,
    /// Слот кольца кадров.
    pub ring_slot: usize,
}

impl FrameTarget {
    /// Цель из кадра цепочки показа.
    pub fn of(frame: &DisplayFrame) -> Self {
        Self {
            command_buffer: frame.command_buffer(),
            pattern_target: frame.pattern_target(),
            format: frame.format(),
            display_color_space: frame.display_color_space(),
            transform: frame.transform(),
            ring_slot: frame.ring_slot(),
        }
    }
}
