//! Сквозной функциональный тест: RAW-кадр → `ViewfinderIsp` → `ImageCompositor` (gpu-vulkan) →
//! экранная цель (offscreen) → readback. Кадр доходит до стадии показа, ожидаемые цвета — в
//! ожидаемых областях экрана, с учётом ориентации сенсора, в одной отправке, как у потока рендера.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use ash::vk;
use camera_contract::geometry::SensorOrientation;
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor, LogicalRect};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::offscreen::OffscreenTarget;
use gpu_vulkan::test_pattern::SurfaceTransform;
use isp_vulkan::viewfinder::{
    IspStageSet, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp, ViewfinderStorageFormat,
};
use viewfinder_support::{
    GpuSource, ParamsSpec, RawFrame, SourceLayout, assert_no_validation_errors, gpu_context_or_skip, reference_upright,
    test_config,
};

/// Ошибка 8-битной цели: 10-бит источник → 8 бит (полшага) + округление компоновщика.
const SCREEN_TOLERANCE: i32 = 1;

#[test]
fn raw_frame_reaches_the_screen_with_expected_colors_and_orientation() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let spec = ParamsSpec::default();
        let params = spec.build();
        // Сенсор 32×24: левая половина — серая карта, правая — красный объект.
        let raw = RawFrame::from_linear(32, 24, spec.pattern, spec.levels(), |x, _, channel| {
            if x < 16 {
                0.25 * spec.neutral[channel]
            } else {
                [0.6, 0.08, 0.05][channel]
            }
        });
        let gpu_source = GpuSource::new(&context, &raw, SourceLayout::Raw10 { padding_bytes: 4 });
        let source = gpu_source.source();
        let geometry = ViewfinderGeometry {
            valid_area: raw.full_rect(),
            crop: raw.full_rect(),
            sensor_orientation: SensorOrientation::Degrees90,
        };
        // Прямая картинка 12×16: поворот на 90° по часовой кладёт левую половину сенсора НАВЕРХ.
        let reference = reference_upright(&raw, raw.full_rect(), SensorOrientation::Degrees90, &params);
        assert_eq!((reference.width, reference.height), (12, 16));

        let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::PreferTenBitUnorm)).unwrap();
        let mut compositor = ImageCompositor::new(&context, OffscreenTarget::FORMAT, 2).unwrap();
        // Экран вдвое больше картинки: каждый пиксель видоискателя — квадрат 2×2.
        let mut target = OffscreenTarget::new(&context, 24, 32).unwrap();
        target
            .render(|command_buffer, pattern_target| {
                source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
                let recorded = isp
                    .record(
                        command_buffer,
                        &ViewfinderFrame {
                            source: &source,
                            params: &params,
                            geometry,
                            display_color_space: spec.display(),
                            stages: IspStageSet::ALL,
                            frame: BackdropFrameNumber(1),
                            frame_slot: 0,
                            reader: ConsumerStage::Fragment,
                        },
                    )
                    .map_err(|error| GpuError::InvalidState {
                        what: error.to_string(),
                    })?;
                compositor.record(
                    command_buffer,
                    pattern_target,
                    &CompositeRequest {
                        source: recorded.output,
                        transform: SurfaceTransform::Identity,
                        destination: LogicalRect::covering(vk::Extent2D { width: 24, height: 32 }),
                        background: BackgroundColor::BLACK,
                        frame_slot: 0,
                    },
                )?;
                source.record_release_for(&context, command_buffer, ConsumerStage::Compute);
                Ok(())
            })
            .unwrap();
        let screen = target.readback().unwrap();

        // Ожидаемые цвета: эталон в центре каждой половины прямой картинки, переведённый в 8 бит.
        let expected_at = |upright_x: u32, upright_y: u32| {
            reference.pixels[(upright_y * reference.width + upright_x) as usize]
                .map(|value| (value * 255.0).round() as i32)
        };
        let gray = expected_at(6, 4);
        let red = expected_at(6, 12);
        assert!(red[0] - red[1] > 60, "эталон красного действительно красный: {red:?}");
        assert!(
            (gray[0] - gray[1]).abs() < 10,
            "эталон серого действительно серый: {gray:?}"
        );
        // Внутренние точки однородных областей (подальше от границы половин — у неё линейная выборка
        // смешивает цвета).
        for (screen_x, screen_y, expected, what) in [
            (4, 4, gray, "верх — серая карта"),
            (20, 10, gray, "верх — серая карта"),
            (4, 22, red, "низ — красный объект"),
            (20, 28, red, "низ — красный объект"),
        ] {
            let pixel = screen.pixel(screen_x, screen_y).unwrap();
            for channel in 0..3 {
                let difference = (i32::from(pixel[channel]) - expected[channel]).abs();
                assert!(
                    difference <= SCREEN_TOLERANCE,
                    "({screen_x}, {screen_y}) {what}: экран {pixel:?}, ожидалось {expected:?}"
                );
            }
        }
        drop(compositor);
        drop(isp);
    }
    context.wait_idle().unwrap();
    assert_eq!(context.live_objects(), baseline, "всё освобождено");
    assert_no_validation_errors(&context);
}
