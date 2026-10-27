//! Общий сэмплер «линейная фильтрация, края зажаты» для compute-набора и компоновщика картинок.
//!
//! Сэмплер неизменяемый (immutable sampler в раскладке набора): вызывающий не может подсунуть
//! другой, а наборы не надо переписывать ради сэмплера.

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedSampler;

/// Создаёт сэмплер: билинейная фильтрация, без mip-уровней, координаты вне `[0, 1]` зажимаются к краю.
///
/// Зажим к краю (а не повтор) важен для видоискателя: при повторе по краю кадра проступила бы полоса
/// с противоположной стороны.
pub(crate) fn create_linear_clamp_sampler(
    context: &GpuContext,
    purpose: &'static str,
) -> Result<OwnedSampler, GpuError> {
    let info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .min_lod(0.0)
        .max_lod(0.0)
        .unnormalized_coordinates(false);
    // SAFETY: устройство валидно; info — живая локальная структура.
    let sampler = run_step(unsafe { context.device().create_sampler(&info, None) }, purpose)?;
    Ok(OwnedSampler::from_raw(context, sampler))
}
