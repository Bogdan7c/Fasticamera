//! Паритет счётчиков приборов на РЕАЛЬНОМ входе: настоящий `ViewfinderIsp` обрабатывает реальные RAW-кропы кадра
//! C7 и синтетические сцены; приборы читают его выход в том же командном буфере так, как будет делать render-loop
//! (ISP с читателем «фрагмент» → барьер чтения для compute подложки → приборы). Эталон — чтение той же картинки
//! видоискателя на CPU → `ShownCodes` → `scope_model::accumulate`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;
mod viewfinder_support;

use gpu_vulkan::backdrop::{BackdropFrameNumber, SubmissionSerial};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::source_window::HorizontalMirror;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame, ScopeInput};
use isp_vulkan::viewfinder::{ViewfinderFrame, ViewfinderIsp, ViewfinderStorageFormat};
use scope_model::{SampleStride, ScopeModelSettings, ScopeRule};
use scope_support::{
    assert_no_validation_errors, cpu_counters, gpu_context_or_skip, picture_size, rule_cases, shown_codes_of,
    small_settings, worst_premultiplied_difference,
};
use viewfinder_support::{
    GpuSource, ParamsSpec, RawFrame, RunRequest, SourceLayout, isp_support, synthetic_scenes, test_config,
};

/// Допуск картинки CIE: четыре шага fp16 у единицы.
const CIE_PICTURE_TOLERANCE: f32 = 2.0e-3;

/// Один кадр: ISP → барьер чтения compute → приборы по всем правилам; сравнивает счётчики с эталоном.
/// Возвращает число сравнений.
fn compare_scopes_on_viewfinder_output(
    context: &GpuContext,
    name: &str,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
    storage: ViewfinderStorageFormat,
) -> (usize, f32) {
    let request = RunRequest::whole_frame(raw, params, spec.display());
    let gpu_source = GpuSource::new(context, raw, SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(context, &test_config(storage)).unwrap();
    let base_settings = small_settings();
    let mut scopes = ColorScopes::new(
        context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: base_settings.clone(),
        },
    )
    .unwrap();
    let rules = rule_cases(&base_settings);
    let mut comparisons = 0;
    let mut worst_cie_picture = 0.0_f32;
    for mirror in [HorizontalMirror::Off, HorizontalMirror::On] {
        for stride in [1_u32, 2] {
            let settings = ScopeModelSettings {
                sample_stride: SampleStride::try_new(stride).unwrap(),
                // Шаг гистограммы — своя настройка: матрица настоящих кропов проходит и его.
                histogram_sample_stride: SampleStride::try_new(stride).unwrap(),
                cie: scope_model::CieSettings {
                    sample_stride: SampleStride::try_new(stride).unwrap(),
                    ..base_settings.cie.clone()
                },
                ..base_settings.clone()
            };
            for (rule_name, rule) in &rules {
                scopes.prepare_picture(picture_size(160, 96)).unwrap();
                let mut outcome = None;
                run_one_shot_blocking(context, |command_buffer| {
                    source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
                    isp.record(
                        command_buffer,
                        &ViewfinderFrame {
                            source: &source,
                            params,
                            geometry: request.geometry,
                            display_color_space: request.display,
                            stages: request.stages,
                            frame: BackdropFrameNumber(1),
                            frame_slot: 0,
                            // Как в render-loop: выход читает фрагментный шейдер; приборы читают в compute.
                            reader: ConsumerStage::Fragment,
                        },
                    )
                    .unwrap();
                    let backdrop = isp.backdrop_source(SubmissionSerial(1)).unwrap();
                    backdrop.record_read_barrier(command_buffer, ConsumerStage::Compute);
                    let image = isp.image().unwrap();
                    outcome = Some(scopes.record(
                        command_buffer,
                        &ScopeFrame {
                            input: ScopeInput {
                                view: image.sampled_view(),
                                window: image.content_window().with_mirror(mirror),
                            },
                            rule,
                            settings: &settings,
                            picture_size: picture_size(160, 96),
                            frame_slot: 0,
                        },
                    ));
                    source.record_release_for(context, command_buffer, ConsumerStage::Compute);
                    Ok(())
                })
                .unwrap();
                outcome.unwrap().unwrap();
                scopes.release_slot_after_fence(0).unwrap();
                isp.collect(0).unwrap();

                // Эталон: чтение ТОЙ ЖЕ картинки видоискателя.
                let image = isp.image().unwrap();
                let texels = read_storage_image_rgba(image.storage_image(), image.current_use()).unwrap();
                let shown = shown_codes_of(&texels, image.extent().width, image.content_extent(), mirror);
                let gpu = scopes.read_back_counters_blocking().unwrap();
                let cpu = cpu_counters(&shown, &settings, rule as &dyn ScopeRule);
                assert_eq!(
                    gpu, cpu,
                    "{name} / {storage:?} / {rule_name} / {mirror:?} / шаг {stride}: счётчики GPU и CPU разошлись"
                );
                // Вход непустой и прибор что-то сосчитал: «оба нули» тут было бы провалом теста.
                assert!(cpu.total() > 0, "{name} / {rule_name}: пустые счётчики");
                comparisons += 1;
                // Диаграмма CIE (сессия 14s): и картинка прибора на реальном входе — в допуске fp16 к эталону.
                if rule_name.starts_with("cie-") {
                    let samples = rule.planned_samples(shown.width(), shown.height(), &settings);
                    let cpu_picture =
                        scope_model::render(&cpu, rule, &settings.render, picture_size(160, 96), samples).unwrap();
                    let readback = scopes.read_back_picture_blocking().unwrap();
                    let worst = worst_premultiplied_difference(&readback.texels, &cpu_picture);
                    assert!(
                        worst <= CIE_PICTURE_TOLERANCE,
                        "{name} / {storage:?} / {rule_name} / {mirror:?}: картинка разошлась на {worst}"
                    );
                    worst_cie_picture = worst_cie_picture.max(worst);
                }
            }
        }
    }
    (comparisons, worst_cie_picture)
}

#[test]
fn counters_match_cpu_on_viewfinder_output_of_synthetic_scenes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let mut comparisons = 0;
    let mut worst_picture = 0.0_f32;
    for (name, raw) in synthetic_scenes(&spec) {
        for storage in [
            ViewfinderStorageFormat::PreferTenBitUnorm,
            ViewfinderStorageFormat::HalfFloatOnly,
        ] {
            let (count, worst) = compare_scopes_on_viewfinder_output(&context, name, &raw, &params, &spec, storage);
            comparisons += count;
            worst_picture = worst_picture.max(worst);
        }
    }
    eprintln!("CIE, синтетика через ISP: наибольшее расхождение картинки {worst_picture:.2e}");
    assert!(comparisons >= 5 * 2 * 2 * 2, "сравнений {comparisons}");
    assert_no_validation_errors(&context);
}

#[test]
fn counters_match_cpu_on_viewfinder_output_of_real_crops() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut comparisons = 0;
    let mut worst_picture = 0.0_f32;
    for crop in ["gray-tile", "towel", "jar", "tap-highlight"] {
        let capture =
            isp_support::real_capture::load_real_capture(&isp_support::assets_dir(), &format!("real-main-lamp-{crop}"));
        let frame = FrameInputs {
            levels: capture.sensor.static_levels(),
        };
        let controls =
            ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Preview, DisplayGamut::DisplayP3)
                .with_legacy_tone();
        let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
        let raw = RawFrame {
            width: capture.raw.size().width,
            height: capture.raw.size().height,
            pattern: capture.raw.pattern(),
            samples: capture.raw.samples().to_vec(),
        };
        let spec = ParamsSpec {
            gamut: DisplayGamut::DisplayP3,
            ..ParamsSpec::default()
        };
        for storage in [
            ViewfinderStorageFormat::PreferTenBitUnorm,
            ViewfinderStorageFormat::HalfFloatOnly,
        ] {
            let (count, worst) = compare_scopes_on_viewfinder_output(&context, crop, &raw, &params, &spec, storage);
            comparisons += count;
            worst_picture = worst_picture.max(worst);
        }
    }
    eprintln!("CIE, реальные кропы: наибольшее расхождение картинки {worst_picture:.2e}");
    // Приборы: waveform, parade, два вектороскопа (три гаммы), Luma, три режима гистограммы и CIE (две шкалы × два охвата).
    assert_eq!(comparisons, 4 * 2 * 2 * 2 * 13, "матрица реальных кропов");
    assert_no_validation_errors(&context);
}
