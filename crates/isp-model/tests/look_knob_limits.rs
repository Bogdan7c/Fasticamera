//! Пределы ручек вида (сессия 14l, пакет A), которые остаются фактами после перехода на `KnobRange` и стадию тона:
//! с какого хода ручки BANDS HUE начинается складка круга оттенков, как теплота сжимает ход на холодном свете,
//! где предел кольца колеса и что отвергает проверка модели. «Кривая основы не строится на краю ручки» больше не
//! бывает — тон вида отдельная стадия (`look::tone_shape`), а край подтяжки AE проверяет `validate_tone_edges`
//! (`src/look/tests/validation_tests.rs`). Сила ручек — `look_knob_strength.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "интеграционный тест: падение — способ сообщить о провале, таблица печатается для владельца"
)]

mod support;

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::look::bands::{LookBands, resolve_bands};
use isp_model::look::model::{InvalidLookModel, LookModelSettings};
use isp_model::look::range::KnobRange;
use isp_model::look::wheels::{InvalidWheelModel, WheelModelSettings};
use isp_model::look::white::{WhiteShiftLimit, shift_white};
use isp_model::look::white_shift;
use look_contract::{BandAdjustment, BandAdjustments, HueBandId, LookSettings};
use support::knob_strength::knob;

// ---------- складка BANDS HUE ----------

/// Шаг сетки оттенка при поиске складки, градусы.
const HUE_GRID_STEP_DEGREES: f32 = 0.05;
/// Наибольший сдвиг оттенка полосы, который допускает проверка модели, градусы.
const MAX_BAND_HUE_RANGE_DEGREES: f32 = 90.0;

/// Модель с диапазоном сдвига полосы 90° и разрешённые полосы с одним сдвигом полосы `band` на `hue_degrees`.
fn bands_with_hue_shift(band: &str, hue_degrees: f32) -> LookBands {
    let model = LookModelSettings {
        band_hue_degrees: KnobRange::symmetric(MAX_BAND_HUE_RANGE_DEGREES),
        ..LookModelSettings::default()
    };
    model.validate().expect("модель с диапазоном полосы 90° допустима");
    let adjustments = BandAdjustments::new(vec![BandAdjustment {
        band: HueBandId::new(band).unwrap(),
        hue: knob(hue_degrees / MAX_BAND_HUE_RANGE_DEGREES),
        saturation: knob(0.0),
        luminance: knob(0.0),
    }])
    .unwrap();
    resolve_bands(&model, &adjustments)
        .bands
        .expect("полоса тронута: стадия полос не пропущена")
}

/// Карта оттенков `θ → θ + Δh(θ)` складывается (порядок цветов переворачивается): где-то на круге её наклон < 0.
fn hue_map_folds(bands: &LookBands) -> bool {
    let steps = (360.0 / HUE_GRID_STEP_DEGREES) as u32;
    let step_radians = HUE_GRID_STEP_DEGREES.to_radians();
    let mapped = |index: u32| {
        let hue = index as f32 * step_radians;
        f64::from(hue + bands.effect_at(hue).hue_shift_radians)
    };
    // Допуск на шум f32 (≈ 5·10⁻⁷ рад на шаге 8,7·10⁻⁴ рад): наклон ниже −0,01 — настоящая складка.
    let slope_floor = -0.01 * f64::from(step_radians);
    (0..steps).any(|index| mapped(index + 1) - mapped(index) < slope_floor)
}

/// Наибольший сдвиг полосы `band` в `sign`-сторону без складки, градусы (деление отрезка пополам).
fn max_unfolded_hue_shift_degrees(band: &str, sign: f32) -> f32 {
    let (mut safe, mut folded) = (0.0_f32, MAX_BAND_HUE_RANGE_DEGREES);
    for _ in 0..24 {
        let middle = 0.5 * (safe + folded);
        if hue_map_folds(&bands_with_hue_shift(band, sign * middle)) {
            folded = middle;
        } else {
            safe = middle;
        }
    }
    safe
}

/// Предел складки, посчитанный по формуле: сдвиг полосы на `Δ` вперёд складывает круг в промежутке до следующего
/// центра, когда `Δ > 2·промежуток/π`; сдвиг назад — в промежутке до предыдущего. Градусы; центры — из модели.
fn analytic_fold_limit_degrees(band: &str, sign: f32) -> f32 {
    let mut centers: Vec<(String, f32)> = LookModelSettings::default()
        .bands
        .iter()
        .map(|definition| (definition.id.as_str().to_owned(), definition.center_degrees))
        .collect();
    centers.sort_by(|left, right| left.1.total_cmp(&right.1));
    let index = centers
        .iter()
        .position(|(id, _)| id == band)
        .expect("полоса есть в модели");
    let count = centers.len();
    let circular_gap = |from: usize, to: usize| (centers[to % count].1 - centers[from % count].1).rem_euclid(360.0);
    let gap = if sign > 0.0 {
        circular_gap(index, index + 1)
    } else {
        circular_gap(index + count - 1, index)
    };
    2.0 * gap / std::f32::consts::PI
}

/// С какого хода ручки (доля края ±60°) начинается складка, по формуле `2·промежуток/π`, по сторонам:
/// (полоса, на +, на −). Промежутки берутся из центров полос по умолчанию (29, 60, 105, 142, 195, 264, 305, 345°).
/// Все 16 сторон складывают до края ручки (самая поздняя — 73 %: aqua на плюсе и blue на минусе): это выбор
/// владельца (решение 245), так допускают и редакторы; тест фиксирует, с какого хода начинается складка.
const FOLD_START_TRAVEL_PERCENT: [(&str, f32, f32); 8] = [
    ("red", 32.9, 46.7),
    ("orange", 47.7, 32.9),
    ("yellow", 39.3, 47.7),
    ("green", 56.2, 39.3),
    ("aqua", 73.2, 56.2),
    ("blue", 43.5, 73.2),
    ("purple", 42.4, 43.5),
    ("magenta", 46.7, 42.4),
];

#[test]
fn band_hue_fold_starts_where_the_formula_says_and_is_allowed_up_to_the_edge() {
    let edge = LookModelSettings::default().band_hue_degrees;
    assert_eq!(edge, KnobRange::symmetric(60.0));
    // Складка у края — решение владельца (245), модель с ним годна.
    assert_eq!(LookModelSettings::default().validate(), Ok(()));
    println!(
        "\nBANDS HUE: с какого хода ручки (% края ±{}°) начинается складка круга оттенков",
        edge.plus
    );
    println!(
        "{:<9}{:>10}{:>10}{:>14}{:>14}",
        "полоса", "+ °", "− °", "+ % хода", "− % хода"
    );
    for (band, expected_plus, expected_minus) in FOLD_START_TRAVEL_PERCENT {
        let mut percents = Vec::new();
        for (sign, expected_percent, edge_degrees) in
            [(1.0_f32, expected_plus, edge.plus), (-1.0, expected_minus, edge.minus)]
        {
            let measured = max_unfolded_hue_shift_degrees(band, sign);
            let formula = analytic_fold_limit_degrees(band, sign);
            // Численная карта на плотной сетке сходится с формулой.
            assert!(
                (measured - formula).abs() <= 0.5,
                "{band} {sign:+}: карта {measured:.1}°, формула {formula:.1}°"
            );
            // Сам предел различим: чуть меньше — порядок цветов цел, чуть больше — сложен.
            assert!(
                !hue_map_folds(&bands_with_hue_shift(band, sign * (measured - 0.5))),
                "{band} {sign:+}: ниже предела уже складывает"
            );
            assert!(
                hue_map_folds(&bands_with_hue_shift(band, sign * (measured + 0.5))),
                "{band} {sign:+}: выше предела не складывает"
            );
            let percent = 100.0 * formula / edge_degrees;
            assert!(
                (percent - expected_percent).abs() <= 1.0,
                "{band} {sign:+}: складка с {percent:.1} % хода, в таблице {expected_percent} %"
            );
            percents.push(percent);
        }
        println!(
            "{band:<9}{:>10.1}{:>10.1}{:>13.0}%{:>13.0}%",
            analytic_fold_limit_degrees(band, 1.0),
            analytic_fold_limit_degrees(band, -1.0),
            percents[0],
            percents[1]
        );
    }
}

#[test]
fn bands_hue_at_the_edge_folds_on_every_side_as_the_table_says() {
    let edge = LookModelSettings::default().band_hue_degrees;
    for (band, expected_plus, expected_minus) in FOLD_START_TRAVEL_PERCENT {
        for (sign, percent, edge_degrees) in [(1.0_f32, expected_plus, edge.plus), (-1.0, expected_minus, edge.minus)] {
            let folds_at_edge = hue_map_folds(&bands_with_hue_shift(band, sign * edge_degrees));
            assert!(
                folds_at_edge && percent < 100.0,
                "{band} {sign:+}: на краю складка = {folds_at_edge}, в таблице начинается с {percent} %"
            );
        }
    }
}

// ---------- теплота: сжатый ход на холодном свете ----------

/// Свет сцены заданной температуры на локусе Планка (Duv 0).
fn planckian_white(kelvin: f64) -> Chromaticity {
    chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .unwrap()
}

/// Белая для рисования при ходе теплоты `travel` на свете `kelvin` по настоящей модели (`look::white_shift`).
fn warmth_white(kelvin: f64, travel: f32) -> (Chromaticity, WhiteShiftLimit) {
    let settings = LookSettings {
        warmth: knob(travel),
        ..LookSettings::NEUTRAL
    };
    let shift = white_shift(&LookModelSettings::default(), &settings);
    let rendering = shift_white(planckian_white(kelvin), shift).unwrap();
    (rendering.white, rendering.limit)
}

fn distance(first: Chromaticity, second: Chromaticity) -> f64 {
    (first.x - second.x).hypot(first.y - second.y)
}

/// Теплота меняет белую НА ВСЁМ ходу на любом свете (сжатие хода, а не обрезка): каждый шаг по 10 % уводит белую
/// дальше от несдвинутой, и обе стороны хода живые. Сжатие видно в исходе ровно там, где до края таблицы ближе края ручки.
#[test]
fn warmth_changes_the_white_on_the_whole_travel_at_every_light_and_compresses_only_on_cold_light() {
    println!("\nWARMTH ±150 майред: свет, сторона, исход на краю хода");
    for kelvin in [2850.0, 5500.0, 6600.0, 8000.0, 10_000.0] {
        let unshifted = planckian_white(kelvin);
        for sign in [-1.0_f32, 1.0] {
            let mut previous = 0.0_f64;
            for step in 1..=10 {
                let (white, _) = warmth_white(kelvin, sign * step as f32 / 10.0);
                let moved = distance(white, unshifted);
                assert!(
                    moved > previous + 1e-4,
                    "{kelvin} К, ход {:+}0 %: белая отошла на {moved}, на шаге раньше {previous} — мёртвый участок",
                    sign * step as f32
                );
                previous = moved;
            }
            let (_, limit) = warmth_white(kelvin, sign);
            println!("{kelvin:>7} К {sign:+}: {limit:?}");
            // Сжат только ход «теплее» (минус майреды) на холодном свете: 8000 К = 125 и 10 000 К = 100 майред.
            let compressed = matches!(limit, WhiteShiftLimit::TravelCompressed { .. });
            assert_eq!(
                compressed,
                sign > 0.0 && kelvin >= 8000.0,
                "{kelvin} К {sign:+}: {limit:?}"
            );
        }
    }
}

// ---------- кольцо колеса и проверка модели ----------

#[test]
fn the_default_ring_is_below_the_monotonicity_limit_and_validation_enforces_the_limit() {
    let wheels = WheelModelSettings::default();
    assert!(
        (wheels.zone_pivot_display_gray - 0.20).abs() < 1e-6,
        "умолчание серого колёс изменилось"
    );
    let limit = wheels.ring_monotonicity_limit();
    println!(
        "\nWHEELS: предел кольца при сером 0,20 = {limit:.4}, умолчание {}",
        wheels.ring_lightness_range
    );
    assert!(
        (0.136..0.1385).contains(&limit),
        "предел кольца {limit}, ждали около 0,138"
    );
    // Умолчание (0,135) — выбор владельца у самого предела, но строго ниже него.
    assert!(
        wheels.ring_lightness_range < limit,
        "умолчание {} не ниже предела {limit}",
        wheels.ring_lightness_range
    );
    assert_eq!(wheels.validate(), Ok(()));
    let ring = |value: f32| WheelModelSettings {
        ring_lightness_range: value,
        ..wheels
    };
    assert_eq!(
        ring(limit - 0.001).validate(),
        Ok(()),
        "кольцо чуть ниже предела должно проходить"
    );
    assert!(
        matches!(
            ring(limit + 0.001).validate(),
            Err(InvalidWheelModel::RingBreaksMonotonicity { .. })
        ),
        "кольцо чуть выше предела обязано отвергаться как поломка монотонности"
    );
}

#[test]
fn wheel_push_default_is_the_largest_allowed_and_above_it_is_rejected() {
    let push = |value: f32| WheelModelSettings {
        push_chroma_range: value,
        ..WheelModelSettings::default()
    };
    assert_eq!(WheelModelSettings::default().push_chroma_range, 0.2);
    assert_eq!(push(0.2).validate(), Ok(()));
    assert!(matches!(
        push(0.21).validate(),
        Err(InvalidWheelModel::OutOfRange {
            field: "push_chroma_range",
            ..
        })
    ));
}

/// Множитель `1 + величина` с минусом больше 1 стал бы отрицательным (цветность сменила бы знак): такой край
/// отвергается. Плюс больше 1 — законный «пересвет цветом» (до 4), он не заперт минусом, как было с одним диапазоном.
#[test]
fn a_multiplier_minus_above_one_is_rejected_and_a_plus_above_one_is_allowed() {
    type Edit = fn(&mut LookModelSettings, KnobRange);
    let knobs: [(&str, Edit, f32); 4] = [
        ("saturation", |model, range| model.saturation = range, 1.0),
        ("vibrance", |model, range| model.vibrance = range, 1.0),
        ("band_saturation", |model, range| model.band_saturation = range, 1.0),
        ("band_luminance", |model, range| model.band_luminance = range, 0.95),
    ];
    for (field, edit, largest_minus) in knobs {
        let with = |range: KnobRange| {
            let mut model = LookModelSettings::default();
            edit(&mut model, range);
            model.validate()
        };
        // Минус на самом пределе допустим, чуть дальше — нет.
        assert_eq!(
            with(KnobRange {
                minus: largest_minus,
                plus: 1.0
            }),
            Ok(()),
            "{field}: минус {largest_minus}"
        );
        match with(KnobRange {
            minus: largest_minus + 0.01,
            plus: 1.0,
        }) {
            Err(InvalidLookModel::OutOfRange { field: rejected, .. }) => assert_eq!(rejected, field),
            other => panic!("{field}: минус больше {largest_minus} должен отвергаться, получили {other:?}"),
        }
        // Плюс 2 при минусе 1 — допустим (модель по умолчанию у насыщенности именно такая).
        assert_eq!(with(KnobRange { minus: 0.5, plus: 2.0 }), Ok(()), "{field}: плюс 2");
    }
    assert_eq!(
        LookModelSettings::default().saturation,
        KnobRange { minus: 1.0, plus: 2.0 }
    );
}
