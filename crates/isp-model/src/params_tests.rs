use std::sync::Arc;

use camera_contract::capabilities::LensShading;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::geometry::{PixelRect, PixelSize};

use super::{CURRENT_ISP_PARAMS_VERSION, FrameInputs, IspParams, ProcessingControls, ResolveError};
use crate::color::chromaticity::Chromaticity;
use crate::color::dng::CameraNeutral;
use crate::demosaic::DemosaicMethod;
use crate::image::DisplayGamut;
use crate::lens_shading::{CorrectionStrength, LensShadingControl, LensShadingSkipReason, LensShadingStep};
use crate::output::{OutputBitDepth, OutputProfile, OutputRendering, UnsupportedOutputProfile};
use crate::sensor::SensorDescription;
use crate::test_support::{
    MAIN_ACTIVE_ARRAY, calibrated_passport, lamp_neutral, main_camera_calibration, main_camera_passport,
};
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
        &ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, gamut).with_legacy_tone(),
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

fn resolve_with_profile(profile: OutputProfile) -> Result<IspParams, ResolveError> {
    let sensor = main_sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Quality, DisplayGamut::Srgb)
        .with_output_profile(profile);
    IspParams::resolve(&sensor, &frame, &controls)
}

#[test]
fn params_carry_the_output_profile_and_derive_the_output_gamut_from_it() {
    for (profile, gamut) in [
        (OutputProfile::srgb(), DisplayGamut::Srgb),
        (OutputProfile::display_p3(), DisplayGamut::DisplayP3),
    ] {
        let params = resolve_with_profile(profile).unwrap();
        assert_eq!(params.output_profile, profile);
        assert_eq!(params.color.gamut(), gamut);
    }
    // Разные профили дают разные матрицы «XYZ → цвета выхода»: охват действительно берётся из профиля.
    let srgb = resolve_with_profile(OutputProfile::srgb()).unwrap();
    let display_p3 = resolve_with_profile(OutputProfile::display_p3()).unwrap();
    assert_ne!(srgb.color.xyz_d50_to_scene(), display_p3.color.xyz_d50_to_scene());
}

#[test]
fn with_defaults_gamut_is_the_sdr_profile_of_that_gamut() {
    let sensor = main_sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let by_gamut = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Quality, DisplayGamut::DisplayP3);
    let by_profile = by_gamut.clone().with_output_profile(OutputProfile::display_p3());
    assert_eq!(
        IspParams::resolve(&sensor, &frame, &by_gamut).unwrap(),
        IspParams::resolve(&sensor, &frame, &by_profile).unwrap()
    );
}

#[test]
fn unsupported_output_profiles_are_refused_with_a_distinct_error() {
    for rendering in [OutputRendering::SceneLog, OutputRendering::Hdr] {
        assert_eq!(
            resolve_with_profile(OutputProfile::display_p3().with_rendering(rendering)),
            Err(ResolveError::OutputProfile(UnsupportedOutputProfile::Rendering(
                rendering
            )))
        );
    }
    assert_eq!(
        resolve_with_profile(OutputProfile::srgb().with_bit_depth(OutputBitDepth::Ten)),
        Err(ResolveError::OutputProfile(UnsupportedOutputProfile::BitDepth(
            OutputBitDepth::Ten
        )))
    );
}

fn calibrated_sensor() -> SensorDescription {
    let passport = calibrated_passport(
        PixelSize::new(64, 48),
        PixelRect {
            left: 0,
            top: 0,
            size: MAIN_ACTIVE_ARRAY,
        },
    );
    SensorDescription::from_stream_passport(&passport, &main_camera_calibration()).unwrap()
}

fn resolve_shading(sensor: &SensorDescription, control: LensShadingControl) -> LensShadingStep {
    let frame = FrameInputs::from_capture_hints(sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls {
        lens_shading: control,
        ..ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb)
            .with_legacy_tone()
    };
    IspParams::resolve(sensor, &frame, &controls).unwrap().lens_shading
}

#[test]
fn vendor_flag_does_not_gate_our_correction_calibration_does() {
    // Паспорт сообщает «вендор применил частично», но калибровка есть — коррекция включается.
    let sensor = calibrated_sensor();
    assert_eq!(sensor.vendor_lens_shading(), LensShading::AppliedByVendor);
    let step = resolve_shading(&sensor, LensShadingControl::default());
    let LensShadingStep::Apply(map) = step else {
        panic!("ожидали Apply, получили {step:?}");
    };
    assert!(
        Arc::ptr_eq(&map, sensor.lens_shading_map().unwrap()),
        "карта потока общая"
    );
    // Без калибровки (тот же флаг вендора) стадия пропускается с причиной.
    assert_eq!(
        resolve_shading(&main_sensor(), LensShadingControl::default()),
        LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated)
    );
}

#[test]
fn lens_shading_control_changes_the_resolved_step() {
    let sensor = calibrated_sensor();
    assert_eq!(
        resolve_shading(&sensor, LensShadingControl::Disabled),
        LensShadingStep::Skip(LensShadingSkipReason::DisabledByControl)
    );
    let full = resolve_shading(&sensor, LensShadingControl::Enabled(CorrectionStrength::FULL));
    let half = resolve_shading(
        &sensor,
        LensShadingControl::Enabled(CorrectionStrength::new(0.5).unwrap()),
    );
    let (LensShadingStep::Apply(full_map), LensShadingStep::Apply(half_map)) = (full, half) else {
        panic!("обе силы включают стадию");
    };
    assert_ne!(full_map, half_map, "смена силы меняет параметры");
    let (full_gain, half_gain) = (full_map.points()[0][0], half_map.points()[0][0]);
    assert!(full_gain > 1.0, "угол калибровки светлее центра: {full_gain}");
    assert_eq!(half_gain, 1.0 + (full_gain - 1.0) * 0.5);
}

#[test]
fn invalid_tone_settings_surface_as_resolve_error() {
    let sensor = main_sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let mut controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb)
        .with_legacy_tone();
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
