//! Регистрация моделей основы тона: каждая из четырёх (`Legacy`, `Agx`, `Aces`, `Filmic`) проходит один и тот же
//! путь — настройки → `IspParams::resolve` → эталонная цепочка — и обязана выдать серый экрана, свой белый точки
//! и различимые ошибки (решения 176, 177, 182).

use camera_contract::frame_passport::CaptureColorHints;

use super::*;
use crate::demosaic::DemosaicMethod;
use crate::image::{DisplayEncodedImage, RawMosaic};
use crate::lens_shading::LensShadingControl;
use crate::params::{FrameInputs, IspParams, ProcessingControls, ResolveError};
use crate::pipeline::{run_reference, run_reference_encoded, run_reference_working};
use crate::sensor::SensorDescription;
use crate::shading_pipeline_tests::{BLACK, BUFFER, WHITE, passport};
use crate::test_support::{LAMP_NEUTRAL, lamp_neutral, main_camera_calibration};

/// Серый предмет экрана у всех моделей — 0,20 (`display_middle_gray` умолчаний), допуск — округление 12-битного RAW.
const DISPLAY_GREY: f32 = 0.20;

fn sensor() -> SensorDescription {
    SensorDescription::from_stream_passport(&passport(), &main_camera_calibration()).unwrap()
}

/// Четыре модели с умолчаниями (то, что даст конфиг без правок).
fn all_models() -> [ToneBase; 4] {
    [
        ToneBase::Legacy,
        ToneBase::Agx(AgxSettings::default()),
        ToneBase::Aces(AcesSettings::default()),
        ToneBase::Filmic(FilmicSettings::default()),
    ]
}

/// Только модели рабочего пространства (у прежней основы другая цепочка).
fn working_models() -> [ToneBase; 3] {
    [
        ToneBase::Agx(AgxSettings::default()),
        ToneBase::Aces(AcesSettings::default()),
        ToneBase::Filmic(FilmicSettings::default()),
    ]
}

fn controls_with(base: ToneBase) -> ProcessingControls {
    ProcessingControls {
        lens_shading: LensShadingControl::Disabled,
        tone_base: base,
        ..ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb)
    }
}

fn resolve(base: ToneBase) -> IspParams {
    let sensor = sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    IspParams::resolve(&sensor, &frame, &controls_with(base)).unwrap()
}

/// Однородный серый кадр яркости `brightness` (в долях белого сенсора) — серый предмет под лампой AWB.
fn uniform_frame(sensor: &SensorDescription, brightness: f64) -> RawMosaic {
    let pattern = sensor.pattern();
    let mut samples = Vec::new();
    for y in 0..BUFFER.height {
        for x in 0..BUFFER.width {
            let signal = (brightness * LAMP_NEUTRAL[pattern.channel_at(x, y).index()]).min(1.0);
            samples.push((f64::from(BLACK) + signal * f64::from(WHITE - BLACK)).round() as u16);
        }
    }
    RawMosaic::new(BUFFER, pattern, samples).unwrap()
}

fn centre(image: &DisplayEncodedImage) -> [f32; 3] {
    image.pixel(image.size().width / 2, image.size().height / 2)
}

#[test]
fn every_model_resolves_through_isp_params_with_its_own_kind_and_policy() {
    for base in all_models() {
        let params = resolve(base);
        assert_eq!(params.tone_base.kind(), base.kind());
        let expected_policy = if matches!(base, ToneBase::Legacy) {
            WhitePolicy::SensorLevel
        } else {
            WhitePolicy::PerPoint
        };
        assert_eq!(params.white_policy, expected_policy, "{:?}", base.kind());
        // Эталон узнаёт модель в `W` только через `working_stage`: у прежней основы его нет.
        assert_eq!(
            params.tone_base.working_stage().is_some(),
            !matches!(base, ToneBase::Legacy)
        );
    }
}

#[test]
fn working_models_share_rec2020_and_legacy_works_in_output_colours() {
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        for base in working_models() {
            assert_eq!(base.working_primaries(gamut), WORKING_PRIMARIES, "{:?}", base.kind());
            assert_eq!(
                resolve(base).tone_base.working_primaries(gamut),
                WORKING_PRIMARIES,
                "{:?}",
                base.kind()
            );
        }
        assert_eq!(ToneBase::Legacy.working_primaries(gamut), gamut.primaries());
    }
}

#[test]
fn scene_grey_lands_exactly_on_the_display_grey_for_every_working_model() {
    for base in working_models() {
        let params = resolve(base);
        let stage = params.tone_base.working_stage().unwrap();
        let grey = AgxSettings::default().scene_middle_gray;
        let display = stage.map_scene_pixel([grey; 3], 1.0);
        for channel in display {
            assert!(
                (channel - DISPLAY_GREY).abs() < 2.0e-3,
                "{:?}: серый сцены дал {display:?}",
                base.kind()
            );
        }
        // Серый остаётся серым (каналы равны с точностью f32) и после выхода в цвета экрана.
        let output = stage.to_output(display);
        assert!((output[0] - output[1]).abs() < 1.0e-5 && (output[1] - output[2]).abs() < 1.0e-5);
    }
}

#[test]
fn point_white_maps_to_exactly_display_white_for_every_working_model() {
    for base in working_models() {
        let params = resolve(base);
        let stage = params.tone_base.working_stage().unwrap();
        // У каждой точки свой белый (решение 177): L = 1 без коррекции, 3,5 в углу с виньетированием.
        for white in [1.0_f32, 2.2, 3.5] {
            let mapped = stage.map_scene_pixel([white; 3], white);
            // Допуск — округление f32 строк матриц (в сумме они дают 1 с точностью ~1e-7).
            for channel in mapped {
                assert!(
                    (channel - 1.0).abs() < 1.0e-5,
                    "{:?}, белый {white}: {mapped:?}",
                    base.kind()
                );
            }
            let output = stage.to_output(mapped);
            for channel in output {
                assert!((channel - 1.0).abs() < 1.0e-5, "{:?}: {output:?}", base.kind());
            }
        }
    }
}

#[test]
fn reference_chain_renders_a_grey_card_on_every_model_through_the_same_entry_point() {
    let sensor = sensor();
    let grey = AgxSettings::default().scene_middle_gray;
    let raw = uniform_frame(&sensor, f64::from(grey));
    let mut renders = Vec::new();
    for base in all_models() {
        let params = resolve(base);
        let image = run_reference_encoded(&raw, &params).unwrap();
        let pixel = centre(&image);
        // Серая карта выглядит как серая: каналы равны (допуск — цветовые матрицы камеры), яркость в середине шкалы.
        assert!(
            (pixel[0] - pixel[1]).abs() < 0.02 && (pixel[1] - pixel[2]).abs() < 0.02,
            "{:?}: {pixel:?}",
            base.kind()
        );
        assert!((0.3..0.7).contains(&pixel[1]), "{:?}: {pixel:?}", base.kind());
        renders.push(image);
    }
    // Модели рисуют по-разному: переключение основы реально меняет кадр, а не игнорируется.
    for first in 0..renders.len() {
        for second in first + 1..renders.len() {
            assert_ne!(
                renders[first], renders[second],
                "модели {first} и {second} дали один кадр"
            );
        }
    }
}

#[test]
fn encoded_entry_point_uses_the_matching_chain_and_refuses_the_wrong_one() {
    let sensor = sensor();
    let raw = uniform_frame(&sensor, 0.075);
    for base in working_models() {
        let params = resolve(base);
        assert_eq!(
            run_reference_encoded(&raw, &params).unwrap(),
            run_reference_working(&raw, &params).unwrap().display_encoded,
            "{:?}",
            base.kind()
        );
        // Прежняя цепочка отказывается считать модель в `W` различимой ошибкой, а не молча рисует.
        assert!(run_reference(&raw, &params).is_err(), "{:?}", base.kind());
    }
    let legacy = resolve(ToneBase::Legacy);
    assert_eq!(
        run_reference_encoded(&raw, &legacy).unwrap(),
        run_reference(&raw, &legacy).unwrap().display_encoded
    );
    assert!(run_reference_working(&raw, &legacy).is_err());
}

#[test]
fn a_blown_highlight_is_pure_white_on_every_working_model() {
    let sensor = sensor();
    // Яркость 20 в долях белого сенсора: пересвет.
    let raw = uniform_frame(&sensor, 20.0);
    for base in working_models() {
        let image = run_reference_encoded(&raw, &resolve(base)).unwrap();
        for channel in centre(&image) {
            assert!(channel > 0.998, "{:?}: пересвет не белый", base.kind());
        }
    }
}

#[test]
fn knobs_reach_the_settings_of_the_selected_model_only() {
    let legacy = ToneCurveSettings::default();
    let amounts = ToneKnobAmounts {
        contrast_log2: 0.5,
        ..ToneKnobAmounts::default()
    };
    let aces = ToneBase::Aces(AcesSettings::default()).with_knobs(&legacy, &amounts);
    let ToneBase::Aces(knobbed) = aces.base else {
        panic!("модель сменилась при применении ручек");
    };
    assert_eq!(knobbed.contrast, AcesSettings::default().contrast * 0.5_f32.exp2());
    assert_eq!(aces.legacy_curve, legacy);
    assert!(aces.unsupported.is_empty());
    let filmic = ToneBase::Filmic(FilmicSettings::default()).with_knobs(&legacy, &amounts);
    let ToneBase::Filmic(knobbed) = filmic.base else {
        panic!("модель сменилась при применении ручек");
    };
    assert_eq!(knobbed.slope, FilmicSettings::default().slope * 0.5_f32.exp2());
    // Нейтральные ручки не меняют настройки ни на бит.
    for base in all_models() {
        assert_eq!(base.with_knobs(&legacy, &ToneKnobAmounts::default()).base, base);
    }
}

#[test]
fn invalid_settings_surface_as_the_error_of_their_own_model() {
    let aces = ToneBase::Aces(AcesSettings {
        scene_middle_gray: 2.0,
        ..AcesSettings::default()
    });
    let filmic = ToneBase::Filmic(FilmicSettings {
        slope: 0.0,
        ..FilmicSettings::default()
    });
    let sensor = sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    assert!(matches!(
        IspParams::resolve(&sensor, &frame, &controls_with(aces)),
        Err(ResolveError::Aces(_))
    ));
    assert!(matches!(
        IspParams::resolve(&sensor, &frame, &controls_with(filmic)),
        Err(ResolveError::Filmic(_))
    ));
    // Тот же отказ у прямого разрешения основы (так её зовут проверка краёв и кадр).
    assert!(matches!(
        aces.resolve(ExposureGain::UNITY, DisplayGamut::Srgb),
        Err(InvalidToneBase::Aces(_))
    ));
}

#[test]
fn tone_base_round_trips_through_json_for_every_model() {
    for base in all_models() {
        let json = serde_json::to_string(&base).unwrap();
        assert_eq!(serde_json::from_str::<ToneBase>(&json).unwrap(), base, "{json}");
    }
    assert_eq!(serde_json::to_string(&ToneBase::Legacy).unwrap(), "\"legacy\"");
    assert!(
        serde_json::to_string(&ToneBase::Aces(AcesSettings::default()))
            .unwrap()
            .starts_with("{\"aces\":")
    );
}
