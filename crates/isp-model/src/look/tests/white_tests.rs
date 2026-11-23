use std::sync::Arc;

use camera_contract::frame_passport::CaptureColorHints;

use super::*;
use crate::color::dng::CameraNeutral;
use crate::color::temperature::Kelvin;
use crate::color::white_point::{CctDuv, Duv, WhitePointError, chromaticity_from_cct_duv};
use crate::demosaic::DemosaicMethod;
use crate::image::DisplayGamut;
use crate::look::LookControls;
use crate::look::model::LookModelSettings;
use crate::look::white::{WhiteShift, WhiteShiftLimit, WhiteShiftSkipReason, shift_white};
use crate::params::{FrameInputs, IspParams, ProcessingControls};
use crate::sensor::SensorDescription;
use crate::test_support::{lamp_neutral, main_camera_calibration, main_camera_passport};

pub(super) fn sensor() -> SensorDescription {
    SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap()
}

pub(super) fn frame(sensor: &SensorDescription) -> FrameInputs {
    FrameInputs::from_capture_hints(sensor, &CaptureColorHints::default()).unwrap()
}

pub(super) fn controls() -> ProcessingControls {
    ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Quality, DisplayGamut::Srgb)
}

fn with_look(settings: LookSettings) -> ProcessingControls {
    controls().with_look(LookControls::new(settings, Arc::new(LookModelSettings::default())))
}

fn resolve(controls: &ProcessingControls) -> IspParams {
    let sensor = sensor();
    IspParams::resolve(&sensor, &frame(&sensor), controls).unwrap()
}

/// Что получит серый предмет сцены (камера видит нейтраль AWB): линейный свет экрана `[R, G, B]`.
fn grey_card(params: &IspParams, neutral: CameraNeutral) -> [f32; 3] {
    let camera = neutral.values().map(|value| value as f32);
    let balanced = params.white_balance.as_array();
    let after_balance = [
        camera[0] * balanced[0],
        camera[1] * balanced[1],
        camera[2] * balanced[2],
    ];
    params
        .color
        .xyz_d50_to_scene()
        .apply(params.color.camera_to_xyz_d50().apply(after_balance))
}

#[test]
fn warmth_makes_a_grey_card_warmer_and_cooler_in_the_right_direction() {
    let neutral = lamp_neutral();
    let base = grey_card(&resolve(&controls()), neutral);
    assert!((base[0] - base[2]).abs() < 1e-3, "без ручки серый не серый: {base:?}");
    let warm = grey_card(&resolve(&with_look(settings_with(|s| s.warmth = knob(1.0)))), neutral);
    let cool = grey_card(&resolve(&with_look(settings_with(|s| s.warmth = knob(-1.0)))), neutral);
    assert!(warm[0] > warm[2] + 0.03, "+ теплота не теплее: {warm:?}");
    assert!(cool[2] > cool[0] + 0.03, "− теплота не холоднее: {cool:?}");
    // Знак оттенка: «+» — пурпурнее (зелёного меньше, чем красного и синего).
    let magenta = grey_card(&resolve(&with_look(settings_with(|s| s.tint = knob(1.0)))), neutral);
    let green = grey_card(&resolve(&with_look(settings_with(|s| s.tint = knob(-1.0)))), neutral);
    assert!(
        magenta[1] < 0.5 * (magenta[0] + magenta[2]) - 0.01,
        "+ оттенок не пурпурный: {magenta:?}"
    );
    assert!(
        green[1] > 0.5 * (green[0] + green[2]) + 0.01,
        "− оттенок не зелёный: {green:?}"
    );
}

#[test]
fn warmth_equals_rendering_with_a_manually_shifted_white() {
    let model = LookModelSettings::default();
    let scene = resolve(&controls());
    let scene_white = scene.color.white().white;
    // Тот же сдвиг, посчитанный вручную: майреды вниз на край диапазона, затем нейтраль профиля.
    let shifted = shift_white(
        scene_white,
        WhiteShift {
            mired: -model.warmth_range_mired,
            duv: 0.0,
        },
    )
    .unwrap()
    .white;
    let neutral = sensor().color_profile().neutral_for_white(shifted).unwrap();
    let by_hand = resolve(&ProcessingControls {
        white_balance: neutral,
        ..controls()
    });
    let by_knob = resolve(&with_look(settings_with(|s| s.warmth = knob(1.0))));
    for (left, right) in by_hand
        .white_balance
        .as_array()
        .iter()
        .zip(by_knob.white_balance.as_array())
    {
        assert!((left - right).abs() < 2e-4, "усиления {left} против {right}");
    }
    // Тот же серый предмет сцены (нейтраль AWB лампы), нарисованный с ручным сдвигом белой.
    let card_hand = grey_card(&by_hand, lamp_neutral());
    let card_knob = grey_card(&by_knob, lamp_neutral());
    // Цвет серого предмета у ручки и у ручного сдвига совпадает (допуск — обратная итерация белой точки DNG).
    for channel in 0..3 {
        assert!(
            (card_hand[channel] - card_knob[channel]).abs() < 3e-3,
            "{card_hand:?} против {card_knob:?}"
        );
    }
    // Размер сдвига: серый стал теплее ровно настолько же, насколько у ручного.
    assert!((card_knob[0] - card_knob[2]) > 0.03);
}

#[test]
fn warmth_and_tint_are_invisible_to_ae_and_to_the_scene_white() {
    let honest = resolve(&controls());
    for settings in [
        settings_with(|s| s.warmth = knob(1.0)),
        settings_with(|s| s.warmth = knob(-1.0)),
        settings_with(|s| s.tint = knob(0.8)),
        settings_with(|s| {
            s.warmth = knob(0.6);
            s.tint = knob(-0.6);
        }),
    ] {
        let looked = resolve(&with_look(settings));
        // AE: веса яркости идентичны честным.
        assert_eq!(looked.scene_luminance_weights(), honest.scene_luminance_weights());
        // Белая точка сцены (диагностика AWB/DNG) — несдвинутая.
        assert_eq!(looked.color.white(), honest.color.white());
        // Не пусто: рисование при этом изменилось.
        assert_ne!(looked.white_balance, honest.white_balance);
    }
}

#[test]
fn neutral_look_gives_the_same_params_as_no_look_at_all() {
    let explicit = with_look(LookSettings::NEUTRAL);
    assert_eq!(resolve(&explicit), resolve(&controls()));
    assert!(resolve(&explicit).look.is_neutral());
}

#[test]
fn shift_at_the_table_edge_is_reported_and_does_not_fail_the_frame() {
    // Очень тёплый свет (≈ 1800 К) и «холоднее» ещё на 60 майред выходит за последнюю линию таблицы.
    let warm_white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(1800.0),
        duv: Duv(0.0),
    })
    .unwrap();
    let neutral = sensor().color_profile().neutral_for_white(warm_white).unwrap();
    let controls = ProcessingControls {
        white_balance: neutral,
        ..with_look(settings_with(|s| s.warmth = knob(-1.0)))
    };
    let params = resolve(&controls);
    assert!(matches!(
        params.look.report.white_shift_limit,
        WhiteShiftLimit::MiredClamped { .. }
    ));
    assert_eq!(params.look.report.unknown_band_ids, Vec::new());
}

/// Белая AWB краснее последней линии таблицы Робертсона: сдвиг посчитать нельзя. Ручка не должна стоить кадра.
fn off_table_neutral() -> CameraNeutral {
    let white = crate::color::chromaticity::Chromaticity { x: 0.60, y: 0.38 };
    sensor().color_profile().neutral_for_white(white).unwrap()
}

#[test]
fn warmth_on_a_white_outside_the_table_renders_unshifted_and_reports_why() {
    let neutral = off_table_neutral();
    let honest = resolve(&ProcessingControls {
        white_balance: neutral,
        ..controls()
    });
    for warmth in [1.0, -1.0] {
        let looked = resolve(&ProcessingControls {
            white_balance: neutral,
            ..with_look(settings_with(|s| s.warmth = knob(warmth)))
        });
        // Рисование = как без теплоты (кадр не потерян и не сдвинут).
        assert_eq!(looked.white_balance, honest.white_balance);
        assert_eq!(looked.color, honest.color);
        // Причина различима в отчёте вида, а не молчание и не строка.
        assert!(
            matches!(
                looked.look.report.white_shift_limit,
                WhiteShiftLimit::NotApplied(WhiteShiftSkipReason::ShiftOutsideTable(
                    WhitePointError::ChromaticityOutOfTable { .. }
                ))
            ),
            "{:?}",
            looked.look.report.white_shift_limit
        );
    }
    // Та же белая без вида: прежний результат, отчёт чист.
    assert_eq!(honest.look.report.white_shift_limit, WhiteShiftLimit::Unlimited);
}
