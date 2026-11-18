//! Оценщик на синтетических плитках: отбор плиток, веса, редукции, скалярный шаг.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::capabilities::RawSupport;
use camera_contract::fixtures::main_wide_camera;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::white_point::{CctDuv, TemperatureClamp, chromaticity_from_cct_duv};
use isp_model::statistics::{AwbStatistics, TileGrid};

use super::*;
use crate::config::LocusTableConfig;

const CELLS: u32 = 4096;

fn profile() -> DngColorProfile {
    let RawSupport::Supported(info) = main_wide_camera().capabilities.raw else {
        panic!("в образце основной камеры есть RAW");
    };
    DngColorProfile::from_calibration_sets(&info.color_calibration).expect("профиль цвета")
}

fn table() -> CameraLocusTable {
    CameraLocusTable::build(&profile(), &LocusTableConfig::default(), 0.02).expect("таблица")
}

/// Плитка серой поверхности под светом `(kelvin, duv)` с освещённостью `level` (G, доли белого).
fn gray_tile(kelvin: f64, duv: f64, level: f32) -> AwbTile {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(duv),
    })
    .expect("белая");
    let neutral = profile().neutral_for_white(white).expect("нейтраль").values();
    colour_tile([neutral[0] as f32 * level, level, neutral[2] as f32 * level], CELLS, 0)
}

/// Плитка однородного цвета `rgb` (доли белого, ДО баланса) с заданными числами непересвеченных/пересвеченных ячеек.
fn colour_tile(rgb: [f32; 3], used: u32, saturated: u32) -> AwbTile {
    let q = rgb.map(isp_model::statistics::quantize_sample);
    AwbTile {
        sum_r: q[0] * used,
        sum_g: 2 * q[1] * used,
        sum_b: q[2] * used,
        used_cells: used,
        saturated_cells: saturated,
    }
}

fn statistics(tiles: Vec<AwbTile>) -> AwbStatistics {
    let count = u32::try_from(tiles.len()).expect("мало плиток");
    AwbStatistics::from_tiles(
        TileGrid {
            columns: count,
            rows: 1,
        },
        tiles,
    )
    .expect("плитки по сетке")
}

fn estimate(tiles: Vec<AwbTile>) -> RawEstimate {
    estimate_scene_light(&statistics(tiles), &table(), &EstimatorConfig::default())
}

fn mired_of(kelvin: f64) -> f32 {
    Kelvin(kelvin).mired() as f32
}

#[test]
fn gray_tiles_give_the_light_they_were_lit_by() {
    let raw = estimate(vec![gray_tile(4000.0, 0.0, 0.4); 50]);
    assert!((raw.mired - mired_of(4000.0)).abs() < 1.0, "{raw:?}");
    assert!(raw.duv.abs() < 0.001, "{raw:?}");
    assert_eq!((raw.accepted_tiles, raw.eligible_tiles), (50, 50));
    assert!((raw.confidence - 1.0).abs() < 0.01, "все плитки серые: уверенность ≈ 1");
}

#[test]
fn dark_saturated_and_empty_channel_tiles_are_not_used_and_never_produce_nan() {
    let dark = gray_tile(4000.0, 0.0, 0.005);
    let bright = gray_tile(4000.0, 0.0, 0.97);
    let fully_clipped = AwbTile {
        sum_r: 0,
        sum_g: 0,
        sum_b: 0,
        used_cells: 0,
        saturated_cells: CELLS,
    };
    let no_blue = AwbTile {
        sum_b: 0,
        ..gray_tile(4000.0, 0.0, 0.4)
    };
    let raw = estimate(vec![dark, bright, fully_clipped, no_blue, AwbTile::default()]);
    assert_eq!((raw.accepted_tiles, raw.eligible_tiles), (0, 0), "{raw:?}");
    assert!(!raw.has_candidates());
    assert!(
        raw.mired.is_finite() && raw.duv.is_finite() && raw.confidence == 0.0,
        "{raw:?}"
    );
}

#[test]
fn tile_with_mostly_clipped_cells_is_rejected_but_a_mostly_clean_one_is_kept() {
    let clean = gray_tile(4000.0, 0.0, 0.4);
    let mostly_clipped = AwbTile {
        saturated_cells: 3 * CELLS,
        ..clean
    };
    let mostly_clean = AwbTile {
        saturated_cells: CELLS / 4,
        ..clean
    };
    assert_eq!(estimate(vec![mostly_clipped]).eligible_tiles, 0);
    assert_eq!(estimate(vec![mostly_clean]).accepted_tiles, 1);
}

#[test]
fn strongly_coloured_tiles_are_eligible_but_never_accepted_as_gray() {
    // Насыщенный зелёный далеко от кривой Планка: пригоден для оценки, но не серый.
    let green = colour_tile([0.05, 0.4, 0.05], CELLS, 0);
    let raw = estimate(vec![green; 40]);
    assert_eq!((raw.accepted_tiles, raw.eligible_tiles), (0, 40), "{raw:?}");
}

#[test]
fn a_few_gray_tiles_among_many_coloured_ones_give_a_low_confidence() {
    let mut tiles = vec![colour_tile([0.05, 0.4, 0.05], CELLS, 0); 90];
    tiles.extend(vec![gray_tile(5000.0, 0.0, 0.4); 10]);
    let raw = estimate(tiles);
    assert!((raw.mired - mired_of(5000.0)).abs() < 1.0);
    assert!((raw.confidence - 0.1).abs() < 1e-3, "10 из 100 равных по весу: {raw:?}");
}

#[test]
fn brighter_tiles_outweigh_dim_ones() {
    // 3 яркие плитки при 6500 К против 30 тусклых (на пороге веса) при 3000 К: побеждает яркий свет.
    let mut tiles = vec![gray_tile(6500.0, 0.0, 0.5); 3];
    tiles.extend(vec![gray_tile(3000.0, 0.0, 0.031); 30]);
    let config = EstimatorConfig {
        reduction: ReductionConfig::WeightedMean,
        ..EstimatorConfig::default()
    };
    let raw = estimate_scene_light(&statistics(tiles), &table(), &config);
    // Без весов среднее было бы ≈ 317 майред (30 из 33 плиток тёплые); вес тусклых ≈ 0,8 % от яркой.
    assert!(
        (raw.mired - mired_of(6500.0)).abs() < 20.0,
        "тусклые почти не весят: {raw:?}"
    );
}

#[test]
fn mode_window_ignores_warmer_decoys_that_the_plain_mean_follows() {
    // 40 серых плиток при 6500 К и по 5 «приманок» (серые при других температурах — как кожа и дерево).
    let mut tiles = vec![gray_tile(6500.0, 0.0, 0.4); 40];
    for kelvin in [3000.0, 3400.0, 3800.0, 4200.0, 4600.0] {
        tiles.extend(vec![gray_tile(kelvin, 0.0, 0.4); 5]);
    }
    let statistics = statistics(tiles);
    let table = table();
    let mean_config = EstimatorConfig {
        reduction: ReductionConfig::WeightedMean,
        ..EstimatorConfig::default()
    };
    let mean = estimate_scene_light(&statistics, &table, &mean_config);
    let mode = estimate_scene_light(&statistics, &table, &EstimatorConfig::default());
    assert!(
        mean.mired > mired_of(6500.0) + 30.0,
        "среднее смещено приманками: {mean:?}"
    );
    assert!(
        (mode.mired - mired_of(6500.0)).abs() < 2.0,
        "окно по моде держится за пик: {mode:?}"
    );
    assert_eq!(mode.accepted_tiles, 40);
}

#[test]
fn finalize_without_candidates_reports_no_candidates_and_keeps_the_eligible_count() {
    let raw = RawEstimate {
        mired: 0.0,
        duv: 0.0,
        confidence: 0.0,
        accepted_tiles: 0,
        eligible_tiles: 77,
    };
    assert_eq!(
        finalize_estimate(raw, table().band()).expect("исход"),
        EstimateOutcome::NoCandidates { eligible_tiles: 77 }
    );
}

#[test]
fn finalize_clamps_to_the_band_and_reports_what_was_clamped() {
    let band = table().band();
    let inside = RawEstimate {
        mired: mired_of(5000.0),
        duv: 0.005,
        confidence: 0.5,
        accepted_tiles: 100,
        eligible_tiles: 200,
    };
    let EstimateOutcome::Estimated(kept) = finalize_estimate(inside, band).expect("исход") else {
        panic!("кандидаты есть");
    };
    assert!(!kept.clamped.is_any(), "внутри полосы ничего не прижато");
    assert!((kept.cct_duv.duv.0 - 0.005).abs() < 1e-9);

    // Duv 0,035 — за допуском итоговой оценки 0,02.
    let too_green = RawEstimate { duv: 0.035, ..inside };
    let EstimateOutcome::Estimated(clamped) = finalize_estimate(too_green, band).expect("исход") else {
        panic!("кандидаты есть");
    };
    assert!(clamped.clamped.is_any(), "{clamped:?}");
    assert!((clamped.cct_duv.duv.0 - 0.02).abs() < 1e-9, "Duv прижат к полосе");
    assert_eq!(clamped.clamped.temperature, TemperatureClamp::WithinBand);
    assert_eq!(clamped.raw, too_green, "сырой выход сохранён для диагностики");
}
