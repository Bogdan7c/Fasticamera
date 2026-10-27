use camera_contract::capabilities::LensShading;
use camera_contract::frame_passport::CaptureColorHints;

use super::{
    CURRENT_ISP_PARAMS_VERSION, FrameInputs, IspParams, LensShadingMapInput, ProcessingControls, ResolveError,
};
use crate::color::chromaticity::Chromaticity;
use crate::color::dng::CameraNeutral;
use crate::demosaic::DemosaicMethod;
use crate::image::DisplayGamut;
use crate::lens_shading::{LensShadingGainMap, LensShadingSkipReason, LensShadingStep};
use crate::sensor::SensorDescription;
use crate::test_support::{lamp_neutral, main_camera_calibration, main_camera_passport};
use crate::tone::{InvalidToneCurve, ToneCurveSettings};

fn main_sensor() -> SensorDescription {
    SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap()
}

fn resolve_for(neutral: CameraNeutral, gamut: DisplayGamut) -> IspParams {
    let sensor = main_sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    IspParams::resolve(
        &sensor,
        &frame,
        &ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, gamut),
    )
    .unwrap()
}

#[test]
fn balanced_gray_maps_to_d50_white_with_luminance_equal_to_input() {
    let params = resolve_for(lamp_neutral(), DisplayGamut::DisplayP3);
    let gray = 0.4_f32;
    let xyz = params.color.camera_to_xyz_d50().apply([gray; 3]);
    let d50 = Chromaticity::D50.to_xyz();
    for channel in 0..3 {
        let expected = gray * d50[channel] as f32;
        assert!((xyz[channel] - expected).abs() < 1e-5, "{xyz:?} против {expected}");
    }
}

#[test]
fn balanced_gray_is_gray_on_both_display_gamuts_for_any_illuminant() {
    // Лампа (C7), дневной свет (усиления вендора днём) и холодная тень — серый остаётся серым.
    let neutrals = [lamp_neutral().values(), [0.46, 1.0, 0.68], [0.40, 1.0, 0.85]];
    for neutral in neutrals {
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let params = resolve_for(CameraNeutral::new(neutral).unwrap(), gamut);
            let scene = params
                .color
                .xyz_d50_to_scene
                .apply(params.color.camera_to_xyz_d50().apply([0.25; 3]));
            for channel in scene {
                assert!((channel - 0.25).abs() < 1e-5, "{neutral:?} {gamut:?}: {scene:?}");
            }
        }
    }
}

#[test]
fn white_estimate_follows_the_neutral() {
    let lamp = resolve_for(lamp_neutral(), DisplayGamut::DisplayP3);
    let daylight = resolve_for(CameraNeutral::new([0.46, 1.0, 0.68]).unwrap(), DisplayGamut::DisplayP3);
    // Лампа теплее дневного света: температура белой точки ниже.
    assert!(lamp.color.white().temperature.0 < daylight.color.white().temperature.0);
    // Кадр C7 «лампа»: белая точка ≈ 4532 K (сверено с colour-hdri в dng_tests).
    assert!(
        (lamp.color.white().temperature.0 - 4532.1).abs() < 1.0,
        "{:?}",
        lamp.color.white()
    );
}

#[test]
fn params_carry_version_pattern_levels_and_choices() {
    let params = resolve_for(lamp_neutral(), DisplayGamut::Srgb);
    assert_eq!(params.version, CURRENT_ISP_PARAMS_VERSION);
    assert_eq!(params.pattern, main_sensor().pattern());
    assert_eq!(params.levels, main_sensor().static_levels());
    assert_eq!(params.demosaic, DemosaicMethod::Quality);
    assert_eq!(params.color.gamut(), DisplayGamut::Srgb);
}

#[test]
fn vendor_corrected_shading_is_skipped_even_when_map_is_provided() {
    let sensor = main_sensor();
    assert_eq!(sensor.vendor_lens_shading(), LensShading::AppliedByVendor);
    let map = LensShadingGainMap::new(2, 2, vec![[2.0; 4]; 4]).unwrap();
    let frame = FrameInputs {
        levels: sensor.static_levels(),
        lens_shading_map: LensShadingMapInput::Provided(map),
    };
    let controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb);
    let params = IspParams::resolve(&sensor, &frame, &controls).unwrap();
    assert_eq!(
        params.lens_shading,
        LensShadingStep::Skip(LensShadingSkipReason::VendorAlreadyApplied)
    );
}

#[test]
fn invalid_tone_settings_surface_as_resolve_error() {
    let sensor = main_sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let mut controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb);
    controls.tone = ToneCurveSettings {
        contrast: -1.0,
        ..ToneCurveSettings::default()
    };
    assert_eq!(
        IspParams::resolve(&sensor, &frame, &controls),
        Err(ResolveError::ToneCurve(InvalidToneCurve::InvalidContrast {
            contrast: -1.0
        }))
    );
}

#[test]
fn balanced_gray_luminance_does_not_depend_on_neutral_scale_or_brightest_channel() {
    let d50 = Chromaticity::D50.to_xyz();
    // Нейтраль с самым ярким красным (max(N) = 1,3) и та же нейтраль в другом масштабе:
    // сбалансированный серый (b, b, b) обязан давать Y = b в обоих случаях — иначе экспозиция
    // кадра зависела бы от того, как нормирована нейтраль.
    for neutral in [
        [1.3, 1.0, 0.6],
        [0.65, 0.5, 0.3],
        lamp_neutral().values().map(|value| value * 4.0),
    ] {
        let params = resolve_for(CameraNeutral::new(neutral).unwrap(), DisplayGamut::Srgb);
        let balanced_gray = 0.4_f32;
        let xyz = params.color.camera_to_xyz_d50().apply([balanced_gray; 3]);
        for channel in 0..3 {
            let expected = balanced_gray * d50[channel] as f32;
            assert!(
                (xyz[channel] - expected).abs() < 1e-5,
                "{neutral:?}: {xyz:?} против {expected}"
            );
        }
    }
}
