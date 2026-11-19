//! [`RasterPass`] — один проход динамического рендеринга и [`DrawCall`] внутри него.

use ash::vk;

use super::attachment::{AttachmentLoad, ColorAttachment};
use super::pipeline::RasterPipeline;
use crate::context::GpuContext;
use crate::error::GpuError;

/// Один вызов рисования: сколько вершин на экземпляр, сколько экземпляров и куда обрезать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawCall {
    /// Вершин на экземпляр (`gl_VertexIndex` пробегает `0..vertices_per_instance`).
    pub vertices_per_instance: u32,
    /// Число экземпляров (`gl_InstanceIndex` пробегает `0..instance_count`).
    pub instance_count: u32,
    /// Прямоугольник отсечения в пикселях цели; `None` — вся цель. Должен лежать внутри цели.
    pub scissor: Option<vk::Rect2D>,
}

/// Открытый проход рендеринга. Пока он жив, командный буфер находится ВНУТРИ рендеринга: барьеры и
/// копирования записывать нельзя. Закрывается [`Self::end`]; брошенный без `end` — закрывается в `Drop`
/// с предупреждением.
pub struct RasterPass<'a> {
    context: &'a GpuContext,
    command_buffer: vk::CommandBuffer,
    target_extent: vk::Extent2D,
    target_format: vk::Format,
    /// `true` после `vkCmdEndRendering`.
    finished: bool,
}

impl<'a> RasterPass<'a> {
    /// Начинает проход в `target`. Цель уже в `COLOR_ATTACHMENT_OPTIMAL` и синхронизирована владельцем
    /// (`ColorTarget::record_prepare_for_render`, переходы кадра swapchain, `record_between_color_passes`).
    ///
    /// Нулевой размер цели и нечисловой цвет очистки — `InvalidArgument` до записи команд. Viewport и
    /// scissor выставляются на всю цель.
    pub fn begin(
        context: &'a GpuContext,
        command_buffer: vk::CommandBuffer,
        target: ColorAttachment,
        load: AttachmentLoad,
    ) -> Result<Self, GpuError> {
        let extent = target.extent();
        if extent.width == 0 || extent.height == 0 {
            return Err(GpuError::InvalidArgument {
                what: "нулевой размер цели растрового прохода".to_owned(),
            });
        }
        let (load_op, clear_value) = match load {
            AttachmentLoad::Preserve => (vk::AttachmentLoadOp::LOAD, vk::ClearValue::default()),
            AttachmentLoad::Clear(color) => {
                if color.iter().any(|channel| !channel.is_finite()) {
                    return Err(GpuError::InvalidArgument {
                        what: format!("нечисловой цвет очистки {color:?}"),
                    });
                }
                (
                    vk::AttachmentLoadOp::CLEAR,
                    vk::ClearValue {
                        color: vk::ClearColorValue { float32: color },
                    },
                )
            }
        };
        let color_attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(target.view())
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(load_op)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(clear_value)];
        let render_area = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };
        let rendering_info = vk::RenderingInfo::default()
            .render_area(render_area)
            .layer_count(1)
            .color_attachments(&color_attachments);
        let device = context.device();
        // SAFETY: командный буфер в состоянии записи вне другого рендеринга, цель в
        // COLOR_ATTACHMENT_OPTIMAL (контракт вызывающего); структуры живут до конца вызова.
        unsafe {
            device.cmd_begin_rendering(command_buffer, &rendering_info);
            device.cmd_set_viewport(command_buffer, 0, &[full_viewport(extent)]);
            device.cmd_set_scissor(command_buffer, 0, &[render_area]);
        }
        Ok(Self {
            context,
            command_buffer,
            target_extent: extent,
            target_format: target.format(),
            finished: false,
        })
    }

    /// Рисует `call` конвейером `pipeline` с набором слота `frame_slot` и push-константами.
    ///
    /// Все проверки — до записи команд (при ошибке проход остаётся открытым и пригодным): формат цели
    /// равен формату конвейера, длина `push_constants` равна объявленной, слот в кольце и все привязки
    /// его набора записаны, scissor внутри цели (все `InvalidArgument`, кроме незаписанных привязок —
    /// `InvalidState`). Вызов с нулём вершин или экземпляров после проверок — допустимая пустая
    /// операция: ничего не записывается.
    pub fn draw(
        &mut self,
        pipeline: &RasterPipeline,
        frame_slot: usize,
        push_constants: &[u8],
        call: DrawCall,
    ) -> Result<(), GpuError> {
        let scissor = match call.scissor {
            Some(rect) => check_scissor_inside(rect, self.target_extent)?,
            None => full_rect(self.target_extent),
        };
        pipeline.check_draw(self.target_format, frame_slot, push_constants.len())?;
        if call.vertices_per_instance == 0 || call.instance_count == 0 {
            return Ok(());
        }
        pipeline.record_bind(self.command_buffer, frame_slot, push_constants)?;
        // SAFETY: командный буфер внутри рендеринга этого прохода; scissor проверен; конвейер привязан выше.
        unsafe {
            // Scissor выставляется в каждом вызове: иначе отсечение предыдущего `draw` «протекло» бы.
            self.context
                .device()
                .cmd_set_scissor(self.command_buffer, 0, &[scissor]);
            self.context.device().cmd_draw(
                self.command_buffer,
                call.vertices_per_instance,
                call.instance_count,
                0,
                0,
            );
        }
        Ok(())
    }

    /// Закрывает проход (`vkCmdEndRendering`).
    pub fn end(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if self.finished {
            return;
        }
        // SAFETY: рендеринг начат в `begin` на этом же командном буфере и ещё не закрыт.
        unsafe { self.context.device().cmd_end_rendering(self.command_buffer) };
        self.finished = true;
    }
}

impl Drop for RasterPass<'_> {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!("RasterPass брошен без end(): рендеринг закрыт в Drop");
            self.finish();
        }
    }
}

/// Viewport на всю цель.
fn full_viewport(extent: vk::Extent2D) -> vk::Viewport {
    vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: extent.width as f32,
        height: extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }
}

/// Прямоугольник на всю цель.
fn full_rect(extent: vk::Extent2D) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent,
    }
}

/// Scissor обязан лежать внутри цели (`VUID-vkCmdSetScissor-x-00595`: неотрицательный offset, сумма
/// offset и extent не больше размера цели — считается в `i64`, без переполнения).
fn check_scissor_inside(rect: vk::Rect2D, target: vk::Extent2D) -> Result<vk::Rect2D, GpuError> {
    let end_x = i64::from(rect.offset.x) + i64::from(rect.extent.width);
    let end_y = i64::from(rect.offset.y) + i64::from(rect.extent.height);
    let inside = rect.offset.x >= 0
        && rect.offset.y >= 0
        && end_x <= i64::from(target.width)
        && end_y <= i64::from(target.height);
    if inside {
        Ok(rect)
    } else {
        Err(GpuError::InvalidArgument {
            what: format!("scissor {rect:?} выходит за цель {}x{}", target.width, target.height),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: vk::Extent2D = vk::Extent2D { width: 16, height: 8 };

    fn rect(x: i32, y: i32, width: u32, height: u32) -> vk::Rect2D {
        vk::Rect2D {
            offset: vk::Offset2D { x, y },
            extent: vk::Extent2D { width, height },
        }
    }

    #[test]
    fn scissor_inside_target_is_accepted_including_exact_fit_and_empty() {
        assert!(check_scissor_inside(rect(0, 0, 16, 8), TARGET).is_ok());
        assert!(check_scissor_inside(rect(15, 7, 1, 1), TARGET).is_ok());
        assert!(check_scissor_inside(rect(4, 4, 0, 0), TARGET).is_ok());
    }

    #[test]
    fn scissor_outside_target_is_rejected_without_overflow() {
        for bad in [
            rect(-1, 0, 4, 4),
            rect(0, -1, 4, 4),
            rect(0, 0, 17, 8),
            rect(12, 0, 5, 8),
            rect(0, 6, 4, 3),
            rect(i32::MAX, 0, u32::MAX, 1),
        ] {
            assert!(
                matches!(check_scissor_inside(bad, TARGET), Err(GpuError::InvalidArgument { .. })),
                "{bad:?}"
            );
        }
    }
}
