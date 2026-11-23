//! Прогрев вариантов пайплайна видоискателя по моделям основы тона (решение 195, сессия 14b).
//!
//! Основа переключается на ходу (свайп вида), а модель — specialization-константа шейдера: без прогрева первый кадр
//! с новой основой компилировал бы пайплайн посреди сессии. Тесты на RADV с validation доказывают: после первого кадра
//! варианты всех четырёх моделей уже есть, смена основы не создаёт ни одного нового пайплайна, а кадр после смены
//! совпадает с эталоном `isp-model` ИМЕННО этой модели.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use gpu_vulkan::backdrop::BackdropFrameNumber;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_vulkan::viewfinder::{
    DEFAULT_PIPELINE_VARIANT_CAPACITY, PrewarmCounters, ToneBasePrewarm, ViewfinderIsp, ViewfinderStorageFormat,
};
use viewfinder_support::{
    ParamsSpec, RunRequest, assert_no_validation_errors, gpu_context_or_skip, parity_with_pixels, readback,
    record_and_submit, reference_upright_encoded, synthetic_scenes, test_config,
};

/// Все четыре модели основы с умолчаниями.
fn every_base() -> [(&'static str, ToneBase); 4] {
    [
        ("legacy", ToneBase::Legacy),
        ("agx", ToneBase::Agx(AgxSettings::default())),
        ("aces", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
    ]
}

fn spec_with(base: ToneBase) -> ParamsSpec {
    ParamsSpec {
        tone_base: base,
        ..ParamsSpec::default()
    }
}

/// Самый важный сценарий: первый кадр прогревает остальные модели, дальше смена основы не создаёт пайплайнов, а
/// картинка каждого кадра равна эталону своей модели.
#[test]
fn first_frame_prewarms_every_base_and_switching_creates_no_pipelines() {
    let Some(context) = gpu_context_or_skip() else { return };
    let config = isp_vulkan::viewfinder::ViewfinderIspConfig {
        tone_base_prewarm: ToneBasePrewarm::AllModels,
        ..test_config(ViewfinderStorageFormat::HalfFloatOnly)
    };
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
    assert_eq!(isp.cached_pipeline_variants(), 0, "до первого кадра пайплайнов нет");

    let legacy_spec = spec_with(ToneBase::Legacy);
    let (_, raw) = synthetic_scenes(&legacy_spec).into_iter().next().unwrap();
    let gpu_source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = gpu_source.source();

    let mut pictures = Vec::new();
    for (frame, (name, base)) in every_base().into_iter().enumerate() {
        let spec = spec_with(base);
        let params = spec.build();
        let request = RunRequest::whole_frame(&raw, &params, spec.display());
        record_and_submit(
            &context,
            &mut isp,
            &request,
            &source,
            frame % 2,
            BackdropFrameNumber(frame as u64 + 1),
        );
        let run = readback(&mut isp, frame % 2);
        if frame == 0 {
            assert_eq!(
                isp.cached_pipeline_variants(),
                4,
                "первый кадр заготовил варианты всех четырёх моделей"
            );
            assert_eq!(
                isp.tone_base_prewarm_counters(),
                PrewarmCounters { created: 3, failed: 0 },
                "прогрето три «чужих» модели, свой вариант не считается прогревом"
            );
        }
        assert_eq!(
            isp.cached_pipeline_variants(),
            4,
            "{name}: смена основы создала новый пайплайн"
        );
        assert_eq!(
            isp.tone_base_prewarm_counters(),
            PrewarmCounters { created: 3, failed: 0 },
            "{name}: после смены основы прогрев не повторяется"
        );
        let reference = reference_upright_encoded(
            &raw,
            raw.full_rect(),
            camera_contract::geometry::SensorOrientation::Degrees0,
            &params,
        );
        let parity = parity_with_pixels(&run, reference.width, reference.height, &reference.pixels);
        assert!(
            parity.math_excess <= viewfinder_support::MATH_EXCESS_TOLERANCE && parity.over_one_lsb == 0.0,
            "{name}: кадр после смены основы не совпал с эталоном этой модели: {parity:?}"
        );
        pictures.push(run.pixels);
    }
    // Модели дают разные картинки: иначе совпадение с эталоном ничего не доказывало бы.
    for earlier in 0..pictures.len() {
        for later in earlier + 1..pictures.len() {
            assert_ne!(
                pictures[earlier], pictures[later],
                "модели {earlier} и {later} дали одну картинку"
            );
        }
    }
    assert_no_validation_errors(&context);
}

/// Без прогрева (`Off`) поведение прежнее: вариант на модель создаётся лениво, счётчик прогрева нулевой.
#[test]
fn without_prewarm_variants_appear_one_per_used_base() {
    let Some(context) = gpu_context_or_skip() else { return };
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    assert_eq!(
        config.tone_base_prewarm,
        ToneBasePrewarm::Off,
        "стенд по умолчанию без прогрева"
    );
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
    let spec = spec_with(ToneBase::Legacy);
    let (_, raw) = synthetic_scenes(&spec).into_iter().next().unwrap();
    let gpu_source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = gpu_source.source();
    for (frame, (_, base)) in every_base().into_iter().take(2).enumerate() {
        let spec = spec_with(base);
        let params = spec.build();
        let request = RunRequest::whole_frame(&raw, &params, spec.display());
        record_and_submit(
            &context,
            &mut isp,
            &request,
            &source,
            frame % 2,
            BackdropFrameNumber(frame as u64 + 1),
        );
        isp.collect(frame % 2).unwrap();
        assert_eq!(isp.cached_pipeline_variants(), frame + 1);
    }
    assert_eq!(isp.tone_base_prewarm_counters(), PrewarmCounters::default());
    assert_no_validation_errors(&context);
}

/// Прогрев занимает только свободные места: при тесном кэше он не вытесняет вариант текущего кадра, кадр рисуется.
#[test]
fn prewarm_never_evicts_the_variant_in_use_when_the_cache_is_tight() {
    let Some(context) = gpu_context_or_skip() else { return };
    let config = isp_vulkan::viewfinder::ViewfinderIspConfig {
        tone_base_prewarm: ToneBasePrewarm::AllModels,
        pipeline_variant_capacity: 2,
        ..test_config(ViewfinderStorageFormat::HalfFloatOnly)
    };
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
    let spec = spec_with(ToneBase::Agx(AgxSettings::default()));
    let params = spec.build();
    let (_, raw) = synthetic_scenes(&spec).into_iter().next().unwrap();
    let gpu_source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = gpu_source.source();
    let request = RunRequest::whole_frame(&raw, &params, spec.display());
    record_and_submit(&context, &mut isp, &request, &source, 0, BackdropFrameNumber(1));
    let run = readback(&mut isp, 0);
    assert_eq!(isp.cached_pipeline_variants(), 2, "кэш заполнен, но не переполнен");
    assert_eq!(
        isp.tone_base_prewarm_counters().created,
        1,
        "прогрето ровно одно свободное место"
    );
    assert_eq!(isp.retired_objects(), 0, "прогрев ничего не вытеснил");
    let reference = reference_upright_encoded(
        &raw,
        raw.full_rect(),
        camera_contract::geometry::SensorOrientation::Degrees0,
        &params,
    );
    let parity = parity_with_pixels(&run, reference.width, reference.height, &reference.pixels);
    assert!(
        parity.math_excess <= viewfinder_support::MATH_EXCESS_TOLERANCE,
        "{parity:?}"
    );
    assert_no_validation_errors(&context);
}

/// Ёмкость по умолчанию вмещает продукт целиком: 4 набора стадий (виньетирование × таблица вида) × 4 модели.
#[test]
fn default_capacity_holds_every_product_stage_set_in_every_base() {
    let stage_sets_in_product = 2 * 2;
    let bases = every_base().len();
    assert_eq!(DEFAULT_PIPELINE_VARIANT_CAPACITY, stage_sets_in_product * bases);
    assert_eq!(
        isp_vulkan::viewfinder::ViewfinderIspConfig::default().pipeline_variant_capacity,
        DEFAULT_PIPELINE_VARIANT_CAPACITY
    );
    assert_eq!(
        isp_vulkan::viewfinder::ViewfinderIspConfig::default().tone_base_prewarm,
        ToneBasePrewarm::AllModels,
        "прогрев по умолчанию включён"
    );
}
