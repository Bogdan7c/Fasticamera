//! Проверка света вспышки по пикселям: чтение зелёного решёткой и решение «освещён / нет / нерешаемо».

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use std::sync::Arc;

use camera_contract::geometry::{PixelRect, PixelSize};
use still_contract::shot::OwnedPixels;

use isp_model::cfa::RgbChannel;
use isp_model::image::RawMosaic;
use isp_model::lens_shading::{LensShadingGainMap, LensShadingSkipReason, apply_lens_shading_with};
use isp_model::levels::normalize_levels;
use isp_model::statistics::{AwbStatisticsInput, AwbStatisticsSettings, compute_awb_statistics};

use super::*;

const SIZE: PixelSize = PixelSize::new(64, 48);
const THRESHOLDS: LitThresholds = LitThresholds {
    min_fraction_of_predicted: 0.5,
    min_predicted_share: 0.2,
};

/// Отсчёты размером `SIZE`: значение пикселя задаёт `value_at(x, y)`.
fn samples_with(value_at: impl Fn(u32, u32) -> u16) -> RawSamples16 {
    let mut bytes = Vec::new();
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            bytes.extend_from_slice(&value_at(x, y).to_le_bytes());
        }
    }
    RawSamples16::new(SIZE, Arc::new(OwnedPixels(bytes))).unwrap()
}

fn levels() -> NormalizationLevels {
    NormalizationLevels::new([64.0; 4], 1064.0).unwrap()
}

fn full_area() -> PixelRect {
    PixelRect {
        left: 0,
        top: 0,
        size: SIZE,
    }
}

/// Без коррекции виньетирования (стадия пропущена управлением): шкала до коррекции, как раньше.
fn no_shading() -> LensShadingStep {
    LensShadingStep::Skip(LensShadingSkipReason::DisabledByControl)
}

fn mean_with(samples: &RawSamples16, shading: &LensShadingStep, area: PixelRect, stride_cells: u32) -> Option<f64> {
    let statistics = AwbStatisticsSettings::default();
    mean_green_level(GreenSampling {
        samples,
        pattern: BayerPattern::BGGR,
        levels: &levels(),
        shading,
        area,
        stride_cells,
        saturation_threshold: statistics.saturation_threshold,
        sum_headroom: statistics.sum_headroom,
    })
}

fn mean(samples: &RawSamples16, area: PixelRect, stride_cells: u32) -> Option<f64> {
    mean_with(samples, &no_shading(), area, stride_cells)
}

/// Карта виньетирования 5×4: усиление растёт от центра к углам до ×3 (во всех каналах одинаково).
fn vignetting_step() -> LensShadingStep {
    let (columns, rows) = (5_u32, 4_u32);
    let points = (0..rows)
        .flat_map(|row| (0..columns).map(move |column| (column, row)))
        .map(|(column, row)| {
            let dx = f64::from(column) / f64::from(columns - 1) - 0.5;
            let dy = f64::from(row) / f64::from(rows - 1) - 0.5;
            let gain = (1.0 + 8.0 * (dx * dx + dy * dy)) as f32;
            [gain; 4]
        })
        .collect();
    let map = LensShadingGainMap::new(columns, rows, points).expect("карта");
    LensShadingStep::Apply(Arc::new(map))
}

/// RAW равномерной сцены: после коррекции `step` каждый отсчёт даёт ровно `scene_level` (до округления до u16).
fn uniform_scene_under(step: &LensShadingStep, scene_level: f32) -> RawSamples16 {
    samples_with(|x, y| {
        let gain = step.gain_for_sample((x, y), SIZE, RgbChannel::Green, LensShadingSampling::PerCellCentre);
        // Нормированный (до коррекции) уровень = сцена / усиление; чёрный 64, диапазон 1000.
        (64.0 + 1000.0 * scene_level / gain).round() as u16
    })
}

/// Уровень сцены так, как его считает статистика видоискателя по тому же RAW: настоящий эталон `isp-model`
/// (нормировка → виньетирование «по ячейке» → суммы плиток), затем формула среднего зелёного плитки.
fn statistics_green_level(samples: &RawSamples16, step: &LensShadingStep) -> f64 {
    let raw_bytes: Vec<u16> = (0..SIZE.height)
        .flat_map(|y| (0..SIZE.width).map(move |x| (x, y)))
        .map(|(x, y)| samples.sample(x, y).expect("отсчёт"))
        .collect();
    let raw = RawMosaic::new(SIZE, BayerPattern::BGGR, raw_bytes).expect("мозаика");
    let normalized = normalize_levels(&raw, &levels());
    let (corrected, _) = apply_lens_shading_with(&normalized, step, LensShadingSampling::PerCellCentre);
    let settings = AwbStatisticsSettings::default();
    let input = AwbStatisticsInput::new(&normalized, &corrected).expect("вход статистики");
    let statistics = compute_awb_statistics(input, &settings).expect("статистика");
    let tiles = statistics.tiles();
    let sum: f64 = tiles
        .iter()
        .map(|tile| {
            let cells = f64::from(tile.used_cells + tile.saturated_cells);
            f64::from(tile.sum_g) / (2.0 * 4095.0 * cells) + f64::from(tile.saturated_cells) / cells
        })
        .sum();
    sum / tiles.len() as f64
}

#[test]
fn mean_reads_only_the_green_samples_with_the_black_level_removed() {
    // BGGR: зелёные — позиции (1,0) и (0,1) квадрата. Зелёные = 564 (→ 0,5), красные и синие = 364 (→ 0,3). Красные и
    // синие НЕ белые: по правилу статистики пересвеченный отсчёт любого цвета делает всю ячейку белой (см. тест ниже).
    let samples = samples_with(|x, y| if (x + y) % 2 == 1 { 564 } else { 364 });
    let level = mean(&samples, full_area(), 1).unwrap();
    assert!((level - 0.5).abs() < 1e-6, "{level}");
}

#[test]
fn clipped_samples_count_as_white_and_a_sparse_grid_agrees_with_the_full_one_on_a_flat_scene() {
    let samples = samples_with(|x, y| if (x + y) % 2 == 1 { 5000 } else { 64 });
    assert!((mean(&samples, full_area(), 1).unwrap() - 1.0).abs() < 1e-6);
    assert!((mean(&samples, full_area(), 4).unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn the_area_limits_where_samples_are_read() {
    // Левая половина тёмная, правая яркая; область — правая половина.
    let samples = samples_with(|x, y| {
        if (x + y) % 2 == 1 {
            if x < 32 { 64 } else { 1064 }
        } else {
            64
        }
    });
    let right = PixelRect {
        left: 32,
        top: 0,
        size: PixelSize::new(32, 48),
    };
    assert!((mean(&samples, right, 1).unwrap() - 1.0).abs() < 1e-6);
    let left = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(32, 48),
    };
    assert!(mean(&samples, left, 1).unwrap().abs() < 1e-6);
}

#[test]
fn an_empty_or_outside_area_gives_nothing_instead_of_a_made_up_number() {
    let samples = samples_with(|_, _| 500);
    let outside = PixelRect {
        left: 100,
        top: 100,
        size: PixelSize::new(8, 8),
    };
    assert_eq!(mean(&samples, outside, 1), None);
}

#[test]
fn a_frame_much_brighter_per_unit_product_than_the_scene_is_lit() {
    // Сцена 0,1 на единицу произведения; кадр 0,4: вспышка дала 75 % света. Предсказывали 60 %.
    let verdict = judge_lit(0.6, Some(0.1), 0.4, THRESHOLDS);
    assert!(matches!(verdict, LitVerdict::Lit { measured_share } if (measured_share - 0.75).abs() < 1e-9));
}

#[test]
fn a_frame_as_bright_as_the_scene_is_not_lit_even_when_the_hal_said_fired() {
    let verdict = judge_lit(0.6, Some(0.1), 0.1, THRESHOLDS);
    assert!(matches!(verdict, LitVerdict::NotLit { measured_share } if measured_share.abs() < 1e-9));
    // Ниже порога от предсказанного: измерили 20 % при 0,5 × 60 % = 30 %.
    assert!(matches!(
        judge_lit(0.6, Some(0.1), 0.125, THRESHOLDS),
        LitVerdict::NotLit { .. }
    ));
    // Чёрный кадр: света нет вовсе.
    assert!(matches!(
        judge_lit(0.6, Some(0.1), 0.0, THRESHOLDS),
        LitVerdict::NotLit { measured_share } if measured_share == 0.0
    ));
}

#[test]
fn a_dim_flash_or_a_missing_scene_level_makes_the_check_undecidable_not_failed() {
    assert_eq!(
        judge_lit(0.1, Some(0.1), 0.1, THRESHOLDS),
        LitVerdict::Undecidable(LitUndecidable::PredictedShareTooLow)
    );
    assert_eq!(
        judge_lit(0.6, None, 0.4, THRESHOLDS),
        LitVerdict::Undecidable(LitUndecidable::NoAmbientLevel)
    );
}

#[test]
fn under_vignetting_the_frame_is_measured_in_the_same_scale_as_the_statistics_of_that_frame() {
    // Дефект 14f: статистика считает ПОСЛЕ коррекции виньетирования, а проверка света читала RAW ДО неё — шкалы
    // расходились в 1,7–1,8 раза и исправный кадр браковался. Равномерная сцена 0,4 под картой до ×3.
    let step = vignetting_step();
    let samples = uniform_scene_under(&step, 0.4);
    let by_statistics = statistics_green_level(&samples, &step);
    assert!((by_statistics - 0.4).abs() < 0.01, "эталон статистики: {by_statistics}");
    let ours = mean_with(&samples, &step, full_area(), 1).unwrap();
    assert!(
        (ours - by_statistics).abs() < 0.02 * by_statistics,
        "проверка света {ours} против статистики {by_statistics}"
    );
    // Редкая решётка на гладкой сцене даёт то же.
    let sparse = mean_with(&samples, &step, full_area(), 4).unwrap();
    assert!((sparse - by_statistics).abs() < 0.03 * by_statistics, "{sparse}");
    // Прежнее поведение (без коррекции) заметно ниже: именно оно давало расхождение шкал.
    let uncorrected = mean_with(&samples, &no_shading(), full_area(), 1).unwrap();
    assert!(uncorrected < 0.75 * by_statistics, "{uncorrected}");
}

#[test]
fn with_the_correction_disabled_the_old_scale_is_kept() {
    // Стадия пропущена: усиление 1, шкала = нормированный зелёный как был (и статистика на этом кадре её же даёт).
    let step = vignetting_step();
    let samples = uniform_scene_under(&step, 0.4);
    let disabled = no_shading();
    let ours = mean_with(&samples, &disabled, full_area(), 1).unwrap();
    let by_statistics = statistics_green_level(&samples, &disabled);
    assert!((ours - by_statistics).abs() < 0.01, "{ours} против {by_statistics}");
    // Эталон: среднее нормированных зелёных отсчётов, посчитанное напрямую.
    let mut direct = (0.0_f64, 0_u32);
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            if (x + y) % 2 == 1 {
                direct.0 += f64::from((f64::from(samples.sample(x, y).unwrap()) as f32 - 64.0) / 1000.0);
                direct.1 += 1;
            }
        }
    }
    assert!((ours - direct.0 / f64::from(direct.1)).abs() < 1e-4);
}

#[test]
fn a_cell_clipped_on_the_raw_sample_is_white_even_when_the_gain_would_push_it_further() {
    // Пересвет решает сырой отсчёт ДО усиления: ячейка с белым отсчётом = 1,0, а не 1,0 × усиление.
    let step = vignetting_step();
    let samples = samples_with(|_, _| 1064);
    let level = mean_with(&samples, &step, full_area(), 1).unwrap();
    assert!((level - 1.0).abs() < 1e-6, "{level}");
}

#[test]
fn corrected_corners_above_white_are_kept_up_to_the_headroom_not_cut_at_one() {
    // Тёмные углы: сырой уровень 0,2, усиление до ×3 → 0,6 < 1; а при сырых 0,9 (ниже порога 0,95) усиление ×3 даёт
    // 2,7: статистика хранит это (запас 16), а не обрезает до 1.
    let step = vignetting_step();
    let samples = samples_with(|_, _| 64 + 900);
    let level = mean_with(&samples, &step, full_area(), 1).unwrap();
    assert!(level > 1.5, "значения выше белого сохранены: {level}");
    let by_statistics = statistics_green_level(&samples, &step);
    assert!(
        (level - by_statistics).abs() < 0.02 * by_statistics,
        "{level} против {by_statistics}"
    );
}

#[test]
fn a_cell_with_one_clipped_red_sample_is_white_for_all_its_greens() {
    // Правило статистики: пересвет любого из четырёх отсчётов ячейки — белая ячейка.
    let samples = samples_with(|x, y| {
        if (x + y) % 2 == 1 {
            564
        } else if x % 2 == 0 && y % 2 == 0 {
            1064
        } else {
            364
        }
    });
    let level = mean(&samples, full_area(), 1).unwrap();
    assert!((level - 1.0).abs() < 1e-6, "{level}");
}
