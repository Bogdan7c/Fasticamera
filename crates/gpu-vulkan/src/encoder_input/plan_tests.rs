//! Тесты чистой политики swapchain цели кодера (без GPU): синтетические возможности surface.

use ash::vk;

use super::*;
use crate::swapchain::SurfaceSupport;

fn extent(width: u32, height: u32) -> vk::Extent2D {
    vk::Extent2D { width, height }
}

/// Возможности «как у headless/encoder surface»: размер выбирает приложение.
fn support() -> SurfaceSupport {
    SurfaceSupport {
        min_image_count: 2,
        max_image_count: 0,
        current_extent: None,
        min_image_extent: extent(1, 1),
        max_image_extent: extent(16384, 16384),
        current_transform: vk::SurfaceTransformFlagsKHR::IDENTITY,
        supported_composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
        supported_usage: vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
    }
}

fn format(format: vk::Format, color_space: vk::ColorSpaceKHR) -> vk::SurfaceFormatKHR {
    vk::SurfaceFormatKHR { format, color_space }
}

fn rgba() -> vk::SurfaceFormatKHR {
    format(vk::Format::R8G8B8A8_UNORM, vk::ColorSpaceKHR::SRGB_NONLINEAR)
}

fn bgra() -> vk::SurfaceFormatKHR {
    format(vk::Format::B8G8R8A8_UNORM, vk::ColorSpaceKHR::SRGB_NONLINEAR)
}

#[test]
fn plan_takes_transfer_dst_plus_color_attachment_fifo_minimum_images_and_given_extent() {
    let plan = plan_encoder_swapchain(&support(), &[bgra(), rgba()], extent(4096, 3072)).unwrap();
    assert_eq!(
        plan.usage,
        vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::COLOR_ATTACHMENT
    );
    assert_eq!(plan.surface_format, rgba(), "RGBA предпочтительнее BGRA");
    assert_eq!(plan.image_count, 2, "минимум, а не min+1");
    assert_eq!((plan.extent.width, plan.extent.height), (4096, 3072));
    assert_eq!(plan.composite_alpha, vk::CompositeAlphaFlagsKHR::OPAQUE);
    assert_eq!(plan.transform, vk::SurfaceTransformFlagsKHR::IDENTITY);
}

#[test]
fn plan_falls_back_to_bgra_and_rejects_ten_bit_and_wrong_color_space_only_lists() {
    let plan = plan_encoder_swapchain(&support(), &[bgra()], extent(10, 10)).unwrap();
    assert_eq!(plan.surface_format, bgra());
    let unsuitable = [
        format(vk::Format::A2B10G10R10_UNORM_PACK32, vk::ColorSpaceKHR::SRGB_NONLINEAR),
        format(vk::Format::R8G8B8A8_UNORM, vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT),
        format(vk::Format::R8G8B8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR),
    ];
    let error = plan_encoder_swapchain(&support(), &unsuitable, extent(10, 10)).unwrap_err();
    assert!(
        matches!(error, EncoderTargetError::NoSuitableFormat { ref available } if available.len() == 3),
        "{error:?}"
    );
}

#[test]
fn plan_requires_transfer_dst_but_color_attachment_is_optional() {
    let mut without_transfer = support();
    without_transfer.supported_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
    let error = plan_encoder_swapchain(&without_transfer, &[rgba()], extent(10, 10)).unwrap_err();
    assert!(
        matches!(error, EncoderTargetError::TransferDstUnsupported { .. }),
        "{error:?}"
    );

    let mut transfer_only = support();
    transfer_only.supported_usage = vk::ImageUsageFlags::TRANSFER_DST;
    let plan = plan_encoder_swapchain(&transfer_only, &[rgba()], extent(10, 10)).unwrap();
    assert_eq!(plan.usage, vk::ImageUsageFlags::TRANSFER_DST);
}

#[test]
fn plan_rejects_zero_extent_and_extent_outside_surface_limits_without_clamping() {
    let zero = plan_encoder_swapchain(&support(), &[rgba()], extent(0, 10)).unwrap_err();
    assert!(matches!(zero, EncoderTargetError::ZeroExtent), "{zero:?}");
    let mut limited = support();
    limited.max_image_extent = extent(4000, 3000);
    let too_big = plan_encoder_swapchain(&limited, &[rgba()], extent(4096, 3072)).unwrap_err();
    assert!(
        matches!(too_big, EncoderTargetError::ExtentOutOfRange { .. }),
        "{too_big:?}"
    );
    limited.min_image_extent = extent(100, 100);
    let too_small = plan_encoder_swapchain(&limited, &[rgba()], extent(99, 200)).unwrap_err();
    assert!(
        matches!(too_small, EncoderTargetError::ExtentOutOfRange { .. }),
        "{too_small:?}"
    );
}

#[test]
fn plan_follows_surface_that_dictates_its_extent() {
    let mut dictated = support();
    dictated.current_extent = Some(extent(1023, 767));
    assert!(plan_encoder_swapchain(&dictated, &[rgba()], extent(1023, 767)).is_ok());
    let error = plan_encoder_swapchain(&dictated, &[rgba()], extent(1024, 768)).unwrap_err();
    assert!(
        matches!(error, EncoderTargetError::ExtentRejectedBySurface { required, .. } if required.width == 1023 && required.height == 767),
        "{error:?}"
    );
}

#[test]
fn plan_image_count_respects_driver_limits_and_zero_minimum() {
    let mut limited = support();
    limited.min_image_count = 3;
    limited.max_image_count = 2;
    assert_eq!(
        plan_encoder_swapchain(&limited, &[rgba()], extent(8, 8))
            .unwrap()
            .image_count,
        2
    );
    limited.min_image_count = 0;
    limited.max_image_count = 0;
    assert_eq!(
        plan_encoder_swapchain(&limited, &[rgba()], extent(8, 8))
            .unwrap()
            .image_count,
        1
    );
}

#[test]
fn plan_alpha_prefers_opaque_then_inherit_and_fails_when_none() {
    let mut inherit = support();
    inherit.supported_composite_alpha =
        vk::CompositeAlphaFlagsKHR::INHERIT | vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED;
    assert_eq!(
        plan_encoder_swapchain(&inherit, &[rgba()], extent(8, 8))
            .unwrap()
            .composite_alpha,
        vk::CompositeAlphaFlagsKHR::INHERIT
    );
    inherit.supported_composite_alpha = vk::CompositeAlphaFlagsKHR::empty();
    let error = plan_encoder_swapchain(&inherit, &[rgba()], extent(8, 8)).unwrap_err();
    assert!(
        matches!(error, EncoderTargetError::NoCompositeAlpha { .. }),
        "{error:?}"
    );
}

#[test]
fn plan_passes_pre_transform_through_without_rotating() {
    let mut rotated = support();
    rotated.current_transform = vk::SurfaceTransformFlagsKHR::ROTATE_90;
    assert_eq!(
        plan_encoder_swapchain(&rotated, &[rgba()], extent(8, 8))
            .unwrap()
            .transform,
        vk::SurfaceTransformFlagsKHR::ROTATE_90
    );
}
