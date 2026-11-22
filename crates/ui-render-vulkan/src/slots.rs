//! Ресурсы слота кадра: буферы экземпляров (CPU пишет, GPU читает) и учёт «в полёте».

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};

use crate::config::UiRenderLimits;
use crate::pack::{
    GLASS_GROUP_BYTES, GLASS_MEMBER_BYTES, GLYPH_INSTANCE_BYTES, IMAGE_INSTANCE_BYTES, SHAPE_INSTANCE_BYTES,
};
use ui_draw::draw_list::MAX_GLASS_GROUP_MEMBERS;

/// Буферы одного слота. Размеры выводятся из пределов кадра при создании и больше не растут.
pub(crate) struct SlotBuffers {
    pub shapes: HostBuffer,
    pub glyphs: HostBuffer,
    pub groups: HostBuffer,
    pub members: HostBuffer,
    pub images: HostBuffer,
}

impl SlotBuffers {
    pub(crate) fn new(context: &GpuContext, limits: &UiRenderLimits) -> Result<Self, GpuError> {
        let storage = |bytes: usize| HostBuffer::new(context, bytes as u64, HostBufferUsage::ReadOnlyStorage);
        Ok(Self {
            shapes: storage(limits.max_shape_instances as usize * SHAPE_INSTANCE_BYTES)?,
            glyphs: storage(limits.max_glyph_instances as usize * GLYPH_INSTANCE_BYTES)?,
            groups: storage(limits.max_glass_groups as usize * GLASS_GROUP_BYTES)?,
            members: storage(limits.max_glass_groups as usize * MAX_GLASS_GROUP_MEMBERS * GLASS_MEMBER_BYTES)?,
            images: storage(limits.max_images as usize * IMAGE_INSTANCE_BYTES)?,
        })
    }
}
