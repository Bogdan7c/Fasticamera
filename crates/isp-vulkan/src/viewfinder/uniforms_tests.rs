use ash::vk;
use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::demosaic::DemosaicMethod;
use isp_model::lens_shading::LensShadingGainMap;
use isp_model::params::{FrameInputs, IspParamsVersion, LensShadingMapInput, ProcessingControls};
use isp_model::statistics::TileGrid;

use super::*;

#[path = "../../../isp-model/tests/support/mod.rs"]
mod support;

fn real_params(gamut: DisplayGamut, demosaic: DemosaicMethod) -> IspParams {
    let capture = support::real_capture::load_real_capture(&support::assets_dir(), "real-main-lamp-gray-tile");
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
        lens_shading_map: LensShadingMapInput::NotProvided,
    };
    let controls = ProcessingControls::with_defaults(capture.neutral, demosaic, gamut);
    IspParams::resolve(&capture.sensor, &frame, &controls).unwrap()
}

fn geometry() -> CheckedGeometry {
    CheckedGeometry {
        crop: PixelRect {
            left: 2,
            top: 4,
            size: PixelSize::new(40, 20),
        },
        output: vk::Extent2D { width: 10, height: 20 },
        allocation: vk::Extent2D { width: 10, height: 20 },
        quarter_turns: 1,
        raw10_row_stride_bytes: 64,
    }
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

#[test]
fn layout_matches_documented_offsets() {
    let params = real_params(DisplayGamut::DisplayP3, DemosaicMethod::Preview);
    let awb = AwbStatisticsSettings {
        grid: TileGrid { columns: 6, rows: 3 },
        saturation_threshold: 0.9,
    };
    let bytes = pack_uniforms(&params, &geometry(), &awb);
    let black = params.levels.black_pattern();
    let white = params.levels.white();
    for (position, cell_black) in black.iter().enumerate() {
        assert_eq!(f32_at(&bytes, position * 4), *cell_black);
        assert_eq!(f32_at(&bytes, 16 + position * 4), white - cell_black);
    }
    // BGGR: позиция 0 — синий, 1 и 2 — зелёные, 3 — красный.
    assert_eq!(params.pattern, BayerPattern::BGGR);
    let gains = params.white_balance.as_array();
    assert_eq!(f32_at(&bytes, 32), gains[2]);
    assert_eq!(f32_at(&bytes, 36), gains[1]);
    assert_eq!(f32_at(&bytes, 44), gains[0]);
    assert_eq!(
        [f32_at(&bytes, 48), f32_at(&bytes, 52), f32_at(&bytes, 56)],
        [white, 1.0, 0.9]
    );
    let to_xyz = params.color.camera_to_xyz_d50().rows;
    let to_display = params.color.xyz_d50_to_scene().rows;
    for row in 0..3 {
        for column in 0..3 {
            assert_eq!(f32_at(&bytes, 64 + row * 16 + column * 4), to_xyz[row][column]);
            assert_eq!(f32_at(&bytes, 112 + row * 16 + column * 4), to_display[row][column]);
        }
        assert_eq!(f32_at(&bytes, 64 + row * 16 + 12), 0.0, "w строки матрицы");
    }
    let gamut = params.gamut_compression.parameters();
    assert_eq!(f32_at(&bytes, 160), gamut.threshold[0]);
    assert_eq!(f32_at(&bytes, 172), gamut.power);
    assert_eq!(f32_at(&bytes, 184), gamut.scale[2]);
    assert_eq!(f32_at(&bytes, 188), 1.0 / gamut.power);
    let tone = params.tone.parameters();
    assert_eq!(
        [
            f32_at(&bytes, 192),
            f32_at(&bytes, 196),
            f32_at(&bytes, 200),
            f32_at(&bytes, 204)
        ],
        [
            tone.contrast,
            tone.scale_pow_contrast,
            tone.white_normalization,
            tone.hue_preservation
        ]
    );
    let words = |offset: usize| [0, 4, 8, 12].map(|step| u32_at(&bytes, offset + step));
    assert_eq!(words(208), [2, 4, 40, 20]);
    assert_eq!(words(224), [10, 20, 1, 64]);
    assert_eq!(words(240), [2, 1, 1, 0], "цвета позиций BGGR");
    assert_eq!(words(256), [6, 3, 0, 0]);
}

#[test]
fn params_for_another_model_gamut_demosaic_or_shading_are_rejected() {
    let params = real_params(DisplayGamut::DisplayP3, DemosaicMethod::Preview);
    assert_eq!(check_params(&params, DisplayColorSpace::DisplayP3), Ok(()));
    assert!(matches!(
        check_params(&params, DisplayColorSpace::Srgb),
        Err(ViewfinderError::GamutMismatch { .. })
    ));
    let srgb = real_params(DisplayGamut::Srgb, DemosaicMethod::Preview);
    assert_eq!(check_params(&srgb, DisplayColorSpace::Srgb), Ok(()));

    let quality = real_params(DisplayGamut::DisplayP3, DemosaicMethod::Quality);
    assert!(matches!(
        check_params(&quality, DisplayColorSpace::DisplayP3),
        Err(ViewfinderError::UnsupportedParams { .. })
    ));
    let mut old = params.clone();
    old.version = IspParamsVersion(0);
    assert!(matches!(
        check_params(&old, DisplayColorSpace::DisplayP3),
        Err(ViewfinderError::ParamsVersionMismatch { .. })
    ));
    let mut shaded = params;
    shaded.lens_shading = LensShadingStep::Apply(LensShadingGainMap::new(2, 2, vec![[1.0; 4]; 4]).unwrap());
    assert!(matches!(
        check_params(&shaded, DisplayColorSpace::DisplayP3),
        Err(ViewfinderError::UnsupportedParams { .. })
    ));
}
