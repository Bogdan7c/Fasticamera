//! Колёса цвета по зонам (решения 186, 191–193): формула, проверка конфига, встраивание в стадию цвета.

use look_contract::{ColorWheel, ColorWheels, WheelPush, WheelZone};

use super::*;
use crate::color::chromaticity::Primaries;
use crate::look::color::LookColorParams;
use crate::look::model::{InvalidLookModel, LookModelSettings};
use crate::look::wheels::{InvalidWheelModel, LookWheels, WheelModelSettings};

/// Конфиг с различимыми числами: если перепутать силу цвета и силу кольца, тест это заметит.
const DISTINCT_MODEL: WheelModelSettings = WheelModelSettings {
    push_chroma_range: 0.08,
    ring_lightness_range: 0.07,
    zone_pivot_display_gray: 0.18,
};

/// Колесо: толчок цвета (оттенок в градусах OKLCh, сила 0…1) и кольцо яркости.
fn wheel(hue_degrees: f32, strength: f32, ring: f32) -> ColorWheel {
    ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, strength).unwrap(),
        ring: knob(ring),
    }
}

/// Только кольцо яркости.
fn ring_only(ring: f32) -> ColorWheel {
    wheel(0.0, 0.0, ring)
}

/// Все колёса нейтральны, кроме одного.
fn only(zone: WheelZone, value: ColorWheel) -> ColorWheels {
    ColorWheels::NEUTRAL.with_wheel(zone, value)
}

/// Четыре кольца сразу (Lift, Gamma, Gain, Offset).
fn rings(lift: f32, gamma: f32, gain: f32, offset: f32) -> ColorWheels {
    ColorWheels::NEUTRAL
        .with_wheel(WheelZone::Lift, ring_only(lift))
        .with_wheel(WheelZone::Gamma, ring_only(gamma))
        .with_wheel(WheelZone::Gain, ring_only(gain))
        .with_wheel(WheelZone::Offset, ring_only(offset))
}

fn resolved(wheels: &ColorWheels, model: &WheelModelSettings) -> LookWheels {
    LookWheels::resolve(wheels, model).expect("колёса тронуты")
}

/// Какой из сдвигов `LookWheels` принадлежит колесу.
fn shift_of(wheels: &LookWheels, zone: WheelZone) -> [f32; 3] {
    match zone {
        WheelZone::Lift => wheels.lift,
        WheelZone::Gamma => wheels.gamma,
        WheelZone::Gain => wheels.gain,
        WheelZone::Offset => wheels.offset,
    }
}

/// Любое тронутое колесо: веса от него не зависят, нужен только `LookWheels`.
fn weights_probe() -> LookWheels {
    resolved(&only(WheelZone::Lift, ring_only(1.0)), &WheelModelSettings::default())
}

/// Какой из трёх весов зоны смотрим.
type WeightPicker = fn(&crate::look::wheels::ZoneWeights) -> f32;

/// Шаг сетки светлоты `[from, to]`.
fn lightness_grid(from: f32, to: f32, steps: u32) -> impl Iterator<Item = f32> {
    (0..=steps).map(move |index| from + (to - from) * index as f32 / steps as f32)
}

// ---------- веса зон ----------

#[test]
fn zone_weights_sum_to_one_and_each_zone_is_exact_where_it_must_be() {
    let probe = weights_probe();
    let pivot = probe.pivot_lightness;
    for lightness in lightness_grid(-0.2, 1.2, 1400) {
        let weights = probe.zone_weights(lightness);
        let sum = weights.shadows + weights.midtones + weights.highlights;
        assert!((sum - 1.0).abs() < 1e-6, "L={lightness}: сумма {sum}");
        assert!(weights.shadows >= 0.0 && weights.highlights >= 0.0 && weights.midtones >= -1e-6);
        if lightness >= pivot {
            assert_eq!(weights.shadows, 0.0, "Lift дотянулся до серого и выше: L={lightness}");
        }
        if lightness <= pivot {
            assert_eq!(
                weights.highlights, 0.0,
                "Gain дотянулся до серого и ниже: L={lightness}"
            );
        }
    }
    let at_pivot = probe.zone_weights(pivot);
    assert_eq!(
        (at_pivot.shadows, at_pivot.midtones, at_pivot.highlights),
        (0.0, 1.0, 0.0)
    );
    let black = probe.zone_weights(0.0);
    assert_eq!((black.shadows, black.midtones, black.highlights), (1.0, 0.0, 0.0));
    let white = probe.zone_weights(1.0);
    assert!(white.highlights > 1.0 - 1e-6 && white.midtones.abs() < 1e-6 && white.shadows == 0.0);
}

#[test]
fn zone_weights_are_c1_smooth_without_steps_in_value_or_slope() {
    let probe = weights_probe();
    let slope = |pick: WeightPicker, lightness: f32| {
        const HALF_STEP: f32 = 1e-3;
        let above = pick(&probe.zone_weights(lightness + HALF_STEP));
        let below = pick(&probe.zone_weights(lightness - HALF_STEP));
        (above - below) / (2.0 * HALF_STEP)
    };
    let pickers: [(&str, WeightPicker); 3] = [
        ("тени", |w| w.shadows),
        ("середина", |w| w.midtones),
        ("света", |w| w.highlights),
    ];
    // Склон smoothstep меняется не быстрее 6/ширина² (ширина самого узкого окна ≈ 0,415 → 35). Излом (C0, но не C1)
    // дал бы скачок склона порядка 1/ширина ≈ 2,4 за один шаг — в сотню раз больше порога.
    const STEP: f32 = 5e-4;
    const SLOPE_CHANGE_PER_STEP_LIMIT: f32 = 40.0 * STEP + 3e-4; // 3e-4 — шум f32 при делении на 2e-3.
    for (name, pick) in pickers {
        let mut previous = slope(pick, -0.1);
        for lightness in lightness_grid(-0.1 + STEP, 1.1, 2400) {
            let current = slope(pick, lightness);
            assert!(
                (current - previous).abs() < SLOPE_CHANGE_PER_STEP_LIMIT,
                "{name}: склон прыгнул на L={lightness}: {previous} -> {current}"
            );
            previous = current;
        }
        // Склон на краях окон равен нулю: слева и справа от серого, чёрного и белого он почти не отличается.
        for edge in [0.0, probe.pivot_lightness, 1.0] {
            assert!(slope(pick, edge).abs() < 0.02, "{name}: склон на краю {edge}");
        }
    }
    // Вторая разность веса на шаге 0,01 ограничена той же оценкой: 35·0,01² = 3,5e-3.
    for (name, pick) in pickers {
        for lightness in lightness_grid(0.0, 1.0, 1000) {
            let h = 0.01;
            let second = pick(&probe.zone_weights(lightness + h)) - 2.0 * pick(&probe.zone_weights(lightness))
                + pick(&probe.zone_weights(lightness - h));
            assert!(second.abs() < 4e-3, "{name}: вторая разность {second} на L={lightness}");
        }
    }
}

// ---------- разрешение и масштаб ----------

#[test]
fn neutral_wheels_resolve_to_none_and_any_touch_resolves_to_some() {
    let model = WheelModelSettings::default();
    assert!(LookWheels::resolve(&ColorWheels::NEUTRAL, &model).is_none());
    for zone in WheelZone::ALL {
        assert!(LookWheels::resolve(&only(zone, ring_only(0.01)), &model).is_some());
        assert!(LookWheels::resolve(&only(zone, wheel(10.0, 0.01, 0.0)), &model).is_some());
    }
}

#[test]
fn each_wheel_lands_in_its_own_slot_scaled_by_the_model_ranges() {
    for zone in WheelZone::ALL {
        let value = wheel(90.0, 0.5, -0.25);
        let resolved = resolved(&only(zone, value), &DISTINCT_MODEL);
        let expected = [0.07 * -0.25, 0.08 * value.push.a(), 0.08 * value.push.b()];
        for other in WheelZone::ALL {
            let shift = shift_of(&resolved, other);
            if other == zone {
                for (channel, want) in expected.iter().enumerate() {
                    assert!((shift[channel] - want).abs() < 1e-7, "{zone:?}: канал {channel}");
                }
            } else {
                assert_eq!(shift, [0.0; 3], "{zone:?} попало в слот {other:?}");
            }
        }
    }
    // Кольцо 1 → ровно ΔL = ring_lightness_range; толчок (1, 0) → ровно Δa = push_chroma_range.
    let full = resolved(&only(WheelZone::Gain, wheel(0.0, 1.0, 1.0)), &DISTINCT_MODEL);
    assert!((full.gain[0] - 0.07).abs() < 1e-7 && (full.gain[1] - 0.08).abs() < 1e-7);
    // Серый зон — из конфига: L_g = Y_g^(1/3).
    assert!((full.pivot_lightness - 0.18_f32.cbrt()).abs() < 1e-6);
}

#[test]
fn lift_acts_only_below_grey_gain_only_above_and_offset_everywhere() {
    let model = DISTINCT_MODEL;
    let pivot = model.pivot_lightness();
    let shift_at = |zone: WheelZone, lightness: f32| {
        let lab = [lightness, 0.01, -0.02];
        let moved = resolved(&only(zone, ring_only(1.0)), &model).apply(lab);
        [moved[0] - lab[0], moved[1] - lab[1], moved[2] - lab[2]]
    };
    // Lift: в чёрном полный ΔL, на сером и выше — ровно ноль (цвет не трогаем: ни Δa, ни Δb).
    assert!((shift_at(WheelZone::Lift, 0.0)[0] - 0.07).abs() < 1e-6);
    assert!(shift_at(WheelZone::Lift, 0.3)[0] > 0.01);
    assert_eq!(shift_at(WheelZone::Lift, pivot), [0.0; 3]);
    assert_eq!(shift_at(WheelZone::Lift, 0.9), [0.0; 3]);
    // Gain — зеркально.
    assert!((shift_at(WheelZone::Gain, 1.0)[0] - 0.07).abs() < 1e-6);
    assert!(shift_at(WheelZone::Gain, 0.8)[0] > 0.01);
    assert_eq!(shift_at(WheelZone::Gain, pivot), [0.0; 3]);
    assert_eq!(shift_at(WheelZone::Gain, 0.1), [0.0; 3]);
    // Gamma: полный сдвиг на сером, нуль на концах.
    assert!((shift_at(WheelZone::Gamma, pivot)[0] - 0.07).abs() < 1e-6);
    assert!(shift_at(WheelZone::Gamma, 0.0)[0].abs() < 1e-6 && shift_at(WheelZone::Gamma, 1.0)[0].abs() < 1e-6);
    // Offset: одинаково везде.
    for lightness in [0.0, 0.2, pivot, 0.8, 1.0] {
        assert!(
            (shift_at(WheelZone::Offset, lightness)[0] - 0.07).abs() < 1e-6,
            "L={lightness}"
        );
    }
}

// ---------- конфиг ----------

#[test]
fn default_wheel_model_is_valid_and_its_monotonicity_limit_is_above_the_default_ring() {
    let model = WheelModelSettings::default();
    model.validate().unwrap();
    assert!(model.ring_lightness_range < model.ring_monotonicity_limit());
    LookModelSettings::default().validate().unwrap();
}

#[test]
fn every_wheel_model_number_is_range_checked_and_nan_is_rejected() {
    let base = WheelModelSettings::default();
    let cases: [(&str, WheelModelSettings); 11] = [
        (
            "push_chroma_range",
            WheelModelSettings {
                push_chroma_range: -0.01,
                ..base
            },
        ),
        (
            "push_chroma_range",
            WheelModelSettings {
                push_chroma_range: 0.2001,
                ..base
            },
        ),
        (
            "push_chroma_range",
            WheelModelSettings {
                push_chroma_range: f32::NAN,
                ..base
            },
        ),
        (
            "push_chroma_range",
            WheelModelSettings {
                push_chroma_range: f32::INFINITY,
                ..base
            },
        ),
        (
            "ring_lightness_range",
            WheelModelSettings {
                ring_lightness_range: -0.01,
                ..base
            },
        ),
        (
            "ring_lightness_range",
            WheelModelSettings {
                ring_lightness_range: 1.01,
                ..base
            },
        ),
        (
            "ring_lightness_range",
            WheelModelSettings {
                ring_lightness_range: f32::NAN,
                ..base
            },
        ),
        (
            "zone_pivot_display_gray",
            WheelModelSettings {
                zone_pivot_display_gray: 0.0005,
                ..base
            },
        ),
        (
            "zone_pivot_display_gray",
            WheelModelSettings {
                zone_pivot_display_gray: 0.8,
                ..base
            },
        ),
        (
            "zone_pivot_display_gray",
            WheelModelSettings {
                zone_pivot_display_gray: f32::NAN,
                ..base
            },
        ),
        (
            "zone_pivot_display_gray",
            WheelModelSettings {
                zone_pivot_display_gray: -0.2,
                ..base
            },
        ),
    ];
    for (expected_field, model) in cases {
        match model.validate() {
            Err(InvalidWheelModel::OutOfRange { field, .. }) => assert_eq!(field, expected_field, "{model:?}"),
            other => panic!("{model:?}: ждали OutOfRange({expected_field}), вышло {other:?}"),
        }
    }
    // Границы диапазонов допустимы (при малом кольце, чтобы не упереться в монотонность).
    for edge in [0.0, 0.2] {
        WheelModelSettings {
            push_chroma_range: edge,
            ..base
        }
        .validate()
        .unwrap();
    }
    WheelModelSettings {
        zone_pivot_display_gray: 0.001,
        ring_lightness_range: 0.01,
        ..base
    }
    .validate()
    .unwrap();
}

#[test]
fn ring_range_at_or_above_the_monotonicity_limit_is_rejected_and_just_below_passes() {
    let base = WheelModelSettings::default();
    let limit = base.ring_monotonicity_limit();
    // Предел по формуле: окно самого узкого края / (2 окна · крутизна 1,5): серый 0,2 → L_g ≈ 0,585, узкое окно 0,415.
    assert!((limit - (1.0 - 0.2_f32.cbrt()) / 3.0).abs() < 1e-6, "{limit}");
    WheelModelSettings {
        ring_lightness_range: limit * 0.999,
        ..base
    }
    .validate()
    .unwrap();
    for broken in [limit, limit * 1.001, 0.5] {
        match (WheelModelSettings {
            ring_lightness_range: broken,
            ..base
        })
        .validate()
        {
            Err(InvalidWheelModel::RingBreaksMonotonicity {
                value,
                limit: reported,
                pivot_lightness,
            }) => {
                assert_eq!(value, broken);
                assert_eq!(reported, limit);
                assert!((pivot_lightness - base.pivot_lightness()).abs() < 1e-7);
            }
            other => panic!("кольцо {broken}: ждали RingBreaksMonotonicity, вышло {other:?}"),
        }
    }
}

#[test]
fn look_model_validation_surfaces_wheel_errors_and_config_json_parses_the_wheels_section() {
    let mut model = LookModelSettings::default();
    model.wheels.ring_lightness_range = 0.5;
    assert!(matches!(
        model.validate(),
        Err(InvalidLookModel::Wheels(
            InvalidWheelModel::RingBreaksMonotonicity { .. }
        ))
    ));
    model.wheels = WheelModelSettings {
        push_chroma_range: 0.5,
        ..WheelModelSettings::default()
    };
    assert!(matches!(
        model.validate(),
        Err(InvalidLookModel::Wheels(InvalidWheelModel::OutOfRange {
            field: "push_chroma_range",
            ..
        }))
    ));

    let parsed: LookModelSettings =
        serde_json::from_str(r#"{"wheels": {"push_chroma_range": 0.08, "zone_pivot_display_gray": 0.25}}"#).unwrap();
    assert_eq!(parsed.wheels.push_chroma_range, 0.08);
    assert_eq!(parsed.wheels.zone_pivot_display_gray, 0.25);
    // Не названное остаётся по умолчанию — и в секции колёс, и во всей модели.
    assert_eq!(
        parsed.wheels.ring_lightness_range,
        WheelModelSettings::default().ring_lightness_range
    );
    assert_eq!(parsed.saturation, LookModelSettings::default().saturation);
    // Опечатка в секции не молчит.
    assert!(serde_json::from_str::<LookModelSettings>(r#"{"wheels": {"push_chroma": 0.08}}"#).is_err());
    // Круг: запись → чтение.
    let json = serde_json::to_string(&parsed).unwrap();
    assert_eq!(serde_json::from_str::<LookModelSettings>(&json).unwrap(), parsed);
}

// ---------- монотонность тона ----------

/// Самая большая из «обратных ходов» светлоты вдоль рампы L (0, если светлота нигде не убывает).
fn worst_lightness_drop(wheels: &LookWheels) -> f32 {
    const STEPS: u32 = 1200; // шаг 0,001: при запасе 0,1 % предела прирост за шаг ≈ 1e-6, шума f32 ≈ 6e-8.
    let ramp: Vec<f32> = lightness_grid(-0.1, 1.1, STEPS)
        .map(|l| wheels.apply([l, 0.0, 0.0])[0])
        .collect();
    ramp.windows(2).map(|pair| pair[0] - pair[1]).fold(0.0, f32::max)
}

#[test]
fn rings_never_make_lightness_decrease_for_every_combination_up_to_the_validated_limit() {
    let default_model = WheelModelSettings::default();
    let limit = default_model.ring_monotonicity_limit();
    let near_limit = WheelModelSettings {
        ring_lightness_range: limit * 0.999,
        ..default_model
    };
    near_limit.validate().unwrap();
    let values = [-1.0_f32, 0.0, 1.0];
    let mut combinations = 0;
    for model in [default_model, near_limit] {
        for lift in values {
            for gamma in values {
                for gain in values {
                    for offset in values {
                        let Some(wheels) = LookWheels::resolve(&rings(lift, gamma, gain, offset), &model) else {
                            continue; // все нули: шаг пропущен, нечего проверять
                        };
                        combinations += 1;
                        let drop = worst_lightness_drop(&wheels);
                        assert!(
                            drop <= 1e-6,
                            "кольца {lift}/{gamma}/{gain}/{offset} ({model:?}): L убывает на {drop}"
                        );
                    }
                }
            }
        }
    }
    assert_eq!(combinations, 2 * 80, "81 сочетание, из них одно нейтральное");
}

#[test]
fn the_monotonicity_check_can_fail_when_the_limit_is_ignored() {
    // Отрицательный контроль: кольцо вдвое сильнее предела (в обход validate) ломает тон хотя бы в одном сочетании.
    let default_model = WheelModelSettings::default();
    let too_strong = WheelModelSettings {
        ring_lightness_range: default_model.ring_monotonicity_limit() * 2.0,
        ..default_model
    };
    assert!(too_strong.validate().is_err());
    let worst = [-1.0_f32, 1.0]
        .into_iter()
        .flat_map(|gamma| [-1.0_f32, 1.0].into_iter().map(move |lift| (lift, gamma)))
        .map(|(lift, gamma)| worst_lightness_drop(&resolved(&rings(lift, gamma, -lift, 0.0), &too_strong)))
        .fold(0.0, f32::max);
    assert!(worst > 1e-3, "рампа осталась монотонной и без предела: {worst}");
}

// ---------- стадия цвета ----------

fn resolve_color(settings: &LookSettings) -> Option<LookColorParams> {
    LookColorParams::resolve(settings, &LookModelSettings::default(), &Primaries::REC_2020)
        .unwrap()
        .params
}

fn blue_lift() -> ColorWheels {
    only(WheelZone::Lift, wheel(265.0, 1.0, 0.0))
}

/// Пиксели для сверки: тёмные, средние, светлые, цветные.
fn sample_pixels() -> Vec<[f32; 3]> {
    let mut pixels: Vec<[f32; 3]> = [0.0, 0.004, 0.02, 0.1, 0.2, 0.5, 0.9, 1.0]
        .iter()
        .map(|&y| [y; 3])
        .collect();
    pixels.extend([[0.3, 0.1, 0.05], [0.05, 0.2, 0.4], [0.7, 0.6, 0.1], [0.02, 0.02, 0.09]]);
    pixels
}

#[test]
fn wheels_alone_enable_the_colour_stage_and_equal_the_oklab_roundtrip_of_the_shifted_lab() {
    let wheels = ColorWheels {
        lift: wheel(265.0, 1.0, 0.5),
        gain: wheel(60.0, 0.8, -0.3),
        ..ColorWheels::NEUTRAL
    };
    let params = resolve_color(&settings_with(|s| s.wheels = wheels)).expect("колёса включают стадию");
    let resolved_wheels = params.wheels.expect("колёса разрешены");
    let mut moved_somewhere = false;
    for pixel in sample_pixels() {
        let shifted_lab = resolved_wheels.apply(workspace().to_lab(pixel));
        let expected = workspace().to_rgb(shifted_lab);
        let actual = params.apply(pixel);
        for channel in 0..3 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 2e-6,
                "{pixel:?} -> {actual:?} против {expected:?}"
            );
        }
        moved_somewhere |= (0..3).any(|channel| (actual[channel] - pixel[channel]).abs() > 1e-3);
    }
    assert!(moved_somewhere, "колёса ничего не сдвинули");
}

#[test]
fn neutral_wheels_leave_the_pre_wheels_colour_path_bit_for_bit() {
    let touched = settings_with(|s| {
        s.saturation = knob(0.5);
        s.vibrance = knob(0.3);
        s.wheels = blue_lift();
    });
    let neutral = settings_with(|s| {
        s.saturation = knob(0.5);
        s.vibrance = knob(0.3);
    });
    let without_wheels = resolve_color(&neutral).unwrap();
    assert!(
        without_wheels.wheels.is_none(),
        "нейтральные колёса должны пропускать шаг"
    );
    // Те же ручки, но колёса тронуты и потом убраны из разрешённых параметров: путь до 14i.
    let mut stripped = resolve_color(&touched).unwrap();
    assert!(stripped.wheels.is_some());
    let with_wheels = stripped;
    stripped.wheels = None;
    let mut wheels_changed_picture = false;
    for pixel in sample_pixels() {
        assert_eq!(without_wheels.apply(pixel), stripped.apply(pixel), "{pixel:?}");
        wheels_changed_picture |= with_wheels.apply(pixel) != stripped.apply(pixel);
    }
    assert!(
        wheels_changed_picture,
        "включённые колёса не изменили ни одного пикселя"
    );
}

#[test]
fn wheels_run_before_saturation_so_full_desaturation_removes_the_wheel_tint() {
    // Нейтральный тёмный серый внутри зоны Lift (Y = 0,02 → L ≈ 0,27 < L_g).
    let dark_grey = [0.02_f32; 3];
    let tinted = resolve_color(&settings_with(|s| s.wheels = blue_lift())).unwrap();
    let (_, tint_chroma, _) = lch(tinted.apply(dark_grey));
    assert!(
        tint_chroma > 0.01,
        "Lift в синеву не покрасил тёмный серый: {tint_chroma}"
    );

    let both = resolve_color(&settings_with(|s| {
        s.wheels = blue_lift();
        s.saturation = knob(-1.0);
    }))
    .unwrap();
    let (_, final_chroma, _) = lch(both.apply(dark_grey));
    assert!(
        final_chroma < 5e-4,
        "насыщенность не увидела цвет колеса (порядок нарушен): цветность {final_chroma}"
    );
}

#[test]
fn rings_only_move_lightness_and_keep_every_grey_grey() {
    let params = resolve_color(&settings_with(|s| s.wheels = rings(1.0, -0.5, 1.0, 0.0))).unwrap();
    let mut max_lightness_shift = 0.0_f32;
    for step in 0..60 {
        let level = 0.003 * 1.12_f32.powi(step);
        let (before, ..) = lch([level; 3]);
        let [lightness, a, b] = workspace().to_lab(params.apply([level; 3]));
        assert!(a.hypot(b) < 2e-4, "серый {level} окрасился кольцами: a={a} b={b}");
        max_lightness_shift = max_lightness_shift.max((lightness - before).abs());
    }
    assert!(
        max_lightness_shift > 0.03,
        "кольца не сдвинули светлоту: {max_lightness_shift}"
    );
}

#[test]
fn zone_zeros_are_exact_for_every_valid_pivot_not_only_the_default() {
    // Свойство «ровно 0 на сером и выше / ниже» при ЛЮБОМ допустимом сером, не только при умолчании. Ревью 14i
    // указало: `L · (1/L_g)` на сером округляется в 0,99999994 (так у ~16 % чисел f32). Замер показал, что
    // smoothstep такого t в f32 всё равно даёт ровно 1, — старая запись тест тоже проходила; окно переписано
    // через `L − L_g`, чтобы точность держалась конструкцией, а не округлением. Тест — страж свойства.
    let touched = only(WheelZone::Gamma, ring_only(0.5));
    for step in 0..=700 {
        let gray = 0.001 + 0.001 * step as f32;
        let model = WheelModelSettings {
            zone_pivot_display_gray: gray,
            ..WheelModelSettings::default()
        };
        if model.validate().is_err() {
            continue;
        }
        let wheels = LookWheels::resolve(&touched, &model).unwrap();
        let pivot = wheels.pivot_lightness;
        for lightness in [pivot, f32::from_bits(pivot.to_bits() + 1), pivot + 0.01, 1.0, 1.3] {
            assert_eq!(
                wheels.zone_weights(lightness).shadows,
                0.0,
                "серый {gray}, L {lightness}"
            );
        }
        for lightness in [pivot, f32::from_bits(pivot.to_bits() - 1), pivot - 0.01, 0.0, -0.2] {
            assert_eq!(
                wheels.zone_weights(lightness).highlights,
                0.0,
                "серый {gray}, L {lightness}"
            );
        }
        let at_pivot = wheels.zone_weights(pivot);
        assert_eq!(at_pivot.midtones, 1.0, "серый {gray}");
    }
}
