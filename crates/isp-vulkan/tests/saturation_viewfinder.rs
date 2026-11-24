//! Адаптер `ViewfinderSaturation` под стендом насыщения на ПК (RADV, полная validation).
//!
//! Проверяется итог, а не вызов: кадры СТЕНДА дают те же пиксели, что обычный путь `ViewfinderIsp` на том же
//! входе (бит в бит); статистика AWB реально забрана с GPU ⌈K/N⌉ раз, а не K; ошибочные настройки отвергаются
//! до GPU; объекты после drop возвращаются к исходному числу.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::viewfinder_crop::ViewfinderCrop;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::saturation::{CancelToken, SaturationBench, SaturationError};
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_vulkan::saturation::{
    PRODUCT_AWB_STATISTICS_EVERY_N_FRAMES, PRODUCT_FRAME_SIZE, RawFrameHolder, SaturationWorkError, SceneContent,
    ViewfinderSaturation, ViewfinderSaturationConfig, synthesize_raw10,
};
use isp_vulkan::viewfinder::{
    IspStageSet, OutputAllocation, ViewfinderGeometry, ViewfinderIspConfig, ViewfinderStorageFormat,
};
use saturation_support::viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, SourceLayout, assert_no_validation_errors, gpu_context_or_skip, run_viewfinder,
    test_config,
};
use saturation_support::{quick_bench_config, run_quick, serialize_gpu_timing};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;

/// Конфиг маленького кадра: сетка AWB и наибольший кадр — как у тестов ISP (`test_config`).
fn small_config(every_n: u32) -> ViewfinderSaturationConfig {
    ViewfinderSaturationConfig {
        frame_size: PixelSize::new(WIDTH, HEIGHT),
        sensor_orientation: SensorOrientation::Degrees0,
        display: DisplayColorSpace::DisplayP3,
        isp: test_config(ViewfinderStorageFormat::PreferTenBitUnorm),
        awb_statistics_every_n_frames: every_n,
        ..ViewfinderSaturationConfig::default()
    }
}

/// Держатель, который нельзя спрашивать: доказывает, что отказ настроек случился ДО использования ресурсов.
struct ForbiddenHolder;

impl RawFrameHolder for ForbiddenHolder {
    fn source(&self) -> MosaicSource {
        panic!("настройки неверны: источник не должен был запрашиваться");
    }
}

#[test]
fn product_defaults_are_the_documented_product_values() {
    let config = ViewfinderSaturationConfig::default();
    assert_eq!(config.frame_size, PixelSize::new(4096, 3072));
    assert_eq!(PRODUCT_FRAME_SIZE, config.frame_size);
    assert_eq!(config.awb_statistics_every_n_frames, 4);
    assert_eq!(PRODUCT_AWB_STATISTICS_EVERY_N_FRAMES, 4);
    assert_eq!(config.stages, IspStageSet::ALL);
    assert_eq!(config.sensor_orientation, SensorOrientation::Degrees90);
    assert_eq!(config.crop, ViewfinderCrop::FULL_SENSOR);
    assert_eq!(config.validate().ok(), Some(()));
    // Расписание продукта: кадры 0, 4, 8 — со статистикой.
    let due: Vec<u64> = (0..9).filter(|&frame| config.statistics_due(frame)).collect();
    assert_eq!(due, vec![0, 4, 8]);
    // Шаг строки буфера камеры: 4096 пикселей RAW10 = 5120 байт.
    let frame = synthesize_raw10(PixelSize::new(4096, 2), SceneContent::default(), 0).unwrap();
    assert_eq!(frame.row_stride_bytes, 5120);
}

#[test]
fn wrong_config_is_a_typed_error_before_any_gpu_work() {
    let Some(context) = gpu_context_or_skip() else { return };
    let params = ParamsSpec::default().build();
    let cases = [
        ViewfinderSaturationConfig {
            frame_size: PixelSize::new(0, HEIGHT),
            ..small_config(4)
        },
        ViewfinderSaturationConfig {
            frame_size: PixelSize::new(WIDTH + 2, HEIGHT),
            ..small_config(4)
        },
        ViewfinderSaturationConfig {
            frame_size: PixelSize::new(WIDTH, HEIGHT + 1),
            ..small_config(4)
        },
        small_config(0),
    ];
    for config in cases {
        assert!(
            matches!(config.validate(), Err(SaturationWorkError::InvalidConfig { .. })),
            "{config:?}"
        );
        let result = ViewfinderSaturation::new(&context, config, params.clone(), Box::new(ForbiddenHolder));
        assert!(matches!(result, Err(SaturationWorkError::InvalidConfig { .. })));
    }
}

#[test]
fn source_of_another_size_is_rejected() {
    let Some(context) = gpu_context_or_skip() else { return };
    let params = ParamsSpec::default().build();
    let small = synthesize_raw10(PixelSize::new(64, 32), SceneContent::default(), 0).unwrap();
    let holder = isp_vulkan::saturation::synthetic_raw10_holder(&context, &small).unwrap();
    let result = ViewfinderSaturation::new(&context, small_config(4), params, holder);
    assert!(matches!(result, Err(SaturationWorkError::InvalidConfig { .. })));
}

/// Обычный путь: тот же ISP-конфиг, кадр и параметры, одна разовая отправка, чтение картинки.
fn plain_output(
    context: &gpu_vulkan::context::GpuContext,
    config: &ViewfinderSaturationConfig,
    spec: &ParamsSpec,
) -> Vec<[f32; 4]> {
    let frame = synthesize_raw10(config.frame_size, config.content, config.raw10_row_padding_bytes).unwrap();
    let raw = RawFrame {
        width: frame.width,
        height: frame.height,
        pattern: spec.pattern,
        samples: frame.samples,
    };
    let params = spec.build();
    let full = PixelRect {
        left: 0,
        top: 0,
        size: config.frame_size,
    };
    let request = RunRequest {
        layout: SourceLayout::Raw10 { padding_bytes: 0 },
        geometry: ViewfinderGeometry {
            valid_area: full,
            crop: full,
            sensor_orientation: config.sensor_orientation,
            allocation: OutputAllocation::Exact,
        },
        ..RunRequest::whole_frame(&raw, &params, config.display)
    };
    run_viewfinder(context, &config.isp, &request).pixels
}

#[test]
fn bench_frames_are_executed_and_match_the_plain_path_bit_for_bit() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let config = small_config(4);
    let mut work = ViewfinderSaturation::new_synthetic(&context, config, spec.build()).unwrap();
    let report = run_quick(&context, &mut work, 2);
    context.wait_idle().unwrap();

    assert!(work.take_failure().is_none());
    let counters = work.counters();
    // Кадры, отправленные стендом, — это ровно кадры трёх корзин итога, и каждый получил release.
    assert_eq!(counters.frames_recorded, report.total_completed_frames());
    assert_eq!(counters.frames_released, report.total_completed_frames());
    // Времена проходов: метка есть, замеры получены на каждом кадре (RADV поддерживает timestamp-ы).
    let times = work.pass_times();
    assert_eq!(times.passes.len(), 1);
    assert_eq!(times.passes[0].label, "isp-end");
    assert_eq!(times.measured_frames, report.total_completed_frames());
    assert_eq!((times.unmeasured_frames, times.mismatched_frames), (0, 0));
    assert!(times.passes[0].median > std::time::Duration::ZERO);
    // Результат: пиксели кадров стенда == обычный путь на том же входе.
    let (size, via_bench) = work.read_output_blocking().unwrap();
    // Демозаик 2×2: выход вдвое меньше кадра по каждой стороне.
    assert_eq!((size.width, size.height), (WIDTH / 2, HEIGHT / 2));
    let plain = plain_output(&context, work.config(), &spec);
    assert_eq!(via_bench.len(), plain.len());
    assert!(via_bench == plain, "картинка кадров стенда отличается от обычного пути");
    // Картинка не вырождена: не константа.
    assert!(via_bench.iter().any(|pixel| *pixel != via_bench[0]));
    drop(work);
    assert_no_validation_errors(&context);
}

#[test]
fn awb_statistics_are_collected_once_per_n_frames_not_every_frame() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    for every_n in [4_u32, 1, 3] {
        let mut work =
            ViewfinderSaturation::new_synthetic(&context, small_config(every_n), ParamsSpec::default().build())
                .unwrap();
        let report = run_quick(&context, &mut work, 2);
        let frames = report.total_completed_frames();
        let counters = work.counters();
        let expected = frames.div_ceil(u64::from(every_n));
        assert_eq!(counters.statistics_recorded, expected, "N={every_n}, K={frames}");
        assert_eq!(counters.statistics_collected, expected, "N={every_n}, K={frames}");
        assert!(work.take_failure().is_none());
        if every_n > 1 {
            assert!(expected < frames, "расписание обязано пропускать кадры");
        }
    }
}

#[test]
fn two_pipeline_variants_exist_only_when_statistics_are_thinned() {
    let Some(context) = gpu_context_or_skip() else { return };
    let thinned =
        ViewfinderSaturation::new_synthetic(&context, small_config(4), ParamsSpec::default().build()).unwrap();
    let every_frame =
        ViewfinderSaturation::new_synthetic(&context, small_config(1), ParamsSpec::default().build()).unwrap();
    // Варианты строятся в конструкторе (в цикле стенда ничего не компилируется).
    assert_eq!(thinned.isp().cached_pipeline_variants(), 2);
    assert_eq!(every_frame.isp().cached_pipeline_variants(), 1);
}

#[test]
fn stand_with_more_slots_than_the_work_has_fails_with_typed_error() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = ViewfinderSaturationConfig {
        isp: ViewfinderIspConfig {
            frame_slots: 2,
            ..small_config(4).isp
        },
        ..small_config(4)
    };
    let mut work = ViewfinderSaturation::new_synthetic(&context, config, ParamsSpec::default().build()).unwrap();
    assert_eq!(work.slots(), 2);
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
            ViewfinderSaturation::new_synthetic(&context, small_config(4), ParamsSpec::default().build()).unwrap();
        run_quick(&context, &mut work, 2);
        context.wait_idle().unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
