//! Функциональные тесты компоновщика compute-набора (сессия 07, окно содержимого — сессия 13) на реальном
//! GPU (RADV, полная validation): раскладка по поворотам дисплея, защита слота, подложка, окно/зеркало/
//! увеличение. Узор compute и остальное — `compute_kit.rs`, стенд — `compute_kit_rig`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;
mod compute_kit_rig;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use compute_kit_rig::{MODE_GRADIENT, MODE_QUADRANTS, PatternRig, assert_close, storage_image};
use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource, SubmissionSerial};
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor, LogicalRect};
use gpu_vulkan::compute::{BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, WorkgroupCount};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::{read_host_buffer, read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::offscreen::OffscreenTarget;
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::storage_image::{StorageFormatPolicy, StorageImage, StorageImagePreviousUse};
use gpu_vulkan::swapchain::DisplayColorSpace;
use gpu_vulkan::test_pattern::SurfaceTransform;

/// Логический пиксель → физический для поворота дисплея `turns` (обратное к шейдеру компоновщика).
fn logical_to_physical(logical_x: u32, logical_y: u32, turns: u32, width: u32, height: u32) -> (u32, u32) {
    match turns {
        1 => (logical_y, height - 1 - logical_x),
        2 => (width - 1 - logical_x, height - 1 - logical_y),
        3 => (width - 1 - logical_y, logical_x),
        _ => (logical_x, logical_y),
    }
}

#[test]
fn compositor_places_quadrants_for_every_display_rotation_with_black_letterbox() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let source = storage_image(&context, 8, 8, StorageFormatPolicy::PreferTenBitUnorm);
        let mut rig = PatternRig::new(&context, source.format(), MODE_QUADRANTS);
        rig.bind(0, &source);
        run_one_shot_blocking(&context, |command_buffer| {
            rig.record(
                command_buffer,
                0,
                &source,
                StorageImagePreviousUse::NotUsedYet,
                ConsumerStage::Fragment,
            )
        })
        .unwrap();
        let source_before =
            read_storage_image_rgba(&source, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();

        let (physical_width, physical_height) = (24_u32, 16_u32);
        let mut compositor = ImageCompositor::new(&context, OffscreenTarget::FORMAT, 2).unwrap();
        for transform in SurfaceTransform::ROTATIONS {
            let turns = transform.quarter_turns().unwrap();
            let (logical_width, logical_height) = if transform.swaps_extent() {
                (physical_height, physical_width)
            } else {
                (physical_width, physical_height)
            };
            // Полосы по 2 логических пикселя слева и справа.
            let destination = LogicalRect {
                x: 2,
                y: 0,
                width: logical_width - 4,
                height: logical_height,
            };
            let mut target = OffscreenTarget::new(&context, physical_width, physical_height).unwrap();
            target
                .render(|command_buffer, pattern_target| {
                    compositor.record(
                        command_buffer,
                        pattern_target,
                        &CompositeRequest {
                            source: source.sampled_view(),
                            transform,
                            destination,
                            background: BackgroundColor::BLACK,
                            frame_slot: 1,
                        },
                    )
                })
                .unwrap();
            let pixels = target.readback().unwrap();
            let sample = |logical_x: u32, logical_y: u32| {
                let (x, y) = logical_to_physical(logical_x, logical_y, turns, physical_width, physical_height);
                pixels.pixel(x, y).unwrap()
            };
            let right = destination.x + destination.width - 2;
            let bottom = logical_height - 2;
            assert_eq!(sample(3, 1), [255, 0, 0, 255], "{transform:?}: левая верхняя — красная");
            assert_eq!(
                sample(right, 1),
                [0, 255, 0, 255],
                "{transform:?}: правая верхняя — зелёная"
            );
            assert_eq!(
                sample(3, bottom),
                [0, 0, 255, 255],
                "{transform:?}: левая нижняя — синяя"
            );
            assert_eq!(
                sample(right, bottom),
                [255, 255, 255, 255],
                "{transform:?}: правая нижняя — белая"
            );
            for bar_x in [0, 1, logical_width - 2, logical_width - 1] {
                assert_eq!(
                    sample(bar_x, logical_height / 2),
                    [0, 0, 0, 255],
                    "{transform:?}: полоса letterbox x={bar_x} чёрная"
                );
            }
        }
        // Компоновщик только читал источник.
        let source_after =
            read_storage_image_rgba(&source, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();
        assert_eq!(source_before, source_after);
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

/// Набор слота компоновщика нельзя менять, пока он привязан в командном буфере (без UPDATE_AFTER_BIND это
/// делает буфер недействительным). Тот же источник — без обновления, безопасно; другой — `InvalidState`
/// до записи команд; после `release_slot_after_fence` — снова можно.
#[test]
fn compositor_refuses_to_rebind_a_slot_still_bound_in_a_command_buffer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let policy = StorageFormatPolicy::PreferTenBitUnorm;
    let quadrants = storage_image(&context, 8, 8, policy);
    let white = storage_image(&context, 8, 8, policy);
    let mut quadrant_rig = PatternRig::new(&context, quadrants.format(), MODE_QUADRANTS);
    quadrant_rig.bind(0, &quadrants);
    let mut gradient_rig = PatternRig::new(&context, white.format(), MODE_GRADIENT);
    gradient_rig.bind(0, &white);
    run_one_shot_blocking(&context, |command_buffer| {
        let fresh = StorageImagePreviousUse::NotUsedYet;
        quadrant_rig.record(command_buffer, 0, &quadrants, fresh, ConsumerStage::Fragment)?;
        gradient_rig.record(command_buffer, 0, &white, fresh, ConsumerStage::Fragment)
    })
    .unwrap();

    let mut compositor = ImageCompositor::new(&context, OffscreenTarget::FORMAT, 1).unwrap();
    let request = |source: &StorageImage| CompositeRequest {
        source: source.sampled_view(),
        transform: SurfaceTransform::Identity,
        destination: LogicalRect::covering(vk::Extent2D { width: 8, height: 8 }),
        background: BackgroundColor::BLACK,
        frame_slot: 0,
    };
    let mut target = OffscreenTarget::new(&context, 8, 8).unwrap();
    target
        .render(|command_buffer, pattern_target| {
            // Тот же источник дважды в одном буфере — набор не обновляется, это законно.
            compositor.record(command_buffer, pattern_target, &request(&quadrants))?;
            compositor.record(command_buffer, pattern_target, &request(&quadrants))?;
            // Другой источник в тот же слот того же буфера — отказ до записи команд.
            let error = compositor
                .record(command_buffer, pattern_target, &request(&white))
                .unwrap_err();
            assert!(matches!(error, GpuError::InvalidState { .. }), "{error:?}");
            Ok(())
        })
        .unwrap();
    let before_release = target.readback().unwrap();
    assert_eq!(
        before_release.pixel(1, 1).unwrap(),
        [255, 0, 0, 255],
        "нарисован первый источник"
    );

    // Fence слота увиден (render блокирующий): слот можно переписать другим источником.
    compositor.release_slot_after_fence(0).unwrap();
    target
        .render(|command_buffer, pattern_target| compositor.record(command_buffer, pattern_target, &request(&white)))
        .unwrap();
    let after_release = target.readback().unwrap();
    assert_ne!(
        after_release.pixel(1, 1).unwrap(),
        before_release.pixel(1, 1).unwrap(),
        "теперь нарисован второй источник"
    );
    assert!(compositor.release_slot_after_fence(1).is_err(), "слот вне кольца");
    assert_no_validation_errors(&context);
}

#[test]
fn backdrop_consumer_samples_published_image_through_combined_sampler() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (12_u32, 10_u32);
    let image = storage_image(&context, width, height, StorageFormatPolicy::HalfFloatOnly);
    let mut rig = PatternRig::new(&context, image.format(), MODE_GRADIENT);
    rig.bind(0, &image);
    // Сабмит 1 (производитель): запись и перевод на чтение.
    run_one_shot_blocking(&context, |command_buffer| {
        rig.record(
            command_buffer,
            0,
            &image,
            StorageImagePreviousUse::NotUsedYet,
            ConsumerStage::Fragment,
        )
    })
    .unwrap();
    let backdrop = BackdropSource::publish(
        &image,
        DisplayColorSpace::Srgb,
        BackdropFrameNumber(1),
        SubmissionSerial(1),
    );
    assert!(backdrop.is_ready_for(SubmissionSerial(2)));

    // Сабмит 2 (потребитель «стекло»): барьер подложки + выборка сэмплером в compute.
    let mut sampler_pipeline = ComputePipeline::new(
        &context,
        &ComputePipelineDesc {
            label: "kit-test-sample",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap();
    let output = HostBuffer::new(
        &context,
        u64::from(width * height) * 16,
        HostBufferUsage::ReadWriteStorage,
    )
    .unwrap();
    sampler_pipeline
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: image.read_only_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: output.binding(),
                },
            ],
        )
        .unwrap();
    let push: Vec<u8> = [width, height].iter().flat_map(|word| word.to_ne_bytes()).collect();
    run_one_shot_blocking(&context, |command_buffer| {
        backdrop.record_read_barrier(command_buffer, ConsumerStage::Compute);
        sampler_pipeline.record_dispatch(
            command_buffer,
            0,
            &push,
            WorkgroupCount::covering(width, height, LocalSize::new(8, 8)?),
        )?;
        output.record_compute_write_to_host_read(command_buffer);
        Ok(())
    })
    .unwrap();
    let sampled: Vec<[f32; 4]> = read_host_buffer(&output)
        .unwrap()
        .chunks_exact(16)
        .map(|texel| {
            let channel = |index: usize| {
                f32::from_ne_bytes([
                    texel[index * 4],
                    texel[index * 4 + 1],
                    texel[index * 4 + 2],
                    texel[index * 4 + 3],
                ])
            };
            [channel(0), channel(1), channel(2), channel(3)]
        })
        .collect();
    let direct =
        read_storage_image_rgba(&image, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();
    // В центрах пикселей линейная выборка возвращает ровно тексель.
    for (index, (from_sampler, from_readback)) in sampled.iter().zip(&direct).enumerate() {
        assert_close(*from_sampler, *from_readback, 1.0e-3, &format!("тексель {index}"));
    }
    assert_no_validation_errors(&context);
}

/// Окно источника (сессия 13): компоновщик читает только левый верхний прямоугольник содержимого картинки
/// постоянной ёмкости, с зеркалом и увеличением, и не подмешивает на краю чужие текселя ёмкости. Картинка 16×16 —
/// градиент (R = x/15, G = y/15), содержимое 8×4: правильный ответ в каждом пикселе известен формулой, а
/// ошибка окна (читали всю картинку, не отзеркалили, не зажали край) даёт другие числа.
#[test]
fn compositor_window_reads_the_content_rect_mirrors_magnifies_and_clamps_the_edge() {
    use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};

    let Some(context) = gpu_context_or_skip() else { return };
    let image = storage_image(&context, 16, 16, StorageFormatPolicy::HalfFloatOnly);
    let mut rig = PatternRig::new(&context, image.format(), MODE_GRADIENT);
    rig.bind(0, &image);
    run_one_shot_blocking(&context, |command_buffer| {
        rig.record(
            command_buffer,
            0,
            &image,
            StorageImagePreviousUse::NotUsedYet,
            ConsumerStage::Fragment,
        )
    })
    .unwrap();
    let content = vk::Extent2D { width: 8, height: 4 };
    let mut compositor = ImageCompositor::new(&context, OffscreenTarget::FORMAT, 1).unwrap();
    let mut compose = |window: SourceWindow, target_size: (u32, u32)| {
        // Слот один: набор дескрипторов можно переписывать только после fence (render блокирующий).
        compositor.release_slot_after_fence(0).unwrap();
        let mut target = OffscreenTarget::new(&context, target_size.0, target_size.1).unwrap();
        target
            .render(|command_buffer, pattern_target| {
                compositor.record_windowed(
                    command_buffer,
                    pattern_target,
                    &CompositeRequest {
                        source: image.sampled_view(),
                        transform: SurfaceTransform::Identity,
                        destination: LogicalRect::covering(vk::Extent2D {
                            width: target_size.0,
                            height: target_size.1,
                        }),
                        background: BackgroundColor::BLACK,
                        frame_slot: 0,
                    },
                    &window,
                )
            })
            .unwrap();
        target.readback().unwrap()
    };
    // Ожидаемый цвет пикселя (x, y) цели `size` по эталону окна: R = (texel.x − 0.5)/15, G = (texel.y − 0.5)/15.
    let expected = |window: &SourceWindow, size: (u32, u32), x: u32, y: u32| {
        let relative = [(x as f32 + 0.5) / size.0 as f32, (y as f32 + 0.5) / size.1 as f32];
        let texel = window.mapping().texel_at(relative);
        [(texel[0] - 0.5) / 15.0, (texel[1] - 0.5) / 15.0]
    };
    let mut check = |what: &str, window: SourceWindow, size: (u32, u32)| {
        let pixels = compose(window, size);
        for y in 0..size.1 {
            for x in 0..size.0 {
                let actual = pixels.pixel(x, y).unwrap();
                let [red, green] = expected(&window, size, x, y);
                for (value, channel) in [(red, actual[0]), (green, actual[1])] {
                    let wanted = (value * 255.0).round();
                    assert!(
                        (f32::from(channel) - wanted).abs() <= 2.0,
                        "{what}: пиксель ({x}, {y}): {actual:?}, ожидалось ≈ {wanted}"
                    );
                }
            }
        }
        pixels
    };
    // 1. Без зеркала: пиксель x читает тексель x (R = x/15), а не x·2 (вся картинка 16 в 8 пикселей).
    let plain = check("содержимое", SourceWindow::of_content(content), (8, 4));
    assert_eq!(plain.pixel(3, 0).unwrap()[0], (3.0_f32 / 15.0 * 255.0).round() as u8);
    // 2. Зеркало: левый пиксель читает правый край содержимого (7), а не 0.
    let mirrored = check(
        "зеркало",
        SourceWindow::of_content(content).with_mirror(HorizontalMirror::On),
        (8, 4),
    );
    assert_eq!(mirrored.pixel(0, 0).unwrap()[0], (7.0_f32 / 15.0 * 255.0).round() as u8);
    assert!(mirrored.pixel(0, 0).unwrap()[0] > mirrored.pixel(7, 0).unwrap()[0]);
    // 3. Увеличение ×2 вокруг центра содержимого: видна центральная половина (тексель x = 2 … 6).
    let magnified = check(
        "увеличение",
        SourceWindow::of_content(content).with_magnification(2.0).unwrap(),
        (8, 4),
    );
    assert!(
        magnified.pixel(0, 0).unwrap()[0] > plain.pixel(0, 0).unwrap()[0],
        "левый край сдвинулся внутрь"
    );
    // 4. Растянутое вдвое назначение 16×8: правый край цели не должен подмешать тексель 8 (за содержимым).
    let stretched = check("край при растяжении", SourceWindow::of_content(content), (16, 8));
    assert!(
        stretched.pixel(15, 0).unwrap()[0] <= (7.0_f32 / 15.0 * 255.0).round() as u8 + 1,
        "правый край содержимого не подмешивает текселя за ним: {:?}",
        stretched.pixel(15, 0)
    );
    assert_no_validation_errors(&context);
}
