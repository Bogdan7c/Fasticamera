//! Замер экспозиции на настоящей статистике ISP: пересвет судится по СЫРОМУ отсчёту (до усиления виньетирования).
//!
//! Сцена: умеренно яркий объект у угла кадра. Угол снят сенсором тёмным (виньетирование), и калибровка
//! поднимает его в ×5: сырой отсчёт объекта 0,3 (ничего не пересвечено), скорректированный — 1,5. До исправления
//! статистика судила пересвет по скорректированному значению, объект считался пересвеченным, и замер просил
//! у камеры на ступени темнее (в живых кадрах — до −2,8 EV на всём кадре). Теперь пересвет — только настоящий
//! сенсорный, а суммы идут по скорректированным значениям с запасом выше белого.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::sync::Arc;

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::exposure::LuminanceWeights;
use isp_model::image::{CameraMosaic, Mosaic, mosaic_stage};
use isp_model::lens_shading::{
    LensShadingControl, LensShadingGainMap, LensShadingSampling, LensShadingStep, apply_lens_shading_with,
};
use isp_model::statistics::{
    AwbStatistics, AwbStatisticsInput, AwbStatisticsSettings, AwbTile, TileGrid, compute_awb_statistics,
    quantize_sample_with_headroom,
};
use scene_estimation::exposure::measurement::ExposureTarget;
use scene_estimation::exposure::{
    ExposureMeasurement, MeteringArea, MeteringConfig, MeteringWeights, TilePlacement, measure_exposure,
};

/// Мозаика 128×96: ячейки 64×48, сетка 8×6 — плитки по 8×8 ячеек.
const SIZE: PixelSize = PixelSize::new(128, 96);
const GRID: TileGrid = TileGrid { columns: 8, rows: 6 };
/// Свет фона (скорректированный, доли белого) и объекта.
const BACKGROUND_LIGHT: f32 = 0.15;
const OBJECT_LIGHT: f32 = 1.5;
/// Усиление в нижнем правом углу карты.
const CORNER_GAIN: f32 = 5.0;

/// «Объект» — плитки столбцов 5…7 и строк 4…5 (нижний правый угол): пиксели 80…127 × 64…95.
fn in_object(x: u32, y: u32) -> bool {
    x >= 80 && y >= 64
}

fn mosaic_from(sample: impl Fn(u32, u32) -> f32, stage: mosaic_stage::Normalized) -> CameraMosaic {
    let samples = (0..SIZE.height)
        .flat_map(|y| (0..SIZE.width).map(move |x| (x, y)))
        .map(|(x, y)| sample(x, y))
        .collect();
    Mosaic::from_samples(SIZE, BayerPattern::BGGR, samples, stage).unwrap()
}

/// Карта 2×2: углы 1, 1, 1 и ×5 внизу справа, во всех каналах.
fn vignette_map() -> Arc<LensShadingGainMap> {
    Arc::new(LensShadingGainMap::new(2, 2, vec![[1.0; 4], [1.0; 4], [1.0; 4], [CORNER_GAIN; 4]]).unwrap())
}

fn weights() -> MeteringWeights {
    MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .unwrap()
}

fn measure(statistics: &AwbStatistics, frame: ExposureProduct) -> ExposureMeasurement {
    measure_exposure(
        statistics,
        &weights(),
        LuminanceWeights::new(0.25, 0.5, 0.25),
        frame,
        ExposureTarget::new(isp_model::tone::ToneCurveSettings::default().scene_middle_gray).unwrap(),
        &MeteringConfig::default(),
    )
    .unwrap()
}

/// Правда о сцене: каждая плитка целиком принята, средний свет плитки — `light(плитка)` (скорректированный).
fn ground_truth_statistics(settings: &AwbStatisticsSettings) -> AwbStatistics {
    let tiles = (0..GRID.columns * GRID.rows)
        .map(|index| {
            let (column, row) = (index % GRID.columns, index / GRID.columns);
            let light = if column >= 5 && row >= 4 {
                OBJECT_LIGHT
            } else {
                BACKGROUND_LIGHT
            };
            let q = quantize_sample_with_headroom(light, settings.sum_headroom);
            let cells = 64;
            AwbTile {
                sum_r: cells * q,
                sum_g: 2 * cells * q,
                sum_b: cells * q,
                used_cells: cells,
                saturated_cells: 0,
            }
        })
        .collect();
    AwbStatistics::from_tiles(GRID, tiles).unwrap()
}

fn required_ev(measurement: &ExposureMeasurement, frame: ExposureProduct) -> f64 {
    measurement.required_product.ev_above(frame)
}

struct Scene {
    raw: CameraMosaic,
    corrected: Mosaic<mosaic_stage::ShadingCorrected>,
}

/// Кадр с виньетированием: сенсор видит `свет / усиление` (в углу объект 1,5/5 = 0,3), а ISP возвращает его усилением.
fn vignetted_scene() -> Scene {
    let step = LensShadingStep::decide(Some(&vignette_map()), LensShadingControl::default());
    let ones = mosaic_from(|_, _| 1.0, mosaic_stage::Normalized);
    let (gains, _) = apply_lens_shading_with(&ones, &step, LensShadingSampling::PerCellCentre);
    let raw = mosaic_from(
        |x, y| {
            let light = if in_object(x, y) {
                OBJECT_LIGHT
            } else {
                BACKGROUND_LIGHT
            };
            light / gains.sample(x, y)
        },
        mosaic_stage::Normalized,
    );
    let (corrected, _) = apply_lens_shading_with(&raw, &step, LensShadingSampling::PerCellCentre);
    Scene { raw, corrected }
}

#[test]
fn moderately_bright_object_under_corner_gain_is_not_clipped_and_ae_matches_the_ground_truth() {
    let scene = vignette_corrected_checks();
    let settings = AwbStatisticsSettings {
        grid: GRID,
        ..AwbStatisticsSettings::default()
    };
    let frame = ExposureProduct::from_seconds_iso(0.01).unwrap();

    // Сырой отсчёт объекта — 1,5/усиление: 0,3 в самом углу и до ~0,6 у границы объекта, всюду меньше 0,95.
    let brightest_raw = (0..SIZE.height)
        .flat_map(|y| (0..SIZE.width).map(move |x| (x, y)))
        .map(|(x, y)| scene.raw.sample(x, y))
        .fold(0.0_f32, f32::max);
    assert!(
        brightest_raw < settings.saturation_threshold,
        "в RAW ничего не пересвечено: {brightest_raw}"
    );
    assert!(
        (0..SIZE.height)
            .flat_map(|y| (0..SIZE.width).map(move |x| (x, y)))
            .any(|(x, y)| scene.corrected.sample(x, y) > 1.0),
        "после коррекции есть значения выше белого"
    );

    // Исправлено: пересвет — по сырому, суммы — по скорректированному.
    let fixed = compute_awb_statistics(
        AwbStatisticsInput::new(&scene.raw, &scene.corrected).unwrap(),
        &settings,
    )
    .unwrap();
    assert_eq!(
        fixed.totals().saturated_cells,
        0,
        "ни одна ячейка не пересвечена по сырому отсчёту"
    );

    // Прежнее поведение (пересвет и суммы по скорректированному, потолок — белый): имитируем явно.
    let corrected_as_normalized = Mosaic::from_samples(
        SIZE,
        BayerPattern::BGGR,
        scene.corrected.samples().to_vec(),
        mosaic_stage::Normalized,
    )
    .unwrap();
    let old_settings = AwbStatisticsSettings {
        sum_headroom: 1.0,
        ..settings
    };
    let old = compute_awb_statistics(
        AwbStatisticsInput::new(&corrected_as_normalized, &scene.corrected).unwrap(),
        &old_settings,
    )
    .unwrap();
    assert!(
        old.totals().saturated_cells > 0,
        "до исправления объект считался пересвеченным"
    );

    // Замер на правде о сцене и на исправленной статистике совпадает; прежний — просит темнее.
    let truth = measure(&ground_truth_statistics(&settings), frame);
    let measured = measure(&fixed, frame);
    let before = measure(&old, frame);
    let (truth_ev, fixed_ev, old_ev) = (
        required_ev(&truth, frame),
        required_ev(&measured, frame),
        required_ev(&before, frame),
    );
    eprintln!("EV: правда {truth_ev:.3}, исправлено {fixed_ev:.3}, до исправления {old_ev:.3}");
    assert!(
        (fixed_ev - truth_ev).abs() < 0.02,
        "исправлено {fixed_ev}, правда {truth_ev}"
    );
    assert!(
        (old_ev - truth_ev).abs() > 0.05,
        "до исправления замер должен отличаться: {old_ev} против {truth_ev}"
    );
    assert_eq!(measured.quality, truth.quality);
}

/// Проверка сцены: карта усилений действительно даёт ≈ ×5 в углу (иначе остальной тест ничего бы не доказывал).
fn vignette_corrected_checks() -> Scene {
    let scene = vignetted_scene();
    let corner_gain = scene.corrected.sample(127, 95) / scene.raw.sample(127, 95);
    assert!(corner_gain > 4.0, "{corner_gain}");
    scene
}

#[test]
fn a_truly_clipped_highlight_in_the_raw_is_still_saturated() {
    // Настоящий пересвет сенсора (0,99) в той же сцене — в плитке (4, 4), где усиление ≈ ×2.
    let scene = vignetted_scene();
    let step = LensShadingStep::decide(Some(&vignette_map()), LensShadingControl::default());
    let raw = mosaic_from(
        |x, y| {
            if (64..80).contains(&x) && (64..80).contains(&y) {
                0.99
            } else {
                scene.raw.sample(x, y)
            }
        },
        mosaic_stage::Normalized,
    );
    let (corrected, _) = apply_lens_shading_with(&raw, &step, LensShadingSampling::PerCellCentre);
    let settings = AwbStatisticsSettings {
        grid: GRID,
        ..AwbStatisticsSettings::default()
    };
    let statistics = compute_awb_statistics(AwbStatisticsInput::new(&raw, &corrected).unwrap(), &settings).unwrap();
    // Клип 16×16 пикселей (64…80) = 8×8 ячеек = ровно плитка (4, 4) — вся пересвечена.
    let tile = statistics.tile(4, 4).unwrap();
    assert_eq!((tile.saturated_cells, tile.used_cells), (64, 0), "{tile:?}");
    assert_eq!(statistics.totals().saturated_cells, 64, "и больше ничего");
}
