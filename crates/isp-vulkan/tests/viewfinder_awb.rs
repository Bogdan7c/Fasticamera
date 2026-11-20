//! Статистика AWB в слитом проходе против эталона `isp_model::statistics::compute_awb_statistics`.
//!
//! * Известная синтетика → известные суммы (посчитаны здесь вручную, без `isp-model`).
//! * Точное равенство с эталоном, когда деление уровней точно: `white − black` = 1024 (степень двойки),
//!   тогда `x / 1024` точен и при честном делении, и при умножении на обратное (так деление делают
//!   драйверы). Проверено на окне shared-памяти (крупные плитки) и на прямом глобальном пути (плитки
//!   мельче рабочей группы), на нескольких размерах группы, с кадрированием и поворотом.
//!   ВАЖНО: точное равенство — свойство RADV (ПК), а не гарантия Vulkan. Спецификация разрешает OpFDiv
//!   ошибку до 2,5 ULP при ЛЮБОМ делителе, и шейдер намеренно делит обычным `/` (скорость, без
//!   «точного» деления). На устройстве (Adreno) гарантированная граница — та же, что в общем случае
//!   ниже: расходиться (на ±1 шаг квантования или флаг пересвета) могут только ячейки в пределах 3 ULP
//!   от границы квантования/порога; остальные ячейки и `used`/`saturated` вне таких ячеек — равны.
//! * Два пути сведения в шейдере ([`AwbReduction`]): группа целиком в одной плитке — `subgroupAdd`
//!   (быстрый), группа на стыке плиток — shared-окно; плюс запасной вариант «только окно». Целые суммы не
//!   зависят от порядка сложения, поэтому оба пути и оба варианта обязаны давать ОДИН И ТОТ ЖЕ результат,
//!   равный эталону. Какой путь у какой группы — считается здесь перебором нитей (независимо от шейдера).
//! * Общий случай (`white − black` = 959): OpFDiv в Vulkan — 2,5 ULP, поэтому нормированный отсчёт GPU
//!   может отличаться от CPU на пару ULP. Квантование `floor(v·4095 + 0,5)` меняется, только если
//!   `v·4095 + 0,5` лежит в паре ULP от целого, а пересвет — только если `v` в паре ULP от порога.
//!   Граница: в плитке без «пограничных по пересвету» ячеек |Δ sum_r|, |Δ sum_b| ≤ число пограничных
//!   по квантованию ячеек, |Δ sum_g| ≤ вдвое больше, `used`/`saturated` равны.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use isp_model::cfa::BayerPattern;
use isp_model::image::CameraMosaic;
use isp_model::statistics::{
    AwbStatistics, AwbStatisticsSettings, TileGrid, cells_size, compute_awb_statistics, quantize_sample, tile_of_cell,
};
use isp_vulkan::viewfinder::{
    AwbReduction, AwbReductionPolicy, OutputAllocation, ViewfinderGeometry, ViewfinderIsp, ViewfinderIspConfig,
    ViewfinderStorageFormat, WorkgroupSizeConfig,
};
use viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, assert_no_validation_errors, gpu_context_or_skip, reference_upright,
    run_viewfinder, test_config,
};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

fn config(grid: TileGrid, workgroup: WorkgroupSizeConfig) -> ViewfinderIspConfig {
    let mut config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    config.awb_statistics.grid = grid;
    config.workgroup_size = workgroup;
    config
}

/// Оба варианта шейдера: с быстрым путём subgroup (на RADV он доступен — проверяется) и запасной.
const BOTH_REDUCTIONS: [(AwbReductionPolicy, AwbReduction); 2] = [
    (
        AwbReductionPolicy::SubgroupWhenSupported,
        AwbReduction::SubgroupSingleTile,
    ),
    (AwbReductionPolicy::SharedMemoryOnly, AwbReduction::SharedMemoryWindow),
];

/// Конфиг с политикой сведения + проверка, что ISP на этом устройстве выбрал ожидаемый вариант.
fn config_with_reduction(
    context: &gpu_vulkan::context::GpuContext,
    base: ViewfinderIspConfig,
    (policy, expected): (AwbReductionPolicy, AwbReduction),
) -> ViewfinderIspConfig {
    let config = ViewfinderIspConfig {
        awb_reduction: policy,
        ..base
    };
    let isp = ViewfinderIsp::new(context, &config).unwrap();
    assert_eq!(
        isp.awb_reduction(),
        expected,
        "{policy:?} на {} (ПК-тесты ждут RADV с арифметикой subgroup)",
        context.capabilities().driver_name
    );
    config
}

/// Пёстрая сцена со встроенным пересветом: значения RAW «псевдослучайны» по пикселю, правая нижняя
/// четверть — частично выше порога 0,95.
fn varied_scene(spec: &ParamsSpec) -> RawFrame {
    let (black, white) = spec.levels();
    let range = f64::from(white) - f64::from(black[0]);
    RawFrame::from_fn(WIDTH, HEIGHT, spec.pattern, |x, y, channel| {
        let hash = (x * 7919 + y * 104_729 + channel as u32 * 31) % 1000;
        let level = if x >= WIDTH / 2 && y >= HEIGHT / 2 && hash.is_multiple_of(5) {
            0.97 + f64::from(hash % 3) * 0.01
        } else {
            f64::from(hash) / 1000.0 * 0.9
        };
        (f64::from(black[0]) + level * range).round() as u16
    })
}

fn gpu_statistics(
    context: &gpu_vulkan::context::GpuContext,
    config: &ViewfinderIspConfig,
    raw: &RawFrame,
    spec: &ParamsSpec,
    geometry: ViewfinderGeometry,
) -> (AwbStatistics, CameraMosaic) {
    let params = spec.build();
    let request = RunRequest {
        geometry,
        ..RunRequest::whole_frame(raw, &params, spec.display())
    };
    let run = run_viewfinder(context, config, &request);
    let reference = reference_upright(raw, geometry.crop, geometry.sensor_orientation, &params);
    (run.report.awb_statistics.unwrap(), reference.run.normalized)
}

fn whole(raw: &RawFrame, orientation: SensorOrientation) -> ViewfinderGeometry {
    ViewfinderGeometry {
        valid_area: raw.full_rect(),
        crop: raw.full_rect(),
        sensor_orientation: orientation,
        allocation: OutputAllocation::Exact,
    }
}

#[test]
fn uniform_mid_gray_gives_hand_computed_sums() {
    let Some(context) = gpu_context_or_skip() else { return };
    // black 64, white 1088: отсчёт 576 → (576 − 64)/1024 = 0,5 → q = floor(0,5·4095 + 0,5) = 2048.
    let spec = ParamsSpec {
        black: [64.0; 4],
        white: 1088.0,
        ..ParamsSpec::default()
    };
    let raw = RawFrame::from_fn(WIDTH, HEIGHT, spec.pattern, |_, _, _| 576);
    let grid = TileGrid { columns: 4, rows: 3 };
    let (statistics, _) = gpu_statistics(
        &context,
        &config(grid, WorkgroupSizeConfig { x: 16, y: 8 }),
        &raw,
        &spec,
        whole(&raw, SensorOrientation::Degrees0),
    );
    // Ячеек 32×24, плитки 8×8 = 64 ячейки.
    for tile in statistics.tiles() {
        assert_eq!(tile.sum_r, 2048 * 64);
        assert_eq!(tile.sum_g, 2 * 2048 * 64, "оба зелёных");
        assert_eq!(tile.sum_b, 2048 * 64);
        assert_eq!(tile.used_cells, 64);
        assert_eq!(tile.saturated_cells, 0);
    }
    assert_no_validation_errors(&context);
}

/// Точно на RADV (см. модуль): на другом драйвере OpFDiv вправе ошибиться на 2,5 ULP и при делителе 2^k,
/// и тогда действует граница теста `general_levels_stay_within_the_documented_rounding_bound` — ячейки в
/// 3 ULP от границы квантования. Тест работает на ПК (RADV), поэтому равенство здесь — проверка RADV.
#[test]
fn statistics_equal_reference_exactly_when_level_division_is_exact() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec {
        black: [64.0; 4],
        white: 1088.0,
        pattern: BayerPattern::GRBG,
        ..ParamsSpec::default()
    };
    let raw = varied_scene(&spec);
    let crop = PixelRect {
        left: 8,
        top: 4,
        size: PixelSize::new(48, 40),
    };
    let cases = [
        // Плитки 6×5 ячеек: окно shared-памяти.
        (
            TileGrid { columns: 4, rows: 4 },
            WorkgroupSizeConfig { x: 16, y: 8 },
            SensorOrientation::Degrees0,
        ),
        // Плитки 1–2 ячейки: группа задевает больше 4×4 плиток — прямой глобальный путь.
        (
            TileGrid { columns: 20, rows: 16 },
            WorkgroupSizeConfig { x: 16, y: 8 },
            SensorOrientation::Degrees90,
        ),
        (
            TileGrid { columns: 5, rows: 3 },
            WorkgroupSizeConfig { x: 8, y: 8 },
            SensorOrientation::Degrees180,
        ),
        (
            TileGrid { columns: 3, rows: 7 },
            WorkgroupSizeConfig { x: 32, y: 4 },
            SensorOrientation::Degrees270,
        ),
    ];
    for (grid, workgroup, orientation) in cases {
        let geometry = ViewfinderGeometry {
            valid_area: raw.full_rect(),
            crop,
            sensor_orientation: orientation,
            allocation: OutputAllocation::Exact,
        };
        for reduction in BOTH_REDUCTIONS {
            let config = config_with_reduction(&context, config(grid, workgroup), reduction);
            let (gpu, normalized) = gpu_statistics(&context, &config, &raw, &spec, geometry);
            let cpu = compute_awb_statistics(&normalized, &config.awb_statistics).unwrap();
            assert_eq!(gpu, cpu, "{grid:?} {workgroup:?} {orientation:?} {reduction:?}");
            let totals = gpu.totals();
            assert!(totals.saturated_cells > 0, "сцена содержит пересвет");
            assert_eq!(
                totals.used_cells + totals.saturated_cells,
                24 * 20,
                "каждая ячейка кадрирования ровно раз"
            );
        }
    }
    assert_no_validation_errors(&context);
}

/// Ячейки, где ±`ulps` ULP нормированного отсчёта меняют квантование (`quantization`) или пересвет.
#[derive(Default, Clone, Copy)]
struct BoundaryCells {
    quantization: u32,
    saturation: u32,
}

fn boundary_cells(normalized: &CameraMosaic, settings: &AwbStatisticsSettings, ulps: u32) -> Vec<BoundaryCells> {
    let grid = settings.grid;
    let cells = cells_size(normalized.size());
    let mut per_tile = vec![BoundaryCells::default(); (grid.columns * grid.rows) as usize];
    for cell_y in 0..cells.height {
        for cell_x in 0..cells.width {
            let (column, row) = tile_of_cell(cell_x, cell_y, cells, grid);
            let tile = &mut per_tile[(row * grid.columns + column) as usize];
            let mut quantization = false;
            let mut saturation = false;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let value = normalized.sample(cell_x * 2 + dx, cell_y * 2 + dy);
                for neighbour in [
                    f32::from_bits(value.to_bits().saturating_sub(ulps)),
                    f32::from_bits(value.to_bits() + ulps),
                ] {
                    quantization |= quantize_sample(neighbour) != quantize_sample(value);
                    let threshold = settings.saturation_threshold;
                    saturation |= (neighbour < threshold) != (value < threshold);
                }
            }
            tile.quantization += u32::from(quantization);
            tile.saturation += u32::from(saturation);
        }
    }
    per_tile
}

/// Все значения RAW от чёрного до белого без самого белого (64…1022) по кругу, в разных позициях CFA.
/// Белый (v = 1,0) не берём: с порогом 1,0 он стоял бы ровно на границе пересвета.
fn level_sweep(spec: &ParamsSpec) -> RawFrame {
    RawFrame::from_fn(WIDTH, HEIGHT, spec.pattern, |x, y, _| {
        (64 + (y * WIDTH + x) * 7 % 959) as u16
    })
}

#[test]
fn general_levels_stay_within_the_documented_rounding_bound() {
    let Some(context) = gpu_context_or_skip() else { return };
    // black 64, white 1023: деление на 959 неточно. У пёстрой сцены пересвет 0,97…0,99 далеко от
    // порога 0,95; у развёртки порог 1,0 — выше любого её отсчёта, все ячейки участвуют в суммах.
    let spec = ParamsSpec::default();
    for (name, raw) in [
        ("пёстрая сцена", varied_scene(&spec)),
        ("развёртка уровней", level_sweep(&spec)),
    ] {
        let mut config = config(TileGrid { columns: 4, rows: 3 }, WorkgroupSizeConfig { x: 16, y: 8 });
        if name == "развёртка уровней" {
            config.awb_statistics.saturation_threshold = 1.0;
        }
        let (gpu, normalized) =
            gpu_statistics(&context, &config, &raw, &spec, whole(&raw, SensorOrientation::Degrees0));
        let cpu = compute_awb_statistics(&normalized, &config.awb_statistics).unwrap();
        // 3 ULP — с запасом над 2,5 ULP OpFDiv.
        let boundaries = boundary_cells(&normalized, &config.awb_statistics, 3);
        let mut differing_tiles = 0;
        for ((gpu_tile, cpu_tile), boundary) in gpu.tiles().iter().zip(cpu.tiles()).zip(&boundaries) {
            assert_eq!(gpu_tile.used_cells, cpu_tile.used_cells, "{name}");
            assert_eq!(gpu_tile.saturated_cells, cpu_tile.saturated_cells, "{name}");
            if boundary.saturation > 0 {
                continue;
            }
            let bound = boundary.quantization;
            assert!(
                gpu_tile.sum_r.abs_diff(cpu_tile.sum_r) <= bound,
                "{name}: {gpu_tile:?} {cpu_tile:?} граница {bound}"
            );
            assert!(
                gpu_tile.sum_g.abs_diff(cpu_tile.sum_g) <= 2 * bound,
                "{name}: {gpu_tile:?} {cpu_tile:?}"
            );
            assert!(
                gpu_tile.sum_b.abs_diff(cpu_tile.sum_b) <= bound,
                "{name}: {gpu_tile:?} {cpu_tile:?}"
            );
            differing_tiles += usize::from(gpu_tile != cpu_tile);
        }
        let boundary_total: u32 = boundaries.iter().map(|tile| tile.quantization).sum();
        let saturation_boundary: u32 = boundaries.iter().map(|tile| tile.saturation).sum();
        assert_eq!(
            saturation_boundary, 0,
            "{name}: сцены выбраны без отсчётов у порога пересвета"
        );
        eprintln!(
            "AWB {name}: пограничных ячеек {boundary_total} из {}, плиток с расхождением {differing_tiles} из {}",
            32 * 24,
            boundaries.len()
        );
    }
    assert_no_validation_errors(&context);
}

/// Сколько рабочих групп целиком в одной плитке (быстрый путь subgroup) и сколько на стыке плиток.
/// Считается ПЕРЕБОРОМ всех нитей каждой группы (без «углового» вывода шейдера) — независимый оракул.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct WorkgroupPaths {
    single_tile: u32,
    straddling: u32,
    /// Групп, где часть нитей вне выхода (край картинки).
    with_threads_outside: u32,
}

fn workgroup_paths(
    crop: PixelSize,
    orientation: SensorOrientation,
    workgroup: WorkgroupSizeConfig,
    grid: TileGrid,
) -> WorkgroupPaths {
    let cells = cells_size(crop);
    let turns = orientation.quarter_turns_clockwise();
    let (output_width, output_height) = if turns % 2 == 1 {
        (cells.height, cells.width)
    } else {
        (cells.width, cells.height)
    };
    // Та же обратная развёртка, что `cell_of_output` в шейдере.
    let cell_of_output = |x: u32, y: u32| match turns {
        1 => (y, cells.height - 1 - x),
        2 => (cells.width - 1 - x, cells.height - 1 - y),
        3 => (cells.width - 1 - y, x),
        _ => (x, y),
    };
    let mut paths = WorkgroupPaths::default();
    for group_y in 0..output_height.div_ceil(workgroup.y) {
        for group_x in 0..output_width.div_ceil(workgroup.x) {
            let mut tiles = std::collections::BTreeSet::new();
            let mut outside = false;
            for y in group_y * workgroup.y..(group_y + 1) * workgroup.y {
                for x in group_x * workgroup.x..(group_x + 1) * workgroup.x {
                    if x >= output_width || y >= output_height {
                        outside = true;
                        continue;
                    }
                    let (cell_x, cell_y) = cell_of_output(x, y);
                    tiles.insert(tile_of_cell(cell_x, cell_y, cells, grid));
                }
            }
            if tiles.len() == 1 {
                paths.single_tile += 1;
            } else {
                paths.straddling += 1;
            }
            paths.with_threads_outside += u32::from(outside);
        }
    }
    paths
}

/// Оба пути шейдера в одном кадре (часть групп целиком в плитке, часть на стыке) и края, где нити группы
/// вне картинки: быстрый путь, запасной вариант и эталон совпадают точно. Нечётный размер ячеек
/// кадрирования (23×19) — крайние группы неполные: нити вне выхода обязаны дать нули в subgroupAdd.
#[test]
fn single_tile_and_straddling_workgroups_match_reference_exactly_in_both_variants() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec {
        black: [64.0; 4],
        white: 1088.0,
        pattern: BayerPattern::RGGB,
        ..ParamsSpec::default()
    };
    let raw = varied_scene(&spec);
    let aligned_crop = raw.full_rect();
    // 46×38 пикселей = 23×19 ячеек: не кратно ни одной группе.
    let odd_crop = PixelRect {
        left: 6,
        top: 4,
        size: PixelSize::new(46, 38),
    };
    let cases = [
        // Плитки 16×12 ячеек: группы 16×8 строк 0 и 2 — внутри плитки, строка 1 — на стыке.
        (
            aligned_crop,
            TileGrid { columns: 2, rows: 2 },
            WorkgroupSizeConfig { x: 16, y: 8 },
            SensorOrientation::Degrees0,
        ),
        (
            aligned_crop,
            TileGrid { columns: 2, rows: 2 },
            WorkgroupSizeConfig { x: 8, y: 8 },
            SensorOrientation::Degrees90,
        ),
        (
            aligned_crop,
            TileGrid { columns: 1, rows: 2 },
            WorkgroupSizeConfig { x: 32, y: 4 },
            SensorOrientation::Degrees180,
        ),
        (
            aligned_crop,
            TileGrid { columns: 2, rows: 1 },
            WorkgroupSizeConfig { x: 16, y: 8 },
            SensorOrientation::Degrees270,
        ),
        // Одна плитка на весь кадр: ВСЕ группы — быстрый путь, крайние — с нитями вне выхода.
        (
            odd_crop,
            TileGrid { columns: 1, rows: 1 },
            WorkgroupSizeConfig { x: 16, y: 8 },
            SensorOrientation::Degrees0,
        ),
        (
            odd_crop,
            TileGrid { columns: 1, rows: 1 },
            WorkgroupSizeConfig { x: 8, y: 8 },
            SensorOrientation::Degrees90,
        ),
        // Нечётный кадр и стыки плиток вместе.
        (
            odd_crop,
            TileGrid { columns: 2, rows: 2 },
            WorkgroupSizeConfig { x: 8, y: 8 },
            SensorOrientation::Degrees270,
        ),
        (
            odd_crop,
            TileGrid { columns: 1, rows: 3 },
            WorkgroupSizeConfig { x: 32, y: 4 },
            SensorOrientation::Degrees180,
        ),
    ];
    let mut seen = WorkgroupPaths::default();
    for (crop, grid, workgroup, orientation) in cases {
        let paths = workgroup_paths(crop.size, orientation, workgroup, grid);
        assert!(
            paths.single_tile > 0,
            "кадр обязан задеть быстрый путь: {paths:?} {grid:?} {workgroup:?}"
        );
        seen.single_tile += paths.single_tile;
        seen.straddling += paths.straddling;
        seen.with_threads_outside += paths.with_threads_outside;
        let geometry = ViewfinderGeometry {
            valid_area: raw.full_rect(),
            crop,
            sensor_orientation: orientation,
            allocation: OutputAllocation::Exact,
        };
        let mut results = Vec::new();
        for reduction in BOTH_REDUCTIONS {
            let config = config_with_reduction(&context, config(grid, workgroup), reduction);
            let (gpu, normalized) = gpu_statistics(&context, &config, &raw, &spec, geometry);
            let cpu = compute_awb_statistics(&normalized, &config.awb_statistics).unwrap();
            assert_eq!(
                gpu, cpu,
                "{crop:?} {grid:?} {workgroup:?} {orientation:?} {reduction:?} {paths:?}"
            );
            let cells = cells_size(crop.size);
            let totals = gpu.totals();
            assert_eq!(
                totals.used_cells + totals.saturated_cells,
                u64::from(cells.width * cells.height)
            );
            results.push(gpu);
        }
        assert_eq!(results[0], results[1], "быстрый и запасной варианты");
    }
    assert!(seen.straddling > 0 && seen.with_threads_outside > 0, "{seen:?}");
    assert_no_validation_errors(&context);
}

/// Одна плитка, кадр целиком в пересвете или целиком тёмный: быстрый путь не путает поля
/// (`saturated` против `used`) и не теряет нули.
#[test]
fn single_tile_fast_path_keeps_saturated_and_used_fields_apart() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec {
        black: [64.0; 4],
        white: 1088.0,
        ..ParamsSpec::default()
    };
    let grid = TileGrid { columns: 1, rows: 1 };
    let workgroup = WorkgroupSizeConfig { x: 16, y: 8 };
    let cells = WIDTH / 2 * (HEIGHT / 2);
    for reduction in BOTH_REDUCTIONS {
        let config = config_with_reduction(&context, config(grid, workgroup), reduction);
        // Белый во всех позициях: пересвет каждой ячейки.
        let white = RawFrame::from_fn(WIDTH, HEIGHT, spec.pattern, |_, _, _| 1088);
        let (statistics, _) = gpu_statistics(
            &context,
            &config,
            &white,
            &spec,
            whole(&white, SensorOrientation::Degrees0),
        );
        let tile = statistics.tiles()[0];
        assert_eq!((tile.sum_r, tile.sum_g, tile.sum_b), (0, 0, 0), "{reduction:?}");
        assert_eq!((tile.used_cells, tile.saturated_cells), (0, cells), "{reduction:?}");
        // Красный 0,25, зелёные 0,5, синий 0,75 (RGB различимы): q = 1024, 2048, 3071.
        let colored = RawFrame::from_fn(WIDTH, HEIGHT, spec.pattern, |_, _, channel| match channel {
            0 => 64 + 256,
            1 => 64 + 512,
            _ => 64 + 768,
        });
        let (statistics, _) = gpu_statistics(
            &context,
            &config,
            &colored,
            &spec,
            whole(&colored, SensorOrientation::Degrees90),
        );
        let tile = statistics.tiles()[0];
        assert_eq!(quantize_sample(0.25), 1024);
        assert_eq!(quantize_sample(0.75), 3071);
        assert_eq!(
            (
                tile.sum_r,
                tile.sum_g,
                tile.sum_b,
                tile.used_cells,
                tile.saturated_cells
            ),
            (1024 * cells, 2 * 2048 * cells, 3071 * cells, cells, 0),
            "{reduction:?}"
        );
    }
    assert_no_validation_errors(&context);
}
