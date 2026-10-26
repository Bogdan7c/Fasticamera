//! Тесты чистой политики: синтетические возможности surface, без GPU.

use super::*;

fn format(format: vk::Format, color_space: vk::ColorSpaceKHR) -> vk::SurfaceFormatKHR {
    vk::SurfaceFormatKHR { format, color_space }
}

fn extent(width: u32, height: u32) -> vk::Extent2D {
    vk::Extent2D { width, height }
}

/// «Как телефон»: размер диктует surface, портрет, прямой поворот.
fn phone_support() -> SurfaceSupport {
    SurfaceSupport {
        min_image_count: 3,
        max_image_count: 0,
        current_extent: Some(extent(1440, 3216)),
        min_image_extent: extent(1, 1),
        max_image_extent: extent(4096, 4096),
        current_transform: vk::SurfaceTransformFlagsKHR::IDENTITY,
        supported_composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE | vk::CompositeAlphaFlagsKHR::INHERIT,
        supported_usage: vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
    }
}

/// «Как headless ПК»: размер не определён, выбирает приложение.
fn headless_support() -> SurfaceSupport {
    SurfaceSupport {
        current_extent: None,
        min_image_count: 1,
        min_image_extent: extent(1, 1),
        max_image_extent: extent(16384, 16384),
        ..phone_support()
    }
}

fn request(size: Option<(u32, u32)>) -> PlanRequest {
    PlanRequest {
        color_depth: ColorDepthPreference::PreferTenBit,
        requested_size: size.map(|(width_px, height_px)| SurfaceSize { width_px, height_px }),
    }
}

fn standard_formats() -> Vec<vk::SurfaceFormatKHR> {
    vec![
        format(vk::Format::R8G8B8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR),
        format(vk::Format::R8G8B8A8_UNORM, vk::ColorSpaceKHR::SRGB_NONLINEAR),
        format(vk::Format::A2B10G10R10_UNORM_PACK32, vk::ColorSpaceKHR::SRGB_NONLINEAR),
    ]
}

#[test]
fn ten_bit_is_preferred_when_available_and_requested() {
    let chosen = choose_surface_format(&standard_formats(), ColorDepthPreference::PreferTenBit).unwrap();
    assert_eq!(chosen.format, vk::Format::A2B10G10R10_UNORM_PACK32);
}

#[test]
fn eight_bit_only_skips_ten_bit_and_never_picks_srgb_encoded_format() {
    let chosen = choose_surface_format(&standard_formats(), ColorDepthPreference::EightBitOnly).unwrap();
    // R8G8B8A8_SRGB стоит в списке первым, но выбирать его нельзя: паттерн принимает только UNORM.
    assert_eq!(chosen.format, vk::Format::R8G8B8A8_UNORM);
}

#[test]
fn falls_back_to_bgra_and_rejects_other_color_spaces() {
    let bgra_only = [format(vk::Format::B8G8R8A8_UNORM, vk::ColorSpaceKHR::SRGB_NONLINEAR)];
    let chosen = choose_surface_format(&bgra_only, ColorDepthPreference::PreferTenBit).unwrap();
    assert_eq!(chosen.format, vk::Format::B8G8R8A8_UNORM);

    let hdr_only = [
        format(
            vk::Format::A2B10G10R10_UNORM_PACK32,
            vk::ColorSpaceKHR::HDR10_ST2084_EXT,
        ),
        format(vk::Format::R8G8B8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR),
    ];
    let error = choose_surface_format(&hdr_only, ColorDepthPreference::PreferTenBit).unwrap_err();
    assert!(matches!(error, GpuError::Unsupported { .. }), "{error:?}");
    assert!(matches!(
        choose_surface_format(&[], ColorDepthPreference::PreferTenBit),
        Err(GpuError::Unsupported { .. })
    ));
}

#[test]
fn image_count_is_min_plus_one_clamped_by_max_with_zero_meaning_unlimited() {
    assert_eq!(choose_image_count(3, 0), 4);
    assert_eq!(choose_image_count(2, 8), 3);
    assert_eq!(choose_image_count(3, 3), 3, "max ограничивает min+1");
    assert_eq!(choose_image_count(u32::MAX, 0), u32::MAX, "без переполнения");
}

#[test]
fn composite_alpha_follows_preference_order() {
    use vk::CompositeAlphaFlagsKHR as Alpha;
    assert_eq!(
        choose_composite_alpha(Alpha::OPAQUE | Alpha::INHERIT),
        Ok(Alpha::OPAQUE)
    );
    assert_eq!(
        choose_composite_alpha(Alpha::INHERIT | Alpha::POST_MULTIPLIED),
        Ok(Alpha::INHERIT)
    );
    assert_eq!(
        choose_composite_alpha(Alpha::POST_MULTIPLIED),
        Ok(Alpha::POST_MULTIPLIED)
    );
    assert!(matches!(
        choose_composite_alpha(Alpha::empty()),
        Err(GpuError::Unsupported { .. })
    ));
}

#[test]
fn defined_current_extent_wins_over_requested_size() {
    let outcome = plan_swapchain(&phone_support(), &standard_formats(), &request(Some((10, 10)))).unwrap();
    let PlanOutcome::Ready(plan) = outcome else {
        panic!("ожидали план, получили {outcome:?}");
    };
    assert_eq!(plan.extent, extent(1440, 3216));
    assert_eq!(plan.image_count, 4);
    assert_eq!(plan.transform, SurfaceTransform::Identity);
    assert_eq!(plan.present_mode, vk::PresentModeKHR::FIFO);
    assert_eq!(plan.composite_alpha, vk::CompositeAlphaFlagsKHR::OPAQUE);
    assert_eq!(plan.logical_extent(), extent(1440, 3216));
}

#[test]
fn rotation_by_quarter_turn_swaps_requested_extent_and_logical_extent_swaps_back() {
    for (flags, transform) in [
        (vk::SurfaceTransformFlagsKHR::ROTATE_90, SurfaceTransform::Rotate90),
        (vk::SurfaceTransformFlagsKHR::ROTATE_270, SurfaceTransform::Rotate270),
    ] {
        let support = SurfaceSupport {
            current_transform: flags,
            ..headless_support()
        };
        let PlanOutcome::Ready(plan) =
            plan_swapchain(&support, &standard_formats(), &request(Some((640, 480)))).unwrap()
        else {
            panic!("ожидали план");
        };
        assert_eq!(plan.transform, transform);
        assert_eq!(
            plan.extent,
            extent(480, 640),
            "{transform:?}: физический размер с обменом сторон"
        );
        assert_eq!(plan.logical_extent(), extent(640, 480));
    }
    // 180° стороны не меняет.
    let support = SurfaceSupport {
        current_transform: vk::SurfaceTransformFlagsKHR::ROTATE_180,
        ..headless_support()
    };
    let PlanOutcome::Ready(plan) = plan_swapchain(&support, &standard_formats(), &request(Some((640, 480)))).unwrap()
    else {
        panic!("ожидали план");
    };
    assert_eq!(plan.extent, extent(640, 480));
}

#[test]
fn requested_extent_is_clamped_to_surface_limits() {
    let support = SurfaceSupport {
        min_image_extent: extent(100, 100),
        max_image_extent: extent(800, 600),
        ..headless_support()
    };
    let PlanOutcome::Ready(plan) = plan_swapchain(&support, &standard_formats(), &request(Some((5000, 50)))).unwrap()
    else {
        panic!("ожидали план");
    };
    assert_eq!(plan.extent, extent(800, 100));
}

#[test]
fn zero_sizes_are_reported_as_zero_extent_not_as_error() {
    let zero_current = SurfaceSupport {
        current_extent: Some(extent(0, 0)),
        ..phone_support()
    };
    assert_eq!(
        plan_swapchain(&zero_current, &standard_formats(), &request(None)),
        Ok(PlanOutcome::ZeroExtent)
    );
    assert_eq!(
        plan_swapchain(&headless_support(), &standard_formats(), &request(Some((0, 480)))),
        Ok(PlanOutcome::ZeroExtent)
    );
}

#[test]
fn undefined_extent_without_requested_size_is_an_invalid_argument() {
    let error = plan_swapchain(&headless_support(), &standard_formats(), &request(None)).unwrap_err();
    assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
}

#[test]
fn mirrored_and_inherit_transforms_are_reported_unsupported() {
    for flags in [
        vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR,
        vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_90,
        vk::SurfaceTransformFlagsKHR::INHERIT,
    ] {
        let support = SurfaceSupport {
            current_transform: flags,
            ..phone_support()
        };
        let error = plan_swapchain(&support, &standard_formats(), &request(None)).unwrap_err();
        assert!(matches!(error, GpuError::Unsupported { .. }), "{flags:?}: {error:?}");
    }
}

#[test]
fn missing_color_attachment_usage_is_unsupported() {
    let support = SurfaceSupport {
        supported_usage: vk::ImageUsageFlags::TRANSFER_DST,
        ..phone_support()
    };
    let error = plan_swapchain(&support, &standard_formats(), &request(None)).unwrap_err();
    assert!(matches!(error, GpuError::Unsupported { .. }), "{error:?}");
}

#[test]
fn surface_support_treats_ffffffff_as_undefined_extent() {
    let capabilities = vk::SurfaceCapabilitiesKHR {
        current_extent: extent(0xFFFF_FFFF, 0xFFFF_FFFF),
        min_image_count: 2,
        ..Default::default()
    };
    assert_eq!(SurfaceSupport::from_vk(&capabilities).current_extent, None);
    let defined = vk::SurfaceCapabilitiesKHR {
        current_extent: extent(10, 20),
        ..Default::default()
    };
    assert_eq!(SurfaceSupport::from_vk(&defined).current_extent, Some(extent(10, 20)));
}
