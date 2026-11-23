use ash::vk;
use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::demosaic::DemosaicMethod;
use std::sync::Arc;

use isp_model::lens_shading::{LensShadingGainMap, LensShadingStep};
use isp_model::params::{FrameInputs, IspParamsVersion, ProcessingControls};
use isp_model::statistics::TileGrid;

use super::*;

#[path = "../../../isp-model/tests/support/mod.rs"]
mod support;

fn real_params(gamut: DisplayGamut, demosaic: DemosaicMethod) -> IspParams {
    let capture = support::real_capture::load_real_capture(&support::assets_dir(), "real-main-lamp-gray-tile");
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    let controls = ProcessingControls::with_defaults(capture.neutral, demosaic, gamut).with_legacy_tone();
    IspParams::resolve(&capture.sensor, &frame, &controls).unwrap()
}

fn geometry() -> CheckedGeometry {
    CheckedGeometry {
        valid_area: PixelRect {
            left: 2,
            top: 2,
            size: PixelSize::new(60, 40),
        },
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
        sum_headroom: 8.0,
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
    assert_eq!(
        words(272),
        [2, 2, 0, 0],
        "смещение полезной области для карты виньетирования"
    );
    assert_eq!(bytes.len(), 18 * 16);
}

#[test]
fn params_for_another_model_or_gamut_are_rejected() {
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
    let mut old = params;
    old.version = IspParamsVersion(0);
    assert!(matches!(
        check_params(&old, DisplayColorSpace::DisplayP3),
        Err(ViewfinderError::ParamsVersionMismatch { .. })
    ));
}

/// Профиль выхода, который GPU не считает (Log, HDR, 10 бит), отвергается различимой ошибкой ДО записи кадра, а
/// поддержанные профили проходят с любым из двух охватов.
#[test]
fn unsupported_output_profiles_are_rejected_by_the_params_check() {
    use isp_model::output::{OutputBitDepth, OutputProfile, OutputRendering, UnsupportedOutputProfile};
    let supported = real_params(DisplayGamut::DisplayP3, DemosaicMethod::Preview);
    for (profile, expected) in [
        (
            OutputProfile::display_p3().with_rendering(OutputRendering::SceneLog),
            UnsupportedOutputProfile::Rendering(OutputRendering::SceneLog),
        ),
        (
            OutputProfile::display_p3().with_rendering(OutputRendering::Hdr),
            UnsupportedOutputProfile::Rendering(OutputRendering::Hdr),
        ),
        (
            OutputProfile::display_p3().with_bit_depth(OutputBitDepth::Ten),
            UnsupportedOutputProfile::BitDepth(OutputBitDepth::Ten),
        ),
    ] {
        // Параметры нельзя разрешить с таким профилем, поэтому подменяем профиль у разрешённых вручную: GPU-проверка
        // обязана отвергнуть и такой кадр (защита от параметров, собранных мимо `resolve`).
        let params = IspParams {
            output_profile: profile,
            ..supported.clone()
        };
        assert_eq!(
            check_params(&params, DisplayColorSpace::DisplayP3),
            Err(ViewfinderError::UnsupportedOutputProfile(expected))
        );
    }
    assert_eq!(check_params(&supported, DisplayColorSpace::DisplayP3), Ok(()));
}

/// Видоискатель принимает параметры кадра с любой из моделей основы тона: отказа «модель не поддержана на GPU»
/// больше нет (проверка параметров не зависит от выбранной модели).
#[test]
fn every_tone_base_model_passes_the_params_check() {
    use isp_model::tone::aces::AcesSettings;
    use isp_model::tone::base::ToneBase;
    use isp_model::tone::filmic::FilmicSettings;
    let capture = support::real_capture::load_real_capture(&support::assets_dir(), "real-main-lamp-gray-tile");
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    for base in [
        ToneBase::Legacy,
        ToneBase::Agx(isp_model::tone::agx_settings::AgxSettings::default()),
        ToneBase::Aces(AcesSettings::default()),
        ToneBase::Filmic(FilmicSettings::default()),
    ] {
        let controls = ProcessingControls {
            tone_base: base,
            ..ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Preview, DisplayGamut::DisplayP3)
        };
        let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
        assert_eq!(check_params(&params, DisplayColorSpace::DisplayP3), Ok(()));
    }
}

#[test]
fn lens_shading_map_in_params_is_accepted() {
    // Раньше проверка отказывала на `Apply`; теперь карта — штатная часть параметров кадра.
    let mut shaded = real_params(DisplayGamut::DisplayP3, DemosaicMethod::Preview);
    shaded.lens_shading = LensShadingStep::Apply(Arc::new(LensShadingGainMap::new(2, 2, vec![[1.0; 4]; 4]).unwrap()));
    assert_eq!(check_params(&shaded, DisplayColorSpace::DisplayP3), Ok(()));
}
