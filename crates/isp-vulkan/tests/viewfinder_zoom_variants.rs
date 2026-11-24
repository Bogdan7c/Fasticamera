//! Плавный зум, смена основы тона и чередование «кадр со статистикой AWB / без неё» не компилируют пайплайны
//! посреди сессии (пакет I5, сессия 16a).
//!
//! Вариант шейдера зависит от раскладки рабочих групп по плиткам статистики ([`AwbGroupLayout`]), а она считается
//! из кадрирования каждого кадра: при плавном зуме признак то «короткий путь», то «общий». Без заготовки обеих
//! раскладок первое переключение признака создавало бы пайплайн (и ещё три — для остальных основ) прямо в кадре
//! видоискателя. Тест проходит весь путь зума туда и обратно с продуктовой каденцией статистики (каждый 4-й кадр),
//! обоими поворотами сенсора и сменой основы на ходу, и на КАЖДОМ шаге сверяет картинку и суммы статистики с
//! эталоном `isp-model`: вариант, созданный заранее, обязан считать так же, как созданный по требованию.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::context::GpuContext;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_vulkan::viewfinder::{
    AwbGroupLayout, AwbReductionPolicy, IspStage, IspStageSet, VariantCacheStats, VariantPrewarm, ViewfinderIsp,
    ViewfinderIspConfig, ViewfinderStorageFormat, WorkgroupSizeConfig,
};
use viewfinder_support::{
    GpuSource, ParamsSpec, RawFrame, RunRequest, SourceLayout, assert_no_validation_errors, cpu_awb_statistics,
    gpu_context_or_skip, parity_with_pixels, readback, record_and_submit, reference_upright, reference_upright_encoded,
    test_config,
};

/// Размер кадра сенсора теста (предел `largest_mosaic` тестового конфига).
const FRAME_WIDTH: u32 = 256;
const FRAME_HEIGHT: u32 = 192;
/// Наибольшая кратность зума пути.
const MAX_ZOOM: f64 = 4.0;
/// Шагов зума в одну сторону (на «круглых» кратностях 1×, 2×, 3×, 4× раскладка меняется по-своему).
const ZOOM_STEPS: u32 = 24;
/// Статистику в продукте считают каждый 4-й кадр.
const STATISTICS_EVERY_NTH_FRAME: usize = 4;
/// Кадров на одну основу тона перед сменой на следующую.
const FRAMES_PER_TONE_BASE: usize = 25;

fn spec_with(base: ToneBase) -> ParamsSpec {
    ParamsSpec {
        // `white − black` = 1024: деление уровней точное, суммы статистики равны эталону бит в бит.
        black: [64.0; 4],
        white: 1088.0,
        tone_base: base,
        ..ParamsSpec::default()
    }
}

fn tone_bases() -> [ToneBase; 4] {
    [
        ToneBase::Legacy,
        ToneBase::Agx(AgxSettings::default()),
        ToneBase::Aces(AcesSettings::default()),
        ToneBase::Filmic(FilmicSettings::default()),
    ]
}

/// Пёстрая сцена: значения «псевдослучайны» по пикселю, чтобы любая ошибка плитки или основы меняла числа.
fn varied_scene(spec: &ParamsSpec) -> RawFrame {
    let (black, white) = spec.levels();
    let range = f64::from(white) - f64::from(black[0]);
    RawFrame::from_fn(FRAME_WIDTH, FRAME_HEIGHT, spec.pattern, |x, y, channel| {
        let hash = (x * 7919 + y * 104_729 + channel as u32 * 31) % 1000;
        let level = f64::from(hash) / 1000.0 * 0.85 + f64::from(x + y) / f64::from(FRAME_WIDTH + FRAME_HEIGHT) * 0.1;
        (f64::from(black[0]) + level * range).round() as u16
    })
}

/// Кадрирование по центру для кратности `zoom` (стороны чётные: ячейка RAW — 2×2).
fn centered_crop(zoom: f64) -> PixelRect {
    let even_floor = |value: f64| (value as u32) & !1;
    let width = even_floor(f64::from(FRAME_WIDTH) / zoom).max(2);
    let height = even_floor(f64::from(FRAME_HEIGHT) / zoom).max(2);
    PixelRect {
        left: ((FRAME_WIDTH - width) / 2) & !1,
        top: ((FRAME_HEIGHT - height) / 2) & !1,
        size: PixelSize::new(width, height),
    }
}

/// Путь зума: от 1× до наибольшей кратности и обратно (граничные кадры не повторяются).
fn zoom_path() -> Vec<PixelRect> {
    let zoom_of = |step: u32| 1.0 + (MAX_ZOOM - 1.0) * f64::from(step) / f64::from(ZOOM_STEPS);
    let way_in = (0..=ZOOM_STEPS).map(zoom_of);
    let way_out = (0..ZOOM_STEPS).rev().map(zoom_of);
    way_in.chain(way_out).map(centered_crop).collect()
}

/// Конфиг теста: рабочая группа 8×8 и сетка 4×3 дают на пути зума ОБЕ раскладки; сведение — shared-память
/// (раскладка важна только ей: у `subgroup` она всегда общая).
fn config(prewarm: VariantPrewarm) -> ViewfinderIspConfig {
    ViewfinderIspConfig {
        workgroup_size: WorkgroupSizeConfig { x: 8, y: 8 },
        awb_reduction: AwbReductionPolicy::SharedMemoryOnly,
        variant_prewarm: prewarm,
        ..test_config(ViewfinderStorageFormat::HalfFloatOnly)
    }
}

/// Итог прохода по пути зума: что видела диагностика кэша после каждого кадра и какие раскладки задел путь.
struct SweepReport {
    /// Счётчики кэша вариантов после каждого кадра по порядку.
    stats_after_each_frame: Vec<VariantCacheStats>,
    /// Раскладки групп по плиткам, виденные на кадрах со статистикой.
    layouts_seen: Vec<AwbGroupLayout>,
}

/// Проходит весь путь зума (оба поворота) с продуктовой каденцией статистики и сменой основы; на КАЖДОМ шаге
/// сверяет картинку и суммы статистики с эталоном `isp-model` и отдаёт счётчики кэша вариантов для проверок вызывающего.
/// `frames_limit` обрезает путь (быстрые проверки без полного прохода).
fn sweep_zoom_path(context: &GpuContext, isp: &mut ViewfinderIsp, frames_limit: usize) -> SweepReport {
    let raw = varied_scene(&spec_with(tone_bases()[0]));
    let gpu_source = GpuSource::new(context, &raw, SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut report = SweepReport {
        stats_after_each_frame: Vec::new(),
        layouts_seen: Vec::new(),
    };
    let mut frame = 0_usize;
    for orientation in [SensorOrientation::Degrees90, SensorOrientation::Degrees0] {
        for crop in zoom_path() {
            if frame == frames_limit {
                return report;
            }
            let base = tone_bases()[(frame / FRAMES_PER_TONE_BASE) % tone_bases().len()];
            let spec = spec_with(base);
            let params = spec.build();
            let with_statistics = frame.is_multiple_of(STATISTICS_EVERY_NTH_FRAME);
            let mut request = RunRequest::whole_frame(&raw, &params, spec.display());
            request.stages = if with_statistics {
                IspStageSet::ALL
            } else {
                IspStageSet::ALL.without(IspStage::AwbStatistics)
            };
            request.geometry.crop = crop;
            request.geometry.sensor_orientation = orientation;
            let slot = frame % isp.frame_slots();
            record_and_submit(
                context,
                isp,
                &request,
                &source,
                slot,
                BackdropFrameNumber(frame as u64 + 1),
            );
            let run = readback(isp, slot);
            let step = format!("кадр {frame}, {orientation:?}, кадрирование {crop:?}, основа {base:?}");
            report.stats_after_each_frame.push(isp.variant_stats());

            // Картинка равна эталону основы этого кадра.
            let reference = reference_upright_encoded(&raw, crop, orientation, &params);
            let parity = parity_with_pixels(&run, reference.width, reference.height, &reference.pixels);
            assert!(
                parity.math_excess <= viewfinder_support::MATH_EXCESS_TOLERANCE && parity.over_one_lsb == 0.0,
                "{step}: картинка не совпала с эталоном: {parity:?}"
            );
            // Статистика: на кадре со статистикой равна эталону точно, на остальных её нет.
            if with_statistics {
                // Статистика считается до тона и от основы не зависит: эталон — прогон с прежней основой.
                let reference_run =
                    reference_upright(&raw, crop, orientation, &spec_with(ToneBase::Legacy).build()).run;
                assert_eq!(
                    run.report.awb_statistics.expect("кадр со статистикой"),
                    cpu_awb_statistics(&reference_run, &config(VariantPrewarm::Off).awb_statistics),
                    "{step}: статистика AWB не совпала с эталоном"
                );
                report
                    .layouts_seen
                    .push(isp.last_awb_group_layout().expect("кадр со статистикой"));
            } else {
                assert!(run.report.awb_statistics.is_none(), "{step}: статистики быть не должно");
            }
            frame += 1;
        }
    }
    report
}

/// Самый важный сценарий пакета: после первого кадра зум, смена основы и чередование статистики НЕ создают ни одного
/// пайплайна и ничего не вытесняют, а картинка и статистика каждого шага равны эталону.
#[test]
fn zoom_sweep_with_statistics_cadence_and_tone_base_changes_creates_no_pipelines_after_the_first_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut isp = ViewfinderIsp::new(&context, &config(VariantPrewarm::AllRelated)).unwrap();
    let report = sweep_zoom_path(&context, &mut isp, usize::MAX);

    let after_first = report.stats_after_each_frame[0];
    // Цена заготовки на этом ПК (RADV) — для отчёта: число пайплайнов и время их создания.
    eprintln!("первый кадр: {after_first:?}");
    assert_eq!(after_first.created_in_frame, 1, "первый кадр создал ровно свой вариант");
    assert_eq!(
        after_first.in_cache, 12,
        "и заготовил семейство: 4 основы × (без статистики + 2 раскладки)"
    );
    for (frame, stats) in report.stats_after_each_frame.iter().enumerate() {
        assert_eq!(
            (
                stats.created_in_frame,
                stats.created_by_prewarm,
                stats.evicted,
                stats.in_cache
            ),
            (1, after_first.created_by_prewarm, 0, after_first.in_cache),
            "кадр {frame} создал или вытеснил пайплайн"
        );
        assert_eq!(
            stats.prewarm_failed + stats.create_failed_in_frame,
            0,
            "кадр {frame}: отказы"
        );
    }
    // Тест что-то доказывает, только если путь зума действительно переключал раскладку.
    for layout in AwbGroupLayout::ALL {
        assert!(
            report.layouts_seen.contains(&layout),
            "путь зума не задел раскладку {layout:?}: {:?}",
            report.layouts_seen
        );
    }
    assert_no_validation_errors(&context);
}

/// Без заготовки поведение прежнее — «создание по требованию»: тот же путь зума даёт верную картинку и статистику,
/// а варианты создаются в кадрах (это и есть рывок, от которого заготовка защищает).
#[test]
fn without_prewarm_the_same_path_creates_variants_in_frames_but_draws_correctly() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut isp = ViewfinderIsp::new(&context, &config(VariantPrewarm::Off)).unwrap();
    let report = sweep_zoom_path(&context, &mut isp, usize::MAX);
    let last = *report.stats_after_each_frame.last().unwrap();
    assert_eq!(last.created_by_prewarm, 0, "заготовки не было");
    assert!(
        last.created_in_frame > 1,
        "варианты создавались по требованию, в кадрах: {last:?}"
    );
    assert_eq!(last.in_cache as u64, last.created_in_frame - last.evicted);
    assert_no_validation_errors(&context);
}

/// Тесный кэш: ёмкость 1 не оставляет места под заготовку (она останавливается, ничего не
/// вытесняя), а каждый кадр, которому нужен другой вариант, создаёт его по требованию и рисует верно.
#[test]
fn a_cache_with_no_room_for_prewarm_still_draws_every_frame_correctly() {
    let Some(context) = gpu_context_or_skip() else { return };
    let tight = ViewfinderIspConfig {
        pipeline_variant_capacity: 1,
        ..config(VariantPrewarm::AllRelated)
    };
    let mut isp = ViewfinderIsp::new(&context, &tight).unwrap();
    let report = sweep_zoom_path(&context, &mut isp, 30);
    let last = *report.stats_after_each_frame.last().unwrap();
    assert_eq!(last.created_by_prewarm, 0, "места под заготовку нет");
    assert_eq!(last.in_cache, 1);
    assert!(
        last.evicted > 0 && last.created_in_frame > 1,
        "кадры создавали по требованию: {last:?}"
    );
    assert_no_validation_errors(&context);
}
