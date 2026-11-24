//! Дайджест выхода адаптеров стенда насыщения на ПК (RADV, полная validation): отпечаток картинки ISP, приборов
//! и V4 не зависит от длины прогона и от кадра со статистикой AWB, следует за картинкой, не трогает её и не
//! существует до первого кадра.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;

use std::time::Duration;

use camera_contract::geometry::{PixelSize, SensorOrientation};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::saturation::{CancelToken, SaturationBench, SaturationWorkload};
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_vulkan::saturation::{
    DEFAULT_GRAY_CODE, HeadroomSaturation, HeadroomSaturationConfig, SaturationWorkError, SceneContent,
    ScopesSaturation, ScopesSaturationConfig, StillSaturation, StillSaturationConfig, ViewfinderSaturation,
    ViewfinderSaturationConfig,
};
use isp_vulkan::viewfinder::{IspStage, IspStageSet, ViewfinderStorageFormat};
use saturation_support::viewfinder_support::{
    ParamsSpec, assert_no_validation_errors, gpu_context_or_skip, test_config,
};
use saturation_support::{bench_config, quality_params, serialize_gpu_timing};
use scope_contract::{ScopeImageSize, ScopeKind};
use scope_model::ScopeModelSettings;

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;

fn viewfinder_config(every_n: u32, content: SceneContent) -> ViewfinderSaturationConfig {
    ViewfinderSaturationConfig {
        frame_size: PixelSize::new(WIDTH, HEIGHT),
        sensor_orientation: SensorOrientation::Degrees0,
        display: DisplayColorSpace::DisplayP3,
        isp: test_config(ViewfinderStorageFormat::PreferTenBitUnorm),
        awb_statistics_every_n_frames: every_n,
        content,
        ..ViewfinderSaturationConfig::default()
    }
}

/// Прогон работы стендом с окном `measure_ms` и ожиданием GPU после него.
fn run_for(context: &GpuContext, work: &mut dyn SaturationWorkload, measure_ms: u64) -> u64 {
    let config = bench_config(2, Duration::from_millis(30), Duration::from_millis(measure_ms));
    let mut bench = SaturationBench::new(context, config).unwrap();
    let report = bench.run(work, &CancelToken::new()).unwrap();
    drop(bench);
    context.wait_idle().unwrap();
    report.total_completed_frames()
}

fn viewfinder_digest(context: &GpuContext, config: ViewfinderSaturationConfig, measure_ms: u64) -> (String, u64) {
    let mut work = ViewfinderSaturation::new_synthetic(context, config, ParamsSpec::default().build()).unwrap();
    let frames = run_for(context, &mut work, measure_ms);
    assert!(work.take_failure().is_none());
    (work.output_digest().unwrap().combined().to_string(), frames)
}

#[test]
fn viewfinder_digest_does_not_depend_on_the_frame_count_or_on_the_awb_statistics_cadence() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let scene = SceneContent::default();
    let (short, short_frames) = viewfinder_digest(&context, viewfinder_config(4, scene), 60);
    let (long, long_frames) = viewfinder_digest(&context, viewfinder_config(4, scene), 400);
    assert!(
        long_frames > short_frames,
        "окна разной длины дали разное число кадров: {short_frames} и {long_frames}"
    );
    assert_eq!(short, long, "дайджест зависит от числа кадров прогона");
    // Статистика AWB только считает: картинка не зависит ни от неё, ни от её каденции.
    for every_n in [1_u32, 3] {
        assert_eq!(
            viewfinder_digest(&context, viewfinder_config(every_n, scene), 100).0,
            short,
            "N={every_n}"
        );
    }
    let without_statistics = ViewfinderSaturationConfig {
        stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
        ..viewfinder_config(4, scene)
    };
    assert_eq!(
        viewfinder_digest(&context, without_statistics, 100).0,
        short,
        "стадия AWB выключена совсем"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn viewfinder_digest_follows_the_picture_and_reading_it_does_not_change_the_picture() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let natural = viewfinder_digest(&context, viewfinder_config(4, SceneContent::default()), 60).0;
    let gray = viewfinder_digest(
        &context,
        viewfinder_config(
            4,
            SceneContent::UniformGray {
                code: DEFAULT_GRAY_CODE,
            },
        ),
        60,
    )
    .0;
    assert_ne!(natural, gray, "другой кадр — другой дайджест");
    let without_tone = ViewfinderSaturationConfig {
        stages: IspStageSet::ALL.without(IspStage::Tone),
        ..viewfinder_config(4, SceneContent::default())
    };
    assert_ne!(
        viewfinder_digest(&context, without_tone, 60).0,
        natural,
        "стадия, меняющая картинку, меняет дайджест"
    );

    let mut work = ViewfinderSaturation::new_synthetic(
        &context,
        viewfinder_config(4, SceneContent::default()),
        ParamsSpec::default().build(),
    )
    .unwrap();
    run_for(&context, &mut work, 60);
    let (_, before) = work.read_output_blocking().unwrap();
    let first = work.output_digest().unwrap();
    assert_eq!(
        work.output_digest().unwrap(),
        first,
        "повторное чтение — тот же дайджест"
    );
    let (_, after) = work.read_output_blocking().unwrap();
    assert_eq!(before, after, "чтение дайджеста не меняет картинку");
    assert_eq!(first.parts().len(), 1);
    assert_eq!(first.parts()[0].label, "image");
    // Какой кадр читали: вариант кадра пишется в счётчики работы, чтобы расхождение на телефоне можно было объяснить.
    assert!(work.counters().last_frame_had_statistics.is_some());
    assert_no_validation_errors(&context);
}

#[test]
fn digest_of_work_that_recorded_no_frame_is_a_typed_empty_frame_error() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let headroom = HeadroomSaturation::new_synthetic(
        &context,
        HeadroomSaturationConfig {
            frame_size: PixelSize::new(512, 384),
            ..HeadroomSaturationConfig::default()
        },
        quality_params(isp_model::image::DisplayGamut::Srgb),
    )
    .unwrap();
    assert!(matches!(
        headroom.output_digest(),
        Err(SaturationWorkError::EmptyFrame { .. })
    ));
    let still = StillSaturation::new_synthetic(
        &context,
        StillSaturationConfig {
            frame_size: PixelSize::new(256, 192),
            ..StillSaturationConfig::default()
        },
        quality_params(isp_model::image::DisplayGamut::Srgb),
    )
    .unwrap();
    assert!(matches!(
        still.output_digest(),
        Err(SaturationWorkError::EmptyFrame { .. })
    ));
    drop((headroom, still));
    assert_no_validation_errors(&context);
}

#[test]
fn headroom_digest_is_stable_between_runs_of_different_length() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let digest = |measure_ms: u64| {
        let mut work = HeadroomSaturation::new_synthetic(
            &context,
            HeadroomSaturationConfig {
                frame_size: PixelSize::new(512, 384),
                ..HeadroomSaturationConfig::default()
            },
            quality_params(isp_model::image::DisplayGamut::Srgb),
        )
        .unwrap();
        let frames = run_for(&context, &mut work, measure_ms);
        (work.output_digest().unwrap().combined().to_string(), frames)
    };
    let (short, short_frames) = digest(80);
    let (long, long_frames) = digest(400);
    assert!(long_frames > short_frames, "{short_frames} и {long_frames}");
    assert_eq!(short, long);
    assert_no_validation_errors(&context);
}

/// Приборы: две части (счётчики и картинка) с test-support; одинаковы у прогонов разной длины и различаются у
/// разных приборов. Без `test-support` дайджест приборов недоступен (см. `ScopesSaturation::output_digest`).
#[test]
fn scopes_digest_has_counters_and_picture_parts_and_does_not_depend_on_the_frame_count() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let digest = |kind: ScopeKind, measure_ms: u64| {
        let config = ScopesSaturationConfig {
            viewfinder: viewfinder_config(1, SceneContent::default()),
            kind,
            settings: ScopeModelSettings {
                trace_columns: 97,
                trace_levels: 128,
                vectorscope_size: 128,
                ..ScopeModelSettings::default()
            },
            picture_size: ScopeImageSize::try_new(160, 96).unwrap(),
            ..ScopesSaturationConfig::default()
        };
        let mut work = ScopesSaturation::new_synthetic(&context, config, ParamsSpec::default().build()).unwrap();
        run_for(&context, &mut work, measure_ms);
        assert!(work.take_failure().is_none());
        work.output_digest().unwrap()
    };
    let short = digest(ScopeKind::Waveform, 60);
    let long = digest(ScopeKind::Waveform, 300);
    assert_eq!(short, long, "дайджест приборов зависит от числа кадров");
    let labels: Vec<&str> = short.parts().iter().map(|part| part.label).collect();
    assert_eq!(labels, ["counters", "picture"]);
    let parade = digest(ScopeKind::RgbParade, 60);
    assert_ne!(short.combined(), parade.combined(), "другой прибор — другой дайджест");
    assert_ne!(
        short.parts()[0].digest,
        parade.parts()[0].digest,
        "счётчики waveform и parade различаются"
    );
    assert_no_validation_errors(&context);
}
