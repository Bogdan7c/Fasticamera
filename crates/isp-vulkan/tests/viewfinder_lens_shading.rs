//! Коррекция виньетирования в слитом проходе видоискателя (GPU, RADV, полная validation) против эталона
//! `isp-model` (`apply_lens_shading_at`).
//!
//! # Допуски и почему именно они
//! * **Сигнал экрана** — те же допуски, что у `viewfinder_parity.rs`: ошибка математики сверх шага хранения
//!   картинки ≤ 1e-5 (замер без виньетирования — 1,2e-7), полшага LSB 10 бит, ноль пикселей хуже 1 LSB.
//!   Ячейка с виньетированием отличается от эталона на те же 2,5 ULP деления уровней, умноженные на
//!   усиление ≤ 2,35, — на порядки меньше допуска.
//! * **Само усиление** считается на GPU бит в бит с CPU (`precise` + деление с правильным округлением,
//!   `shaders/lens_shading.glsl`): единичная карта и пропуск стадии дают ОДИНАКОВЫЕ пиксели, а выключенная
//!   стадия при готовой карте — тоже; это проверяется равенством массивов, без допуска.
//! * **Статистика AWB после виньетирования** — граница из `viewfinder_awb.rs`, но на 6 ULP вместо 3. Вывод:
//!   нормированный отсчёт GPU отличается от CPU не больше чем на 2,5 ULP значения `n` (деление уровней,
//!   `u_n` — ULP числа `n`). Оба умножают на ОДНО И ТО ЖЕ усиление `g` ≤ 2,35 (одной операцией с
//!   правильным округлением), поэтому разница произведения `p = n·g` ≤ 2,5·`u_n`·g + 0,5 ULP(p). У p ULP
//!   не меньше `u_n·g/2` (p < 2·2^⌊log2 p⌋, n ≥ 2^⌊log2 n⌋ = `u_n`·2^23), отсюда 2,5·`u_n`·g ≤ 5 ULP(p)
//!   и в сумме ≤ 5,5 ULP — берём 6. Граница ячеек «в пределах 6 ULP от границы квантования» считается
//!   по эталонной мозаике ПОСЛЕ виньетирования, как и сама статистика.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use isp_model::image::DisplayGamut;
use isp_model::lens_shading::{CorrectionStrength, LensShadingGainMap, LensShadingSampling};
use isp_model::statistics::compute_awb_statistics;
use isp_model::transfer::srgb_decode;
use isp_vulkan::lens_shading_map::{LensShadingMapError, LensShadingMapLimits};
use isp_vulkan::viewfinder::{
    IspStage, IspStageSet, OutputAllocation, ViewfinderError, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp,
    ViewfinderStorageFormat,
};
use lens_shading_support::{asymmetric_map, identity_map, skipped, with_map};
use viewfinder_support::{
    GpuSource, ParamsSpec, RawFrame, RunRequest, SourceLayout, assert_no_validation_errors, boundary_cells,
    gpu_context_or_skip, parity, readback, record_and_submit, reference_upright, reference_upright_in_valid_area,
    reference_upright_in_valid_area_sampled, run_viewfinder, test_config,
};

/// Допуск ошибки математики сверх шага хранения (см. `viewfinder_parity.rs`).
const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;
/// Полшага 10-битного выхода.
const HALF_LSB: f32 = 0.5 / 1023.0;
/// Граница пограничных ячеек AWB после виньетирования, ULP (вывод — в документации модуля).
const SHADED_BOUNDARY_ULPS: u32 = 6;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
/// Отступы буфера вокруг полезной области в тесте со смещением: слева 4, сверху 2, справа 4, снизу 6.
const VALID_LEFT: u32 = 4;
const VALID_TOP: u32 = 2;
const BUFFER_WIDTH: u32 = VALID_LEFT + WIDTH + 4;
const BUFFER_HEIGHT: u32 = VALID_TOP + HEIGHT + 6;

const ALL_ORIENTATIONS: [SensorOrientation; 4] = [
    SensorOrientation::Degrees0,
    SensorOrientation::Degrees90,
    SensorOrientation::Degrees180,
    SensorOrientation::Degrees270,
];

/// Линейный свет сцены в пикселе `(x, y)` ПОЛЕЗНОЙ области: плавный градиент плюс шашечки (чтобы сдвиг
/// на один пиксель менял значения) — 0,04…0,25 до виньетирования, ≤ 0,6 после (ниже порога пересвета AWB).
fn scene_light(x: u32, y: u32, channel: usize) -> f64 {
    let gradient = 0.5 * f64::from(x) / f64::from(WIDTH - 1) + 0.5 * f64::from(y) / f64::from(HEIGHT - 1);
    let checker = f64::from((x / 4 + y / 4) % 2);
    0.04 + 0.12 * gradient + 0.03 * checker + 0.015 * channel as f64
}

/// Кадр размером с полезную область (RAW16 или RAW10 — по раскладке запуска).
fn valid_area_frame(spec: &ParamsSpec) -> RawFrame {
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), scene_light)
}

/// Кадр-буфер больше полезной области: за её пределами — белый (1023). Если шейдер прочтёт или
/// неверно отсчитает координаты от края буфера, а не от полезной области, паритет сломается.
fn padded_buffer_frame(spec: &ParamsSpec) -> (RawFrame, PixelRect) {
    let inner = valid_area_frame(spec);
    let valid = PixelRect {
        left: VALID_LEFT,
        top: VALID_TOP,
        size: PixelSize::new(WIDTH, HEIGHT),
    };
    let frame = RawFrame::from_fn(BUFFER_WIDTH, BUFFER_HEIGHT, spec.pattern, |x, y, _| {
        let inside = (VALID_LEFT..VALID_LEFT + WIDTH).contains(&x) && (VALID_TOP..VALID_TOP + HEIGHT).contains(&y);
        if inside {
            inner.samples[((y - VALID_TOP) * WIDTH + (x - VALID_LEFT)) as usize]
        } else {
            1023
        }
    });
    (frame, valid)
}

fn geometry(valid_area: PixelRect, crop: PixelRect, orientation: SensorOrientation) -> ViewfinderGeometry {
    ViewfinderGeometry {
        valid_area,
        crop,
        sensor_orientation: orientation,
        allocation: OutputAllocation::Exact,
    }
}

/// Сверка с эталоном: размер выхода, допуски `viewfinder_parity.rs`, и картинка действительно изменена
/// виньетированием (иначе совпадение с эталоном ничего бы не доказало).
fn assert_matches_reference(
    name: &str,
    run: &viewfinder_support::ViewfinderRun,
    reference: &viewfinder_support::UprightReference,
) {
    let result = parity(run, reference);
    assert!(result.math_excess <= MATH_EXCESS_TOLERANCE, "{name}: {result:?}");
    assert!(result.max_abs <= HALF_LSB + MATH_EXCESS_TOLERANCE, "{name}: {result:?}");
    assert_eq!(result.over_one_lsb, 0.0, "{name}: {result:?}");
}

/// Эталон без виньетирования для тех же кропа и ориентации.
fn unshaded_difference(
    run: &viewfinder_support::ViewfinderRun,
    unshaded: &viewfinder_support::UprightReference,
) -> f32 {
    run.pixels
        .iter()
        .zip(&unshaded.pixels)
        .map(|(gpu, cpu)| {
            (0..3)
                .map(|channel| (gpu[channel] - cpu[channel]).abs())
                .fold(0.0_f32, f32::max)
        })
        .fold(0.0_f32, f32::max)
}

#[test]
fn whole_frame_matches_reference_for_raw16_and_raw10_in_both_storage_formats() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let plain = spec.build();
    let params = with_map(&plain, &asymmetric_map(5, 4));
    let raw = valid_area_frame(&spec);
    let reference = reference_upright(&raw, raw.full_rect(), SensorOrientation::Degrees0, &params);
    let unshaded = reference_upright(&raw, raw.full_rect(), SensorOrientation::Degrees0, &plain);
    for layout in [SourceLayout::Raw16, SourceLayout::Raw10 { padding_bytes: 6 }] {
        for format in [
            ViewfinderStorageFormat::HalfFloatOnly,
            ViewfinderStorageFormat::PreferTenBitUnorm,
        ] {
            let request = RunRequest {
                layout,
                ..RunRequest::whole_frame(&raw, &params, spec.display())
            };
            let run = run_viewfinder(&context, &test_config(format), &request);
            assert_matches_reference(&format!("{layout:?}/{format:?}"), &run, &reference);
            // Карта 2,35× в углу и 1× в начале: без коррекции картинка заметно другая.
            let effect = unshaded_difference(&run, &unshaded);
            assert!(
                effect > 0.05,
                "{layout:?}/{format:?}: коррекция почти не изменила картинку ({effect})"
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn crop_origin_inside_offset_valid_area_and_every_rotation_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = with_map(&spec.build(), &asymmetric_map(6, 5));
    let (raw, valid) = padded_buffer_frame(&spec);
    // Кадрирование НЕ в начале полезной области (в её координатах — (6, 6)): виньетирование обязано
    // брать усиление по полезной области, а не по кропу и не по буферу.
    let crop = PixelRect {
        left: VALID_LEFT + 6,
        top: VALID_TOP + 6,
        size: PixelSize::new(40, 30),
    };
    for layout in [SourceLayout::Raw16, SourceLayout::Raw10 { padding_bytes: 6 }] {
        for orientation in ALL_ORIENTATIONS {
            let request = RunRequest {
                layout,
                geometry: geometry(valid, crop, orientation),
                ..RunRequest::whole_frame(&raw, &params, spec.display())
            };
            let run = run_viewfinder(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
            let reference = reference_upright_in_valid_area(&raw, valid, crop, orientation, &params);
            assert_matches_reference(&format!("{layout:?}/{orientation:?}"), &run, &reference);
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn per_cell_evaluation_matches_reference_at_grid_boundaries_and_edges_and_differs_from_per_sample() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let plain = spec.build();
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let raw = valid_area_frame(&spec);
    // 22×17 над 64×48: шаг сетки 3 и 47/16 пикселя — границы клеток проходят и между чётным и нечётным
    // пикселем, и внутри ячеек, центры ячеек пересекают много клеток; 2×2 — одна клетка на весь кадр
    // (крайний случай: левый узел всегда 0, правый край — последний узел); 5×4 — крупная сетка.
    for (columns, rows) in [(22, 17), (2, 2), (5, 4)] {
        let map = asymmetric_map(columns, rows);
        let params = with_map(&plain, &map);
        let request = RunRequest::whole_frame(&raw, &params, spec.display());
        let run = run_viewfinder(&context, &config, &request);
        let reference = reference_upright(&raw, raw.full_rect(), SensorOrientation::Degrees0, &params);
        assert_matches_reference(&format!("{columns}x{rows}"), &run, &reference);
        // Правая и нижняя ячейки (центр на 1,5 пикселя от края) — отдельно: самое далёкое от начала место.
        let (last_x, last_y) = (run.width - 1, run.height - 1);
        let (gpu_edge, cpu_edge) = (run.pixel(last_x, last_y), reference.pixels[reference.pixels.len() - 1]);
        for channel in 0..3 {
            assert!((gpu_edge[channel] - cpu_edge[channel]).abs() <= HALF_LSB + MATH_EXCESS_TOLERANCE);
        }
        // Статистика AWB в том же допуске.
        let cpu = compute_awb_statistics(&reference.run.shading_corrected, &config.awb_statistics).unwrap();
        let gpu = run.report.awb_statistics.clone().unwrap();
        let boundaries = boundary_cells(
            &reference.run.shading_corrected,
            &config.awb_statistics,
            SHADED_BOUNDARY_ULPS,
        );
        for ((gpu_tile, cpu_tile), boundary) in gpu.tiles().iter().zip(cpu.tiles()).zip(&boundaries) {
            assert_eq!(gpu_tile.used_cells, cpu_tile.used_cells, "{columns}x{rows}");
            assert!(
                gpu_tile.sum_r.abs_diff(cpu_tile.sum_r) <= boundary.quantization,
                "{columns}x{rows}"
            );
            assert!(
                gpu_tile.sum_g.abs_diff(cpu_tile.sum_g) <= 2 * boundary.quantization,
                "{columns}x{rows}"
            );
            assert!(
                gpu_tile.sum_b.abs_diff(cpu_tile.sum_b) <= boundary.quantization,
                "{columns}x{rows}"
            );
        }
        // Контроль различимости: на крутой карте 22×17 выборка «по отсчёту» от GPU заметно отличается — тест
        // действительно проверяет выборку по центру ячейки, а не любую гладкую.
        if columns == 22 {
            let per_sample = reference_upright_in_valid_area_sampled(
                &raw,
                raw.full_rect(),
                raw.full_rect(),
                SensorOrientation::Degrees0,
                &params,
                LensShadingSampling::PerSample,
            );
            let difference = parity(&run, &per_sample);
            assert!(
                difference.math_excess > 1.0e-4,
                "по отсчёту неотличимо от по ячейке: {difference:?}"
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn reference_in_valid_area_equals_run_reference_when_crop_is_the_whole_area() {
    // Страховка самого эталона теста: копия конвейера в `reference_upright_in_valid_area` не расходится
    // с `run_reference` — иначе паритет выше сверял бы GPU с неверной моделью.
    let spec = ParamsSpec::default();
    let params = with_map(&spec.build(), &asymmetric_map(5, 4));
    let raw = valid_area_frame(&spec);
    let whole = raw.full_rect();
    for orientation in ALL_ORIENTATIONS {
        let direct = reference_upright(&raw, whole, orientation, &params);
        let placed = reference_upright_in_valid_area(&raw, whole, whole, orientation, &params);
        assert_eq!(direct.pixels, placed.pixels, "{orientation:?}");
        assert_eq!(
            direct.run.shading_corrected, placed.run.shading_corrected,
            "{orientation:?}"
        );
    }
}

#[test]
fn awb_statistics_are_computed_after_lens_shading_within_the_documented_bound() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let plain = spec.build();
    let params = with_map(&plain, &asymmetric_map(6, 5));
    let (raw, valid) = padded_buffer_frame(&spec);
    let crop = PixelRect {
        left: VALID_LEFT + 6,
        top: VALID_TOP + 6,
        size: PixelSize::new(40, 30),
    };
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    for orientation in [SensorOrientation::Degrees0, SensorOrientation::Degrees90] {
        let request = |params| RunRequest {
            geometry: geometry(valid, crop, orientation),
            ..RunRequest::whole_frame(&raw, params, spec.display())
        };
        let shaded_run = run_viewfinder(&context, &config, &request(&params));
        let gpu = shaded_run.report.awb_statistics.clone().unwrap();
        let reference = reference_upright_in_valid_area(&raw, valid, crop, orientation, &params);
        let cpu = compute_awb_statistics(&reference.run.shading_corrected, &config.awb_statistics).unwrap();
        let boundaries = boundary_cells(
            &reference.run.shading_corrected,
            &config.awb_statistics,
            SHADED_BOUNDARY_ULPS,
        );
        for ((gpu_tile, cpu_tile), boundary) in gpu.tiles().iter().zip(cpu.tiles()).zip(&boundaries) {
            assert_eq!(boundary.saturation, 0, "сцена выбрана без отсчётов у порога пересвета");
            assert_eq!(gpu_tile.used_cells, cpu_tile.used_cells, "{orientation:?}");
            assert_eq!(gpu_tile.saturated_cells, cpu_tile.saturated_cells, "{orientation:?}");
            let bound = boundary.quantization;
            assert!(
                gpu_tile.sum_r.abs_diff(cpu_tile.sum_r) <= bound,
                "{orientation:?}: {gpu_tile:?} {cpu_tile:?} граница {bound}"
            );
            assert!(
                gpu_tile.sum_g.abs_diff(cpu_tile.sum_g) <= 2 * bound,
                "{orientation:?}: {gpu_tile:?} {cpu_tile:?}"
            );
            assert!(
                gpu_tile.sum_b.abs_diff(cpu_tile.sum_b) <= bound,
                "{orientation:?}: {gpu_tile:?} {cpu_tile:?}"
            );
        }
        // Статистика ПОСЛЕ виньетирования: без карты те же плитки заметно темнее (усиление до 2,35×).
        let plain_run = run_viewfinder(&context, &config, &request(&plain));
        let plain_totals = plain_run.report.awb_statistics.unwrap().totals();
        let shaded_totals = gpu.totals();
        let (plain_sum, shaded_sum) = (
            plain_totals.sum_r + plain_totals.sum_g + plain_totals.sum_b,
            shaded_totals.sum_r + shaded_totals.sum_g + shaded_totals.sum_b,
        );
        assert!(
            shaded_sum as f64 > 1.2 * plain_sum as f64,
            "{orientation:?}: статистика не видит коррекцию ({plain_sum} → {shaded_sum})"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn identity_map_and_every_way_to_switch_the_stage_off_reproduce_the_unshaded_image_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let plain = skipped(&spec.build());
    let raw = valid_area_frame(&spec);
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let run_with = |params: &isp_model::params::IspParams, stages: IspStageSet| {
        let request = RunRequest {
            stages,
            ..RunRequest::whole_frame(&raw, params, spec.display())
        };
        run_viewfinder(&context, &config, &request)
    };
    let baseline = run_with(&plain, IspStageSet::ALL);
    let baseline_statistics = baseline.report.awb_statistics.clone().unwrap();
    let off = IspStageSet::ALL.without(IspStage::LensShading);

    let variants = [
        (
            "единичная карта, стадия включена",
            run_with(&with_map(&plain, &identity_map(4, 3)), IspStageSet::ALL),
        ),
        (
            "карта готова, стадия выключена набором",
            run_with(&with_map(&plain, &asymmetric_map(5, 4)), off),
        ),
        ("пропуск и набор без стадии", run_with(&plain, off)),
    ];
    for (name, run) in variants {
        assert_eq!(run.pixels, baseline.pixels, "{name}: картинка");
        assert_eq!(
            run.report.awb_statistics.as_ref(),
            Some(&baseline_statistics),
            "{name}: статистика AWB"
        );
    }
    // Контроль чувствительности: рабочая карта картинку меняет.
    let shaded = run_with(&with_map(&plain, &asymmetric_map(5, 4)), IspStageSet::ALL);
    assert_ne!(shaded.pixels, baseline.pixels);
    assert_no_validation_errors(&context);
}

#[test]
fn map_changes_between_frames_of_one_isp_are_picked_up_and_unchanged_maps_are_reused() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let plain = spec.build();
    let raw = valid_area_frame(&spec);
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let first = asymmetric_map(5, 4);
    let second = asymmetric_map(7, 6);
    let sequence = [
        (0, with_map(&plain, &first)),
        (1, with_map(&plain, &second)),
        (0, with_map(&plain, &second)),
        (1, skipped(&plain)),
        (0, with_map(&plain, &first)),
        (0, with_map(&plain, &first)),
        (1, with_map(&plain, &first)),
    ];
    // Один ISP на всю последовательность: буфер карты слота переиспользуется и перезаписывается.
    let gpu_source = GpuSource::new(&context, &raw, SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
    for (index, (slot, params)) in sequence.iter().enumerate() {
        let request = RunRequest::whole_frame(&raw, params, spec.display());
        record_and_submit(
            &context,
            &mut isp,
            &request,
            &source,
            *slot,
            BackdropFrameNumber(index as u64 + 1),
        );
        let run = readback(&mut isp, *slot);
        let fresh = run_viewfinder(&context, &config, &request);
        assert_eq!(
            run.pixels, fresh.pixels,
            "кадр {index} (слот {slot}) отличается от свежего ISP"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn map_larger_than_the_buffer_is_an_explicit_error_and_the_isp_stays_usable() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let plain = spec.build();
    let raw = valid_area_frame(&spec);
    let mut config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    config.lens_shading_map = LensShadingMapLimits {
        max_columns: 4,
        max_rows: 4,
    };
    let gpu_source = GpuSource::new(&context, &raw, SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();

    let too_big = with_map(&plain, &asymmetric_map(5, 5));
    let mut outcome = None;
    run_one_shot_blocking(&context, |command_buffer| {
        outcome = Some(isp.record(
            command_buffer,
            &ViewfinderFrame {
                source: &source,
                params: &too_big,
                geometry: geometry(raw.full_rect(), raw.full_rect(), SensorOrientation::Degrees0),
                display_color_space: spec.display(),
                stages: IspStageSet::ALL,
                frame: BackdropFrameNumber(1),
                frame_slot: 0,
                reader: ConsumerStage::Fragment,
            },
        ));
        Ok(())
    })
    .unwrap();
    assert!(
        matches!(
            outcome,
            Some(Err(ViewfinderError::LensShadingMap(LensShadingMapError::TooLarge {
                columns: 5,
                rows: 5,
                max_columns: 4,
                max_rows: 4
            })))
        ),
        "{outcome:?}"
    );

    // Карта в пределах лимита после отказа обрабатывается как обычно и совпадает с эталоном.
    let fitting = with_map(&plain, &asymmetric_map(4, 4));
    let request = RunRequest::whole_frame(&raw, &fitting, spec.display());
    record_and_submit(&context, &mut isp, &request, &source, 0, BackdropFrameNumber(2));
    let run = readback(&mut isp, 0);
    let reference = reference_upright(&raw, raw.full_rect(), SensorOrientation::Degrees0, &fitting);
    assert_matches_reference("после отказа", &run, &reference);
    assert_no_validation_errors(&context);
}

// ===== Функциональный тест: настоящее плоское поле через настоящий видоискатель ====================

/// Сторона окна у угла и в центре на выходе, пиксели (8 пикселей сенсора = 4 пикселя выхода).
const WINDOW_OUTPUT_PIXELS: u32 = 4;
/// Допуск «угол / центр» после коррекции: ±12 % (как у CPU-теста пакета B).
const CORNER_TOLERANCE_RELATIVE: f64 = 0.12;
/// Допуск среднего по углам, EV.
const CORNER_TOLERANCE_MEAN_EV: f64 = 0.1;
/// Без коррекции углы тёмные: меньше 0,3 центра (замер CPU-теста: 0,2).
const UNCORRECTED_CORNER_LIMIT: f64 = 0.3;

/// Средняя яркость (линейный свет экрана, Rec. 709) окна `WINDOW_OUTPUT_PIXELS²` с левым верхним углом `(left, top)`.
fn window_luminance(run: &viewfinder_support::ViewfinderRun, left: u32, top: u32) -> f64 {
    let mut sum = 0.0_f64;
    for y in top..top + WINDOW_OUTPUT_PIXELS {
        for x in left..left + WINDOW_OUTPUT_PIXELS {
            let [red, green, blue, _] = run.pixel(x, y);
            let linear = |encoded: f32| f64::from(srgb_decode(encoded));
            sum += 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
        }
    }
    sum / f64::from(WINDOW_OUTPUT_PIXELS * WINDOW_OUTPUT_PIXELS)
}

/// Отношения «угол / центр» яркости на выходе, четыре угла.
fn corner_ratios(run: &viewfinder_support::ViewfinderRun) -> [f64; 4] {
    let (width, height) = (run.width, run.height);
    let centre = window_luminance(
        run,
        width / 2 - WINDOW_OUTPUT_PIXELS / 2,
        height / 2 - WINDOW_OUTPUT_PIXELS / 2,
    );
    [
        (0, 0),
        (width - WINDOW_OUTPUT_PIXELS, 0),
        (0, height - WINDOW_OUTPUT_PIXELS),
        (width - WINDOW_OUTPUT_PIXELS, height - WINDOW_OUTPUT_PIXELS),
    ]
    .map(|(left, top)| window_luminance(run, left, top) / centre)
}

/// Реальное held-out плоское поле (256×192 BGGR, уменьшенный сенсор 4096×3072) → настоящая калибровка камеры
/// «2» → карта потока → GPU-видоискатель → читаем итоговую картинку. Это «доходит до стадии показа»: проверяется
/// не карта и не шейдер по отдельности, а видимый результат всего прохода.
fn flat_field_through_viewfinder(
    context: &GpuContext,
    stages: IspStageSet,
) -> (viewfinder_support::ViewfinderRun, [f64; 4]) {
    let asset = viewfinder_support::isp_support::load_asset("flat-field-main-heldout");
    assert_eq!(asset.field("extra.cfa_arrangement").as_str(), Some("BGGR"));
    let black: Vec<f32> = asset
        .field("extra.black_level_per_position")
        .as_array()
        .expect("чёрный")
        .iter()
        .map(|value| value.as_f64().expect("число") as f32)
        .collect();
    let spec = ParamsSpec {
        black: [black[0], black[1], black[2], black[3]],
        white: asset.field("extra.white_level").as_f64().expect("белый") as f32,
        gamut: DisplayGamut::Srgb,
        ..ParamsSpec::default()
    };
    let raw = RawFrame {
        width: asset.width,
        height: asset.height,
        pattern: spec.pattern,
        samples: asset.u16_samples().to_vec(),
    };
    let map = LensShadingGainMap::for_stream(
        &lens_shading_support::real_calibration_of_camera_2(),
        &camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(lens_shading_support::ACTIVE_ARRAY),
        PixelSize::new(asset.width, asset.height),
        CorrectionStrength::FULL,
    )
    .expect("карта потока из настоящей калибровки");
    let params = with_map(&spec.build(), &std::sync::Arc::new(map));
    let request = RunRequest {
        stages,
        ..RunRequest::whole_frame(&raw, &params, spec.display())
    };
    let run = run_viewfinder(context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
    let ratios = corner_ratios(&run);
    (run, ratios)
}

#[test]
fn real_flat_field_is_flat_on_screen_with_the_stage_and_dark_in_the_corners_without_it() {
    let Some(context) = gpu_context_or_skip() else { return };

    let (_, corrected) = flat_field_through_viewfinder(&context, IspStageSet::ALL);
    eprintln!("плоское поле, коррекция включена: угол/центр = {corrected:.3?}");
    for (corner, ratio) in corrected.iter().enumerate() {
        assert!(
            (ratio - 1.0).abs() <= CORNER_TOLERANCE_RELATIVE,
            "угол {corner}: {ratio:.3} (допуск ±{CORNER_TOLERANCE_RELATIVE})"
        );
    }
    let mean_ev = corrected.iter().map(|ratio| ratio.log2()).sum::<f64>() / 4.0;
    assert!(
        mean_ev.abs() <= CORNER_TOLERANCE_MEAN_EV,
        "средний угол отстоит от центра на {mean_ev:.3} EV (допуск ±{CORNER_TOLERANCE_MEAN_EV})"
    );

    let (_, uncorrected) = flat_field_through_viewfinder(&context, IspStageSet::ALL.without(IspStage::LensShading));
    eprintln!("плоское поле, коррекция выключена: угол/центр = {uncorrected:.3?}");
    for (corner, ratio) in uncorrected.iter().enumerate() {
        assert!(
            *ratio < UNCORRECTED_CORNER_LIMIT,
            "без коррекции угол {corner} темнее центра: {ratio:.3} (нужно < {UNCORRECTED_CORNER_LIMIT})"
        );
    }
    assert_no_validation_errors(&context);
}
