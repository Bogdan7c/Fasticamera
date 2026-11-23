//! Основа filmic: каждый участок кривой по формуле против независимого расчёта, свойства решений 72 и 177,
//! условия разрешимости, ручки, экспорт чисел для шейдера. Сломанная формула роняет тест.

use std::sync::Arc;

use super::*;
use crate::exposure::ExposureGain;

pub(super) const ALL_NORMS: [FilmicNorm; 4] = [
    FilmicNorm::PerChannel,
    FilmicNorm::MaxRgb,
    FilmicNorm::Luminance,
    FilmicNorm::PowerNorm,
];

pub(super) fn base_with(settings: FilmicSettings) -> Arc<FilmicBase> {
    FilmicBase::resolve(&settings, DisplayGamut::Srgb).unwrap()
}

pub(super) fn base_for_norm(norm: FilmicNorm) -> Arc<FilmicBase> {
    base_with(FilmicSettings {
        norm,
        ..FilmicSettings::default()
    })
}

/// Опорная функция `E(u, p)` из доки модуля.
fn basis(u: f64, p: f64) -> f64 {
    u / (1.0 + u.powf(p)).powf(1.0 / p)
}

/// Какой участок кривой посчитан (для проверки покрытия сетки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Segment {
    Black,
    Toe,
    Linear,
    Shoulder,
    White,
}

/// Кривая из доки модуля в её ПЕРВОНАЧАЛЬНОМ виде (через `k = s·w/(r^p − 1)^(1/p)` и `E(u, p)`) в f64 — форма, не
/// совпадающая с рабочей (там `k` убран подстановкой).
fn reference_curve(settings: &FilmicSettings, light: f64, white_level: f64) -> (f64, Segment) {
    let slope = f64::from(settings.slope);
    let gamma = f64::from(settings.display_gamma);
    let grey = f64::from(settings.scene_middle_gray);
    let y_p = f64::from(settings.display_middle_gray).powf(1.0 / gamma);
    let e = (light / grey).log2();
    let delta_white = (white_level.max(1.0) / grey).log2();
    let delta_black = f64::from(settings.black_depth_ev);
    if e >= delta_white {
        return (1.0, Segment::White);
    }
    let toe_share = f64::from(settings.toe_linear_share);
    let shoulder_share = f64::from(settings.shoulder_linear_share);
    let below = toe_share * y_p / slope;
    let above = shoulder_share * (1.0 - y_p) / slope;
    if e > above {
        let value = y_p + shoulder_share * (1.0 - y_p);
        let headroom = 1.0 - value;
        let width = delta_white - above;
        let power = f64::from(settings.shoulder_power);
        let ratio = slope * width / headroom;
        let knee = slope * width / (ratio.powf(power) - 1.0).powf(1.0 / power);
        (
            value + knee * basis(slope * (e - above) / knee, power),
            Segment::Shoulder,
        )
    } else if e >= -below {
        (y_p + slope * e, Segment::Linear)
    } else {
        let value = (1.0 - toe_share) * y_p;
        let width = delta_black - below;
        let depth = -below - e;
        if depth >= width {
            return (0.0, Segment::Black);
        }
        let power = f64::from(settings.toe_power);
        let ratio = slope * width / value;
        let knee = slope * width / (ratio.powf(power) - 1.0).powf(1.0 / power);
        (value - knee * basis(slope * depth / knee, power), Segment::Toe)
    }
}

#[test]
fn every_curve_segment_matches_the_independent_f64_calculation() {
    let settings = FilmicSettings::default();
    let base = base_with(settings);
    let grey = f64::from(settings.scene_middle_gray);
    let mut counts = std::collections::HashMap::new();
    for white_level in [1.0_f32, 1.5, 3.2] {
        for step in 0..=1450 {
            let e = -9.5 + f64::from(step) * 0.01;
            // Свет кодируется в f32 так же, как приходит в основу; ссылка берёт ровно то же число.
            let light = (grey * e.exp2()) as f32;
            let (expected, segment) = reference_curve(&settings, f64::from(light), f64::from(white_level));
            let actual = f64::from(base.curve_value(light, white_level));
            // Допуск — округление f32 (логарифм, две степени, деление), на порядок выше шума f32 и на два
            // ниже любой ошибки формулы (перепутанная степень даёт ≥ 1e-2).
            assert!(
                (actual - expected).abs() < 3e-5,
                "L={white_level} e={e:.2} {segment:?}: {actual} против {expected}"
            );
            *counts.entry(segment).or_insert(0_u32) += 1;
        }
    }
    // Тест не пустой: каждый участок проверен на сотнях точек.
    for segment in [
        Segment::Black,
        Segment::Toe,
        Segment::Linear,
        Segment::Shoulder,
        Segment::White,
    ] {
        assert!(
            counts.get(&segment).copied().unwrap_or(0) > 100,
            "{segment:?}: {counts:?}"
        );
    }
}

#[test]
fn grey_maps_to_the_requested_display_grey_for_every_norm_and_gain() {
    // Серый сцены с подтяжкой AE `k` (решение 72): свет `scene_middle_gray / k` становится серым экрана.
    for norm in ALL_NORMS {
        for gain_ev in [-0.5_f32, -0.2, 0.0, 0.3, 0.5] {
            let gain = ExposureGain::from_ev(gain_ev).unwrap();
            let settings = FilmicSettings {
                norm,
                ..FilmicSettings::default()
            };
            let base = base_with(settings.with_exposure_gain(gain));
            let grey = settings.scene_middle_gray / gain.multiplier();
            let display = base.map_scene_pixel([grey; 3], 1.0);
            for channel in display {
                assert!(
                    (channel - settings.display_middle_gray).abs() < 1.5e-6,
                    "{norm:?} k={gain_ev}: {display:?}"
                );
            }
        }
    }
}

#[test]
fn white_of_the_point_maps_to_exactly_one_and_grey_does_not_depend_on_it() {
    for norm in ALL_NORMS {
        let base = base_for_norm(norm);
        for white_level in [1.0_f32, 1.37, 2.0, 4.5] {
            // Свет на белом точки и выше него — ровно белый экрана.
            for factor in [1.0_f32, 1.5, 40.0] {
                let display = base.map_scene_pixel([white_level * factor; 3], white_level);
                assert_eq!(display, [1.0; 3], "{norm:?} L={white_level} ×{factor}");
            }
            // Серый (и всё в линейном участке) от белого не зависит: те же биты.
            let grey = FilmicSettings::default().scene_middle_gray;
            assert_eq!(
                base.map_scene_pixel([grey; 3], white_level),
                base.map_scene_pixel([grey; 3], 1.0),
                "{norm:?} L={white_level}"
            );
        }
    }
}

#[test]
fn grey_stays_grey_from_minus_twelve_to_plus_six_ev_for_every_norm() {
    let grey = f64::from(FilmicSettings::default().scene_middle_gray);
    for norm in ALL_NORMS {
        let base = base_for_norm(norm);
        for step in 0..=360 {
            let e = -12.0 + f64::from(step) * 0.05;
            let level = (grey * e.exp2()) as f32;
            let display = base.map_scene_pixel([level; 3], 1.0);
            let spread =
                display.iter().copied().fold(f32::MIN, f32::max) - display.iter().copied().fold(f32::MAX, f32::min);
            // Включая участок обесцвечивания (OKLab туда и обратно): остаток — округление матриц f32.
            assert!(spread < 1e-5 * display[0].max(1e-3), "{norm:?} e={e:.2}: {display:?}");
        }
    }
}

/// Наибольшая разность значения и наклона в стыке по двум сторонам (шаг `h` ступеней).
fn junction_jump(base: &FilmicBase, grey: f32, join_ev: f64, half_step: f64) -> (f64, f64, f64) {
    let at = |e: f64| f64::from(base.curve_value((f64::from(grey) * e.exp2()) as f32, 1.0));
    let (left, middle, right) = (at(join_ev - half_step), at(join_ev), at(join_ev + half_step));
    let slope_left = (middle - left) / half_step;
    let slope_right = (right - middle) / half_step;
    let curvature = (right - 2.0 * middle + left) / (half_step * half_step);
    (slope_left, slope_right, curvature)
}

#[test]
fn value_and_slope_are_continuous_at_both_junctions() {
    let settings = FilmicSettings::default();
    let base = base_with(settings);
    let (below, above) = base.linear_extent_ev();
    let slope = f64::from(settings.slope);
    for (name, join) in [("носок", -f64::from(below)), ("плечо", f64::from(above))] {
        let (left, right, curvature) = junction_jump(&base, settings.scene_middle_gray, join, 0.01);
        // Наклон с обеих сторон равен `s` (C1); у выбранных степеней `p > 1` ещё и кривизна у стыка мала (C2).
        assert!((left - slope).abs() < 2e-3, "{name}: слева {left}");
        assert!((right - slope).abs() < 2e-3, "{name}: справа {right}");
        assert!(curvature.abs() < 0.05, "{name}: кривизна {curvature}");
    }
}

#[test]
fn junction_check_catches_a_slope_mismatch() {
    // Проверка стыка не пустая: у кривой с «неправильным» наклоном линейного участка (подмена `s` в формуле на
    // другой при том же стыке) скачок наклона виден. Берём стык при другом наклоне: сетка смещена на ступень.
    let settings = FilmicSettings::default();
    let base = base_with(settings);
    let (below, _) = base.linear_extent_ev();
    let (left, right, _) = junction_jump(&base, settings.scene_middle_gray, -f64::from(below) - 1.0, 0.01);
    assert!((left - right).abs() > 2e-3 || (left - f64::from(settings.slope)).abs() > 2e-3);
}

#[test]
fn curve_is_monotone_and_hits_both_targets() {
    let grey = f64::from(FilmicSettings::default().scene_middle_gray);
    let variants = [
        FilmicSettings::default(),
        FilmicSettings {
            toe_power: 0.6,
            shoulder_power: 6.0,
            ..FilmicSettings::default()
        },
        FilmicSettings {
            toe_linear_share: 0.95,
            shoulder_linear_share: 0.05,
            ..FilmicSettings::default()
        },
        FilmicSettings {
            toe_linear_share: 0.05,
            shoulder_linear_share: 0.95,
            slope: 0.3,
            ..FilmicSettings::default()
        },
    ];
    for settings in variants {
        let base = base_with(settings);
        for white_level in [1.0_f32, 2.5] {
            let mut previous = -1.0_f32;
            for step in 0..=2400 {
                let e = -10.0 + f64::from(step) * 0.0075;
                let value = base.curve_value((grey * e.exp2()) as f32, white_level);
                assert!((0.0..=1.0).contains(&value), "{settings:?} e={e}: {value}");
                assert!(
                    value >= previous,
                    "{settings:?} L={white_level} e={e}: {previous} -> {value}"
                );
                previous = value;
            }
            let black = (grey * (-f64::from(settings.black_depth_ev)).exp2()) as f32;
            assert_eq!(base.curve_value(black * 0.999, white_level), 0.0);
            assert_eq!(base.curve_value(white_level, white_level), 1.0);
        }
    }
}

#[test]
fn nan_and_non_positive_light_give_black_for_every_norm() {
    for norm in ALL_NORMS {
        let base = base_for_norm(norm);
        for pixel in [
            [f32::NAN, 0.3, 0.3],
            [0.3, 0.3, f32::NAN],
            [0.0; 3],
            [-0.5; 3],
            [-1.0, -2.0, -0.1],
        ] {
            assert_eq!(base.map_scene_pixel(pixel, 1.0), [0.0; 3], "{norm:?} {pixel:?}");
        }
        // Бесконечность и огромный свет — белый, без NaN.
        for pixel in [[f32::INFINITY, 0.2, 0.0], [1e30, 1e30, 1e30], [3.0e38, 0.0, 0.0]] {
            let display = base.map_scene_pixel(pixel, 1.0);
            assert!(
                display.iter().all(|c| c.is_finite() && (0.0..=1.0 + 1e-5).contains(c)),
                "{norm:?} {pixel:?} -> {display:?}"
            );
        }
    }
}

#[test]
fn unsolvable_shoulder_and_toe_are_distinct_errors_with_the_exact_threshold() {
    let default = FilmicSettings::default();
    let y_p = f64::from(default.display_middle_gray).powf(1.0 / f64::from(default.display_gamma));
    // Плечо: s·Δ_w > 1 − y_p при L = 1.
    let headroom = (1.0 / f64::from(default.scene_middle_gray)).log2();
    let shoulder_threshold = (1.0 - y_p) / headroom;
    let steep_enough = FilmicSettings {
        slope: (shoulder_threshold * 1.001) as f32,
        ..default
    };
    steep_enough.validate().unwrap();
    let too_shallow = FilmicSettings {
        slope: (shoulder_threshold * 0.999) as f32,
        ..default
    };
    match too_shallow.validate() {
        Err(InvalidFilmic::ShoulderTooShallow { minimum_slope, .. }) => {
            assert!(
                (f64::from(minimum_slope) - shoulder_threshold).abs() < 1e-6,
                "{minimum_slope}"
            );
            // Подсказка рабочая: наклон не меньше названного — кривая строится.
            FilmicSettings {
                slope: minimum_slope * 1.0001,
                ..default
            }
            .validate()
            .unwrap();
        }
        other => panic!("ожидалась ShoulderTooShallow, получено {other:?}"),
    }
    // Носок: s·Δ_b > y_p (глубину делаем малой, чтобы условие носка было строже условия плеча).
    let shallow_black = FilmicSettings {
        black_depth_ev: 2.5,
        ..default
    };
    let toe_threshold = y_p / 2.5;
    assert!(toe_threshold > shoulder_threshold);
    FilmicSettings {
        slope: (toe_threshold * 1.001) as f32,
        ..shallow_black
    }
    .validate()
    .unwrap();
    match (FilmicSettings {
        slope: (toe_threshold * 0.999) as f32,
        ..shallow_black
    })
    .validate()
    {
        Err(InvalidFilmic::ToeTooShallow {
            minimum_slope,
            minimum_depth_ev,
            ..
        }) => {
            assert!((f64::from(minimum_slope) - toe_threshold).abs() < 1e-6);
            assert!(minimum_depth_ev > 0.0);
        }
        other => panic!("ожидалась ToeTooShallow, получено {other:?}"),
    }
}

#[test]
fn out_of_range_numbers_name_their_field() {
    let default = FilmicSettings::default();
    let cases: [(FilmicSettings, &str); 8] = [
        (
            FilmicSettings {
                scene_middle_gray: 1.0,
                ..default
            },
            "scene_middle_gray",
        ),
        (
            FilmicSettings {
                display_middle_gray: f32::NAN,
                ..default
            },
            "display_middle_gray",
        ),
        (
            FilmicSettings {
                toe_linear_share: 0.0,
                ..default
            },
            "toe_linear_share",
        ),
        (
            FilmicSettings {
                shoulder_linear_share: 1.0,
                ..default
            },
            "shoulder_linear_share",
        ),
        (
            FilmicSettings {
                display_gamma: -1.0,
                ..default
            },
            "display_gamma",
        ),
        (
            FilmicSettings {
                toe_power: 0.0,
                ..default
            },
            "toe_power",
        ),
        (
            FilmicSettings {
                shoulder_power: f32::INFINITY,
                ..default
            },
            "shoulder_power",
        ),
        (
            FilmicSettings {
                desaturation_onset: 1.0,
                ..default
            },
            "desaturation_onset",
        ),
    ];
    for (settings, field) in cases {
        let message = settings.validate().unwrap_err().to_string();
        assert!(message.contains(field), "{field}: {message}");
    }
}

#[test]
fn invalid_gamut_compression_is_reported_as_a_shared_stage_error() {
    let settings = FilmicSettings {
        output_gamut_compression: crate::gamut::GamutCompressionSettings {
            threshold: [1.5; 3],
            ..FilmicSettings::DEFAULT_OUTPUT_COMPRESSION
        },
        ..FilmicSettings::default()
    };
    assert!(matches!(settings.validate(), Err(InvalidFilmic::SharedStage(_))));
}

#[test]
fn settings_round_trip_through_json_and_reject_unknown_fields() {
    let settings = FilmicSettings {
        norm: FilmicNorm::Luminance,
        ..FilmicSettings::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    assert!(json.contains("\"luminance\""), "{json}");
    assert_eq!(serde_json::from_str::<FilmicSettings>(&json).unwrap(), settings);
    // Пустой раздел — умолчания; лишнее поле отвергается.
    assert_eq!(
        serde_json::from_str::<FilmicSettings>("{}").unwrap(),
        FilmicSettings::default()
    );
    assert!(serde_json::from_str::<FilmicSettings>(r#"{"slope": 0.2, "latitude": 3}"#).is_err());
}

/// Норма пикселя по определению из доки (независимо от кода): коэффициенты яркости Rec. 2020 из ITU-R BT.2020.
fn reference_norm(norm: FilmicNorm, pixel: [f64; 3]) -> f64 {
    match norm {
        FilmicNorm::PerChannel | FilmicNorm::MaxRgb => pixel.iter().copied().fold(0.0, f64::max),
        FilmicNorm::Luminance => 0.2627 * pixel[0] + 0.6780 * pixel[1] + 0.0593 * pixel[2],
        FilmicNorm::PowerNorm => {
            let cubes: f64 = pixel.iter().map(|c| c.powi(3)).sum();
            let squares: f64 = pixel.iter().map(|c| c.powi(2)).sum();
            cubes / squares
        }
    }
}

#[test]
fn chroma_preserving_norms_scale_rgb_by_curve_over_norm() {
    // Ниже порога обесцвечивания цвет — это вход, умноженный на `y^γ / N`: отношения каналов не меняются, а масштаб
    // задаёт именно норма из определения (максимум / яркость / степенная).
    let settings = FilmicSettings::default();
    for norm in [FilmicNorm::MaxRgb, FilmicNorm::Luminance, FilmicNorm::PowerNorm] {
        let base = base_for_norm(norm);
        for pixel in [
            [0.05_f32, 0.02, 0.01],
            [0.01, 0.04, 0.02],
            [0.02, 0.02, 0.06],
            [0.08, 0.08, 0.08],
        ] {
            let wide = pixel.map(f64::from);
            let reference = reference_norm(norm, wide);
            let curved = f64::from(base.curve_value(reference as f32, 1.0));
            assert!(
                curved < f64::from(settings.desaturation_onset),
                "{pixel:?}: вне зоны без обесцвечивания"
            );
            let scale = curved.powf(f64::from(settings.display_gamma)) / reference;
            let display = base.map_scene_pixel(pixel, 1.0);
            for channel in 0..3 {
                let expected = wide[channel] * scale;
                assert!(
                    (f64::from(display[channel]) - expected).abs() < 2e-5 * expected.max(1e-3),
                    "{norm:?} {pixel:?}: канал {channel}: {} против {expected}",
                    display[channel]
                );
            }
        }
    }
}

#[test]
fn per_channel_norm_runs_the_curve_on_each_channel_independently() {
    let settings = FilmicSettings::default();
    let base = base_for_norm(FilmicNorm::PerChannel);
    let pixel = [0.4_f32, 0.05, 0.003];
    let display = base.map_scene_pixel(pixel, 1.7);
    for channel in 0..3 {
        let expected = base.curve_value(pixel[channel], 1.7).powf(settings.display_gamma);
        assert_eq!(display[channel], expected, "канал {channel}");
    }
}
