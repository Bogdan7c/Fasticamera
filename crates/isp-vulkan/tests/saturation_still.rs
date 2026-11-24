//! Адаптер `StillSaturation` (проявление снимка целиком) под стендом насыщения на ПК (RADV, полная validation).
//!
//! Итог: картинка снимка, проявленная в кадрах стенда, совпадает бит в бит с проявлением обычным путём
//! (`PhotoDeveloper` по полосам, каждая в своей отправке) на том же RAW и параметрах; миниатюры готовы.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::duplicate_mod,
    reason = "в тестах падение и есть способ сообщить о провале; photo_support и viewfinder_support оба подключают общий support isp-model"
)]

mod photo_support;
mod quality_support;
mod saturation_support;

use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::saturation::{CancelToken, SaturationBench, SaturationError};
use isp_model::image::DisplayGamut;
use isp_vulkan::photo::{PhotoConfig, RawCapturer, RgbaReadback};
use isp_vulkan::saturation::{
    SaturationWorkError, StillSaturation, StillSaturationConfig, synthesize_raw10, synthetic_raw10_holder,
};
use photo_support::{Placement, SourceFrame, SourceKind, capture, develop, developer_for, upload};
use saturation_support::viewfinder_support::{assert_no_validation_errors, gpu_context_or_skip};
use saturation_support::{quality_params, quick_bench_config, run_quick, serialize_gpu_timing};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;

fn small_config() -> StillSaturationConfig {
    StillSaturationConfig {
        frame_size: PixelSize::new(WIDTH, HEIGHT),
        photo: PhotoConfig {
            slice_rows: 64,
            ..PhotoConfig::default()
        },
        ..StillSaturationConfig::default()
    }
}

/// Обычный путь: тот же RAW, захват и проявление по полосам в отдельных отправках, картинка прочитана с GPU.
fn plain_image(context: &gpu_vulkan::context::GpuContext, config: &StillSaturationConfig) -> Vec<u8> {
    let frame = synthesize_raw10(config.frame_size, config.content, config.raw10_row_padding_bytes).unwrap();
    let template = photo_support::real_frame("real-main-lamp-gray-tile");
    let mosaic = isp_model::image::RawMosaic::new(config.frame_size, template.raw.pattern(), frame.samples).unwrap();
    let source = SourceFrame {
        name: "saturation-still".to_owned(),
        raw: mosaic,
        params: quality_params(DisplayGamut::Srgb),
        sensor: template.sensor,
        neutral: template.neutral,
    };
    let gpu_frame = upload(
        context,
        &source.raw,
        SourceKind::Raw10,
        Placement {
            left: 0,
            top: 0,
            right_padding: 0,
            bottom_padding: 0,
            padding_value: 0,
        },
    );
    let mut capturer = RawCapturer::new(context, &config.photo).unwrap();
    let raw = capture(context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(context, config.photo.slice_rows, config.frame_size.width);
    let crop = PixelRect {
        left: 0,
        top: 0,
        size: config.frame_size,
    };
    develop(context, &mut developer, &raw, &source, crop, RgbaReadback::NotNeeded).image_rgba
}

#[test]
fn bench_developments_are_finished_with_thumbnails_and_match_the_plain_path() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = small_config();
    let mut work = StillSaturation::new_synthetic(&context, config, quality_params(DisplayGamut::Srgb)).unwrap();
    // Полосы 64 строки: 3 полосы по высоте кадра 192 + отделка (миниатюра) — больше одного шага на снимок.
    assert!(work.steps_per_still() >= 3, "{}", work.steps_per_still());
    let report = run_quick(&context, &mut work, 2);
    context.wait_idle().unwrap();
    work.finish_pending_blocking().unwrap();
    assert!(work.take_failure().is_none());

    let counters = work.counters();
    let frames = report.total_completed_frames();
    assert_eq!((counters.frames_recorded, counters.frames_released), (frames, frames));
    // Каждый записанный кадр — завершённая проявка с миниатюрой (ни одна не потеряна в ожидании маркера).
    assert_eq!(counters.developments_finished, frames);
    assert_eq!(counters.thumbnails_ready, frames);
    // Времён по проходам у снимка нет: все кадры «без замера», расхождений нет.
    let times = work.pass_times();
    assert_eq!(
        (times.measured_frames, times.unmeasured_frames, times.mismatched_frames),
        (0, frames, 0)
    );
    // Результат: картинка снимка == обычный путь бит в бит; не вырождена.
    let via_bench = work.read_last_image_blocking().unwrap();
    let plain = plain_image(&context, work.config());
    assert_eq!(via_bench.len(), (WIDTH * HEIGHT * 4) as usize);
    assert!(via_bench == plain, "снимок кадров стенда отличается от обычного пути");
    assert!(via_bench.iter().any(|&byte| byte != via_bench[0]));
    drop(work);
    assert_no_validation_errors(&context);
}

#[test]
fn wrong_config_is_typed_error_and_too_many_stand_slots_fail() {
    let Some(context) = gpu_context_or_skip() else { return };
    for config in [
        StillSaturationConfig {
            frame_size: PixelSize::new(0, HEIGHT),
            ..small_config()
        },
        StillSaturationConfig {
            frame_size: PixelSize::new(WIDTH, HEIGHT + 1),
            ..small_config()
        },
        StillSaturationConfig {
            frame_slots: 0,
            ..small_config()
        },
        StillSaturationConfig {
            photo: PhotoConfig {
                slice_rows: 3,
                ..PhotoConfig::default()
            },
            ..small_config()
        },
    ] {
        assert!(config.validate().is_err(), "{config:?}");
    }
    let other = synthesize_raw10(PixelSize::new(64, 32), small_config().content, 0).unwrap();
    let holder = synthetic_raw10_holder(&context, &other).unwrap();
    assert!(matches!(
        StillSaturation::new(&context, small_config(), quality_params(DisplayGamut::Srgb), holder),
        Err(SaturationWorkError::InvalidConfig { .. })
    ));
    let _serial = serialize_gpu_timing();
    let mut work =
        StillSaturation::new_synthetic(&context, small_config(), quality_params(DisplayGamut::Srgb)).unwrap();
    let mut bench = SaturationBench::new(&context, quick_bench_config(3)).unwrap();
    let result = bench.run(&mut work, &CancelToken::new());
    assert!(matches!(result, Err(SaturationError::Workload { .. })), "{result:?}");
    assert!(matches!(
        work.take_failure(),
        Some(SaturationWorkError::SlotOutOfRange { slot: 2, slots: 2 })
    ));
}

#[test]
fn objects_return_to_baseline_after_drop() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut work =
            StillSaturation::new_synthetic(&context, small_config(), quality_params(DisplayGamut::Srgb)).unwrap();
        run_quick(&context, &mut work, 2);
        context.wait_idle().unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
