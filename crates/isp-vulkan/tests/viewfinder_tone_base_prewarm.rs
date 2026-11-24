//! Заготовка вариантов пайплайна видоискателя по моделям основы тона (решение 195, сессия 14b; семейство вариантов —
//! пакет I5 сессии 16a).
//!
//! Основа переключается на ходу (свайп вида), а модель — specialization-константа шейдера: без заготовки первый кадр
//! с новой основой компилировал бы пайплайн посреди сессии. Тесты на RADV с validation доказывают: после первого кадра
//! варианты всех четырёх моделей уже есть, смена основы не создаёт ни одного нового пайплайна, а кадр после смены
//! совпадает с эталоном `isp-model` ИМЕННО этой модели. Кроме моделей, заготавливаются кадр без статистики и обе
//! раскладки AWB (зум): их проверяет `viewfinder_zoom_variants`.

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
    DEFAULT_PIPELINE_VARIANT_CAPACITY, VariantPrewarm, ViewfinderIsp, ViewfinderStorageFormat,
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
        variant_prewarm: VariantPrewarm::OnRenderThread,
        ..test_config(ViewfinderStorageFormat::HalfFloatOnly)
    };
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
    assert_eq!(isp.cached_pipeline_variants(), 0, "до первого кадра пайплайнов нет");
    // Кадр со статистикой AWB (все стадии): основа × (без статистики + две раскладки группы по плиткам).
    let family = every_base().len() * 3;

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
                family,
                "первый кадр заготовил варианты всех моделей (и классов статистики) своего набора стадий"
            );
            let stats = isp.variant_stats();
            assert_eq!(
                (stats.created_in_frame, stats.created_by_prewarm, stats.prewarm_failed),
                (1, family as u64 - 1, 0),
                "в кадре создан свой вариант, остальное — заготовка"
            );
        }
        assert_eq!(
            isp.cached_pipeline_variants(),
            family,
            "{name}: смена основы создала новый пайплайн"
        );
        let stats = isp.variant_stats();
        assert_eq!(
            (stats.created_in_frame, stats.created_by_prewarm, stats.evicted),
            (1, family as u64 - 1, 0),
            "{name}: после смены основы ни создания в кадре, ни повторной заготовки, ни вытеснения"
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
        config.variant_prewarm,
        VariantPrewarm::Off,
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
    let stats = isp.variant_stats();
    assert_eq!(
        (stats.created_in_frame, stats.created_by_prewarm, stats.prewarm_failed),
        (2, 0, 0),
        "оба варианта созданы по требованию в кадре, заготовки не было"
    );
    assert_no_validation_errors(&context);
}

/// Прогрев занимает только свободные места: при тесном кэше он не вытесняет вариант текущего кадра, кадр рисуется.
#[test]
fn prewarm_never_evicts_the_variant_in_use_when_the_cache_is_tight() {
    let Some(context) = gpu_context_or_skip() else { return };
    let config = isp_vulkan::viewfinder::ViewfinderIspConfig {
        variant_prewarm: VariantPrewarm::OnRenderThread,
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
        isp.variant_stats().created_by_prewarm,
        1,
        "заготовлено ровно одно свободное место"
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

/// Ёмкость по умолчанию вмещает продукт целиком: 4 набора стадий (виньетирование × таблица вида) × 3 класса статистики
/// (без неё и две раскладки групп по плиткам AWB; статистика идёт каждый 4-й кадр, зум меняет раскладку) × 4 модели.
#[test]
fn default_capacity_holds_every_product_stage_set_in_every_base() {
    let stage_sets_in_product = 2 * 2;
    let statistics_classes = 1 + isp_vulkan::viewfinder::AwbGroupLayout::ALL.len();
    let bases = every_base().len();
    assert_eq!(
        DEFAULT_PIPELINE_VARIANT_CAPACITY,
        stage_sets_in_product * statistics_classes * bases
    );
    assert_eq!(DEFAULT_PIPELINE_VARIANT_CAPACITY, 48);
    assert_eq!(
        isp_vulkan::viewfinder::ViewfinderIspConfig::default().pipeline_variant_capacity,
        DEFAULT_PIPELINE_VARIANT_CAPACITY
    );
    assert_eq!(
        isp_vulkan::viewfinder::ViewfinderIspConfig::default().variant_prewarm,
        VariantPrewarm::InBackground,
        "прогрев по умолчанию включён"
    );
}
