//! Типы одного кадра показа: что получает рисующий код и что возвращает `present`.
//!
//! Жизненный цикл кадра: `acquire_frame` → [`DisplayFrame`] (команды уже в записи, картинка в
//! раскладке `COLOR_ATTACHMENT_OPTIMAL`) → вызывающий рисует (`TestPattern::record` и т.п.) →
//! `present` (или `abandon_frame`, если рисование не удалось). Кадр нельзя «потерять»: `Drop`
//! без `present` пишет ошибку в лог, а цель не выдаст следующий кадр, пока этот не завершён.

use ash::vk;
use shell_contract::clock::MonotonicNanos;

use super::config::DisplayColorSpace;
use crate::frame_ring::SlotIndex;
use crate::test_pattern::{PatternTarget, SurfaceTransform};

/// Кадр, выданный на рисование. Поля приватны: целостность (слот, картинка, поколение swapchain)
/// знает только цель, которая этот кадр выдала.
#[derive(Debug)]
pub struct DisplayFrame {
    pub(super) slot: SlotIndex,
    pub(super) image_index: u32,
    pub(super) image: vk::Image,
    pub(super) command_buffer: vk::CommandBuffer,
    pub(super) pattern_target: PatternTarget,
    pub(super) format: vk::Format,
    pub(super) display_color_space: DisplayColorSpace,
    pub(super) transform: SurfaceTransform,
    pub(super) frame_index: u32,
    pub(super) swapchain_generation: u64,
    /// Acquire сообщил `SUBOPTIMAL`: рисовать можно, но после показа swapchain надо пересоздать.
    pub(super) acquired_suboptimal: bool,
    /// Кадр завершён (`present`/`abandon_frame`); иначе `Drop` сообщит о потере.
    pub(super) finished: bool,
}

impl DisplayFrame {
    /// Командный буфер, в который вызывающий дописывает рисование (запись уже начата).
    pub fn command_buffer(&self) -> vk::CommandBuffer {
        self.command_buffer
    }

    /// Цель рисования в виде, который принимает `TestPattern::record`: представление и физический размер.
    pub fn pattern_target(&self) -> PatternTarget {
        self.pattern_target
    }

    /// Формат цветовой цели (с ним должен быть создан пайплайн рисования).
    pub fn format(&self) -> vk::Format {
        self.format
    }

    /// Цветовое пространство, в котором дисплей поймёт значения кадра (sRGB или Display P3).
    pub fn display_color_space(&self) -> DisplayColorSpace {
        self.display_color_space
    }

    /// Поворот дисплея: рисовать надо уже повёрнутой картинкой.
    pub fn transform(&self) -> SurfaceTransform {
        self.transform
    }

    /// Порядковый номер кадра (с нуля, циклически по `u32`); он же `present_id` для display timing.
    pub fn frame_index(&self) -> u32 {
        self.frame_index
    }

    /// Номер слота кольца кадров в полёте. Слот снова выдаётся, только когда GPU закончил его прошлый
    /// кадр (fence слота сигнален при acquire), поэтому по слотам можно заводить собственные
    /// ресурсы кадра (например, наборы дескрипторов) без риска переписать то, что GPU ещё читает.
    pub fn ring_slot(&self) -> usize {
        self.slot.get()
    }

    /// Номер картинки swapchain (для диагностики).
    pub fn image_index(&self) -> u32 {
        self.image_index
    }
}

impl Drop for DisplayFrame {
    fn drop(&mut self) {
        if !self.finished {
            tracing::error!(
                target: "fasticamera::gpu::swapchain",
                frame_index = self.frame_index,
                "DisplayFrame уничтожен без present/abandon_frame: картинка и семафор слота потеряны, цель не выдаст новый кадр"
            );
        }
    }
}

/// Итог `acquire_frame`.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// Кадр выдан, рисуйте и вызывайте `present`.
    Ready(DisplayFrame),
    /// За `acquire_timeout` слот или картинка не освободились. Не ошибка: цикл рендера просто
    /// проверяет команды/остановку и пробует снова.
    TimedOut,
    /// Swapchain устарел (`OUT_OF_DATE`/`SUBOPTIMAL` на прошлом кадре, смена размера): вызовите `recreate`.
    NeedsRecreate,
}

/// Запрос показа.
#[derive(Debug, Clone, Copy, Default)]
pub struct PresentRequest {
    /// Желаемое время показа (`CLOCK_MONOTONIC`) для `VK_GOOGLE_display_timing`; `None` — как можно раньше.
    /// Без расширения (ПК) игнорируется.
    pub desired_present_time: Option<MonotonicNanos>,
}

/// Итог `present`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentOutcome {
    /// Кадр поставлен в очередь показа.
    Presented {
        /// Идентификатор показа (= `frame_index`); по нему находятся записи `drain_past_presentations`.
        present_id: u32,
    },
    /// Показан, но swapchain больше не оптимален (`SUBOPTIMAL`): пересоздайте при удобном случае.
    Suboptimal {
        /// Идентификатор показа.
        present_id: u32,
    },
    /// Показ не принят: swapchain устарел (`OUT_OF_DATE`), нужен `recreate`.
    NeedsRecreate,
}

/// Переход раскладки картинки swapchain, который записывает цель вокруг рисования вызывающего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LayoutTransition {
    /// После acquire: `UNDEFINED` → `COLOR_ATTACHMENT_OPTIMAL` (содержимое не нужно: паттерн его перезапишет).
    AcquiredToColorAttachment,
    /// Перед показом: `COLOR_ATTACHMENT_OPTIMAL` → `PRESENT_SRC_KHR`.
    ColorAttachmentToPresent,
}

impl LayoutTransition {
    /// Барьер synchronization2 для этого перехода.
    fn barrier(self, image: vk::Image) -> vk::ImageMemoryBarrier2<'static> {
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let base = vk::ImageMemoryBarrier2::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range);
        match self {
            // Первая зависимость — семафор acquire, который ждёт стадию COLOR_ATTACHMENT_OUTPUT;
            // барьер с той же стадией замыкает цепочку «получили → меняем раскладку → пишем».
            LayoutTransition::AcquiredToColorAttachment => base
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            // Видимость записи для показа обеспечивает семафор «рендер готов», поэтому вторая
            // половина барьера — NONE (так требует sync2 для перехода в PRESENT_SRC).
            LayoutTransition::ColorAttachmentToPresent => base
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_access_mask(vk::AccessFlags2::NONE)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR),
        }
    }

    /// Записывает барьер в командный буфер (в состоянии записи).
    pub(super) fn record(self, device: &ash::Device, command_buffer: vk::CommandBuffer, image: vk::Image) {
        let barriers = [self.barrier(image)];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: командный буфер в состоянии записи (контракт вызывающего), image — картинка swapchain.
        unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_chain_layouts_in_order() {
        let image = vk::Image::null();
        let to_draw = LayoutTransition::AcquiredToColorAttachment.barrier(image);
        let to_present = LayoutTransition::ColorAttachmentToPresent.barrier(image);
        assert_eq!(to_draw.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(
            to_draw.new_layout, to_present.old_layout,
            "вторая стадия продолжает первую"
        );
        assert_eq!(to_present.new_layout, vk::ImageLayout::PRESENT_SRC_KHR);
        assert_eq!(to_draw.dst_access_mask, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(to_present.src_access_mask, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
    }
}
