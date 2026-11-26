//! Упоры теплоты и оттенка (решения 288, 293) на настоящем DNG-профиле телефона: упор есть только там, где картинка
//! правда перестаёт меняться (упор в пригодное), и по построению совпадает с началом урезания в `shifted_neutral`.
//! Сжатый ход холодного света упором НЕ считается.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "интеграционный тест: падение — способ сообщить о провале"
)]

mod support;

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::{CameraNeutral, DngCalibration, DngColorProfile};
use isp_model::color::matrix::Matrix3;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::look::model::LookModelSettings;
use isp_model::look::white::{RenderingNeutral, WhiteShiftLimit, WhiteShiftSkipReason, shifted_neutral};
use isp_model::look::white_shift;
use isp_model::look::white_stops::{TravelStop, WhiteKnobStops, white_knob_stops};
use look_contract::{LookKnob, LookSettings};
use support::assets_dir;
use support::real_capture::load_real_capture;

/// Свет сцены без Duv: белая и нейтраль камеры по профилю.
fn scene_at(profile: &DngColorProfile, kelvin: f64) -> (Chromaticity, CameraNeutral) {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .unwrap();
    (white, profile.neutral_for_white(white).unwrap())
}

fn settings(warmth: f32, tint: f32) -> LookSettings {
    LookSettings {
        warmth: LookKnob::new(warmth).unwrap(),
        tint: LookKnob::new(tint).unwrap(),
        ..LookSettings::NEUTRAL
    }
}

/// Упоры при данных ручках на свете `kelvin` по профилю фикстуры телефона.
fn stops_on_phone(kelvin: f64, warmth: f32, tint: f32) -> WhiteKnobStops {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-towel");
    let profile = capture.sensor.color_profile();
    let (white, scene) = scene_at(profile, kelvin);
    white_knob_stops(
        profile,
        white,
        scene,
        &LookModelSettings::default(),
        &settings(warmth, tint),
    )
    .unwrap()
}

/// То, что рисует кадр при этих ручках: исход урезания для того же света и модели.
fn rendering_on_phone(kelvin: f64, warmth: f32, tint: f32) -> RenderingNeutral {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-towel");
    let profile = capture.sensor.color_profile();
    let (white, scene) = scene_at(profile, kelvin);
    let shift = white_shift(&LookModelSettings::default(), &settings(warmth, tint));
    shifted_neutral(profile, white, scene, shift).unwrap()
}

fn stop_fraction(stop: TravelStop) -> f32 {
    match stop {
        TravelStop::At(fraction) => fraction.get(),
        other => panic!("ожидали упор, а не {other:?}"),
    }
}

/// Наибольшее расхождение каналов нейтрали (относительное к зелёному-нормировке не нужно: каналы одного порядка).
fn neutral_distance(first: &RenderingNeutral, second: &RenderingNeutral) -> f64 {
    let (first, second) = (first.neutral.values(), second.neutral.values());
    (0..3)
        .map(|index| (first[index] - second[index]).abs())
        .fold(0.0, f64::max)
}

#[test]
fn neutral_daylight_has_no_stops_on_either_knob() {
    let stops = stops_on_phone(5500.0, 0.0, 0.0);
    for side in [stops.warmth.minus, stops.warmth.plus, stops.tint.minus, stops.tint.plus] {
        assert_eq!(side, TravelStop::Unbounded, "{stops:?}");
    }
}

/// Холодный свет сжимает ход теплоты к краю таблицы — но картинка меняется до края шкалы, значит, упора нет, и это
/// видно по самому кадру: нейтраль на краю отличается от нейтрали чуть раньше.
#[test]
fn compressed_travel_of_cold_light_is_not_a_stop() {
    for kelvin in [8000.0, 12_000.0] {
        let stops = stops_on_phone(kelvin, 0.0, 0.0);
        assert_eq!(stops.warmth.plus, TravelStop::Unbounded, "{kelvin} К: {stops:?}");
        let at_edge = rendering_on_phone(kelvin, 1.0, 0.0);
        assert!(
            matches!(at_edge.limit, WhiteShiftLimit::TravelCompressed { .. }),
            "{kelvin} К: ход должен быть сжат: {:?}",
            at_edge.limit
        );
        let before_edge = rendering_on_phone(kelvin, 0.95, 0.0);
        assert!(
            neutral_distance(&at_edge, &before_edge) > 1e-4,
            "{kelvin} К: картинка у края шкалы уже стоит на месте"
        );
        for side in [stops.warmth.minus, stops.tint.minus, stops.tint.plus] {
            assert_eq!(side, TravelStop::Unbounded, "{kelvin} К: {stops:?}");
        }
    }
}

/// Тёплая лампа, теплота в минус, оттенок к краю («+»): упор оттенка есть, до него сдвиг применён целиком, после —
/// урезан, а картинка дальше упора не меняется.
#[test]
fn warm_lamp_tint_stop_splits_full_shift_from_cut_back_and_freezes_the_picture() {
    let stops = stops_on_phone(2000.0, -1.0, 0.0);
    assert_eq!(stops.tint.minus, TravelStop::Unbounded);
    let stop = stop_fraction(stops.tint.plus);
    assert!(
        (0.3..0.4).contains(&stop),
        "для лампы 2000 К при теплоте −1 ждали около 0,34: {stop}"
    );

    let below = rendering_on_phone(2000.0, -1.0, stop - 0.01);
    assert!(
        !matches!(below.limit, WhiteShiftLimit::ReducedToUsable { .. }),
        "ниже упора сдвиг должен применяться целиком: {:?}",
        below.limit
    );
    let at_stop = rendering_on_phone(2000.0, -1.0, stop);
    assert!(
        !matches!(at_stop.limit, WhiteShiftLimit::ReducedToUsable { .. }),
        "{:?}",
        at_stop.limit
    );

    // Сразу за упором — урезано, и дальше до края шкалы картинка одна и та же (точность поиска 2⁻¹⁴ хода).
    let after = rendering_on_phone(2000.0, -1.0, stop + 0.02);
    assert!(
        matches!(after.limit, WhiteShiftLimit::ReducedToUsable { .. }),
        "{:?}",
        after.limit
    );
    let at_edge = rendering_on_phone(2000.0, -1.0, 1.0);
    assert!(matches!(at_edge.limit, WhiteShiftLimit::ReducedToUsable { .. }));
    assert!(
        neutral_distance(&after, &at_stop) < 1e-3,
        "за упором картинка поехала от упора"
    );
    assert!(
        neutral_distance(&at_edge, &at_stop) < 1e-3,
        "на краю шкалы картинка не равна упорной"
    );
    // А ниже упора картинка живая: она отличается от упорной заметно.
    let well_below = rendering_on_phone(2000.0, -1.0, stop - 0.15);
    assert!(
        neutral_distance(&well_below, &at_stop) > 5e-3,
        "ниже упора ручка должна менять картинку"
    );
}

/// Упор оттенка зависит от теплоты: чем теплее картинка, тем дальше упор.
#[test]
fn tint_stop_moves_with_the_current_warmth() {
    let cold_side = stop_fraction(stops_on_phone(2000.0, -1.0, 0.0).tint.plus);
    let neutral_warmth = stop_fraction(stops_on_phone(2000.0, 0.0, 0.0).tint.plus);
    assert!(cold_side < neutral_warmth - 0.05, "{cold_side} против {neutral_warmth}");
    let warmest = stop_fraction(stops_on_phone(2000.0, 1.0, 0.0).tint.plus);
    assert!(warmest > neutral_warmth + 0.2, "{warmest} против {neutral_warmth}");
    // На более тёплом свете при той же теплоте упор у оттенка исчезает совсем.
    assert_eq!(stops_on_phone(2400.0, 1.0, 0.0).tint.plus, TravelStop::Unbounded);
}

/// Упор теплоты не зависит от текущего оттенка, а оттенок на упор теплоты не влияет: на настоящем профиле теплота
/// сама по себе не отваливается, поэтому упора у неё нет при любом оттенке.
#[test]
fn warmth_has_no_stop_on_the_phone_profile_whatever_the_tint() {
    for tint in [-1.0, 0.0, 1.0] {
        let stops = stops_on_phone(2000.0, 0.0, tint);
        assert_eq!(stops.warmth.minus, TravelStop::Unbounded, "оттенок {tint}");
        assert_eq!(stops.warmth.plus, TravelStop::Unbounded, "оттенок {tint}");
    }
}

#[test]
fn stops_are_deterministic() {
    assert_eq!(stops_on_phone(2000.0, -0.3, 0.2), stops_on_phone(2000.0, -0.3, 0.2));
}

/// Белая сцены вне таблицы: ручки не делают ничего, и это отдельная причина, а не «упоров нет».
#[test]
fn a_scene_outside_the_table_reports_why_the_knobs_do_nothing() {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-towel");
    let profile = capture.sensor.color_profile();
    let (_, scene) = scene_at(profile, 5500.0);
    let far_blue = Chromaticity { x: 0.05, y: 0.05 };
    let outcome = white_knob_stops(
        profile,
        far_blue,
        scene,
        &LookModelSettings::default(),
        &settings(0.0, 0.0),
    );
    assert!(
        matches!(outcome, Err(WhiteShiftSkipReason::ShiftOutsideTable(_))),
        "{outcome:?}"
    );
}

/// Профиль, у которого красный канал почти не виден у синеватого света: теплота в «+» (белая синее) отваливается
/// ещё без оттенка. На настоящем профиле такого угла нет, поэтому профиль придуман: ColorMatrix диагональная.
fn red_starved_profile() -> DngColorProfile {
    DngColorProfile::new(
        DngCalibration {
            temperature: Kelvin(6500.0),
            color_matrix: Matrix3::diagonal([0.03, 1.0, 1.0]),
            forward_matrix: None,
            camera_calibration: Matrix3::diagonal([1.0, 1.0, 1.0]),
        },
        None,
        [1.0, 1.0, 1.0],
    )
    .unwrap()
}

/// Теплота с настоящим упором: дальше него картинка не меняется (оттенок обнулён урезанием), а оттенок при теплоте
/// за упором мёртв с самого нуля.
#[test]
fn warmth_stop_freezes_the_picture_and_kills_tint_beyond_it() {
    let profile = red_starved_profile();
    let model = LookModelSettings::default();
    let (white, scene) = scene_at(&profile, 2850.0);
    let stops_for =
        |warmth: f32, tint: f32| white_knob_stops(&profile, white, scene, &model, &settings(warmth, tint)).unwrap();
    let render = |warmth: f32, tint: f32| {
        shifted_neutral(&profile, white, scene, white_shift(&model, &settings(warmth, tint))).unwrap()
    };

    let stops = stops_for(0.0, 0.0);
    assert_eq!(stops.warmth.minus, TravelStop::Unbounded, "{stops:?}");
    let stop = stop_fraction(stops.warmth.plus);
    assert!((0.1..0.9).contains(&stop), "{stop}");
    // Упор теплоты не зависит от оттенка.
    assert_eq!(stops_for(0.0, 0.7).warmth, stops.warmth);

    let below = render(stop - 0.02, 0.0);
    assert!(
        !matches!(below.limit, WhiteShiftLimit::ReducedToUsable { .. }),
        "{:?}",
        below.limit
    );
    let after = render(stop + 0.02, 0.0);
    assert!(
        matches!(after.limit, WhiteShiftLimit::ReducedToUsable { .. }),
        "{:?}",
        after.limit
    );
    let at_edge = render(1.0, 0.0);
    assert!(
        neutral_distance(&at_edge, &after) < 1e-3,
        "за упором теплоты картинка поехала"
    );
    let well_below = render(stop - 0.1, 0.0);
    assert!(
        neutral_distance(&well_below, &after) > 1e-3,
        "ниже упора теплота должна менять картинку"
    );

    // Теплота за упором: оттенок не меняет картинку вовсе — мёртв с нуля.
    let beyond = stops_for(1.0, 0.0);
    assert_eq!(beyond.tint.plus, TravelStop::DeadFromCentre, "{beyond:?}");
    assert_eq!(beyond.tint.minus, TravelStop::DeadFromCentre, "{beyond:?}");
    assert!(neutral_distance(&render(1.0, 0.0), &render(1.0, 1.0)) < 1e-3);
}
