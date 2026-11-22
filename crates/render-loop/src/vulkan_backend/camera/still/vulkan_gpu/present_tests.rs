//! Тесты канала показа в кодер на настоящем GPU (headless-surface, validation FULL): показ доходит до `Done` без
//! ожиданий, цель разбирается неблокирующе, а для устройства без fence-ов показа (шов отказов) цель откладывается
//! и разбирается остановкой, не на потоке рендера.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

use std::time::{Duration, Instant};

use ash::vk;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::one_shot_queue::OneShotSubmitter;
use gpu_vulkan::rgba8_image::Rgba8StorageImage;
use gpu_vulkan::validation::ValidationMode;
use shell_contract::surface::NativeSurface;

use super::PresentChannel;
use crate::vulkan_backend::camera::still::gpu::PresentProgress;
use crate::vulkan_backend::camera::still::test_support::fake_encoder_surface;

const EVENTUALLY: Duration = Duration::from_secs(10);

fn context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::HeadlessPresentation, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            assert_eq!(
                std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref(),
                Ok("1"),
                "GPU недоступен: {error}"
            );
            None
        }
    }
}

/// Картинка в раскладке, из которой её читает показ (как после проявления).
fn ready_image(context: &GpuContext) -> Rgba8StorageImage {
    let image = Rgba8StorageImage::new(context, vk::Extent2D { width: 64, height: 48 }).unwrap();
    run_one_shot_blocking(context, |command_buffer| {
        image.record_prepare_for_compute_write(command_buffer);
        image.record_compute_write_to_transfer_read(command_buffer);
        Ok(())
    })
    .unwrap();
    image
}

fn surface() -> NativeSurface {
    fake_encoder_surface()
}

/// Опрашивает показ, пока он не закончится; каждый опрос обязан вернуться быстро (без ожидания GPU).
fn poll_until_finished(channel: &mut PresentChannel, submitter: &mut OneShotSubmitter) -> PresentProgress {
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        let started = Instant::now();
        let progress = channel.poll(submitter);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "опрос показа не должен ждать GPU"
        );
        match progress {
            PresentProgress::Pending => {
                assert!(Instant::now() < deadline, "показ не закончился за {EVENTUALLY:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
            other => return other,
        }
    }
}

#[test]
fn a_tracked_present_finishes_by_polling_and_returns_every_object() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let image = ready_image(&context);
        let mut submitter = OneShotSubmitter::new(&context, 4).unwrap();
        let mut channel = PresentChannel::new(2);
        assert_eq!(
            channel.poll(&mut submitter),
            PresentProgress::Idle,
            "нет показа — нет ответа"
        );
        channel.start(&context, &mut submitter, &image, surface()).unwrap();
        assert!(channel.in_flight());
        assert_eq!(poll_until_finished(&mut channel, &mut submitter), PresentProgress::Done);
        assert!(!channel.in_flight());
        assert_eq!(channel.retired_len(), 0, "с fence-ами показа цель разобрана сразу");
    }
    context.wait_idle().unwrap();
    assert_eq!(
        context.live_objects().total() - baseline.total(),
        0,
        "объектов GPU не осталось"
    );
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn without_present_fences_the_present_completes_at_once_and_the_target_waits_for_the_stop() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let image = ready_image(&context);
        let mut submitter = OneShotSubmitter::new(&context, 4).unwrap();
        let mut channel = PresentChannel::new(2);
        channel.inject_untracked_presents();
        for round in 1..=2 {
            channel.start(&context, &mut submitter, &image, surface()).unwrap();
            assert_eq!(poll_until_finished(&mut channel, &mut submitter), PresentProgress::Done);
            assert_eq!(channel.retired_len(), round, "цель отложена, а не разобрана ожиданием");
        }
        assert!(
            context.live_objects().total() > baseline.total() + 1,
            "отложенные цели ещё держат swapchain"
        );
        // Остановка (GPU простаивает): цели разбираются.
        channel.settle_after_idle(&mut submitter);
        assert_eq!(channel.retired_len(), 0);
    }
    context.wait_idle().unwrap();
    assert_eq!(
        context.live_objects().total() - baseline.total(),
        0,
        "после остановки объектов не осталось"
    );
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn retired_targets_beyond_the_capacity_are_leaked_with_accounting_not_destroyed() {
    let Some(context) = context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let image = ready_image(&context);
        let mut submitter = OneShotSubmitter::new(&context, 4).unwrap();
        let mut channel = PresentChannel::new(1);
        channel.inject_untracked_presents();
        for _ in 0..2 {
            channel.start(&context, &mut submitter, &image, surface()).unwrap();
            assert_eq!(poll_until_finished(&mut channel, &mut submitter), PresentProgress::Done);
        }
        assert_eq!(channel.retired_len(), 1, "держим не больше ёмкости");
        channel.settle_after_idle(&mut submitter);
    }
    context.wait_idle().unwrap();
    assert!(
        context.live_objects().total() > baseline.total(),
        "вытесненная цель осталась в учтённой утечке (видна в живых объектах)"
    );
}
