//! Проход UI после «композитора» в кадр swapchain (сессия 11) на `HeadlessPresentation` с настоящим
//! драйвером и полной validation (включая синхронизацию): первый проход рисует фон (тестовый паттерн
//! с очисткой цели), `record_between_color_passes` связывает проходы, второй проход рисует прямоугольники
//! с `AttachmentLoad::Preserve` и смешиванием поверх фона, затем кадр показывается.
//!
//! Ограничение стенда: у headless-WSI нет чтения показанных картинок (см. `swapchain_headless.rs`),
//! поэтому здесь доказывается ПРОХОЖДЕНИЕ кадров и отсутствие ошибок validation (в том числе RAW-гонки
//! на загрузке цвета без барьера), а содержимое второго прохода поверх загруженного проверяют
//! тесты `raster_draw.rs` в `ColorTarget` (тот же `RasterPass`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod raster_support;

use ash::vk;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::host_buffer::HostBuffer;
use gpu_vulkan::raster::{
    AttachmentLoad, BlendMode, ColorAttachment, DrawCall, RasterPass, record_between_color_passes,
};
use gpu_vulkan::swapchain::{
    AcquireOutcome, DisplayTarget, DisplayTargetConfig, PresentOutcome, PresentRequest, PresentationSurface,
};
use gpu_vulkan::test_pattern::TestPattern;
use gpu_vulkan::validation::ValidationMode;
use raster_support::{RectInstance, instances_write, rect_buffer, rect_pipeline, target_size_push};
use shell_contract::surface::SurfaceSize;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
const FRAMES: u32 = 12;

fn presentation_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::HeadlessPresentation, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

#[test]
fn ui_pass_after_the_background_pass_presents_frames_without_validation_errors() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let config = DisplayTargetConfig {
            requested_size: Some(SurfaceSize {
                width_px: 97,
                height_px: 61,
            }),
            ..DisplayTargetConfig::default()
        };
        let mut target = DisplayTarget::attach(&context, PresentationSurface::Headless, config)
            .map_err(|error| error.error)
            .expect("attach на headless-surface");
        let info = target.info().unwrap();
        let background = TestPattern::new(&context, info.format).unwrap();
        // Слот набора дескрипторов = слот кольца кадров; буфер экземпляров на слот, чтобы CPU не писал
        // то, что GPU ещё читает.
        let slots = info.frames_in_flight;
        let mut pipeline = rect_pipeline(&context, info.format, BlendMode::PremultipliedAlpha, slots);
        let buffers: Vec<HostBuffer> = (0..slots)
            .map(|_| {
                rect_buffer(
                    &context,
                    &[
                        RectInstance::new(4, 4, 40, 20, [0.5, 0.0, 0.0, 0.5]),
                        RectInstance::new(30, 30, 50, 25, [0.0, 0.25, 0.25, 0.5]),
                    ],
                )
            })
            .collect();
        let mut presented = 0_u32;
        for _ in 0..FRAMES {
            let frame = match target.acquire_frame().expect("acquire_frame") {
                AcquireOutcome::Ready(frame) => frame,
                other => panic!("ожидали Ready, получили {other:?}"),
            };
            let slot = frame.ring_slot();
            // Fence слота кольца уже дождались при acquire: набор слота можно переписывать.
            pipeline.release_slot_after_fence(slot).unwrap();
            pipeline.bind(slot, &[instances_write(&buffers[slot])]).unwrap();
            let command_buffer = frame.command_buffer();
            // Проход 1 («композитор»): фон с очисткой цели.
            background
                .record(
                    command_buffer,
                    frame.pattern_target(),
                    frame.frame_index(),
                    frame.transform(),
                )
                .unwrap();
            record_between_color_passes(&context, command_buffer);
            // Проход 2 (UI): те же пиксели кадра, загрузка содержимого, смешивание поверх.
            let attachment = ColorAttachment::from_display_target(frame.pattern_target(), frame.format());
            let extent: vk::Extent2D = attachment.extent();
            let mut pass = RasterPass::begin(&context, command_buffer, attachment, AttachmentLoad::Preserve).unwrap();
            pass.draw(
                &pipeline,
                slot,
                &target_size_push(extent),
                DrawCall {
                    vertices_per_instance: 6,
                    instance_count: 2,
                    scissor: None,
                },
            )
            .unwrap();
            pass.end();
            match target.present(frame, PresentRequest::default()).expect("present") {
                PresentOutcome::Presented { .. } => presented += 1,
                other => panic!("ожидали Presented, получили {other:?}"),
            }
        }
        assert_eq!(presented, FRAMES);
        context.wait_idle().unwrap();
        target.detach().map_err(|error| error.error).expect("detach");
    }
    assert_eq!(context.live_objects(), baseline);
    context.wait_idle().unwrap();
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}
