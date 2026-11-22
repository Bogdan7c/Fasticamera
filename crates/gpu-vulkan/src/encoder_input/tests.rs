//! Тесты цели кодера на настоящем GPU: headless-surface (`VK_EXT_headless_surface`, RADV), validation FULL.
//!
//! Ограничение стенда: у headless-WSI нет чтения показанных картинок, поэтому БАЙТЫ в swapchain здесь не
//! проверить. Доказывается другое: исходная картинка с узором настоящая и не портится показом, копирование
//! проходит acquire → blit → present на настоящем драйвере без единой ошибки validation (раскладки,
//! барьеры, синхронизация), работа завершается (`poll → Completed`), поверхность возвращается, ничего не течёт.

use std::time::{Duration, Instant};

use ash::vk;

use super::*;
use crate::context::GpuContext;
use crate::context::test_support::presentation_context_or_skip;
use crate::host_buffer::{HostBuffer, HostBufferUsage};
use crate::one_shot_queue::{CompletedWork, OneShotSubmitter, WorkFailure, WorkStatus};
use crate::rgba8_image::Rgba8StorageImage;
use crate::swapchain::PresentationSurface;

const EVENTUALLY: Duration = Duration::from_secs(10);

/// Цвет пикселя узора: разные для x, y и их сочетания, поэтому перепутанные оси/сдвиги видны.
fn pattern_pixel(x: u32, y: u32) -> [u8; 4] {
    [(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255]
}

fn pattern_bytes(extent: vk::Extent2D) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(extent.width as usize * extent.height as usize * 4);
    for y in 0..extent.height {
        for x in 0..extent.width {
            bytes.extend_from_slice(&pattern_pixel(x, y));
        }
    }
    bytes
}

/// Картинка с узором в раскладке `TRANSFER_SOURCE_LAYOUT` (как её оставляет проявка снимка).
fn patterned_image(context: &GpuContext, extent: vk::Extent2D) -> Rgba8StorageImage {
    let image = Rgba8StorageImage::new(context, extent).unwrap();
    let bytes = pattern_bytes(extent);
    let mut staging = HostBuffer::new(context, bytes.len() as u64, HostBufferUsage::ReadOnlyStorage).unwrap();
    staging.write_bytes(0, &bytes).unwrap();
    crate::kit_test_support::run_one_shot_blocking(context, |command_buffer| {
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let to_dst = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image.handle())
            .subresource_range(range)];
        let to_src = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image.handle())
            .subresource_range(range)];
        let region = [vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })];
        // SAFETY: командный буфер в состоянии записи; картинка с usage TRANSFER_DST, буфер — TRANSFER_SRC.
        unsafe {
            let device = context.device();
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_dst),
            );
            device.cmd_copy_buffer_to_image(
                command_buffer,
                staging.buffer_handle(),
                image.handle(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &region,
            );
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_src),
            );
        }
        Ok(())
    })
    .unwrap();
    image
}

fn wait_completed(submitter: &mut OneShotSubmitter, work: &crate::one_shot_queue::SubmittedWork) -> CompletedWork {
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        match submitter.poll(work) {
            WorkStatus::Completed(proof) => return proof,
            WorkStatus::Pending => {
                assert!(Instant::now() < deadline, "работа не завершилась за {EVENTUALLY:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
            WorkStatus::Failed(failure) => panic!("опрос не удался: {failure:?}"),
        }
    }
}

fn assert_clean(context: &GpuContext) {
    context.wait_idle().unwrap();
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

fn extent(width: u32, height: u32) -> vk::Extent2D {
    vk::Extent2D { width, height }
}

/// Полный цикл: attach → present_once → poll → detach; размеры «круглый» и нечётный.
fn present_cycle(context: &GpuContext, size: vk::Extent2D) {
    let baseline = context.live_objects();
    {
        let image = patterned_image(context, size);
        let expected = pattern_bytes(size);
        let mut submitter = OneShotSubmitter::new(context, 2).unwrap();
        let mut target = EncoderInputTarget::attach(context, PresentationSurface::Headless, size).unwrap();
        let info = target.info().unwrap();
        assert_eq!((info.extent.width, info.extent.height), (size.width, size.height));
        assert!(
            info.usage.contains(vk::ImageUsageFlags::TRANSFER_DST),
            "usage: {:?}",
            info.usage
        );
        assert_eq!(info.color_space, vk::ColorSpaceKHR::SRGB_NONLINEAR);
        assert!(info.image_count >= 1);

        let work = target.present_once(&mut submitter, &image).unwrap();
        let proof = wait_completed(&mut submitter, &work);
        assert_eq!(proof.id(), work.id());

        // Показ не испортил исходник: узор на месте, а картинка осталась в раскладке копирования.
        assert!(
            image.read_back_blocking().unwrap() == expected,
            "исходная картинка {}x{} изменилась после показа",
            size.width,
            size.height
        );
        let surface = target.detach(&proof).unwrap();
        assert!(matches!(surface, PresentationSurface::Headless));
        assert_eq!(submitter.in_flight_count(), 0);
    }
    assert_eq!(context.live_objects(), baseline, "после detach живых объектов нет");
    assert_clean(context);
}

#[test]
fn presents_a_4096x3072_pattern_and_returns_the_surface_without_leaks() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    present_cycle(&context, extent(4096, 3072));
}

#[test]
fn presents_an_odd_1023x767_pattern_and_returns_the_surface_without_leaks() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    present_cycle(&context, extent(1023, 767));
}

#[test]
fn second_present_is_refused_without_touching_the_gpu() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let size = extent(64, 48);
        let image = patterned_image(&context, size);
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        let mut target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, size).unwrap();
        let work = target.present_once(&mut submitter, &image).unwrap();
        let in_flight = submitter.in_flight_count();
        let second = target.present_once(&mut submitter, &image);
        assert!(
            matches!(
                second,
                Err(PresentOnceError::NotSubmitted(EncoderTargetError::AlreadyPresented))
            ),
            "{second:?}"
        );
        assert_eq!(submitter.in_flight_count(), in_flight, "отказ ничего не отправил");
        let proof = wait_completed(&mut submitter, &work);
        target.detach(&proof).unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn image_of_other_size_is_refused_and_target_stays_reusable() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let size = extent(64, 48);
        let wrong = patterned_image(&context, extent(48, 64));
        let right = patterned_image(&context, size);
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        let mut target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, size).unwrap();
        let refused = target.present_once(&mut submitter, &wrong);
        assert!(
            matches!(
                refused,
                Err(PresentOnceError::NotSubmitted(
                    EncoderTargetError::ImageExtentMismatch { .. }
                ))
            ),
            "{refused:?}"
        );
        assert_eq!(submitter.in_flight_count(), 0);
        // Отказ до acquire оставил цель пригодной: верная картинка показывается.
        let work = target.present_once(&mut submitter, &right).unwrap();
        let proof = wait_completed(&mut submitter, &work);
        target.detach(&proof).unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn detach_with_proof_of_another_work_returns_the_target_untouched() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let size = extent(32, 32);
        let image = patterned_image(&context, size);
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        let mut target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, size).unwrap();
        let work = target.present_once(&mut submitter, &image).unwrap();
        let proof = wait_completed(&mut submitter, &work);
        // Чужая работа: завершённая пустая отправка.
        let other = submitter.submit(Vec::new(), |_| Ok::<(), GpuError>(())).unwrap();
        let other_proof = wait_completed(&mut submitter, &other);
        assert_ne!(other_proof.id(), proof.id());

        let target = match target.detach(&other_proof) {
            Err(EncoderDetachError::ProofMismatch { target }) => target,
            other => panic!("ожидали ProofMismatch, получили {other:?}"),
        };
        assert!(target.info().is_some(), "цель не разобрана");
        // Разбор без доказательства при работе в полёте тоже не ломает цель.
        let target = match target.detach_unpresented() {
            Err(EncoderDetachError::ProofRequired { target }) => target,
            other => panic!("ожидали ProofRequired, получили {other:?}"),
        };
        target.detach(&proof).unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn unused_target_detaches_without_waiting_and_returns_the_surface() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    let target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, extent(16, 16)).unwrap();
    assert!(context.live_objects().total() > baseline.total());
    let surface = target.detach_unpresented().unwrap();
    assert!(matches!(surface, PresentationSurface::Headless));
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

#[test]
fn attach_with_zero_extent_returns_the_surface_and_creates_nothing() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    let refused = EncoderInputTarget::attach(&context, PresentationSurface::Headless, extent(0, 10)).unwrap_err();
    assert!(
        matches!(refused.error, EncoderTargetError::ZeroExtent),
        "{:?}",
        refused.error
    );
    assert!(
        matches!(refused.surface, PresentationSurface::Headless),
        "поверхность возвращена"
    );
    assert_eq!(context.live_objects(), baseline, "частично созданное уничтожено");
}

#[test]
fn foreign_submitter_work_cannot_prove_completion_of_target_work() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let size = extent(16, 16);
    let image = patterned_image(&context, size);
    let mut submitter = OneShotSubmitter::new(&context, 1).unwrap();
    let mut stranger = OneShotSubmitter::new(&context, 1).unwrap();
    let mut target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, size).unwrap();
    let work = target.present_once(&mut submitter, &image).unwrap();
    assert!(matches!(
        stranger.poll(&work),
        WorkStatus::Failed(WorkFailure::ForeignWork)
    ));
    let proof = wait_completed(&mut submitter, &work);
    target.detach(&proof).unwrap();
    drop((submitter, stranger, image));
    assert_clean(&context);
}

/// Неблокирующий разбор: после завершения копирования `try_detach` не ждёт показ — отвечает «готово» либо
/// «ещё идёт» (тогда опрашиваем позже), а итог тот же, что у `detach`: поверхность возвращена, объектов нет.
#[test]
fn try_detach_polls_presents_without_waiting_and_returns_the_surface() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let size = extent(64, 48);
        let image = patterned_image(&context, size);
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        let mut target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, size).unwrap();
        let work = target.present_once(&mut submitter, &image).unwrap();
        let proof = wait_completed(&mut submitter, &work);
        // Чужое доказательство: цель возвращается нетронутой и после этого.
        let other = submitter.submit(Vec::new(), |_| Ok::<(), GpuError>(())).unwrap();
        let other_proof = wait_completed(&mut submitter, &other);
        let mut target = match target.try_detach(&other_proof) {
            Err(EncoderDetachError::ProofMismatch { target }) => *target,
            other => panic!("ожидали ProofMismatch, получили {other:?}"),
        };
        let deadline = Instant::now() + EVENTUALLY;
        let surface = loop {
            let started = Instant::now();
            let progress = target.try_detach(&proof).unwrap();
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "try_detach не должен ждать показ"
            );
            match progress {
                DetachProgress::Detached(surface) => break surface,
                DetachProgress::PresentsPending(pending) => {
                    assert!(Instant::now() < deadline, "показ не закончился за {EVENTUALLY:?}");
                    target = *pending;
                    std::thread::sleep(Duration::from_millis(1));
                }
                DetachProgress::DeviceWideWaitRequired(_) => {
                    panic!("RADV поддерживает maintenance1: опрос fence'ов обязан работать")
                }
            }
        };
        assert!(matches!(surface, PresentationSurface::Headless));
    }
    assert_eq!(context.live_objects(), baseline, "после try_detach живых объектов нет");
    assert_clean(&context);
}

/// Без `VK_EXT_swapchain_maintenance1` показ опросом не отследить: `try_detach` честно говорит
/// «нужна остановка устройства», цель возвращается нетронутой и разбирается обычным `detach`.
#[test]
fn try_detach_without_present_fences_asks_for_device_wide_wait_and_keeps_the_target() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let size = extent(32, 32);
        let image = patterned_image(&context, size);
        let mut submitter = OneShotSubmitter::new(&context, 2).unwrap();
        let mut target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, size).unwrap();
        target.force_device_wide_wait_for_test();
        let work = target.present_once(&mut submitter, &image).unwrap();
        let proof = wait_completed(&mut submitter, &work);
        let target = match target.try_detach(&proof).unwrap() {
            DetachProgress::DeviceWideWaitRequired(target) => *target,
            other => panic!("ожидали DeviceWideWaitRequired, получили {other:?}"),
        };
        assert!(target.info().is_some(), "цель не разобрана и не потеряна");
        assert!(matches!(target.detach(&proof).unwrap(), PresentationSurface::Headless));
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}

/// Цель, в которую ничего не отправляли, разбирается неблокирующим путём сразу.
#[test]
fn try_detach_unpresented_releases_an_unused_target_immediately() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    let target = EncoderInputTarget::attach(&context, PresentationSurface::Headless, extent(16, 16)).unwrap();
    match target.try_detach_unpresented().unwrap() {
        DetachProgress::Detached(surface) => assert!(matches!(surface, PresentationSurface::Headless)),
        other => panic!("ожидали Detached, получили {other:?}"),
    }
    assert_eq!(context.live_objects(), baseline);
    assert_clean(&context);
}
