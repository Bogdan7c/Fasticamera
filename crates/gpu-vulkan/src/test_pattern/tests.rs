//! Тесты, которым нужен доступ к внутренностям: ошибки до записи команд.

use ash::vk;

use super::*;
use crate::context::test_support::gpu_context_or_skip;

#[test]
fn unsupported_format_is_rejected_without_creating_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let error = TestPattern::new(&context, vk::Format::R8G8B8A8_SRGB).err().unwrap();
    assert!(matches!(error, GpuError::Unsupported { .. }), "{error:?}");
    assert_eq!(context.live_objects(), baseline);
}

#[test]
fn pipeline_builds_for_every_supported_format_and_frees_everything() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    for format in [
        vk::Format::R8G8B8A8_UNORM,
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::A2B10G10R10_UNORM_PACK32,
    ] {
        let pattern = TestPattern::new(&context, format).unwrap();
        assert_eq!(pattern.color_format(), format);
    }
    assert_eq!(context.live_objects(), baseline);
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn record_rejects_mirrored_transform_and_empty_target_before_touching_the_command_buffer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let pattern = TestPattern::new(&context, vk::Format::R8G8B8A8_UNORM).unwrap();
    // Нулевой командный буфер: если бы запись началась, validation/драйвер упали бы.
    let target = PatternTarget {
        view: vk::ImageView::null(),
        extent: vk::Extent2D { width: 4, height: 4 },
    };
    let mirrored = pattern
        .record(vk::CommandBuffer::null(), target, 0, SurfaceTransform::HorizontalMirror)
        .unwrap_err();
    assert!(matches!(mirrored, GpuError::Unsupported { .. }), "{mirrored:?}");
    let empty = PatternTarget {
        view: vk::ImageView::null(),
        extent: vk::Extent2D { width: 0, height: 4 },
    };
    let empty_error = pattern
        .record(vk::CommandBuffer::null(), empty, 0, SurfaceTransform::Identity)
        .unwrap_err();
    assert!(
        matches!(empty_error, GpuError::InvalidArgument { .. }),
        "{empty_error:?}"
    );
}
