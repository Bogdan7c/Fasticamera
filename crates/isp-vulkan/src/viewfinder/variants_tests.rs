//! Тесты кэша вариантов на настоящем GPU (`Headless`, validation): заготовка, отказы, вытеснение, счётчики.
//!
//! Отказы создания подставляются швом `prepare_with` (настоящий пайплайн для всех ключей, кроме выбранных): кадр,
//! который рисуется уже созданным вариантом, не должен страдать от отказа заготовки.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::validation::ValidationMode;

use super::*;
use crate::viewfinder::variant_family::{VARIANTS_PER_STAGE_SET, product_key};

/// Контекст `Headless` или `None` при осознанном пропуске (`FASTICAMERA_SKIP_GPU_TESTS=1`, как у остальных тестов).
fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error}");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите FASTICAMERA_SKIP_GPU_TESTS=1, чтобы пропустить осознанно");
            }
        }
    }
}

fn cache(capacity: usize) -> VariantCache {
    VariantCache::new(
        capacity,
        LocalSize::new(8, 8).unwrap(),
        2,
        AwbReduction::SharedMemoryWindow,
    )
}

/// Ключ продукта: все стадии, `General`.
fn key(tone_base: ToneBaseKind) -> VariantKey {
    crate::viewfinder::variant_family::product_key(tone_base, AwbGroupLayout::General)
}

/// Подготовка настоящим создателем.
fn prepare(
    cache: &mut VariantCache,
    context: &GpuContext,
    key: VariantKey,
    serial: u64,
    prewarm: VariantPrewarm,
) -> Result<EvictedPipeline, ViewfinderError> {
    cache.prepare(context, key, RecordSerial(serial), prewarm)
}

fn refusal() -> ViewfinderError {
    ViewfinderError::InvalidConfig {
        what: "тестовый отказ создания пайплайна".to_owned(),
    }
}

#[test]
fn creating_a_key_prewarms_its_whole_family_and_counts_it_apart_from_the_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(48);
    let evicted = prepare(
        &mut cache,
        &context,
        key(ToneBaseKind::Agx),
        1,
        VariantPrewarm::OnRenderThread,
    )
    .unwrap();
    assert!(evicted.is_none(), "в пустом кэше вытеснять нечего");
    let stats = cache.stats();
    assert_eq!(stats.in_cache, VARIANTS_PER_STAGE_SET, "ключ и всё его семейство");
    assert_eq!(stats.created_in_frame, 1, "в кадре создан только сам ключ");
    assert_eq!(stats.created_by_prewarm, VARIANTS_PER_STAGE_SET as u64 - 1);
    assert_eq!(
        (stats.prewarm_failed, stats.create_failed_in_frame, stats.evicted),
        (0, 0, 0)
    );
    assert!(
        stats.last_frame_compile > std::time::Duration::ZERO,
        "время создания измерено"
    );
    assert!(
        stats.last_prewarm >= stats.longest_frame_compile,
        "заготовка длиннее одного создания"
    );
    // Повторный кадр с тем же ключом и кадр с заготовленным соседом ничего не создают.
    prepare(
        &mut cache,
        &context,
        key(ToneBaseKind::Agx),
        2,
        VariantPrewarm::OnRenderThread,
    )
    .unwrap();
    prepare(
        &mut cache,
        &context,
        key(ToneBaseKind::Filmic),
        3,
        VariantPrewarm::OnRenderThread,
    )
    .unwrap();
    assert_eq!(cache.stats().created_in_frame, 1);
    assert_eq!(cache.stats().created_by_prewarm, VARIANTS_PER_STAGE_SET as u64 - 1);
}

#[test]
fn with_prewarm_off_variants_are_created_on_demand_one_by_one() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(48);
    prepare(&mut cache, &context, key(ToneBaseKind::Legacy), 1, VariantPrewarm::Off).unwrap();
    assert_eq!(cache.stats().in_cache, 1);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 2, VariantPrewarm::Off).unwrap();
    let stats = cache.stats();
    assert_eq!(
        (stats.in_cache, stats.created_in_frame, stats.created_by_prewarm),
        (2, 2, 0)
    );
    assert_eq!(stats.last_prewarm, std::time::Duration::ZERO, "заготовки не было");
}

#[test]
fn a_failed_prewarm_neither_breaks_the_frame_nor_blocks_the_other_siblings() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(48);
    let builder = cache.builder;
    // Отказывает создание всех вариантов модели Aces (кроме самого кадрового ключа — он Agx).
    let mut build = |variant: VariantKey| {
        if variant.tone_base == ToneBaseKind::Aces {
            Err(refusal())
        } else {
            builder.build(&context, variant)
        }
    };
    let outcome = cache.prepare_with(
        key(ToneBaseKind::Agx),
        RecordSerial(1),
        VariantPrewarm::OnRenderThread,
        &mut build,
    );
    assert!(outcome.is_ok(), "отказ заготовки не роняет кадр");
    let stats = cache.stats();
    let aces_variants = 3_u64; // три класса статистики модели Aces
    assert_eq!(stats.prewarm_failed, aces_variants);
    assert_eq!(
        stats.created_by_prewarm,
        VARIANTS_PER_STAGE_SET as u64 - 1 - aces_variants
    );
    assert_eq!(stats.in_cache, VARIANTS_PER_STAGE_SET - aces_variants as usize);
    // Кадр с вариантом, который не заготовился, создаёт его по требованию (прежнее поведение).
    prepare(&mut cache, &context, key(ToneBaseKind::Aces), 2, VariantPrewarm::Off).unwrap();
    assert_eq!(cache.stats().created_in_frame, 2);
}

#[test]
fn a_failed_frame_creation_is_counted_and_leaves_the_cache_untouched() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(4);
    prepare(&mut cache, &context, key(ToneBaseKind::Legacy), 1, VariantPrewarm::Off).unwrap();
    let mut build = |_: VariantKey| Err(refusal());
    let outcome = cache.prepare_with(
        key(ToneBaseKind::Agx),
        RecordSerial(2),
        VariantPrewarm::OnRenderThread,
        &mut build,
    );
    assert!(
        outcome.is_err(),
        "ошибка создания нужного кадру варианта — ошибка кадра"
    );
    let stats = cache.stats();
    assert_eq!(
        (stats.in_cache, stats.created_in_frame, stats.create_failed_in_frame),
        (1, 1, 1)
    );
    assert_eq!(
        stats.created_by_prewarm + stats.prewarm_failed,
        0,
        "после отказа заготовка не начиналась"
    );
}

#[test]
fn prewarm_takes_only_free_slots_and_never_evicts() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(3);
    let evicted = prepare(
        &mut cache,
        &context,
        key(ToneBaseKind::Agx),
        1,
        VariantPrewarm::OnRenderThread,
    )
    .unwrap();
    assert!(evicted.is_none());
    let stats = cache.stats();
    assert_eq!((stats.in_cache, stats.created_by_prewarm, stats.evicted), (3, 2, 0));
}

#[test]
fn an_unused_prewarmed_variant_is_evicted_before_a_working_one() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(3);
    // Рабочий вариант и две заготовки (самые нужные: тот же тон без статистики и другая раскладка).
    let working = key(ToneBaseKind::Agx);
    prepare(&mut cache, &context, working, 100, VariantPrewarm::OnRenderThread).unwrap();
    // Новый ключ вне семейства (другой набор стадий) в полном кэше вытесняет ЗАГОТОВКУ, а не рабочий вариант.
    let other_stage_set = VariantKey {
        stages: working.stages.without(IspStage::LookLut),
        ..working
    };
    let evicted = prepare(&mut cache, &context, other_stage_set, 101, VariantPrewarm::Off).unwrap();
    assert!(evicted.is_some(), "кэш полон: кого-то вытеснили");
    assert_eq!(cache.stats().evicted, 1);
    let created_before = cache.stats().created_in_frame;
    prepare(&mut cache, &context, working, 102, VariantPrewarm::Off).unwrap();
    assert_eq!(
        cache.stats().created_in_frame,
        created_before,
        "рабочий вариант остался в кэше"
    );
}

#[test]
fn using_a_variant_refreshes_its_recency_so_the_idle_one_is_evicted() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = cache(2);
    let (first, second, third) = (
        key(ToneBaseKind::Legacy),
        key(ToneBaseKind::Agx),
        key(ToneBaseKind::Aces),
    );
    prepare(&mut cache, &context, first, 1, VariantPrewarm::Off).unwrap();
    prepare(&mut cache, &context, second, 2, VariantPrewarm::Off).unwrap();
    // Первый использован позже второго: вытесняться должен второй.
    prepare(&mut cache, &context, first, 3, VariantPrewarm::Off).unwrap();
    prepare(&mut cache, &context, third, 4, VariantPrewarm::Off).unwrap();
    assert_eq!(cache.stats().evicted, 1);
    let created = cache.stats().created_in_frame;
    prepare(&mut cache, &context, first, 5, VariantPrewarm::Off).unwrap();
    assert_eq!(
        cache.stats().created_in_frame,
        created,
        "недавно использованный остался"
    );
    prepare(&mut cache, &context, second, 6, VariantPrewarm::Off).unwrap();
    assert_eq!(
        cache.stats().created_in_frame,
        created + 1,
        "простаивавший был вытеснен и создан заново"
    );
}

#[test]
fn a_no_statistics_key_prewarms_statistics_variants_only_after_statistics_were_used() {
    let Some(context) = gpu_context_or_skip() else { return };
    let no_statistics = |tone_base| VariantKey {
        stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
        ..product_key(tone_base, AwbGroupLayout::General)
    };
    // Статистика ещё не использовалась: кадр без неё заготавливает только другие основы.
    let mut cache = cache(48);
    prepare(
        &mut cache,
        &context,
        no_statistics(ToneBaseKind::Legacy),
        1,
        VariantPrewarm::OnRenderThread,
    )
    .unwrap();
    assert_eq!(cache.stats().in_cache, ToneBaseKind::ALL.len());
    // Статистику использовали: кадр со статистикой создан в кадре (его не было), а новый ключ без неё
    // (другой набор стадий) заготавливает всё семейство, включая варианты со статистикой.
    prepare(
        &mut cache,
        &context,
        key(ToneBaseKind::Legacy),
        2,
        VariantPrewarm::OnRenderThread,
    )
    .unwrap();
    let other_stage_set = VariantKey {
        stages: IspStageSet::ALL
            .without(IspStage::AwbStatistics)
            .without(IspStage::LookLut),
        ..no_statistics(ToneBaseKind::Legacy)
    };
    let before = cache.stats().in_cache;
    prepare(&mut cache, &context, other_stage_set, 3, VariantPrewarm::OnRenderThread).unwrap();
    assert_eq!(cache.stats().in_cache, before + VARIANTS_PER_STAGE_SET);
}
