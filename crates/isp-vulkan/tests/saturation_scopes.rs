//! Адаптер `ScopesSaturation` под стендом насыщения на ПК (RADV, полная validation).
//!
//! Итог проверяется на счётчиках GPU: после прогона они совпадают с эталоном `scope_model::accumulate` на той же
//! картинке видоискателя бит в бит (то есть прибор реально исполнялся по настоящему входу), а серый вход даёт
//! сосредоточенные счётчики.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;

use camera_contract::geometry::{PixelSize, SensorOrientation};
use gpu_vulkan::saturation::{CancelToken, SaturationBench, SaturationError};
use isp_vulkan::saturation::{
    SaturationWorkError, SceneContent, ScopesSaturation, ScopesSaturationConfig, ViewfinderSaturationConfig,
};
use isp_vulkan::viewfinder::ViewfinderStorageFormat;
use saturation_support::viewfinder_support::{
    ParamsSpec, assert_no_validation_errors, gpu_context_or_skip, test_config,
};
use saturation_support::{quick_bench_config, run_quick, serialize_gpu_timing};
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{HorizontalOrder, ResolvedScope, ScopeModelSettings, ShownCodes, accumulate};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;

fn config(kind: ScopeKind, content: SceneContent) -> ScopesSaturationConfig {
    ScopesSaturationConfig {
        viewfinder: ViewfinderSaturationConfig {
            frame_size: PixelSize::new(WIDTH, HEIGHT),
            sensor_orientation: SensorOrientation::Degrees0,
            isp: test_config(ViewfinderStorageFormat::PreferTenBitUnorm),
            awb_statistics_every_n_frames: 1,
            ..ViewfinderSaturationConfig::default()
        },
        input_content: content,
        kind,
        // Маленькие сетки, как в тестах приборов: эталон CPU считается быстро.
        settings: ScopeModelSettings {
            trace_columns: 97,
            trace_levels: 128,
            vectorscope_size: 128,
            ..ScopeModelSettings::default()
        },
        picture_size: ScopeImageSize::try_new(160, 96).unwrap(),
        ..ScopesSaturationConfig::default()
    }
}

/// Эталонные счётчики CPU на картинке видоискателя, с которой считались приборы.
fn reference_counters(work: &ScopesSaturation) -> scope_model::ScopeCounters {
    let config = work.config();
    let (size, pixels) = work.input().read_output_blocking().unwrap();
    let shown = ShownCodes::from_display_rgba(size.width, size.height, &pixels, HorizontalOrder::AsStored).unwrap();
    let request = ScopeRequest {
        kind: config.kind,
        vectorscope_axes: config.vectorscope_axes,
        scan: config.scan,
        image_size: config.picture_size,
    };
    let gamut = isp_vulkan::viewfinder::gamut_for_display(config.viewfinder.display);
    let rule = ResolvedScope::resolve(&request, &config.settings, gamut).unwrap();
    accumulate(&shown, config.settings.sample_stride, &rule).unwrap()
}

#[test]
fn every_scope_kind_counters_match_cpu_reference_after_a_saturated_run() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let cases = [
        (ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709),
        (ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709),
        (ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709),
        (ScopeKind::Vectorscope, VectorscopeAxes::OklabAb),
    ];
    for (kind, axes) in cases {
        let scopes_config = ScopesSaturationConfig {
            vectorscope_axes: axes,
            ..config(kind, SceneContent::default())
        };
        let mut work = ScopesSaturation::new_synthetic(&context, scopes_config, ParamsSpec::default().build()).unwrap();
        let report = run_quick(&context, &mut work, 2);
        context.wait_idle().unwrap();
        assert!(work.take_failure().is_none(), "{kind:?}/{axes:?}");

        let counters = work.counters();
        assert_eq!(counters.frames_recorded, report.total_completed_frames());
        assert_eq!(counters.frames_released, report.total_completed_frames());
        // Времена: два прохода в порядке записи, замер на каждом кадре.
        let times = work.pass_times();
        let labels: Vec<_> = times.passes.iter().map(|pass| pass.label).collect();
        assert_eq!(labels, ["accumulate-end", "render-end"]);
        assert_eq!(times.measured_frames, report.total_completed_frames());
        assert_eq!((times.unmeasured_frames, times.mismatched_frames), (0, 0));
        assert!(times.passes.iter().all(|pass| !pass.median.is_zero()), "{times:?}");
        // Счётчики GPU == эталон CPU на той же картинке (бит в бит) и не пусты.
        let on_gpu = work.scopes().read_back_counters_blocking().unwrap();
        assert!(
            on_gpu.counts().iter().any(|&count| count > 0),
            "{kind:?}: счётчики пусты"
        );
        assert_eq!(on_gpu, reference_counters(&work), "{kind:?}/{axes:?}");
        // Картинка прибора записана стендом: счётчик записанных кадров диагностики растёт вместе с кадрами.
        assert_eq!(work.scopes().pass_counters().recorded, report.total_completed_frames());
    }
    assert_no_validation_errors(&context);
}

#[test]
fn uniform_gray_input_piles_every_sample_into_one_vectorscope_cell() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let gray = SceneContent::UniformGray { code: 256 };
    let mut work = ScopesSaturation::new_synthetic(
        &context,
        config(ScopeKind::Vectorscope, gray),
        ParamsSpec::default().build(),
    )
    .unwrap();
    run_quick(&context, &mut work, 2);
    context.wait_idle().unwrap();
    assert!(work.take_failure().is_none());
    let on_gpu = work.scopes().read_back_counters_blocking().unwrap();
    let occupied: Vec<u32> = on_gpu.counts().iter().copied().filter(|&count| count > 0).collect();
    assert_eq!(occupied.len(), 1, "серый кадр = одна ячейка вектороскопа");
    // В эту ячейку легли все отсчёты кадра (с учётом прореживания).
    let reference = reference_counters(&work);
    assert_eq!(on_gpu, reference);
    // И естественная сцена на том же приборе занимает много ячеек: серый вход — действительно другой вход.
    let mut natural = ScopesSaturation::new_synthetic(
        &context,
        config(ScopeKind::Vectorscope, SceneContent::default()),
        ParamsSpec::default().build(),
    )
    .unwrap();
    run_quick(&context, &mut natural, 2);
    context.wait_idle().unwrap();
    let natural_cells = natural
        .scopes()
        .read_back_counters_blocking()
        .unwrap()
        .counts()
        .iter()
        .filter(|&&count| count > 0)
        .count();
    assert!(natural_cells > 20, "естественная сцена: {natural_cells} ячеек");
}

#[test]
fn wrong_config_is_a_typed_error_and_slot_overflow_is_reported() {
    let Some(context) = gpu_context_or_skip() else { return };
    let bad_slots = ScopesSaturationConfig {
        frame_slots: 0,
        ..config(ScopeKind::Waveform, SceneContent::default())
    };
    assert!(matches!(
        bad_slots.validate(),
        Err(SaturationWorkError::InvalidConfig { .. })
    ));
    let bad_frame = ScopesSaturationConfig {
        viewfinder: ViewfinderSaturationConfig {
            frame_size: PixelSize::new(WIDTH + 1, HEIGHT),
            ..config(ScopeKind::Waveform, SceneContent::default()).viewfinder
        },
        ..config(ScopeKind::Waveform, SceneContent::default())
    };
    assert!(matches!(
        ScopesSaturation::new_synthetic(&context, bad_frame, ParamsSpec::default().build()),
        Err(SaturationWorkError::InvalidConfig { .. })
    ));
    // Стенд с тремя слотами против двух слотов работы: типизированный отказ, а не пустые кадры.
    let _serial = serialize_gpu_timing();
    let mut work = ScopesSaturation::new_synthetic(
        &context,
        config(ScopeKind::Waveform, SceneContent::default()),
        ParamsSpec::default().build(),
    )
    .unwrap();
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
        let mut work = ScopesSaturation::new_synthetic(
            &context,
            config(ScopeKind::RgbParade, SceneContent::default()),
            ParamsSpec::default().build(),
        )
        .unwrap();
        run_quick(&context, &mut work, 2);
        context.wait_idle().unwrap();
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
