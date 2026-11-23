//! Основа ACES: каждый шаг формулы против независимого эталона в f64 (`aces_reference`), обязательные свойства
//! решений 72 и 177, ручки тона, проверка настроек, выгрузка для шейдера.

use std::sync::Arc;

use super::reference::{AP0_TO_AP1, AP0_TO_XYZ, AP1_TO_XYZ, CtlSpline, Reference, sweeteners as reference_sweeteners};
use super::settings::AcesSettings;
use super::spline::SplineSpec;
use super::sweeteners::{SweetenerParameters, apply_sweeteners, atan2_polynomial};
use super::{AP0, AP1, AcesBase, AcesMatrices, AcesParameters, aces_splines};
use crate::color::matrix::PixelMatrix;
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;

pub(super) fn default_base() -> Arc<AcesBase> {
    AcesBase::resolve(&AcesSettings::default(), DisplayGamut::Srgb).unwrap()
}

pub(super) fn base_for(settings: &AcesSettings) -> Arc<AcesBase> {
    AcesBase::resolve(settings, DisplayGamut::Srgb).unwrap()
}

pub(super) fn grey_pixel(value: f32) -> [f32; 3] {
    [value; 3]
}

/// Свет нейтрали на `e` ступенях над серым сцены.
pub(super) fn grey_at(settings: &AcesSettings, e: f32) -> f32 {
    settings.scene_middle_gray * e.exp2()
}

fn spread(pixel: [f32; 3]) -> f32 {
    pixel[0].max(pixel[1]).max(pixel[2]) - pixel[0].min(pixel[1]).min(pixel[2])
}

// ---- Сплайны: опубликованные опорные значения и независимый разбор CTL ----

#[test]
fn splines_hit_the_published_anchor_points() {
    let (rrt, odt) = aces_splines();
    // RRT: серая карта 0,18 → 4,8 нит; крайние точки — 0,0001 и 10 000 нит (`RRT_PARAMS`).
    assert!((rrt.forward(0.18) - 4.8).abs() < 1e-8);
    // minPoint лежит ниже зажима входа HALF_MIN (как в CTL), поэтому проверяем в логарифмах.
    assert!((rrt.forward_log10((0.18 * (-15.0_f64).exp2()).log10()) + 4.0).abs() < 1e-9);
    assert!((rrt.forward(0.18 * 18.0_f64.exp2()) / 10_000.0 - 1.0).abs() < 1e-9);
    // ODT 48 нит: точки задаются значениями RRT и дают 0,02 / 4,8 / 48 нит (`ODT_48nits`).
    let at = |ev: f64| odt.forward(rrt.forward(0.18 * ev.exp2()));
    assert!((at(-6.5) - 0.02).abs() < 1e-9, "{}", at(-6.5));
    assert!((at(0.0) - 4.8).abs() < 1e-8, "{}", at(0.0));
    assert!((at(6.5) / 48.0 - 1.0).abs() < 1e-9, "{}", at(6.5));
    // Серая карта через всю цепочку ACES до поправки окружения: (4,8 − 0,02)/(48 − 0,02) = 0,099625.
    let normalised = (at(0.0) - 0.02) / (48.0 - 0.02);
    assert!((normalised - 0.099_625).abs() < 1e-6, "{normalised}");
}

#[test]
fn closed_form_splines_match_the_independent_ctl_port() {
    let (rrt, odt) = aces_splines();
    let ctl_rrt = CtlSpline::c5();
    let ctl_odt = CtlSpline::c9(&ctl_rrt);
    // C9 строится по значениям C5: сами точки сходятся.
    for (ctl_point, ev) in [(ctl_odt.min(), -6.5_f64), (ctl_odt.mid(), 0.0), (ctl_odt.max(), 6.5)] {
        let expected = rrt.forward(0.18 * ev.exp2());
        assert!((ctl_point.0 / expected - 1.0).abs() < 1e-9, "точка C9 на {ev} ступенях");
    }
    for step in -220..=140 {
        let x = 0.18 * (f64::from(step) / 10.0).exp2();
        let ours = rrt.forward(x);
        let expected = ctl_rrt.forward(x);
        assert!(
            (ours / expected - 1.0).abs() < 1e-9,
            "C5 x = {x}: {ours} против {expected}"
        );
        let ours = odt.forward(expected);
        let expected = ctl_odt.forward(expected);
        assert!(
            (ours / expected - 1.0).abs() < 1e-9,
            "C9 y = {x}: {ours} против {expected}"
        );
    }
}

#[test]
fn reverse_splines_invert_the_forward_ones_and_match_the_ctl_reverse() {
    let (rrt, odt) = aces_splines();
    let ctl_rrt = CtlSpline::c5();
    let ctl_odt = CtlSpline::c9(&ctl_rrt);
    for step in -60..=60 {
        let x = 0.18 * (f64::from(step) / 10.0).exp2();
        let y = rrt.forward(x);
        assert!((rrt.reverse(y) / x - 1.0).abs() < 1e-8, "C5 x = {x}");
        assert!(
            (ctl_rrt.reverse(y) / rrt.reverse(y) - 1.0).abs() < 1e-8,
            "C5 обратная x = {x}"
        );
        let z = odt.forward(y);
        if z > 0.0200001 && z < 47.99 {
            assert!((odt.reverse(z) / y - 1.0).abs() < 1e-7, "C9 y = {y}");
            assert!(
                (ctl_odt.reverse(z) / odt.reverse(z) - 1.0).abs() < 1e-7,
                "C9 обратная y = {y}"
            );
        }
    }
}

#[test]
fn f32_spline_follows_the_f64_spline() {
    let (rrt, odt) = aces_splines();
    let (rrt32, odt32) = (rrt.parameters(), odt.parameters());
    for step in -300..=200 {
        let log_x = f64::from(step) / 100.0 - 1.0;
        let first = rrt32.forward_log10(log_x as f32);
        assert!(
            (f64::from(first) - rrt.forward_log10(log_x)).abs() < 3e-6,
            "C5 lx = {log_x}"
        );
        let log_y = f64::from(step) / 150.0;
        let second = odt32.forward_log10(log_y as f32);
        assert!(
            (f64::from(second) - odt.forward_log10(log_y)).abs() < 3e-6,
            "C9 ly = {log_y}"
        );
    }
}

#[test]
fn spline_rejects_nothing_and_stays_finite_on_extreme_inputs() {
    let (rrt, odt) = aces_splines();
    let (rrt32, odt32) = (rrt.parameters(), odt.parameters());
    for log_x in [-40.0_f32, -4.3, 0.0, 4.3, 40.0] {
        assert!(rrt32.forward_log10(log_x).is_finite());
        assert!(odt32.forward_log10(log_x).is_finite());
    }
    assert!(SplineSpec::rrt().forward(0.0).is_finite());
}

// ---- Матрицы AP0/AP1 ----

fn assert_rows_close(ours: [[f64; 3]; 3], published: [[f64; 3]; 3], tolerance: f64, what: &str) {
    for row in 0..3 {
        for column in 0..3 {
            assert!(
                (ours[row][column] - published[row][column]).abs() < tolerance,
                "{what}[{row}][{column}]: {} против {}",
                ours[row][column],
                published[row][column]
            );
        }
    }
}

#[test]
fn derived_aces_matrices_match_the_published_ones() {
    let ap0 = AP0.rgb_to_xyz().unwrap();
    let ap1 = AP1.rgb_to_xyz().unwrap();
    assert_rows_close(ap0.rows, AP0_TO_XYZ, 2e-8, "AP0→XYZ");
    assert_rows_close(ap1.rows, AP1_TO_XYZ, 2e-8, "AP1→XYZ");
    let matrices = AcesMatrices::resolve(&AcesSettings::default()).unwrap();
    assert_rows_close(matrices.ap0_to_ap1.rows, AP0_TO_AP1, 2e-8, "AP0→AP1");
    // Строка яркости AP1 — вторая строка AP1→XYZ.
    assert_eq!(matrices.ap1_luminance_row, ap1.rows[1]);
}

#[test]
fn white_maps_to_white_through_every_color_matrix() {
    let matrices = AcesMatrices::resolve(&AcesSettings::default()).unwrap();
    for (name, matrix) in [
        ("W→AP0", matrices.working_to_ap0),
        ("AP0→AP1", matrices.ap0_to_ap1),
        ("AP1→W", matrices.ap1_to_working),
    ] {
        let white = matrix.apply([1.0; 3]);
        for channel in white {
            assert!((channel - 1.0).abs() < 1e-12, "{name}: {white:?}");
        }
    }
}

// ---- Свечение и красный модификатор ----

#[test]
fn atan2_polynomial_matches_libm_in_all_quadrants() {
    let mut worst: f64 = 0.0;
    for degrees in 0..720 {
        let angle = f64::from(degrees) * 0.5_f64.to_radians() - std::f64::consts::PI;
        for radius in [1e-3_f64, 1.0, 40.0] {
            let (y, x) = (radius * angle.sin(), radius * angle.cos());
            let ours = f64::from(atan2_polynomial(y as f32, x as f32));
            worst = worst.max((ours - y.atan2(x)).abs());
        }
    }
    assert!(worst < 3e-7, "наибольшая ошибка {worst} рад");
    assert_eq!(atan2_polynomial(0.0, 0.0), 0.0);
}

#[test]
fn sweeteners_match_the_independent_reference_and_are_not_vacuous() {
    let settings = AcesSettings::default();
    let parameters = SweetenerParameters {
        glow_gain: settings.glow_gain,
        red_modifier_scale: settings.red_modifier_scale,
    };
    // Насыщенный тёмный красный (свечение работает), красно-оранжевый средний (красный модификатор),
    // кожа, зелёный и синий вне зоны красного, почти серый.
    let colours: [[f32; 3]; 7] = [
        [0.06, 0.004, 0.003],
        [0.40, 0.12, 0.03],
        [0.35, 0.22, 0.17],
        [0.05, 0.30, 0.04],
        [0.03, 0.05, 0.45],
        [0.18, 0.1795, 0.181],
        [0.02, 0.01, 0.015],
    ];
    for colour in colours {
        let ours = apply_sweeteners(&parameters, colour);
        let expected = reference_sweeteners(&settings, colour.map(f64::from));
        for channel in 0..3 {
            assert!(
                (f64::from(ours[channel]) - expected[channel]).abs() < 2e-6,
                "{colour:?}: канал {channel}: {ours:?} против {expected:?}"
            );
        }
    }
    let glow = apply_sweeteners(&parameters, colours[0]);
    assert!(
        glow[1] > colours[0][1] * 1.01,
        "свечение не подняло насыщенный тёмный цвет: {glow:?}"
    );
    let red = apply_sweeteners(&parameters, colours[1]);
    assert!(
        red[0] < colours[1][0] * 0.95,
        "красный модификатор не уменьшил красный: {red:?}"
    );
    // Отключённые числа делают шаг тождественным.
    let off = SweetenerParameters {
        glow_gain: 0.0,
        red_modifier_scale: 1.0,
    };
    for colour in colours {
        assert_eq!(apply_sweeteners(&off, colour), colour);
    }
}

#[test]
fn sweeteners_leave_exact_neutrals_untouched() {
    let parameters = SweetenerParameters {
        glow_gain: 0.05,
        red_modifier_scale: 0.82,
    };
    for value in [0.0_f32, 1e-6, 0.02, 0.18, 4.0, 400.0] {
        assert_eq!(apply_sweeteners(&parameters, [value; 3]), [value; 3], "{value}");
    }
}

// ---- Серый, белый точки, нейтраль ----

#[test]
fn middle_gray_lands_on_the_configured_display_gray_for_every_exposure_gain_and_gray() {
    for display_gray in [0.1_f32, 0.2, 0.3] {
        for gain in [0.7_f32, 1.0, 1.41, 2.0] {
            let settings = AcesSettings {
                display_middle_gray: display_gray,
                ..AcesSettings::default()
            };
            let lifted = settings.with_exposure_gain(ExposureGain::new(gain).unwrap());
            let base = base_for(&lifted);
            let output = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray / gain), 1.0);
            for channel in output {
                assert!(
                    (channel - display_gray).abs() < 3e-6,
                    "серый {display_gray}, k = {gain}: {output:?}"
                );
            }
        }
    }
}

#[test]
fn closed_form_exposure_shift_agrees_with_the_bisected_reference() {
    for display_gray in [0.1_f32, 0.2, 0.35] {
        let settings = AcesSettings {
            display_middle_gray: display_gray,
            ..AcesSettings::default()
        };
        let shift = f64::from(base_for(&settings).tone_scale().shift_ev);
        let reference = Reference::new(&settings);
        assert!(
            (shift - reference.shift_ev).abs() < 2e-6,
            "{shift} против {}",
            reference.shift_ev
        );
    }
    // Чистый ACES даёт серой карте ≈ 0,104, поэтому серому 0,20 нужен положительный сдвиг (около +0,67 ступени).
    let shift = default_base().tone_scale().shift_ev;
    assert!(shift > 0.6 && shift < 0.75, "{shift}");
}

#[test]
fn grey_stays_grey_over_the_whole_range() {
    let settings = AcesSettings::default();
    let base = default_base();
    for step in -120..=60 {
        let value = grey_at(&settings, step as f32 / 10.0);
        for white in [1.0, 4.0] {
            let working = base.map_scene_pixel(grey_pixel(value), white);
            assert!(spread(working) < 3e-6, "W: {value}, белый {white}: {working:?}");
            let output = base.to_output(working);
            assert!(spread(output) < 3e-6, "выход: {value}, белый {white}: {output:?}");
        }
    }
}

#[test]
fn point_white_maps_to_exactly_one_for_any_level_and_grey_does_not_depend_on_it() {
    let settings = AcesSettings::default();
    let base = default_base();
    let grey_alone = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray), 1.0);
    for white in [1.0_f32, 1.3, 2.0, 4.0, 8.0, 16.0, 40.0, 1000.0] {
        let at_white = base.map_scene_pixel(grey_pixel(white), white);
        for channel in at_white {
            assert!((channel - 1.0).abs() < 3e-6, "белый {white}: {at_white:?}");
        }
        // Пересвет ярче белого точки — тоже белый (кривая плоская), а не выброс.
        let above = base.map_scene_pixel(grey_pixel(white * 1.5), white);
        for channel in above {
            assert!((channel - 1.0).abs() < 3e-6, "выше белого {white}: {above:?}");
        }
        // Серый не зависит от белого: арифметика одной и той же ветки, поэтому ровно бит в бит.
        assert_eq!(
            base.map_scene_pixel(grey_pixel(settings.scene_middle_gray), white),
            grey_alone
        );
    }
}

#[test]
fn point_white_is_white_at_every_exposure_gain_and_output_gamut() {
    for gain in [0.7_f32, 1.0, 1.41] {
        let lifted = AcesSettings::default().with_exposure_gain(ExposureGain::new(gain).unwrap());
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let base = AcesBase::resolve(&lifted, gamut).unwrap();
            for white in [1.0_f32, 3.0] {
                let output = base.to_output(base.map_scene_pixel(grey_pixel(white), white));
                for channel in output {
                    assert!(
                        (channel - 1.0).abs() < 3e-6,
                        "k = {gain}, {gamut:?}, белый {white}: {output:?}"
                    );
                }
            }
        }
    }
}

pub(super) fn tone_curve(base: &AcesBase, white: f32) -> impl Fn(f32) -> f32 + '_ {
    let tone = *base.tone_scale();
    let white_ev = tone.white_ev(white);
    // Значение канала AP1 на `e` ступенях над серой картой ACES (0,18).
    move |e| tone.linear_cv(0.18 * e.exp2(), white_ev)
}

#[test]
fn curve_is_monotone_for_every_white_and_reaches_one_with_zero_slope() {
    let base = default_base();
    let tone = *base.tone_scale();
    for white in [1.0_f32, 1.5, 3.0, 10.0, 30.0, 1e4] {
        let curve = tone_curve(&base, white);
        let mut previous = curve(-9.0);
        for step in -899..=900 {
            let e = step as f32 / 100.0;
            let value = curve(e);
            assert!(
                value >= previous - 1e-7,
                "белый {white}, e = {e}: {previous} -> {value}"
            );
            assert!(value <= 1.0, "белый {white}, e = {e}: {value}");
            previous = value;
        }
        // Наклон в белом точки равен нулю: гладкий переход в зажим.
        let white_ev = tone.white_ev(white);
        let near = (curve(white_ev) - curve(white_ev - 0.01)) / 0.01;
        assert!(near < 0.01, "белый {white}: наклон у белого {near}");
        assert!(curve(white_ev) == 1.0 && curve(white_ev + 1.0) == 1.0);
    }
}

#[test]
fn curve_is_c1_at_the_knee_and_at_the_middle_gray() {
    let base = default_base();
    let tone = *base.tone_scale();
    for white in [1.0_f32, 4.0] {
        let curve = tone_curve(&base, white);
        // `curve` принимает e в ступенях над 0,18, а колено — в ступенях над серым после формы осей (контраст 1: то же).
        let slope = |centre: f32, step: f32| -> (f32, f32) {
            let left = (curve(centre) - curve(centre - step)) / step;
            let right = (curve(centre + step) - curve(centre)) / step;
            (left, right)
        };
        let (left, right) = slope(tone.knee_ev, 0.01);
        assert!(
            (left - right).abs() < 0.015 && (right - tone.knee_slope).abs() < 0.015,
            "колено, белый {white}: {left} / {right} против {}",
            tone.knee_slope
        );
        let (left, right) = slope(0.0, 0.01);
        assert!((left - right).abs() < 0.012, "серый, белый {white}: {left} / {right}");
    }
}

#[test]
fn highlight_path_into_white_has_no_steps() {
    let base = default_base();
    for white in [1.0_f32, 4.0] {
        let curve = tone_curve(&base, white);
        let mut previous = curve(0.0);
        for step in 1..=700 {
            let value = curve(step as f32 / 100.0);
            // Шаг 0,01 ступени: наибольшая крутизна кривой около 0,3 на ступень.
            assert!(
                value - previous < 0.0035,
                "белый {white}, e = {}: шаг {}",
                step,
                value - previous
            );
            previous = value;
        }
    }
}

#[test]
fn nan_is_black_and_hostile_pixels_never_produce_nan_or_outliers() {
    let base = default_base();
    assert_eq!(base.map_scene_pixel([f32::NAN, 0.5, 0.5], 1.0), [0.0; 3]);
    assert_eq!(base.map_scene_pixel([0.5, f32::NAN, 0.5], 1.0), [0.0; 3]);
    assert_eq!(base.map_scene_pixel([0.0; 3], 1.0), [0.0; 3]);
    assert_eq!(base.map_scene_pixel([-0.3, -1.0, -5.0], 1.0), [0.0; 3]);
    for pixel in [
        [-0.2_f32, 0.1, 0.4],
        [3.0, -1.0, 0.0],
        [1e-30; 3],
        [50.0, 0.0, 0.0],
        [0.0, 1e6, 0.0],
        [f32::INFINITY, 0.0, 0.2],
        [1e30, 1e30, 1e30],
        [f32::MAX, 0.0, 0.0],
    ] {
        let working = base.map_scene_pixel(pixel, 1.0);
        assert!(
            working
                .iter()
                .all(|channel| channel.is_finite() && (-0.5..=1.5).contains(channel)),
            "{pixel:?} -> {working:?}"
        );
        let output = base.to_output(working);
        assert!(
            output.iter().all(|channel| (0.0..=1.0).contains(channel)),
            "{pixel:?} -> {output:?}"
        );
    }
}

// ---- Весь пиксель против независимого эталона f64 ----

fn assert_matches_reference(settings: &AcesSettings, tolerance: f64) {
    let base = base_for(settings);
    let reference = Reference::new(settings);
    let grey = settings.scene_middle_gray;
    let colours: [[f32; 3]; 9] = [
        [0.5, 0.5, 0.5],
        [0.9, 0.3, 0.1],
        [0.3, 0.6, 0.2],
        [0.1, 0.2, 0.9],
        [0.6, 0.45, 0.35],
        [0.02, 0.01, 0.005],
        [0.8, 0.8, 0.2],
        [0.05, 0.3, 0.3],
        [1.0, 1.0, 1.0],
    ];
    for colour in colours {
        for exposure in [-4.0_f32, -1.0, 0.0, 1.0, 2.0, 3.0] {
            for white in [1.0_f32, 2.5, 6.0] {
                let scene = colour.map(|channel| channel * grey * exposure.exp2());
                let ours = base.map_scene_pixel(scene, white);
                let expected = reference.pixel(scene.map(f64::from), f64::from(white));
                for channel in 0..3 {
                    assert!(
                        (f64::from(ours[channel]) - expected[channel]).abs() < tolerance,
                        "{colour:?} e={exposure} L={white} канал {channel}: {ours:?} против {expected:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn whole_pixel_matches_the_independent_f64_chain_with_default_settings() {
    assert_matches_reference(&AcesSettings::default(), 3e-5);
}

#[test]
fn whole_pixel_matches_the_independent_f64_chain_with_knobs_and_other_settings() {
    assert_matches_reference(
        &AcesSettings {
            contrast: 1.2,
            shadows_ev: 0.8,
            highlights_ev: -0.7,
            display_middle_gray: 0.25,
            knee_ev: 1.0,
            glow_gain: 0.1,
            red_modifier_scale: 0.6,
            rrt_saturation: 0.9,
            odt_saturation: 0.85,
            surround_gamma: 1.0,
            ..AcesSettings::default()
        },
        4e-5,
    );
}

// ---- Выгрузка для шейдера ----

/// То, что делает шейдер: читает ТОЛЬКО числа выгрузки (повторяет порядок формул цепочки модуля).
fn shader_pixel(parameters: &AcesParameters, scene: [f32; 3], white: f32) -> [f32; 3] {
    let apply = |rows: &[[f32; 3]; 3], pixel: [f32; 3]| PixelMatrix { rows: *rows }.apply(pixel);
    let aces = scene.map(|channel| (channel * parameters.exposure_scale).clamp(0.0, 65504.0));
    let sweetened = apply_sweeteners(&parameters.sweeteners, apply(&parameters.working_to_ap0, aces));
    let ap1 = apply(
        &parameters.ap0_to_ap1,
        sweetened.map(|channel| channel.clamp(0.0, 65504.0)),
    )
    .map(|channel| channel.clamp(0.0, 65504.0));
    let row = parameters.ap1_luminance;
    let luminance = row[0] * ap1[0] + row[1] * ap1[1] + row[2] * ap1[2];
    let ap1 = ap1.map(|channel| parameters.rrt_saturation * channel + (1.0 - parameters.rrt_saturation) * luminance);
    let white_ev = parameters.tone.white_ev(white);
    let cv = ap1.map(|channel| parameters.tone.linear_cv(channel, white_ev));
    let luminance = row[0] * cv[0] + row[1] * cv[1] + row[2] * cv[2];
    let factor = if luminance <= super::SURROUND_BLACK_LUMINANCE {
        1.0
    } else {
        luminance.powf(parameters.surround_exponent)
    };
    apply(&parameters.ap1_to_working, cv.map(|channel| channel * factor))
}

#[test]
fn gpu_snapshot_reproduces_the_formula_bit_for_bit() {
    let settings = AcesSettings {
        contrast: 1.1,
        shadows_ev: 0.5,
        highlights_ev: -0.4,
        ..AcesSettings::default()
    };
    let base = base_for(&settings);
    let parameters = base.parameters();
    for colour in [[0.9_f32, 0.3, 0.1], [0.2, 0.2, 0.2], [0.04, 0.5, 0.1], [3.0, 2.0, 1.0]] {
        for scale in [0.02_f32, 0.3, 1.0, 4.0] {
            for white in [1.0_f32, 2.0, 7.5] {
                let scene = colour.map(|channel| channel * scale * 0.075);
                assert_eq!(
                    base.map_scene_pixel(scene, white),
                    shader_pixel(&parameters, scene, white),
                    "{scene:?}, белый {white}"
                );
            }
        }
    }
    // Выгрузка содержит ровно те числа, что считают модель и общий шаг выхода.
    assert_eq!(parameters.exposure_scale, 0.18 / settings.scene_middle_gray);
    assert_eq!(parameters.tone.grey, settings.scene_middle_gray);
    assert!(parameters.input_compression.is_none());
    let working = [0.4_f32, 0.2, 0.1];
    let expected = PixelMatrix {
        rows: parameters.working_to_output,
    }
    .apply(working);
    let output = base.to_output(working);
    for channel in 0..3 {
        assert!(
            (output[channel] - expected[channel].clamp(0.0, 1.0)).abs() < 1e-6,
            "{output:?} против {expected:?}"
        );
    }
}

#[test]
fn exposure_gain_moves_the_grey_but_leaves_the_point_white_white() {
    let settings = AcesSettings::default();
    let lifted = settings.with_exposure_gain(ExposureGain::new(2.0).unwrap());
    let base = base_for(&lifted);
    let at_new_grey = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray / 2.0), 1.0);
    assert!(
        at_new_grey
            .iter()
            .all(|c| (c - settings.display_middle_gray).abs() < 3e-6),
        "{at_new_grey:?}"
    );
    let at_old_grey = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray), 1.0);
    assert!(
        at_old_grey[0] > settings.display_middle_gray + 0.05,
        "светлее прежнего серого"
    );
    // Без подтяжки настройки не меняются на бит.
    assert_eq!(settings.with_exposure_gain(ExposureGain::UNITY), settings);
}

#[test]
fn default_settings_survive_the_extreme_residual_ae_gains() {
    for gain in [0.5_f32.exp2().recip(), 0.5_f32.exp2()] {
        let adjusted = AcesSettings::default().with_exposure_gain(ExposureGain::new(gain).unwrap());
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            assert!(AcesBase::resolve(&adjusted, gamut).is_ok(), "k = {gain}");
        }
    }
}
