use camera_contract::geometry::PixelSize;

use super::{
    AwbStatistics, AwbStatisticsInput, AwbStatisticsSettings, AwbTile, AwbTotals, DEFAULT_SUM_HEADROOM,
    InvalidAwbStatistics, MAX_CELLS_PER_TILE, MAX_SUM_HEADROOM, QUANT_MAX, TileCountMismatch, TileGrid,
    compute_awb_statistics, max_cells_per_tile, quantize_sample, quantize_sample_with_headroom, tile_of_cell,
};
use crate::cfa::{BayerPattern, RgbChannel};
use crate::image::{CameraMosaic, Mosaic, RawMosaic, ShadingCorrectedMosaic, mosaic_stage};
use crate::lens_shading::{LensShadingSkipReason, LensShadingStep, apply_lens_shading};
use crate::levels::{NormalizationLevels, normalize_levels};

/// Цвет ячейки 2×2: красный, два зелёных (в порядке позиций квадрата) и синий.
#[derive(Clone, Copy)]
struct CellColor {
    red: f32,
    greens: [f32; 2],
    blue: f32,
}

impl CellColor {
    const fn uniform(red: f32, green: f32, blue: f32) -> Self {
        Self {
            red,
            greens: [green, green],
            blue,
        }
    }
}

/// Мозаика `cells_width × cells_height` ячеек: цвет каждой ячейки раскладывается по позициям
/// квадрата согласно `pattern`.
fn mosaic_from_cells(
    pattern: BayerPattern,
    cells: PixelSize,
    color_of_cell: impl Fn(u32, u32) -> CellColor,
) -> ShadingCorrectedMosaic {
    let size = PixelSize::new(cells.width * 2, cells.height * 2);
    let mut samples = vec![0.0; size.pixel_count() as usize];
    for y in 0..size.height {
        for x in 0..size.width {
            let color = color_of_cell(x / 2, y / 2);
            // Какой это зелёный (первый или второй по порядку позиций квадрата): сколько
            // зелёных позиций стоит в квадрате раньше этой.
            let position = (y % 2) * 2 + x % 2;
            let green_rank = (0..position)
                .filter(|&earlier| pattern.channel_at(earlier % 2, earlier / 2) == RgbChannel::Green)
                .count();
            samples[(y * size.width + x) as usize] = match pattern.channel_at(x, y) {
                RgbChannel::Red => color.red,
                RgbChannel::Green => color.greens[green_rank],
                RgbChannel::Blue => color.blue,
            };
        }
    }
    Mosaic::from_samples(size, pattern, samples, mosaic_stage::ShadingCorrected).unwrap()
}

/// Настройки с запасом суммы 1,0 (прежнее поведение: потолок — белый сенсора).
fn settings(columns: u32, rows: u32, saturation_threshold: f32) -> AwbStatisticsSettings {
    settings_with_headroom(columns, rows, saturation_threshold, 1.0)
}

fn settings_with_headroom(
    columns: u32,
    rows: u32,
    saturation_threshold: f32,
    sum_headroom: f32,
) -> AwbStatisticsSettings {
    AwbStatisticsSettings {
        grid: TileGrid { columns, rows },
        saturation_threshold,
        sum_headroom,
    }
}

/// Те же отсчёты как нормированная мозаика (виньетирования не было: до и после — одно и то же).
fn as_normalized(mosaic: &ShadingCorrectedMosaic) -> CameraMosaic {
    Mosaic::from_samples(
        mosaic.size(),
        mosaic.pattern(),
        mosaic.samples().to_vec(),
        mosaic_stage::Normalized,
    )
    .unwrap()
}

/// Статистика мозаики, в которой виньетирование ничего не изменило.
fn statistics_of(
    mosaic: &ShadingCorrectedMosaic,
    settings: &AwbStatisticsSettings,
) -> Result<AwbStatistics, InvalidAwbStatistics> {
    let normalized = as_normalized(mosaic);
    compute_awb_statistics(AwbStatisticsInput::new(&normalized, mosaic)?, settings)
}

/// Пара мозаик одного кадра: нормированная (`raw_level`) и скорректированная (`raw_level · gain`) — один цвет на всю
/// мозаику `cells`, каждый канал со своим уровнем.
fn gained_pair(cells: PixelSize, raw: CellColor, gain: f32) -> (CameraMosaic, ShadingCorrectedMosaic) {
    let scale = |color: CellColor| CellColor {
        red: color.red * gain,
        greens: color.greens.map(|green| green * gain),
        blue: color.blue * gain,
    };
    let normalized = as_normalized(&mosaic_from_cells(BayerPattern::RGGB, cells, |_, _| raw));
    let corrected = mosaic_from_cells(BayerPattern::RGGB, cells, |_, _| scale(raw));
    (normalized, corrected)
}

#[test]
fn quantization_rounds_half_up_and_clamps() {
    assert_eq!(quantize_sample(0.0), 0);
    assert_eq!(quantize_sample(1.0), QUANT_MAX);
    assert_eq!(quantize_sample(2.5), QUANT_MAX, "ярче белого — потолок");
    assert_eq!(quantize_sample(-0.3), 0, "отрицательное — ноль");
    // 0,5 · 4095 = 2047,5 ровно: половина округляется вверх.
    assert_eq!(quantize_sample(0.5), 2048);
    // 0,25 · 4095 = 1023,75 → 1024; 0,125 · 4095 = 511,875 → 512.
    assert_eq!(quantize_sample(0.25), 1024);
    assert_eq!(quantize_sample(0.125), 512);
    // Наибольшее f32 ниже 1 тоже даёт потолок: 4094,99975… + 0,5 → 4095.
    assert_eq!(quantize_sample(1.0_f32.next_down()), QUANT_MAX);
}

#[test]
fn known_mosaic_gives_hand_computed_sums() {
    // RGGB 4×4 = ячейки 2×2, сетка 2×1: левый столбец ячеек — плитка 0, правый — плитка 1.
    // Строки пикселей (R G / G B):
    let samples = vec![
        0.5, 0.25, 0.125, 0.0, //
        0.25, 0.125, 0.0, 0.5, //
        0.25, 0.5, 0.0625, 0.25, //
        0.125, 0.0, 0.5, 0.125,
    ];
    let mosaic = Mosaic::from_samples(
        PixelSize::new(4, 4),
        BayerPattern::RGGB,
        samples,
        mosaic_stage::ShadingCorrected,
    )
    .unwrap();
    let statistics = statistics_of(&mosaic, &settings(2, 1, 0.95)).unwrap();
    // Плитка 0: ячейки (0,0) R=0,5 G=0,25+0,25 B=0,125 и (0,1) R=0,25 G=0,5+0,125 B=0,0.
    // q: 0,5→2048, 0,25→1024, 0,125→512, 0→0.
    assert_eq!(
        *statistics.tile(0, 0).unwrap(),
        AwbTile {
            sum_r: 2048 + 1024,
            sum_g: 1024 + 1024 + 2048 + 512,
            sum_b: 512,
            used_cells: 2,
            saturated_cells: 0,
        }
    );
    // Плитка 1: ячейки (1,0) R=0,125 G=0+0 B=0,5 и (1,1) R=0,0625 G=0,25+0,5 B=0,125.
    // 0,0625 · 4095 = 255,9375 → 256.
    assert_eq!(
        *statistics.tile(1, 0).unwrap(),
        AwbTile {
            sum_r: 512 + 256,
            sum_g: 1024 + 2048,
            sum_b: 2048 + 512,
            used_cells: 2,
            saturated_cells: 0,
        }
    );
    assert_eq!(statistics.tile(2, 0), None, "за сеткой плиток нет");
    assert_eq!(statistics.tile(0, 1), None);
}

#[test]
fn red_and_blue_follow_the_cfa_layout() {
    // Одни и те же четыре отсчёта; меняется только раскладка. В RGGB красный — позиция 0,
    // в BGGR — позиция 3: суммы R и B обязаны поменяться местами.
    let samples = vec![0.5, 0.25, 0.25, 0.125];
    let size = PixelSize::new(2, 2);
    let rggb = Mosaic::from_samples(
        size,
        BayerPattern::RGGB,
        samples.clone(),
        mosaic_stage::ShadingCorrected,
    )
    .unwrap();
    let bggr = Mosaic::from_samples(size, BayerPattern::BGGR, samples, mosaic_stage::ShadingCorrected).unwrap();
    let grid = settings(1, 1, 0.95);
    let rggb_tile = statistics_of(&rggb, &grid).unwrap().tiles()[0];
    let bggr_tile = statistics_of(&bggr, &grid).unwrap().tiles()[0];
    assert_eq!((rggb_tile.sum_r, rggb_tile.sum_g, rggb_tile.sum_b), (2048, 2048, 512));
    assert_eq!((bggr_tile.sum_r, bggr_tile.sum_g, bggr_tile.sum_b), (512, 2048, 2048));
}

#[test]
fn every_bayer_layout_gives_the_same_sums_for_the_same_scene() {
    let cells = PixelSize::new(6, 4);
    // Каждая ячейка своего цвета, зелёные в ячейке разные — проверяется и сумма ОБОИХ зелёных.
    let color_of_cell = |cell_x: u32, cell_y: u32| CellColor {
        red: 0.05 * (cell_x + 1) as f32,
        greens: [0.1 + 0.03 * cell_y as f32, 0.2],
        blue: 0.6 - 0.07 * cell_x as f32,
    };
    let grid = settings(3, 2, 0.95);
    let expected = statistics_of(&mosaic_from_cells(BayerPattern::RGGB, cells, color_of_cell), &grid).unwrap();
    // Плитка (0,0): tile_x = sx·3/6 и tile_y = sy·2/4 дают ячейки sx 0…1, sy 0…1.
    // Те же выражения f32, что в `color_of_cell`, — чтобы входы совпали бит в бит.
    let (red_column_0, red_column_1) = (0.05_f32 * 1.0, 0.05_f32 * 2.0);
    let (green_row_0, green_row_1) = (0.1_f32 + 0.03 * 0.0, 0.1_f32 + 0.03 * 1.0);
    let tile_red: u32 = [red_column_0, red_column_1, red_column_0, red_column_1]
        .iter()
        .map(|&value| quantize_sample(value))
        .sum();
    assert_eq!(expected.tile(0, 0).unwrap().sum_r, tile_red);
    let tile_green: u32 = [green_row_0, 0.2, green_row_0, 0.2, green_row_1, 0.2, green_row_1, 0.2]
        .iter()
        .map(|&value| quantize_sample(value))
        .sum();
    assert_eq!(expected.tile(0, 0).unwrap().sum_g, tile_green);
    for pattern in [BayerPattern::GRBG, BayerPattern::GBRG, BayerPattern::BGGR] {
        let statistics = statistics_of(&mosaic_from_cells(pattern, cells, color_of_cell), &grid).unwrap();
        assert_eq!(statistics, expected, "{pattern:?}");
    }
}

#[test]
fn saturated_and_nan_cells_are_excluded_but_counted() {
    // 4 ячейки в ряд, сетка 1×1, порог 0,95.
    let colors = [
        CellColor::uniform(0.5, 0.25, 0.125),
        // Ровно на пороге: `!(0,95 < 0,95)` — исключается.
        CellColor {
            red: 0.1,
            greens: [0.1, 0.95],
            blue: 0.1,
        },
        CellColor::uniform(f32::NAN, 0.1, 0.1),
        // Ярче белого сенсора (уровни сверху не обрезают).
        CellColor::uniform(0.1, 0.1, 1.7),
    ];
    let mosaic = mosaic_from_cells(BayerPattern::GRBG, PixelSize::new(4, 1), |cell_x, _| {
        colors[cell_x as usize]
    });
    let statistics = statistics_of(&mosaic, &settings(1, 1, 0.95)).unwrap();
    assert_eq!(
        statistics.tiles(),
        [AwbTile {
            sum_r: 2048,
            sum_g: 2048,
            sum_b: 512,
            used_cells: 1,
            saturated_cells: 3,
        }]
    );
    // Чуть ниже порога ячейка уже принимается.
    let below = mosaic_from_cells(BayerPattern::GRBG, PixelSize::new(1, 1), |_, _| {
        CellColor::uniform(0.1, 0.95_f32.next_down(), 0.1)
    });
    let accepted = statistics_of(&below, &settings(1, 1, 0.95)).unwrap();
    assert_eq!(
        (accepted.tiles()[0].used_cells, accepted.tiles()[0].saturated_cells),
        (1, 0)
    );
}

#[test]
fn tiles_on_non_divisible_sizes_follow_integer_formula() {
    // Ячейки 5×3, сетка 2×2: tile_x = sx·2/5 → 0,0,0,1,1; tile_y = sy·2/3 → 0,0,1.
    let cells = PixelSize::new(5, 3);
    let grid = TileGrid { columns: 2, rows: 2 };
    let columns: Vec<u32> = (0..5).map(|cell_x| tile_of_cell(cell_x, 0, cells, grid).0).collect();
    let rows: Vec<u32> = (0..3).map(|cell_y| tile_of_cell(0, cell_y, cells, grid).1).collect();
    assert_eq!(columns, [0, 0, 0, 1, 1]);
    assert_eq!(rows, [0, 0, 1]);
    // Красный каждой ячейки кодирует её столбец: так видно, КАКИЕ ячейки попали в плитку.
    let mosaic = mosaic_from_cells(BayerPattern::BGGR, cells, |cell_x, _| {
        CellColor::uniform(0.125 * (cell_x + 1) as f32, 0.25, 0.5)
    });
    let statistics = statistics_of(&mosaic, &settings(2, 2, 0.95)).unwrap();
    // Квантованный красный столбцов 0…4: 0,125→512, 0,25→1024, 0,375→1536 (1535,625+0,5), 0,5→2048, 0,625→2559,375+0,5→2559.
    let red_of_columns = [512, 1024, 1536, 2048, 2559];
    let left_red: u32 = red_of_columns[..3].iter().sum();
    let right_red: u32 = red_of_columns[3..].iter().sum();
    let expected_tile = |sum_r: u32, used_cells: u32| AwbTile {
        sum_r,
        sum_g: used_cells * 2 * 1024,
        sum_b: used_cells * 2048,
        used_cells,
        saturated_cells: 0,
    };
    assert_eq!(*statistics.tile(0, 0).unwrap(), expected_tile(left_red * 2, 6));
    assert_eq!(*statistics.tile(1, 0).unwrap(), expected_tile(right_red * 2, 4));
    assert_eq!(*statistics.tile(0, 1).unwrap(), expected_tile(left_red, 3));
    assert_eq!(*statistics.tile(1, 1).unwrap(), expected_tile(right_red, 2));
    // Итог кадра в u64 сходится с суммой плиток: каждая ячейка учтена ровно один раз.
    assert_eq!(
        statistics.totals(),
        AwbTotals {
            sum_r: u64::from((left_red + right_red) * 3),
            sum_g: 15 * 2 * 1024,
            sum_b: 15 * 2048,
            used_cells: 15,
            saturated_cells: 0,
        }
    );
}

#[test]
fn statistics_of_levels_output_match_raw_counts() {
    // Сквозной путь, как в шейдере: RAW → уровни → статистика. При чёрном 0 и белом 4095
    // нормированный отсчёт k/4095 квантуется обратно ровно в k.
    let raw_samples: Vec<u16> = (0..16).map(|index| 100 + index * 37).collect();
    let raw = RawMosaic::new(PixelSize::new(4, 4), BayerPattern::GBRG, raw_samples.clone()).unwrap();
    let levels = NormalizationLevels::new([0.0; 4], 4095.0).unwrap();
    // Стадия виньетирования пропущена (нет калибровки): мозаика не меняется.
    let skipped = LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated);
    let (shading_corrected, _) = apply_lens_shading(&normalize_levels(&raw, &levels), &skipped);
    let statistics = statistics_of(&shading_corrected, &settings(1, 1, 0.95)).unwrap();
    let raw_sum_of = |channel: RgbChannel| -> u32 {
        (0..16u32)
            .filter(|&index| BayerPattern::GBRG.channel_at(index % 4, index / 4) == channel)
            .map(|index| u32::from(raw_samples[index as usize]))
            .sum()
    };
    let tile = statistics.tiles()[0];
    assert_eq!(tile.sum_r, raw_sum_of(RgbChannel::Red));
    assert_eq!(tile.sum_g, raw_sum_of(RgbChannel::Green));
    assert_eq!(tile.sum_b, raw_sum_of(RgbChannel::Blue));
    assert_eq!(tile.used_cells, 4);
}

#[test]
fn worst_case_tile_at_the_limit_does_not_overflow() {
    // Плитка ровно из MAX_CELLS_PER_TILE ячеек, все отсчёты квантуются в 4095 и не пересвечены
    // (порог 1,0, отсчёт — наибольшее f32 ниже 1). В отладке переполнение u32 упало бы паникой.
    let cells = PixelSize::new(MAX_CELLS_PER_TILE, 1);
    let almost_white = 1.0_f32.next_down();
    let mosaic = mosaic_from_cells(BayerPattern::RGGB, cells, |_, _| {
        CellColor::uniform(almost_white, almost_white, almost_white)
    });
    let statistics = statistics_of(&mosaic, &settings(1, 1, 1.0)).unwrap();
    let tile = statistics.tiles()[0];
    assert_eq!(tile.used_cells, MAX_CELLS_PER_TILE);
    assert_eq!(
        u64::from(tile.sum_g),
        u64::from(MAX_CELLS_PER_TILE) * 2 * u64::from(QUANT_MAX)
    );
    assert_eq!(tile.sum_r, MAX_CELLS_PER_TILE * QUANT_MAX);
}

#[test]
fn grids_that_could_overflow_are_rejected() {
    // Одна ячейка сверх предела на плитку — отказ.
    let over_limit = PixelSize::new((MAX_CELLS_PER_TILE + 1) * 2, 2);
    assert_eq!(
        settings(1, 1, 0.95).check_for_mosaic(over_limit),
        Err(InvalidAwbStatistics::TileTooLarge {
            columns: 1,
            rows: 1,
            cells_per_tile: u64::from(MAX_CELLS_PER_TILE) + 1,
            max_cells: MAX_CELLS_PER_TILE,
        })
    );
    // Основная камера OnePlus 11 (4080×3060): сетка 2×2 — плитки 1020×765 ячеек, слишком крупно…
    let main_camera = PixelSize::new(4080, 3060);
    assert!(matches!(
        settings(2, 2, 0.95).check_for_mosaic(main_camera),
        Err(InvalidAwbStatistics::TileTooLarge {
            cells_per_tile: 780_300,
            ..
        })
    ));
    // …а сетка по умолчанию проходит с большим запасом.
    assert_eq!(AwbStatisticsSettings::default().check_for_mosaic(main_camera), Ok(()));
    // Произведение `sx · columns` в u32 у шейдера переполнилось бы.
    assert!(matches!(
        settings(100_000, 1, 0.95).check_for_mosaic(PixelSize::new(200_000, 2)),
        Err(InvalidAwbStatistics::TileIndexOverflow { .. })
    ));
}

#[test]
fn invalid_settings_are_rejected_with_reason() {
    let size = PixelSize::new(8, 8);
    assert_eq!(
        settings(0, 2, 0.95).check_for_mosaic(size),
        Err(InvalidAwbStatistics::EmptyGrid { columns: 0, rows: 2 })
    );
    for threshold in [0.0, -0.5, 1.01, f32::NAN, f32::INFINITY] {
        assert!(
            matches!(
                settings(2, 2, threshold).check_for_mosaic(size),
                Err(InvalidAwbStatistics::SaturationThresholdOutOfRange { .. })
            ),
            "{threshold}"
        );
    }
    // Ячеек 4×4, а столбцов 5 — была бы пустая плитка.
    assert_eq!(
        settings(5, 2, 0.95).check_for_mosaic(size),
        Err(InvalidAwbStatistics::GridFinerThanCells {
            columns: 5,
            rows: 2,
            cells_width: 4,
            cells_height: 4,
        })
    );
    // Эталон проверяет то же и не считает ничего на негодных настройках.
    let mosaic = mosaic_from_cells(BayerPattern::RGGB, PixelSize::new(4, 4), |_, _| {
        CellColor::uniform(0.1, 0.1, 0.1)
    });
    assert!(statistics_of(&mosaic, &settings(5, 2, 0.95)).is_err());
}

#[test]
fn readback_tiles_must_match_the_grid() {
    let grid = TileGrid { columns: 2, rows: 3 };
    assert_eq!(
        AwbStatistics::from_tiles(grid, vec![AwbTile::default(); 5]),
        Err(TileCountMismatch {
            columns: 2,
            rows: 3,
            expected: 6,
            actual: 5,
        })
    );
    let tiles: Vec<AwbTile> = (0..6)
        .map(|index| AwbTile {
            sum_r: index,
            ..AwbTile::default()
        })
        .collect();
    let statistics = AwbStatistics::from_tiles(grid, tiles).unwrap();
    // Порядок по строкам: (1, 2) → индекс 2·2 + 1 = 5.
    assert_eq!(statistics.tile(1, 2).unwrap().sum_r, 5);
    assert_eq!(statistics.tile(0, 1).unwrap().sum_r, 2);
}

#[test]
fn settings_round_trip_through_config_and_reject_unknown_fields() {
    let defaults = AwbStatisticsSettings::default();
    let text = serde_json::to_string(&defaults).unwrap();
    assert_eq!(serde_json::from_str::<AwbStatisticsSettings>(&text).unwrap(), defaults);
    let typo = r#"{"grid":{"columns":4,"rows":3},"saturation_threshhold":0.9}"#;
    assert!(serde_json::from_str::<AwbStatisticsSettings>(typo).is_err());
}

#[test]
fn clipping_is_judged_on_the_raw_sample_and_sums_on_the_corrected_one() {
    // Сырой отсчёт 0,3 (не пересвечен) под усилением ×5: после виньетирования 1,5 > порога 0,95 и > 1.
    let cells = PixelSize::new(2, 2);
    let (normalized, corrected) = gained_pair(cells, CellColor::uniform(0.3, 0.3, 0.3), 5.0);
    let settings = settings_with_headroom(1, 1, 0.95, DEFAULT_SUM_HEADROOM);
    let statistics =
        compute_awb_statistics(AwbStatisticsInput::new(&normalized, &corrected).unwrap(), &settings).unwrap();
    let tile = statistics.tiles()[0];
    assert_eq!(
        (tile.used_cells, tile.saturated_cells),
        (4, 0),
        "пересвета в RAW нет: {tile:?}"
    );
    // Сумма — по скорректированному значению 1,5 (а не обрезана до 1): 1,5 · 4095 + 0,5 → 6143.
    let one = quantize_sample_with_headroom(1.5, DEFAULT_SUM_HEADROOM);
    assert_eq!(one, 6143);
    assert_eq!((tile.sum_r, tile.sum_g, tile.sum_b), (4 * one, 8 * one, 4 * one));
}

#[test]
fn a_truly_clipped_raw_sample_is_still_saturated_even_when_the_gain_is_one() {
    // Один отсчёт ячейки в RAW ≥ порога: ячейка исключена, а остальные ячейки (без пересвета) принимаются.
    let cells = PixelSize::new(2, 1);
    let clipped_cell = |cell_x: u32| {
        if cell_x == 0 {
            CellColor {
                red: 0.2,
                greens: [0.2, 0.97],
                blue: 0.2,
            }
        } else {
            CellColor::uniform(0.2, 0.2, 0.2)
        }
    };
    let normalized = as_normalized(&mosaic_from_cells(BayerPattern::RGGB, cells, |cell_x, _| {
        clipped_cell(cell_x)
    }));
    // Усиление ×1 (ничего не меняет) и ×3 у пересвеченной ячейки не должно ничего «лечить».
    for gain in [1.0_f32, 3.0] {
        let corrected = mosaic_from_cells(BayerPattern::RGGB, cells, |cell_x, _| {
            let color = clipped_cell(cell_x);
            CellColor {
                red: color.red * gain,
                greens: color.greens.map(|g| g * gain),
                blue: color.blue * gain,
            }
        });
        let settings = settings_with_headroom(1, 1, 0.95, DEFAULT_SUM_HEADROOM);
        let statistics =
            compute_awb_statistics(AwbStatisticsInput::new(&normalized, &corrected).unwrap(), &settings).unwrap();
        let tile = statistics.tiles()[0];
        assert_eq!((tile.used_cells, tile.saturated_cells), (1, 1), "усиление {gain}");
    }
}

#[test]
fn sums_clamp_at_the_headroom_not_at_white() {
    let cells = PixelSize::new(1, 1);
    let (normalized, corrected) = gained_pair(cells, CellColor::uniform(0.5, 0.5, 0.5), 100.0);
    let statistics = compute_awb_statistics(
        AwbStatisticsInput::new(&normalized, &corrected).unwrap(),
        &settings_with_headroom(1, 1, 0.95, 16.0),
    )
    .unwrap();
    let ceiling = 16 * QUANT_MAX;
    let tile = statistics.tiles()[0];
    assert_eq!((tile.sum_r, tile.sum_g, tile.sum_b), (ceiling, 2 * ceiling, ceiling));
}

#[test]
fn inputs_of_different_size_are_rejected() {
    let small = as_normalized(&mosaic_from_cells(BayerPattern::RGGB, PixelSize::new(2, 2), |_, _| {
        CellColor::uniform(0.1, 0.1, 0.1)
    }));
    let large = mosaic_from_cells(BayerPattern::RGGB, PixelSize::new(4, 2), |_, _| {
        CellColor::uniform(0.1, 0.1, 0.1)
    });
    assert!(matches!(
        AwbStatisticsInput::new(&small, &large),
        Err(InvalidAwbStatistics::InputSizeMismatch { .. })
    ));
}

#[test]
fn worst_case_tile_at_the_headroom_limit_does_not_overflow_and_one_cell_more_is_rejected() {
    let headroom = DEFAULT_SUM_HEADROOM;
    let limit = max_cells_per_tile(headroom);
    let ceiling = 16 * QUANT_MAX;
    assert_eq!(limit, u32::MAX / (2 * ceiling));
    // Плитка ровно из `limit` ячеек, все отсчёты на потолке: суммы не переполняются (в отладке упало бы паникой).
    let cells = PixelSize::new(limit, 1);
    let normalized = as_normalized(&mosaic_from_cells(BayerPattern::RGGB, cells, |_, _| {
        CellColor::uniform(0.1, 0.1, 0.1)
    }));
    let corrected = mosaic_from_cells(BayerPattern::RGGB, cells, |_, _| {
        CellColor::uniform(100.0, 100.0, 100.0)
    });
    let settings = settings_with_headroom(1, 1, 0.95, headroom);
    let statistics =
        compute_awb_statistics(AwbStatisticsInput::new(&normalized, &corrected).unwrap(), &settings).unwrap();
    assert_eq!(
        u64::from(statistics.tiles()[0].sum_g),
        u64::from(limit) * 2 * u64::from(ceiling)
    );
    // Одна ячейка сверх — явный отказ.
    let over = PixelSize::new((limit + 1) * 2, 2);
    assert_eq!(
        settings.check_for_mosaic(over),
        Err(InvalidAwbStatistics::TileTooLarge {
            columns: 1,
            rows: 1,
            cells_per_tile: u64::from(limit) + 1,
            max_cells: limit,
        })
    );
}

#[test]
fn startup_check_covers_the_largest_supported_frame_and_validates_the_headroom() {
    // Умолчания выдерживают наибольший кадр.
    assert_eq!(
        AwbStatisticsSettings::default().check_for_largest_supported_frame(),
        Ok(())
    );
    // Слишком грубая сетка на наибольшем кадре — отказ при запуске, а не переполнение на кадре.
    assert!(matches!(
        settings_with_headroom(8, 6, 0.95, DEFAULT_SUM_HEADROOM).check_for_largest_supported_frame(),
        Err(InvalidAwbStatistics::TileTooLarge { .. })
    ));
    // Сетка 8×6 при запасе 16 не выдерживает (плитка 512×512 ячеек), а при запасе 1 — выдерживает.
    assert!(
        settings_with_headroom(8, 6, 0.95, 1.0)
            .check_for_largest_supported_frame()
            .is_ok()
    );
    for headroom in [0.5, f32::NAN, f32::INFINITY, MAX_SUM_HEADROOM + 1.0] {
        assert!(
            matches!(
                settings_with_headroom(32, 24, 0.95, headroom).check_for_mosaic(PixelSize::new(64, 48)),
                Err(InvalidAwbStatistics::SumHeadroomOutOfRange { .. })
            ),
            "{headroom}"
        );
    }
}

#[test]
fn config_without_headroom_gets_the_default_and_the_headroom_round_trips() {
    let legacy = r#"{"grid":{"columns":4,"rows":3},"saturation_threshold":0.9}"#;
    let parsed: AwbStatisticsSettings = serde_json::from_str(legacy).unwrap();
    assert_eq!(parsed.sum_headroom, DEFAULT_SUM_HEADROOM);
}
