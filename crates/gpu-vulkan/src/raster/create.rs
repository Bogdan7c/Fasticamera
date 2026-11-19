//! Создание `VkPipelineLayout` и графического конвейера растрового набора.

use ash::vk;

use super::pipeline::{BlendMode, RasterPipelineDesc};
use crate::compute::descriptors::DescriptorRing;
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedPipeline, OwnedPipelineLayout};

/// Стадии, которым видны привязки набора и push-константы растрового конвейера.
pub(super) const RASTER_STAGES: vk::ShaderStageFlags =
    vk::ShaderStageFlags::from_raw(vk::ShaderStageFlags::VERTEX.as_raw() | vk::ShaderStageFlags::FRAGMENT.as_raw());

/// Layout: опциональная раскладка набора (нет привязок — набора нет) + push-константы `VERTEX|FRAGMENT`.
pub(super) fn create_pipeline_layout(
    context: &GpuContext,
    descriptors: Option<&DescriptorRing>,
    push_constant_bytes: u32,
) -> Result<OwnedPipelineLayout, GpuError> {
    let set_layouts: Vec<vk::DescriptorSetLayout> = descriptors.map(DescriptorRing::set_layout).into_iter().collect();
    let push_ranges = [vk::PushConstantRange::default()
        .stage_flags(RASTER_STAGES)
        .offset(0)
        .size(push_constant_bytes)];
    let used_ranges: &[vk::PushConstantRange] = if push_constant_bytes == 0 { &[] } else { &push_ranges };
    let layout_info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(&set_layouts)
        .push_constant_ranges(used_ranges);
    // SAFETY: устройство валидно; массивы живут до конца вызова.
    let layout = run_step(
        unsafe { context.device().create_pipeline_layout(&layout_info, None) },
        "vkCreatePipelineLayout(raster)",
    )?;
    Ok(OwnedPipelineLayout::from_raw(context, layout))
}

/// Состояние смешивания одного цветового вложения.
fn blend_attachment(blend: BlendMode) -> vk::PipelineColorBlendAttachmentState {
    let base = vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA);
    match blend {
        BlendMode::Opaque => base.blend_enable(false),
        // Premultiplied alpha: результат = src + dst · (1 − src.a) и для цвета, и для альфы.
        BlendMode::PremultipliedAlpha => base
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD),
    }
}

/// Графический конвейер: без вершинного ввода, `TRIANGLE_LIST`, без отсечения, 1 sample, динамические
/// viewport и scissor, динамический рендеринг в один цвет без глубины.
pub(super) fn create_graphics_pipeline(
    context: &GpuContext,
    desc: &RasterPipelineDesc<'_>,
    layout: &OwnedPipelineLayout,
) -> Result<OwnedPipeline, GpuError> {
    let device = context.device();
    let vertex_module = context.create_shader_module(desc.vertex_spirv)?;
    let fragment_module = context.create_shader_module(desc.fragment_spirv)?;
    // Specialization-константы применяются к обеим стадиям; значения — всегда u32 (как у compute).
    let constant_bytes: Vec<u8> = desc
        .specialization
        .iter()
        .flat_map(|constant| constant.value.to_ne_bytes())
        .collect();
    let map_entries: Vec<vk::SpecializationMapEntry> = (0_u32..)
        .zip(desc.specialization)
        .map(|(index, constant)| {
            vk::SpecializationMapEntry::default()
                .constant_id(constant.constant_id)
                .offset(index * 4)
                .size(std::mem::size_of::<u32>())
        })
        .collect();
    let specialization_info = vk::SpecializationInfo::default()
        .map_entries(&map_entries)
        .data(&constant_bytes);
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex_module.handle())
            .name(c"main")
            .specialization_info(&specialization_info),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment_module.handle())
            .name(c"main")
            .specialization_info(&specialization_info),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly =
        vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample =
        vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachments = [blend_attachment(desc.blend)];
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let color_formats = [desc.color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
    let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic_state)
        .layout(layout.handle())
        .push_next(&mut rendering_info);
    // SAFETY: модули, константы и все структуры живут до конца вызова; кэша пайплайнов нет.
    let pipelines = unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None) }
        .map_err(|(_, result)| GpuError::Vulkan {
            step: "vkCreateGraphicsPipelines(raster)",
            result,
        })?;
    let handle = pipelines.first().copied().ok_or(GpuError::InvalidState {
        what: "vkCreateGraphicsPipelines(raster) не вернул пайплайн".to_owned(),
    })?;
    Ok(OwnedPipeline::from_raw(context, handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_alpha_uses_one_and_one_minus_source_alpha_for_color_and_alpha() {
        let state = blend_attachment(BlendMode::PremultipliedAlpha);
        assert_eq!(state.blend_enable, vk::TRUE);
        assert_eq!(state.src_color_blend_factor, vk::BlendFactor::ONE);
        assert_eq!(state.dst_color_blend_factor, vk::BlendFactor::ONE_MINUS_SRC_ALPHA);
        assert_eq!(state.src_alpha_blend_factor, vk::BlendFactor::ONE);
        assert_eq!(state.dst_alpha_blend_factor, vk::BlendFactor::ONE_MINUS_SRC_ALPHA);
        assert_eq!(state.color_blend_op, vk::BlendOp::ADD);
        assert_eq!(state.alpha_blend_op, vk::BlendOp::ADD);
    }

    #[test]
    fn opaque_disables_blending_and_writes_all_channels() {
        let state = blend_attachment(BlendMode::Opaque);
        assert_eq!(state.blend_enable, vk::FALSE);
        assert_eq!(state.color_write_mask, vk::ColorComponentFlags::RGBA);
    }
}
