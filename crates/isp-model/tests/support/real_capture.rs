//! Реальный RAW из фикстуры `make_real_crops.py`: мозаика + описание сенсора + нейтраль из манифеста.

use std::path::Path;

use camera_contract::capabilities::{LensShading, ReferenceIlluminant};
use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::{CameraNeutral, DngCalibration, DngColorProfile};
use isp_model::color::matrix::Matrix3;
use isp_model::color::temperature::calibration_illuminant_temperature;
use isp_model::image::RawMosaic;
use isp_model::levels::NormalizationLevels;
use isp_model::sensor::SensorDescription;

use super::{TestAsset, load_asset_from};

/// Кадр, готовый для `IspParams::resolve` и `run_reference`.
pub struct RealCapture {
    pub asset: TestAsset,
    pub raw: RawMosaic,
    pub sensor: SensorDescription,
    pub neutral: CameraNeutral,
}

pub fn load_real_capture(dir: &Path, name: &str) -> RealCapture {
    let asset = load_asset_from(dir, name);
    let pattern = match asset.field("extra.cfa").as_str() {
        Some("bggr") => BayerPattern::BGGR,
        Some("rggb") => BayerPattern::RGGB,
        Some("grbg") => BayerPattern::GRBG,
        Some("gbrg") => BayerPattern::GBRG,
        other => panic!("неизвестная CFA {other:?} в {name}"),
    };
    let raw = RawMosaic::new(
        PixelSize::new(asset.width, asset.height),
        pattern,
        asset.u16_samples().to_vec(),
    )
    .unwrap_or_else(|error| panic!("мозаика {name}: {error}"));
    let black: [f32; 4] = f64_array(asset.field("extra.black_level")).map(|value| value as f32);
    let white = asset
        .field("extra.white_level")
        .as_f64()
        .unwrap_or_else(|| panic!("нет white_level")) as f32;
    let levels = NormalizationLevels::new(black, white).unwrap_or_else(|error| panic!("уровни {name}: {error}"));
    let profile = color_profile(&asset);
    let sensor = SensorDescription::new(pattern, levels, profile, LensShading::AppliedByVendor);
    let neutral = CameraNeutral::new(f64_array(asset.field("extra.neutral")))
        .unwrap_or_else(|error| panic!("нейтраль {name}: {error}"));
    RealCapture {
        asset,
        raw,
        sensor,
        neutral,
    }
}

/// DNG-профиль из двух калибровок манифеста (порядок как у HAL: иллюминант 1, затем 2).
fn color_profile(asset: &TestAsset) -> DngColorProfile {
    let calibration = |index: usize| {
        let illuminant_code = asset.field("extra.illuminants")[index]
            .as_u64()
            .unwrap_or_else(|| panic!("иллюминант"));
        let illuminant = ReferenceIlluminant::from_exif_code(u8::try_from(illuminant_code).unwrap_or(0));
        DngCalibration {
            temperature: calibration_illuminant_temperature(illuminant).unwrap_or_else(|error| panic!("{error}")),
            color_matrix: matrix(&asset.field("extra.color_matrices")[index]),
            forward_matrix: Some(matrix(&asset.field("extra.forward_matrices")[index])),
            camera_calibration: matrix(&asset.field("extra.calibrations")[index]),
        }
    };
    DngColorProfile::new(calibration(0), Some(calibration(1)), [1.0, 1.0, 1.0])
        .unwrap_or_else(|error| panic!("профиль {}: {error}", asset.name))
}

fn matrix(value: &serde_json::Value) -> Matrix3 {
    let values: [f64; 9] = f64_array(value);
    Matrix3::from_rows([
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ])
}

fn f64_array<const N: usize>(value: &serde_json::Value) -> [f64; N] {
    let items = value
        .as_array()
        .unwrap_or_else(|| panic!("ожидался массив, а не {value}"));
    assert_eq!(items.len(), N, "длина массива {value}");
    std::array::from_fn(|index| items[index].as_f64().unwrap_or_else(|| panic!("не число в {value}")))
}
