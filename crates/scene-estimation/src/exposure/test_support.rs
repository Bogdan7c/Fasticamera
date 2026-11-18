//! Общие помощники тестов AE: синтетическая статистика, веса яркости настоящего ISP и цели.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::capabilities::{LensShading, RawSupport};
use camera_contract::fixtures::main_wide_camera;
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::{CameraNeutral, DngColorProfile};
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::demosaic::DemosaicMethod;
use isp_model::exposure::LuminanceWeights;
use isp_model::image::DisplayGamut;
use isp_model::levels::NormalizationLevels;
use isp_model::params::{FrameInputs, IspParams, LensShadingMapInput, ProcessingControls};
use isp_model::sensor::SensorDescription;
use isp_model::statistics::{AwbStatistics, AwbTile, TileGrid, quantize_sample};

use super::measurement::ExposureTarget;

/// Сетка боевой конфигурации: 32×24.
pub(super) const GRID: TileGrid = TileGrid { columns: 32, rows: 24 };

/// Ячеек в плитке синтетической статистики.
pub(super) const CELLS: u32 = 4096;

/// Порог пересвета статистики (как в `AwbStatisticsSettings::default`).
const SATURATION_THRESHOLD: f32 = 0.95;

/// Цель яркости: тот же средний серый, что у тона по умолчанию.
pub(super) fn target() -> ExposureTarget {
    ExposureTarget::new(isp_model::tone::ToneCurveSettings::default().scene_middle_gray).unwrap()
}

/// Веса яркости, при которых серый `(g, g, g)` даёт яркость `g` (сумма весов = 1).
pub(super) fn plain_luminance() -> LuminanceWeights {
    LuminanceWeights::new(0.25, 0.5, 0.25)
}

/// Плитка однородного цвета `rgb` (доли белого, ДО баланса): пересвеченная целиком, если канал ≥ порога.
pub(super) fn tile_of(rgb: [f32; 3]) -> AwbTile {
    if rgb
        .iter()
        .any(|&channel| channel.is_nan() || channel >= SATURATION_THRESHOLD)
    {
        return AwbTile {
            saturated_cells: CELLS,
            ..AwbTile::default()
        };
    }
    let q = rgb.map(quantize_sample);
    AwbTile {
        sum_r: q[0] * CELLS,
        sum_g: 2 * q[1] * CELLS,
        sum_b: q[2] * CELLS,
        used_cells: CELLS,
        saturated_cells: 0,
    }
}

/// Плитка, в которой доля `saturated_share` ячеек пересвечена, остальные — серые `level`.
pub(super) fn partly_clipped_tile(level: f32, saturated_share: f32) -> AwbTile {
    let saturated = (CELLS as f32 * saturated_share) as u32;
    let used = CELLS - saturated;
    let q = quantize_sample(level);
    AwbTile {
        sum_r: q * used,
        sum_g: 2 * q * used,
        sum_b: q * used,
        used_cells: used,
        saturated_cells: saturated,
    }
}

/// Статистика сетки 32×24: цвет плитки задаёт `colour_of(column, row)`.
pub(super) fn stats_from(colour_of: impl Fn(u32, u32) -> [f32; 3]) -> AwbStatistics {
    stats_from_tiles(|column, row| tile_of(colour_of(column, row)))
}

/// Статистика сетки 32×24 из готовых плиток.
pub(super) fn stats_from_tiles(tile_of: impl Fn(u32, u32) -> AwbTile) -> AwbStatistics {
    let mut tiles = Vec::new();
    for row in 0..GRID.rows {
        for column in 0..GRID.columns {
            tiles.push(tile_of(column, row));
        }
    }
    AwbStatistics::from_tiles(GRID, tiles).unwrap()
}

/// Однородный серый кадр яркостью `level` (G, доли белого).
pub(super) fn uniform_gray(level: f32) -> AwbStatistics {
    stats_from(|_, _| [level; 3])
}

/// Произведение «выдержка × ISO».
pub(super) fn product(seconds_iso: f64) -> ExposureProduct {
    ExposureProduct::from_seconds_iso(seconds_iso).unwrap()
}

/// Профиль цвета OnePlus 11 (из образца камеры контракта).
pub(super) fn color_profile() -> DngColorProfile {
    let RawSupport::Supported(info) = main_wide_camera().capabilities.raw else {
        panic!("в образце основной камеры есть RAW");
    };
    DngColorProfile::from_calibration_sets(&info.color_calibration).unwrap()
}

/// Нейтраль камеры для света `kelvin` на кривой Планка.
pub(super) fn neutral_for(kelvin: f64) -> CameraNeutral {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .unwrap();
    color_profile().neutral_for_white(white).unwrap()
}

/// Параметры ISP настоящего кадра под нейтраль `neutral` (веса яркости берутся отсюда).
pub(super) fn isp_params_for(neutral: CameraNeutral) -> IspParams {
    let levels = NormalizationLevels::new([64.0; 4], 1023.0).unwrap();
    let sensor = SensorDescription::new(
        BayerPattern::BGGR,
        levels,
        color_profile(),
        LensShading::AppliedByVendor,
    );
    let frame = FrameInputs {
        levels,
        lens_shading_map: LensShadingMapInput::NotProvided,
    };
    let controls = ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, DisplayGamut::Srgb);
    IspParams::resolve(&sensor, &frame, &controls).unwrap()
}
