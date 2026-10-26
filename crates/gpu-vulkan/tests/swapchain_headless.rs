//! Сквозные тесты показа на ПК: `DisplayTarget` на `VK_EXT_headless_surface` с настоящим драйвером
//! и полной validation (включая синхронизацию). Любая ошибка validation = провал теста.
//!
//! Ограничение стенда: у headless-WSI нет чтения показанных картинок, поэтому содержимое кадров
//! здесь проверить нельзя — оно проверяется паритетными тестами `offscreen_pattern` (тот же
//! `TestPattern`). Здесь доказывается другое: кадры реально проходят acquire → рисование →
//! переходы раскладки → submit → present, пересоздание работает, ничего не течёт.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::time::Duration;

use ash::vk;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::{
    AcquireOutcome, ColorDepthPreference, DisplayFrame, DisplayTarget, DisplayTargetConfig, FramesInFlight,
    PresentOutcome, PresentRequest, PresentationSurface, RecreateOutcome, SwapchainError,
};
use gpu_vulkan::test_pattern::{SurfaceTransform, TestPattern};
use gpu_vulkan::validation::ValidationMode;
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
/// Нечётные размеры ловят ошибки округления и перепутанные оси.
const START_SIZE: SurfaceSize = SurfaceSize {
    width_px: 321,
    height_px: 201,
};
const RESIZED: SurfaceSize = SurfaceSize {
    width_px: 257,
    height_px: 145,
};

/// Контекст профиля `HeadlessPresentation` с полной validation; нет GPU — провал, если не задан пропуск.
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

fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

fn config_with_size(size: SurfaceSize) -> DisplayTargetConfig {
    DisplayTargetConfig {
        requested_size: Some(size),
        ..DisplayTargetConfig::default()
    }
}

fn attach(context: &GpuContext, config: DisplayTargetConfig) -> DisplayTarget {
    DisplayTarget::attach(context, PresentationSurface::Headless, config)
        .map_err(|error| error.error)
        .expect("attach на headless-surface")
}

/// Берёт кадр; таймаут или NeedsRecreate здесь — провал (на headless их быть не должно).
fn acquire_ready(target: &mut DisplayTarget) -> DisplayFrame {
    match target.acquire_frame().expect("acquire_frame") {
        AcquireOutcome::Ready(frame) => frame,
        other => panic!("ожидали Ready, получили {other:?}"),
    }
}

/// Рисует паттерн в кадр и показывает; возвращает номер кадра и итог.
fn draw_and_present(
    target: &mut DisplayTarget,
    pattern: &TestPattern,
    request: PresentRequest,
) -> (u32, PresentOutcome) {
    let frame = acquire_ready(target);
    let frame_index = frame.frame_index();
    pattern
        .record(
            frame.command_buffer(),
            frame.pattern_target(),
            frame_index,
            frame.transform(),
        )
        .expect("запись паттерна в кадр swapchain");
    let outcome = target.present(frame, request).expect("present");
    (frame_index, outcome)
}

#[test]
fn thirty_frames_resize_midway_and_detach_leave_no_objects_and_no_validation_errors() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let mut target = attach(&context, config_with_size(START_SIZE));
        let first_info = target.info().unwrap();
        assert_eq!(
            first_info.extent,
            vk::Extent2D {
                width: 321,
                height: 201
            }
        );
        assert_eq!(first_info.frames_in_flight, 2);
        let pattern = TestPattern::new(&context, first_info.format).expect("пайплайн под формат swapchain");

        let mut presented_ids = Vec::new();
        for step in 0..30 {
            if step == 15 {
                let outcome = target.recreate(Some(RESIZED)).expect("recreate");
                assert!(
                    matches!(outcome, RecreateOutcome::Recreated { format_changed: false }),
                    "{outcome:?}"
                );
                let info = target.info().unwrap();
                assert_eq!(
                    info.extent,
                    vk::Extent2D {
                        width: 257,
                        height: 145
                    }
                );
                assert_eq!(info.recreation_count, 1);
            }
            let expected_extent = if step < 15 {
                vk::Extent2D {
                    width: 321,
                    height: 201,
                }
            } else {
                vk::Extent2D {
                    width: 257,
                    height: 145,
                }
            };
            let frame = acquire_ready(&mut target);
            assert_eq!(frame.pattern_target().extent, expected_extent, "шаг {step}");
            let frame_index = frame.frame_index();
            pattern
                .record(
                    frame.command_buffer(),
                    frame.pattern_target(),
                    frame_index,
                    frame.transform(),
                )
                .unwrap();
            match target.present(frame, PresentRequest::default()).expect("present") {
                PresentOutcome::Presented { present_id } => presented_ids.push(present_id),
                other => panic!("шаг {step}: ожидали Presented, получили {other:?}"),
            }
        }
        // Номера кадров идут подряд и без пропусков: ни один кадр не потерян и не продублирован.
        assert_eq!(presented_ids, (0..30).collect::<Vec<u32>>());

        // На ПК-стенде display timing нет: это «не поддерживается», а не ошибка.
        assert!(!target.info().unwrap().display_timing_supported);
        assert_eq!(target.refresh_cycle().unwrap(), None);
        assert!(target.drain_past_presentations().unwrap().is_empty());

        let surface = target.detach().map_err(|error| error.error).expect("detach");
        assert!(matches!(surface, PresentationSurface::Headless));
    }
    assert_eq!(
        context.live_objects(),
        baseline,
        "после detach и drop паттерна объектов не больше исходного"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn twenty_attach_present_detach_cycles_do_not_grow_live_objects() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    for cycle in 0..20 {
        let mut target = attach(&context, config_with_size(START_SIZE));
        let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
        for _ in 0..3 {
            let (_, outcome) = draw_and_present(&mut target, &pattern, PresentRequest::default());
            assert!(
                matches!(outcome, PresentOutcome::Presented { .. }),
                "цикл {cycle}: {outcome:?}"
            );
        }
        target.detach().map_err(|error| error.error).expect("detach");
        drop(pattern);
        assert_eq!(
            context.live_objects(),
            baseline,
            "рост объектов на цикле {cycle}:\n{}",
            context.live_objects()
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn dropping_target_without_detach_releases_everything_in_the_same_order() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();
    {
        let mut target = attach(&context, config_with_size(START_SIZE));
        let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
        draw_and_present(&mut target, &pattern, PresentRequest::default());
        draw_and_present(&mut target, &pattern, PresentRequest::default());
        // Цель уничтожается без detach ПЕРВОЙ: её Drop дожидается GPU, и только потом можно
        // уничтожать паттерн (иначе пайплайн ещё занят записанными кадрами — validation это ловит).
        drop(target);
        drop(pattern);
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn unfinished_frame_blocks_next_acquire_and_recreate_until_abandoned() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut target = attach(&context, config_with_size(START_SIZE));
    let frame = acquire_ready(&mut target);

    let second = target.acquire_frame().unwrap_err();
    assert!(
        matches!(second, SwapchainError::Gpu(GpuError::InvalidState { .. })),
        "{second:?}"
    );
    let recreate = target.recreate(None).unwrap_err();
    assert!(
        matches!(recreate, SwapchainError::Gpu(GpuError::InvalidState { .. })),
        "{recreate:?}"
    );

    // abandon_frame показывает недорисованный кадр и освобождает цель для следующего.
    let outcome = target.abandon_frame(frame).unwrap();
    assert!(
        matches!(outcome, PresentOutcome::Presented { present_id: 0 }),
        "{outcome:?}"
    );
    let next = acquire_ready(&mut target);
    assert_eq!(next.frame_index(), 1);
    target.abandon_frame(next).unwrap();
    target.detach().map_err(|error| error.error).unwrap();
    assert_no_validation_errors(&context);
}

#[test]
fn desired_present_time_is_accepted_without_display_timing_extension() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut target = attach(&context, config_with_size(START_SIZE));
    let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
    let request = PresentRequest {
        desired_present_time: Some(MonotonicNanos::from_nanos(123_456_789)),
    };
    let (frame_index, outcome) = draw_and_present(&mut target, &pattern, request);
    assert_eq!(
        outcome,
        PresentOutcome::Presented {
            present_id: frame_index
        }
    );
    target.detach().map_err(|error| error.error).unwrap();
    assert_no_validation_errors(&context);
}

#[test]
fn zero_sized_recreate_is_deferred_then_recovers_with_real_size() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let mut target = attach(&context, config_with_size(START_SIZE));
    let pattern = TestPattern::new(&context, target.info().unwrap().format).unwrap();
    draw_and_present(&mut target, &pattern, PresentRequest::default());

    let zero = SurfaceSize {
        width_px: 0,
        height_px: 480,
    };
    let outcome = target.recreate(Some(zero)).unwrap();
    assert!(matches!(outcome, RecreateOutcome::Deferred(_)), "{outcome:?}");
    assert!(
        target.needs_recreate().unwrap(),
        "пока размера нет, swapchain считается устаревшим"
    );
    assert!(matches!(target.acquire_frame().unwrap(), AcquireOutcome::NeedsRecreate));
    assert_eq!(
        target.info().unwrap().recreation_count,
        0,
        "отложенное пересоздание не считается"
    );

    let recovered = target.recreate(Some(RESIZED)).unwrap();
    assert!(matches!(recovered, RecreateOutcome::Recreated { .. }), "{recovered:?}");
    assert!(!target.needs_recreate().unwrap());
    let (_, outcome) = draw_and_present(&mut target, &pattern, PresentRequest::default());
    assert!(matches!(outcome, PresentOutcome::Presented { .. }));
    target.detach().map_err(|error| error.error).unwrap();
    assert_no_validation_errors(&context);
}

#[test]
fn failed_attach_returns_the_surface_and_creates_nothing() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let baseline = context.live_objects();

    // Размер не задан, а headless-surface его не диктует: InvalidArgument, окно возвращено.
    let no_size = DisplayTarget::attach(&context, PresentationSurface::Headless, DisplayTargetConfig::default())
        .expect_err("без размера attach невозможен");
    assert!(
        matches!(no_size.error, SwapchainError::Gpu(GpuError::InvalidArgument { .. })),
        "{:?}",
        no_size.error
    );
    assert!(matches!(no_size.surface, PresentationSurface::Headless));

    // Нулевой размер: типизированный ZeroExtent.
    let zero = DisplayTarget::attach(
        &context,
        PresentationSurface::Headless,
        config_with_size(SurfaceSize {
            width_px: 0,
            height_px: 10,
        }),
    )
    .expect_err("нулевой размер");
    assert_eq!(zero.error, SwapchainError::ZeroExtent);
    assert_eq!(context.live_objects(), baseline, "ошибка attach не оставляет объектов");

    // Контекст без WSI (профиль Headless): честный ExtensionNotEnabled, а не паника.
    let plain = GpuContext::new(&GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL)).unwrap();
    let plain_baseline = plain.live_objects();
    let unsupported = DisplayTarget::attach(&plain, PresentationSurface::Headless, config_with_size(START_SIZE))
        .expect_err("без VK_EXT_headless_surface");
    assert!(
        matches!(
            unsupported.error,
            SwapchainError::Gpu(GpuError::ExtensionNotEnabled { .. })
        ),
        "{:?}",
        unsupported.error
    );
    assert_eq!(plain.live_objects(), plain_baseline);
    assert_no_validation_errors(&context);
    assert_no_validation_errors(&plain);
}

#[test]
fn chosen_format_always_builds_a_test_pattern_and_depth_preference_is_honoured() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };

    let mut preferred = attach(&context, config_with_size(START_SIZE));
    let preferred_info = preferred.info().unwrap();
    eprintln!("headless-формат по умолчанию: {:?}", preferred_info.format);
    let ten_bit = matches!(
        preferred_info.format,
        vk::Format::A2B10G10R10_UNORM_PACK32 | vk::Format::A2R10G10B10_UNORM_PACK32
    );
    // Паттерн обязан строиться под ту глубину, которую выбрала политика (в том числе 10-бит).
    let pattern = TestPattern::new(&context, preferred_info.format).expect("паттерн под выбранный формат");
    let (_, outcome) = draw_and_present(&mut preferred, &pattern, PresentRequest::default());
    assert!(matches!(outcome, PresentOutcome::Presented { .. }));
    preferred.detach().map_err(|error| error.error).unwrap();

    // Принудительно 8 бит: UNORM8 вне зависимости от того, что умеет surface.
    let eight_bit_config = DisplayTargetConfig {
        color_depth: ColorDepthPreference::EightBitOnly,
        ..config_with_size(START_SIZE)
    };
    let eight = attach(&context, eight_bit_config);
    let eight_format = eight.info().unwrap().format;
    assert!(
        matches!(eight_format, vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM),
        "{eight_format:?}"
    );
    TestPattern::new(&context, eight_format).expect("паттерн UNORM8");
    eight.detach().map_err(|error| error.error).unwrap();
    if ten_bit {
        assert_ne!(
            preferred_info.format, eight_format,
            "10-бит предпочтён и отличается от 8-бит"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn single_frame_in_flight_ring_still_pipelines_frames_and_headless_reports_identity_transform() {
    let Some(context) = presentation_context_or_skip() else {
        return;
    };
    let config = DisplayTargetConfig {
        frames_in_flight: FramesInFlight::new(1).unwrap(),
        acquire_timeout: Duration::from_millis(500),
        ..config_with_size(START_SIZE)
    };
    let mut target = attach(&context, config);
    let info = target.info().unwrap();
    assert_eq!(info.frames_in_flight, 1);
    assert_eq!(info.transform, SurfaceTransform::Identity);
    assert_eq!(info.present_mode, vk::PresentModeKHR::FIFO);
    let pattern = TestPattern::new(&context, info.format).unwrap();
    for expected in 0..10_u32 {
        let (frame_index, outcome) = draw_and_present(&mut target, &pattern, PresentRequest::default());
        assert_eq!(frame_index, expected);
        assert!(matches!(outcome, PresentOutcome::Presented { .. }));
    }
    target.detach().map_err(|error| error.error).unwrap();
    assert_no_validation_errors(&context);
}
