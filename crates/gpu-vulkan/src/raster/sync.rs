//! Барьер между двумя проходами в одну цель.

use ash::vk;

use crate::context::GpuContext;

/// Записывает барьер памяти между двумя растровыми проходами в ОДНУ цель (композитор → UI): запись
/// цвета первого прохода (`COLOR_ATTACHMENT_OUTPUT`/`COLOR_ATTACHMENT_WRITE`) становится видимой для
/// загрузки и смешивания второго (`COLOR_ATTACHMENT_OUTPUT`, чтение и запись).
///
/// Раскладку не меняет: цель остаётся в `COLOR_ATTACHMENT_OPTIMAL` (так кадр swapchain лежит между
/// acquire и present). Звать вне рендеринга, после `RasterPass::end` (или композитора) и до
/// `RasterPass::begin` с [`super::AttachmentLoad::Preserve`]. Без барьера слой синхронизации validation
/// сообщает об опасности «чтение после записи» на загрузке цвета.
pub fn record_between_color_passes(context: &GpuContext, command_buffer: vk::CommandBuffer) {
    let barriers = [between_color_passes_barrier()];
    let dependency = vk::DependencyInfo::default().memory_barriers(&barriers);
    // SAFETY: командный буфер в состоянии записи вне динамического рендеринга (контракт вызывающего);
    // barriers живут до конца вызова.
    unsafe { context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
}

/// Сам барьер памяти: вынесен, чтобы маски проверялись тестом (слой синхронизации validation на
/// аппаратном порядке записи цвета в одной стадии не всегда замечает отсутствие барьера, поэтому
/// корректность масок закрепляется явно).
fn between_color_passes_barrier() -> vk::MemoryBarrier2<'static> {
    vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barrier_makes_first_pass_color_writes_visible_to_second_pass_load_and_blend() {
        let barrier = between_color_passes_barrier();
        assert_eq!(barrier.src_stage_mask, vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT);
        assert_eq!(barrier.src_access_mask, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(barrier.dst_stage_mask, vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT);
        // Загрузка (loadOp = LOAD) и смешивание читают цвет, запись — следующие draw.
        assert!(
            barrier
                .dst_access_mask
                .contains(vk::AccessFlags2::COLOR_ATTACHMENT_READ)
        );
        assert!(
            barrier
                .dst_access_mask
                .contains(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
        );
    }
}
