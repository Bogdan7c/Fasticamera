//! Полоски Lift / Gamma / Gain / Offset по R, G, B (решения 339–343): формула, основа каналов, порядок стадий,
//! проверка конфига.

use std::sync::Arc;

use look_contract::{ChannelPrimaries, PrimaryChannel, WheelZone};

use super::*;
use crate::color::chromaticity::Primaries;
use crate::color::matrix::{Matrix3, PixelMatrix};
use crate::image::DisplayGamut;
use crate::look::model::{InvalidLookModel, LookModelSettings};
use crate::look::primaries::{
    ChannelBasis, InvalidPrimariesModel, LookPrimaries, PrimariesEncoding, PrimariesModelSettings,
};
use crate::look::{CURRENT_LOOK_PARAMS_VERSION, LookControls, LookParams};

/// Допуск на шум f32 в цепочке «матрица → кодирование → кривая → декодирование → матрица».
const TOLERANCE: f32 = 2e-5;

/// Все ручки нейтральны, кроме одной.
fn only(zone: WheelZone, channel: PrimaryChannel, value: f32) -> ChannelPrimaries {
    ChannelPrimaries::NEUTRAL.with_value(zone, channel, knob(value))
}

/// Разрешает полоски для рабочего пространства `working` с моделью по умолчанию.
fn resolved(primaries: &ChannelPrimaries, working: &Primaries) -> LookPrimaries {
    resolved_with(primaries, &PrimariesModelSettings::default(), working)
}

fn resolved_with(primaries: &ChannelPrimaries, model: &PrimariesModelSettings, working: &Primaries) -> LookPrimaries {
    LookPrimaries::resolve(primaries, model, working)
        .unwrap()
        .expect("полоски тронуты")
}

/// Модель, в которой каналы — само рабочее пространство: матриц нет, считаем каналы напрямую.
fn working_basis_model() -> PrimariesModelSettings {
    PrimariesModelSettings {
        channel_basis: ChannelBasis::Working,
        ..PrimariesModelSettings::default()
    }
}

/// Значение кривой канала в шкале кодирования.
fn encoded_curve(bars: &LookPrimaries, channel: usize, light: f32) -> f32 {
    bars.encoding.encode(bars.curve(channel, light))
}

fn close(first: f32, second: f32) -> bool {
    (first - second).abs() < TOLERANCE
}

#[test]
fn neutral_primaries_resolve_to_none_and_any_single_bar_resolves_to_some() {
    let model = PrimariesModelSettings::default();
    assert_eq!(
        LookPrimaries::resolve(&ChannelPrimaries::NEUTRAL, &model, &Primaries::REC_2020).unwrap(),
        None
    );
    for zone in WheelZone::ALL {
        for channel in PrimaryChannel::ALL {
            let touched = only(zone, channel, 0.01);
            assert!(
                LookPrimaries::resolve(&touched, &model, &Primaries::REC_2020)
                    .unwrap()
                    .is_some(),
                "{zone:?}/{channel:?}"
            );
        }
    }
}

#[test]
fn neutral_look_params_pass_every_pixel_bit_for_bit_and_a_single_bar_counts_as_a_display_stage() {
    let neutral = LookParams::resolve(&LookControls::default(), &Primaries::REC_2020).unwrap();
    assert!(neutral.primaries.is_none() && neutral.is_neutral() && neutral.has_no_display_stages());
    for pixel in [[0.0, 0.0, 0.0], [0.2, 0.5, 0.1], [-0.01, 0.3, 1.04], [1.0, 1.0, 1.0]] {
        assert_eq!(neutral.apply_to_pixel(pixel), pixel);
    }
    let barred = LookControls::new(
        settings_with(|settings| settings.primaries = only(WheelZone::Gain, PrimaryChannel::Red, 0.5)),
        Arc::new(LookModelSettings::default()),
    );
    let params = LookParams::resolve(&barred, &Primaries::REC_2020).unwrap();
    assert!(params.primaries.is_some());
    assert!(!params.is_neutral() && !params.has_no_display_stages());
    assert_eq!(params.version, CURRENT_LOOK_PARAMS_VERSION);
    assert_eq!(CURRENT_LOOK_PARAMS_VERSION.0, 4);
}

#[test]
fn encoding_is_odd_hits_zero_and_one_and_round_trips_including_negative_and_over_one() {
    let encoding = PrimariesEncoding::new(0.01);
    assert_eq!(encoding.encode(0.0), 0.0);
    assert_eq!(encoding.decode(0.0), 0.0);
    assert!(close(encoding.encode(1.0), 1.0) && close(encoding.decode(1.0), 1.0));
    for light in [-1.5_f32, -0.3, -0.02, -1e-6, 1e-6, 0.004, 0.18, 0.5, 1.0, 1.07, 3.0] {
        assert_eq!(encoding.encode(-light), -encoding.encode(light), "нечётность {light}");
        let back = encoding.decode(encoding.encode(light));
        assert!((back - light).abs() < 1e-5 * light.abs().max(1.0), "{light} -> {back}");
    }
    // Наклон в нуле конечен: у чистого √x он был бы бесконечным (причина «носка»).
    let slope = encoding.encode(1e-6) / 1e-6;
    assert!(
        slope.is_finite() && slope > 1.0 && slope < 1000.0,
        "наклон в нуле {slope}"
    );
}

#[test]
fn every_one_of_twelve_knobs_moves_its_own_channel_at_its_own_zone_and_nothing_else() {
    // Основа каналов = `W`: матриц нет, значит чужой канал обязан остаться ровно на месте.
    let model = working_basis_model();
    // Проба зоны: свет, в котором зона действует; и свет, где она обязана не действовать.
    let probes: [(WheelZone, f32, f32); 4] = [
        (WheelZone::Lift, 0.0, 1.0),   // Lift двигает чёрное, белое — нет
        (WheelZone::Gain, 1.0, 0.0),   // Gain двигает белое, чёрное — нет
        (WheelZone::Gamma, 0.2, 0.0),  // Gamma двигает середину, чёрное — нет
        (WheelZone::Offset, 0.3, 0.0), // Offset двигает всё; чёрное тоже, отдельной «неподвижной» точки нет
    ];
    for (zone, active_light, still_light) in probes {
        for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
            for knob_value in [1.0_f32, -1.0] {
                let bars = resolved_with(&only(zone, channel, knob_value), &model, &Primaries::REC_2020);
                let moved = bars.apply([active_light; 3]);
                let channels_moved: Vec<usize> = moved
                    .iter()
                    .enumerate()
                    .filter(|(_, value)| (**value - active_light).abs() > 1e-4)
                    .map(|(index, _)| index)
                    .collect();
                assert_eq!(
                    channels_moved,
                    vec![channel_index],
                    "{zone:?}/{channel:?}={knob_value}: сдвинуты каналы {channels_moved:?}, ждали только {channel_index}"
                );
                // Направление: «+» поднимает свет в зоне, «−» опускает.
                assert_eq!(
                    moved[channel_index] > active_light,
                    knob_value > 0.0,
                    "{zone:?}/{channel:?}={knob_value}: направление"
                );
                if zone != WheelZone::Offset {
                    let still = bars.apply([still_light; 3]);
                    for index in 0..3 {
                        assert!(
                            (still[index] - still_light).abs() < 1e-6,
                            "{zone:?}/{channel:?}: неподвижная точка {still_light} ушла в канале {index}: {still:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn lift_moves_black_to_l_and_keeps_white_gain_moves_white_to_g_and_keeps_black() {
    let model = PrimariesModelSettings::default();
    let lift = resolved(&only(WheelZone::Lift, PrimaryChannel::Green, 1.0), &Primaries::REC_2020);
    assert!(close(encoded_curve(&lift, 1, 0.0), model.lift_range), "чёрное -> l");
    assert!(close(lift.curve(1, 1.0), 1.0), "белое неподвижно");
    let negative = resolved(
        &only(WheelZone::Lift, PrimaryChannel::Green, -1.0),
        &Primaries::REC_2020,
    );
    assert!(close(encoded_curve(&negative, 1, 0.0), -model.lift_range));

    let gain = resolved(&only(WheelZone::Gain, PrimaryChannel::Blue, 1.0), &Primaries::REC_2020);
    assert!(
        close(encoded_curve(&gain, 2, 1.0), model.gain_stops_in_sqrt.exp2()),
        "белое -> G"
    );
    assert_eq!(gain.curve(2, 0.0), 0.0, "чёрное неподвижно ровно");
    let dimmed = resolved(&only(WheelZone::Gain, PrimaryChannel::Blue, -1.0), &Primaries::REC_2020);
    assert!(close(
        encoded_curve(&dimmed, 2, 1.0),
        (-model.gain_stops_in_sqrt).exp2()
    ));
}

#[test]
fn gamma_keeps_black_and_white_and_moves_grey_in_the_knob_direction() {
    let up = resolved(&only(WheelZone::Gamma, PrimaryChannel::Red, 1.0), &Primaries::REC_2020);
    let down = resolved(&only(WheelZone::Gamma, PrimaryChannel::Red, -1.0), &Primaries::REC_2020);
    for bars in [&up, &down] {
        assert_eq!(bars.curve(0, 0.0), 0.0);
        assert!(close(bars.curve(0, 1.0), 1.0));
    }
    let grey = 0.2;
    assert!(up.curve(0, grey) > grey + 0.01 && down.curve(0, grey) < grey - 0.01);
    // Середина двигается заметно сильнее краёв: на 0,02 и 0,95 сдвиг меньше, чем на сером.
    let middle_shift = up.curve(0, grey) - grey;
    assert!(up.curve(0, 0.02) - 0.02 < middle_shift && up.curve(0, 0.95) - 0.95 < middle_shift);
}

#[test]
fn offset_shifts_every_light_by_an_equal_amount_in_the_encoding() {
    let model = PrimariesModelSettings::default();
    let bars = resolved(&only(WheelZone::Offset, PrimaryChannel::Red, 1.0), &Primaries::REC_2020);
    for light in [-0.05_f32, 0.0, 0.01, 0.2, 0.7, 1.0, 1.05] {
        let shift = encoded_curve(&bars, 0, light) - bars.encoding.encode(light);
        assert!(close(shift, model.offset_range), "{light}: сдвиг {shift}");
    }
}

#[test]
fn each_of_twelve_knobs_lands_in_its_own_coefficient_slot() {
    let model = PrimariesModelSettings::default();
    for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
        let lift = resolved(&only(WheelZone::Lift, channel, 1.0), &Primaries::REC_2020);
        let gamma = resolved(&only(WheelZone::Gamma, channel, 1.0), &Primaries::REC_2020);
        let gain = resolved(&only(WheelZone::Gain, channel, 1.0), &Primaries::REC_2020);
        let offset = resolved(&only(WheelZone::Offset, channel, 1.0), &Primaries::REC_2020);
        let mut expected_lift = [0.0; 3];
        expected_lift[channel_index] = model.lift_range;
        let mut expected_gamma = [1.0; 3];
        expected_gamma[channel_index] = model.gamma_slope_stops.exp2();
        let mut expected_gain = [1.0; 3];
        expected_gain[channel_index] = model.gain_stops_in_sqrt.exp2();
        let mut expected_offset = [0.0; 3];
        expected_offset[channel_index] = model.offset_range;
        assert_eq!(lift.lift, expected_lift);
        assert_eq!(gamma.gamma_slope, expected_gamma);
        assert_eq!(gain.gain, expected_gain);
        assert_eq!(offset.offset, expected_offset);
        // Остальные зоны при тронутой одной остаются нейтральными: перестановка зон видна здесь.
        assert_eq!(
            (lift.gamma_slope, lift.gain, lift.offset),
            ([1.0; 3], [1.0; 3], [0.0; 3])
        );
        assert_eq!(
            (gain.lift, gain.gamma_slope, gain.offset),
            ([0.0; 3], [1.0; 3], [0.0; 3])
        );
    }
}

/// Все 81 сочетание краёв `{−1, 0, +1}` четырёх зон одного канала.
fn edge_combinations(channel: PrimaryChannel) -> Vec<ChannelPrimaries> {
    let edges = [-1.0_f32, 0.0, 1.0];
    let mut combinations = Vec::new();
    for lift in edges {
        for gamma in edges {
            for gain in edges {
                for offset in edges {
                    combinations.push(
                        ChannelPrimaries::NEUTRAL
                            .with_value(WheelZone::Lift, channel, knob(lift))
                            .with_value(WheelZone::Gamma, channel, knob(gamma))
                            .with_value(WheelZone::Gain, channel, knob(gain))
                            .with_value(WheelZone::Offset, channel, knob(offset)),
                    );
                }
            }
        }
    }
    combinations
}

/// Клин света `−0,02…1,07` (диапазон AgX) с шагом 0,001.
fn wedge() -> impl Iterator<Item = f32> {
    (0..=1090).map(|step| -0.02 + step as f32 * 0.001)
}

#[test]
fn every_edge_combination_of_one_channel_keeps_the_wedge_monotonic_and_finite() {
    for (index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
        let mut checked = 0;
        for combination in edge_combinations(channel) {
            if combination.is_neutral() {
                continue;
            }
            let bars = resolved(&combination, &Primaries::REC_2020);
            let mut previous = f32::NEG_INFINITY;
            for light in wedge() {
                let value = bars.curve(index, light);
                assert!(value.is_finite(), "{channel:?} {combination:?} x={light}");
                assert!(
                    value >= previous,
                    "{channel:?} {combination:?}: на x={light} {value} < {previous}"
                );
                previous = value;
            }
            checked += 1;
        }
        assert_eq!(checked, 80, "81 сочетание без нейтрали");
    }
}

#[test]
fn equal_knobs_on_all_three_channels_keep_grey_grey_and_monotonic_in_every_basis_and_working_space() {
    let workings = [
        ("Rec.2020", Primaries::REC_2020),
        ("sRGB (прежняя основа)", Primaries::SRGB),
        ("P3 (прежняя основа)", Primaries::DISPLAY_P3),
    ];
    for (name, working) in workings {
        for basis in [ChannelBasis::DisplayP3, ChannelBasis::Working] {
            let model = PrimariesModelSettings {
                channel_basis: basis,
                ..PrimariesModelSettings::default()
            };
            for combination in edge_combinations(PrimaryChannel::Red) {
                if combination.is_neutral() {
                    continue;
                }
                // Те же четыре значения на всех трёх каналах.
                let mut equal = ChannelPrimaries::NEUTRAL;
                for zone in WheelZone::ALL {
                    for channel in PrimaryChannel::ALL {
                        equal = equal.with_value(zone, channel, combination.value(zone, PrimaryChannel::Red));
                    }
                }
                let bars = resolved_with(&equal, &model, &working);
                let mut previous = f32::NEG_INFINITY;
                for light in wedge() {
                    let [red, green, blue] = bars.apply([light; 3]);
                    assert!(
                        (red - green).abs() < TOLERANCE.max(1e-5 * light.abs())
                            && (green - blue).abs() < TOLERANCE.max(1e-5 * light.abs()),
                        "{name} {basis:?} x={light}: серый разошёлся {red} {green} {blue}"
                    );
                    assert!(
                        red.is_finite() && red >= previous - TOLERANCE,
                        "{name} {basis:?} x={light}"
                    );
                    previous = red;
                }
            }
        }
    }
}

#[test]
fn extreme_knobs_stay_finite_for_inputs_below_zero_and_above_one() {
    let extreme = {
        let mut all = ChannelPrimaries::NEUTRAL;
        for (zone_index, zone) in WheelZone::ALL.into_iter().enumerate() {
            for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
                let sign = if (zone_index + channel_index) % 2 == 0 {
                    1.0
                } else {
                    -1.0
                };
                all = all.with_value(zone, channel, knob(sign));
            }
        }
        all
    };
    let bars = resolved(&extreme, &Primaries::REC_2020);
    for value in [-10.0_f32, -1.0, -0.2, -1e-9, 0.0, 1e-9, 1.0, 1.5, 4.0, 100.0] {
        let moved = bars.apply([value, -value, value * 0.5]);
        assert!(moved.iter().all(|channel| channel.is_finite()), "{value}: {moved:?}");
    }
}

/// `W → линейный Display P3` из независимой цепочки модуля `working_space` (Rec. 2020 → P3).
fn working_to_p3(working: &Primaries) -> PixelMatrix {
    Primaries::DISPLAY_P3
        .rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&working.rgb_to_xyz().unwrap())
        .to_f32()
}

#[test]
fn a_bar_of_one_p3_channel_moves_only_that_p3_channel_for_every_working_space() {
    let probe = [0.35_f32, 0.2, 0.45];
    for working in [Primaries::REC_2020, Primaries::SRGB] {
        let to_p3 = working_to_p3(&working);
        let before = to_p3.apply(probe);
        for zone in WheelZone::ALL {
            for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
                let bars = resolved(&only(zone, channel, 1.0), &working);
                let after = to_p3.apply(bars.apply(probe));
                for index in 0..3 {
                    let delta = (after[index] - before[index]).abs();
                    if index == channel_index {
                        assert!(delta > 1e-3, "{zone:?}/{channel:?}: свой канал не сдвинулся ({delta})");
                    } else {
                        assert!(delta < 1e-4, "{zone:?}/{channel:?}: задет P3-канал {index} на {delta}");
                    }
                }
            }
        }
    }
}

#[test]
fn matching_working_space_gives_exact_identity_matrices_and_plain_per_channel_curves() {
    // Прежняя основа на экране P3: `W` = P3, основа каналов P3 → матрицы единичные точно.
    let bars = resolved(&only(WheelZone::Gain, PrimaryChannel::Red, 0.7), &Primaries::DISPLAY_P3);
    let identity = Matrix3::IDENTITY.to_f32().rows;
    assert_eq!((bars.working_to_basis, bars.basis_to_working), (identity, identity));
    let pixel = [0.4_f32, 0.25, 0.1];
    assert_eq!(
        bars.apply(pixel),
        [
            bars.curve(0, pixel[0]),
            bars.curve(1, pixel[1]),
            bars.curve(2, pixel[2])
        ]
    );
    // `channel_basis = working` на Rec. 2020 — то же самое.
    let working_basis = resolved_with(
        &only(WheelZone::Gain, PrimaryChannel::Red, 0.7),
        &working_basis_model(),
        &Primaries::REC_2020,
    );
    assert_eq!(working_basis.working_to_basis, identity);
    // sRGB и Rec. 2020 при основе P3 — НЕ единичные, и пара матриц взаимно обратна.
    for working in [Primaries::SRGB, Primaries::REC_2020] {
        let bars = resolved(&only(WheelZone::Gain, PrimaryChannel::Red, 0.7), &working);
        assert_ne!(bars.working_to_basis, identity);
        let (forward, backward) = (
            PixelMatrix {
                rows: bars.working_to_basis,
            },
            PixelMatrix {
                rows: bars.basis_to_working,
            },
        );
        for probe in [[0.3_f32, 0.6, 0.1], [1.0, 1.0, 1.0], [0.0, 0.2, 0.9]] {
            let round = backward.apply(forward.apply(probe));
            assert!(
                (0..3).all(|index| (round[index] - probe[index]).abs() < 1e-6),
                "{probe:?}"
            );
        }
    }
}

#[test]
fn working_to_p3_matrix_agrees_with_the_shared_working_space_helper_for_rec2020() {
    let bars = resolved(&only(WheelZone::Lift, PrimaryChannel::Red, 0.1), &Primaries::REC_2020);
    let reference = crate::color::working_space::working_to_output(DisplayGamut::DisplayP3)
        .unwrap()
        .to_f32()
        .rows;
    for (ours, expected) in bars.working_to_basis.iter().flatten().zip(reference.iter().flatten()) {
        assert!((ours - expected).abs() < 1e-6, "{ours} против {expected}");
    }
}

#[test]
fn order_is_tone_then_bars_then_colour_and_the_swapped_orders_differ() {
    let pixel = [0.45_f32, 0.12, 0.2];
    let settings = settings_with(|settings| {
        settings.contrast = knob(0.8);
        settings.primaries = only(WheelZone::Gain, PrimaryChannel::Red, 1.0);
        settings.saturation = knob(0.5);
    });
    let controls = LookControls::new(settings, Arc::new(LookModelSettings::default()));
    let params = LookParams::resolve(&controls, &Primaries::REC_2020).unwrap();
    let (tone, bars, color) = (
        params.tone.expect("тон"),
        params.primaries.expect("полоски"),
        params.color.expect("цвет"),
    );
    let expected = color.apply(bars.apply(tone.apply(pixel)));
    assert_eq!(params.apply_to_pixel(pixel), expected);
    let max_difference = |first: [f32; 3], second: [f32; 3]| {
        (0..3)
            .map(|index| (first[index] - second[index]).abs())
            .fold(0.0_f32, f32::max)
    };
    // Перестановки соседей дают заметно другую картинку: тест поймал бы любую.
    assert!(max_difference(expected, color.apply(tone.apply(bars.apply(pixel)))) > 1e-3);
    assert!(max_difference(expected, bars.apply(color.apply(tone.apply(pixel)))) > 1e-3);
    assert!(max_difference(expected, color.apply(tone.apply(bars.apply(pixel)))) > 1e-3);
}

#[test]
fn full_desaturation_after_the_bars_removes_the_bar_tint() {
    let controls = LookControls::new(
        settings_with(|settings| {
            settings.primaries = only(WheelZone::Gain, PrimaryChannel::Red, 1.0);
            settings.saturation = knob(-1.0);
        }),
        Arc::new(LookModelSettings::default()),
    );
    let params = LookParams::resolve(&controls, &Primaries::REC_2020).unwrap();
    // Серый вход после Gain R «+» окрасился бы; насыщенность −1, стоящая ПОСЛЕ полосок, возвращает серый.
    let barred_only = params.primaries.unwrap().apply([0.3; 3]);
    assert!(
        (barred_only[0] - barred_only[1]).abs() > 1e-2,
        "полоски обязаны окрасить серый"
    );
    let [red, green, blue] = params.apply_to_pixel([0.3; 3]);
    assert!(
        (red - green).abs() < 1e-4 && (green - blue).abs() < 1e-4,
        "{red} {green} {blue}"
    );
}

#[test]
fn default_model_is_valid_and_every_config_error_is_distinguishable() {
    PrimariesModelSettings::default()
        .validate()
        .expect("умолчание допустимо");
    LookModelSettings::default()
        .validate()
        .expect("модель вида с умолчанием допустима");
    let with = |edit: fn(&mut PrimariesModelSettings)| {
        let mut model = PrimariesModelSettings::default();
        edit(&mut model);
        model.validate()
    };
    assert!(matches!(
        with(|model| model.encoding_toe = f32::NAN),
        Err(InvalidPrimariesModel::NotFinite {
            field: "encoding_toe",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.offset_range = f32::INFINITY),
        Err(InvalidPrimariesModel::NotFinite {
            field: "offset_range",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.encoding_toe = 0.0),
        Err(InvalidPrimariesModel::EncodingToeNotPositive { .. })
    ));
    assert!(matches!(
        with(|model| model.encoding_toe = -0.01),
        Err(InvalidPrimariesModel::EncodingToeNotPositive { .. })
    ));
    assert!(matches!(
        with(|model| model.encoding_toe = 2.0),
        Err(InvalidPrimariesModel::OutOfRange {
            field: "encoding_toe",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.lift_range = -0.1),
        Err(InvalidPrimariesModel::OutOfRange {
            field: "lift_range",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.gamma_slope_stops = -0.5),
        Err(InvalidPrimariesModel::OutOfRange {
            field: "gamma_slope_stops",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.gain_stops_in_sqrt = -0.25),
        Err(InvalidPrimariesModel::OutOfRange {
            field: "gain_stops_in_sqrt",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.offset_range = -0.05),
        Err(InvalidPrimariesModel::OutOfRange {
            field: "offset_range",
            ..
        })
    ));
    assert!(matches!(
        with(|model| model.offset_range = 0.9),
        Err(InvalidPrimariesModel::OutOfRange {
            field: "offset_range",
            ..
        })
    ));
}

#[test]
fn monotonicity_limit_is_a_strict_border_at_lift_equal_to_two_to_minus_gain_stops() {
    // gain_stops = 1: предел lift ровно 0,5 (степень двойки — точно в f32).
    let model = |lift_range: f32| PrimariesModelSettings {
        lift_range,
        gain_stops_in_sqrt: 1.0,
        ..PrimariesModelSettings::default()
    };
    assert!(model(0.4999).validate().is_ok(), "чуть ниже предела — можно");
    match model(0.5).validate() {
        Err(InvalidPrimariesModel::BreaksMonotonicity { gain_floor, .. }) => assert_eq!(gain_floor, 0.5),
        other => panic!("на пределе ждали отказ монотонности, получили {other:?}"),
    }
    assert!(matches!(
        model(0.6).validate(),
        Err(InvalidPrimariesModel::BreaksMonotonicity { .. })
    ));
}

#[test]
fn at_the_largest_valid_lift_the_wedge_is_still_monotonic_and_just_past_it_the_check_could_fail() {
    // Самый большой допустимый lift при gain_stops = 1: чуть меньше 0,5; на краях ручек клин ещё растёт.
    let model = PrimariesModelSettings {
        lift_range: 0.4999,
        gain_stops_in_sqrt: 1.0,
        ..PrimariesModelSettings::default()
    };
    model.validate().unwrap();
    let edges = ChannelPrimaries::NEUTRAL
        .with_value(WheelZone::Lift, PrimaryChannel::Green, knob(1.0))
        .with_value(WheelZone::Gain, PrimaryChannel::Green, knob(-1.0));
    let bars = resolved_with(&edges, &model, &Primaries::REC_2020);
    let mut previous = f32::NEG_INFINITY;
    for light in wedge() {
        let value = bars.curve(1, light);
        assert!(value >= previous, "x={light}");
        previous = value;
    }
    // Без проверки (lift 0,6 > 0,5) тот же клин переворачивается: проверка конфига нужна и работает.
    let broken = PrimariesModelSettings {
        lift_range: 0.6,
        ..model
    };
    let flipped = resolved_with(&edges, &broken, &Primaries::REC_2020);
    assert!(
        flipped.curve(1, 1.0) < flipped.curve(1, 0.0),
        "белое темнее чёрного без проверки"
    );
}

#[test]
fn look_model_validation_surfaces_primaries_errors_and_config_json_parses_the_section() {
    let broken = LookModelSettings {
        primaries: PrimariesModelSettings {
            encoding_toe: 0.0,
            ..PrimariesModelSettings::default()
        },
        ..LookModelSettings::default()
    };
    assert!(matches!(
        broken.validate(),
        Err(InvalidLookModel::Primaries(
            InvalidPrimariesModel::EncodingToeNotPositive { .. }
        ))
    ));
    let parsed: LookModelSettings = serde_json::from_str(
        r#"{"primaries":{"channel_basis":"working","encoding_toe":0.02,"lift_range":0.2,"gamma_slope_stops":1.0,"gain_stops_in_sqrt":0.5,"offset_range":0.1}}"#,
    )
    .unwrap();
    assert_eq!(
        parsed.primaries,
        PrimariesModelSettings {
            channel_basis: ChannelBasis::Working,
            encoding_toe: 0.02,
            lift_range: 0.2,
            gamma_slope_stops: 1.0,
            gain_stops_in_sqrt: 0.5,
            offset_range: 0.1,
        }
    );
    assert_eq!(
        parsed.wheels,
        LookModelSettings::default().wheels,
        "остальные секции — умолчания"
    );
    assert!(serde_json::from_str::<LookModelSettings>(r#"{"primaries":{"unknown":1}}"#).is_err());
    assert!(serde_json::from_str::<LookModelSettings>(r#"{"primaries":{"channel_basis":"rec709"}}"#).is_err());
    let empty: LookModelSettings = serde_json::from_str(r#"{"primaries":{}}"#).unwrap();
    assert_eq!(empty.primaries.channel_basis, ChannelBasis::DisplayP3);
}
