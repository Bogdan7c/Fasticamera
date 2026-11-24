//! Адаптер `HeadroomSaturation` (цепочка стенда V4) под стендом насыщения на ПК (RADV, полная validation).
//!
//! Итог: картинка приёмника после прогона стенда равна картинке `VideoHeadroomWorkload` вне стенда на том же
//! входе (бит в бит); метки проходов — 10 в нужном порядке (шумодав выключен) или 11 (справочно включён).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;

use ash::vk;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::frame_sink::{FrameSink, OffscreenFrameSink, OffscreenFrameSinkDesc};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raw_mosaic_preview::ValidArea;
use gpu_vulkan::saturation::{CancelToken, SaturationBench, SaturationError};
use isp_model::image::DisplayGamut;
use isp_vulkan::quality::{StreamGeneration, VideoFrameInput, VideoHeadroomWorkload};
use isp_vulkan::saturation::{
    HEADROOM_FRAME_SIZE, HeadroomSaturation, HeadroomSaturationConfig, SaturationWorkError, SceneContent,
    synthesize_raw10, synthetic_raw10_holder,
};
use saturation_support::viewfinder_support::{assert_no_validation_errors, gpu_context_or_skip};
use saturation_support::{quality_params, quick_bench_config, run_quick, serialize_gpu_timing};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 384;

fn small_config() -> HeadroomSaturationConfig {
    HeadroomSaturationConfig {
        frame_size: PixelSize::new(WIDTH, HEIGHT),
        ..HeadroomSaturationConfig::default()
    }
}

/// Обычный путь: тот же стенд V4 и вход, ОДИН кадр в разовой отправке вне стенда насыщения.
fn plain_output(context: &gpu_vulkan::context::GpuContext, config: &HeadroomSaturationConfig) -> Vec<[f32; 4]> {
    let frame = synthesize_raw10(config.frame_size, config.content, config.raw10_row_padding_bytes).unwrap();
    // Независимый путь: тестовый `SyntheticRaw10` (адаптер стенда держит кадр в `HostRawFrame`).
    let raw = gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw10::new(
        context,
        frame.width,
        frame.height,
        frame.row_stride_bytes,
        &frame.bytes,
    )
    .unwrap();
    let source = raw.source();
    let extent = vk::Extent2D {
        width: WIDTH,
        height: HEIGHT,
    };
    let mut workload = VideoHeadroomWorkload::new(context, extent, &config.headroom).unwrap();
    let mut sink = OffscreenFrameSink::new(
        context,
        &OffscreenFrameSinkDesc {
            extent,
            format_policy: config.headroom.output_format.storage_policy(),
            frame_slots: config.headroom.frame_slots,
            reader: ConsumerStage::Fragment,
        },
    )
    .unwrap();
    let params = quality_params(DisplayGamut::Srgb);
    let input = VideoFrameInput {
        source: &source,
        params: &params,
        valid_area: ValidArea {
            left: 0,
            top: 0,
            width: WIDTH,
            height: HEIGHT,
        },
        stream: StreamGeneration(1),
        sequence: 0,
        presentation_time_ns: 0,
    };
    run_one_shot_blocking(context, |command_buffer| {
        source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
        workload.record(command_buffer, 0, &input, &mut sink).map_err(|error| {
            gpu_vulkan::error::GpuError::InvalidState {
                what: error.to_string(),
            }
        })?;
        source.record_release_for(context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    sink.after_submit(0).unwrap();
    let pixels = read_storage_image_rgba(sink.image(0).unwrap(), sink.last_use(0).unwrap()).unwrap();
    drop(workload);
    pixels
}

#[test]
fn product_defaults_have_denoise_off_and_reference_switch_adds_the_eleventh_pass() {
    let config = HeadroomSaturationConfig::default();
    assert_eq!(config.frame_size, PixelSize::new(4096, 2304));
    assert_eq!(HEADROOM_FRAME_SIZE, config.frame_size);
    assert!(!config.headroom.temporal_denoise.enabled);
    let labels = config.pass_labels();
    assert_eq!(labels.len(), 10);
    assert_eq!((labels[0], labels[9]), ("levels", "output"));
    let with_denoise = config.with_temporal_denoise_reference();
    let labels = with_denoise.pass_labels();
    assert_eq!(labels.len(), 11);
    assert_eq!(labels[9], "temporal_denoise");
}

#[test]
fn bench_frames_are_executed_published_and_match_the_plain_path_bit_for_bit() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = small_config();
    let mut work = HeadroomSaturation::new_synthetic(&context, config, quality_params(DisplayGamut::Srgb)).unwrap();
    let report = run_quick(&context, &mut work, 2);
    context.wait_idle().unwrap();
    assert!(work.take_failure().is_none());

    let counters = work.counters();
    let frames = report.total_completed_frames();
    assert_eq!(
        (
            counters.frames_recorded,
            counters.frames_released,
            counters.frames_published
        ),
        (frames, frames, frames)
    );
    // Метки: 10 проходов в порядке записи, замер на каждом кадре, ни одного расхождения длины.
    let times = work.pass_times();
    let labels: Vec<_> = times.passes.iter().map(|pass| pass.label).collect();
    assert_eq!(labels, work.labels());
    assert_eq!(labels.len(), 10);
    assert_eq!(times.measured_frames, frames);
    assert_eq!((times.unmeasured_frames, times.mismatched_frames), (0, 0));
    assert!(times.passes.iter().all(|pass| !pass.median.is_zero()), "{times:?}");
    assert!(times.total_median() > std::time::Duration::ZERO);
    // Результат == обычный путь; картинка не вырождена.
    let via_bench = work.read_last_output_blocking().unwrap();
    let plain = plain_output(&context, work.config());
    assert_eq!(via_bench.len(), plain.len());
    assert!(via_bench == plain, "картинка кадров стенда отличается от обычного пути");
    assert!(via_bench.iter().any(|pixel| *pixel != via_bench[0]));
    drop(work);
    assert_no_validation_errors(&context);
}

#[test]
fn reference_denoise_adds_a_pass_and_still_measures_every_frame() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = small_config().with_temporal_denoise_reference();
    let mut work = HeadroomSaturation::new_synthetic(&context, config, quality_params(DisplayGamut::Srgb)).unwrap();
    let report = run_quick(&context, &mut work, 2);
    let times = work.pass_times();
    assert_eq!(times.passes.len(), 11);
    assert_eq!(times.measured_frames, report.total_completed_frames());
    assert_eq!(times.mismatched_frames, 0);
    assert!(work.take_failure().is_none());
}

#[test]
fn wrong_config_and_foreign_source_are_typed_errors_and_too_many_stand_slots_fail() {
    let Some(context) = gpu_context_or_skip() else { return };
    let bad_sizes = [
        PixelSize::new(0, 384),
        PixelSize::new(510, 384),
        PixelSize::new(512, 383),
    ];
    for frame_size in bad_sizes {
        let config = HeadroomSaturationConfig {
            frame_size,
            ..small_config()
        };
        assert!(
            matches!(config.validate(), Err(SaturationWorkError::InvalidConfig { .. })),
            "{frame_size:?}"
        );
    }
    let other = synthesize_raw10(PixelSize::new(64, 32), SceneContent::default(), 0).unwrap();
    let holder = synthetic_raw10_holder(&context, &other).unwrap();
    assert!(matches!(
        HeadroomSaturation::new(&context, small_config(), quality_params(DisplayGamut::Srgb), holder),
        Err(SaturationWorkError::InvalidConfig { .. })
    ));
    let _serial = serialize_gpu_timing();
    let mut work =
        HeadroomSaturation::new_synthetic(&context, small_config(), quality_params(DisplayGamut::Srgb)).unwrap();
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
            HeadroomSaturation::new_synthetic(&context, small_config(), quality_params(DisplayGamut::Srgb)).unwrap();
        run_quick(&context, &mut work, 2);
        context.wait_idle().unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
