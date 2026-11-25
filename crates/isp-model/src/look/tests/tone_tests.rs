//! Стадия тона вида (контраст, средние тона, тени, света) — решения 236, 242–244.
//!
//! Требования владельца, каждое тестом: чёрное остаётся чёрным, белое — белым; кривая растёт на всех краях; серый
//! двигают только средние тона; нейтраль пропускается; сила в обе стороны не ниже выбранной; цвет тянется ровно на
//! заданную долю, каналы не вылетают за `0…1`. Одинаковость на основах и пороги — `tests/look_knob_strength.rs`.

use look_contract::LookSettings;

use super::{hue_distance, knob, lch, settings_with};
use crate::look::model::{InvalidLookModel, LookModelSettings};
use crate::look::range::KnobRange;
use crate::look::tone_shape::{LookTone, ToneShapeSettings};
use crate::look::{LookControls, LookParams};
use crate::transfer::srgb_encode;

/// Ход ручек теста: контраст, средние тона, тени, света.
type Travel = [f32; 4];

fn settings(travel: Travel) -> LookSettings {
    settings_with(|look| {
        look.contrast = knob(travel[0]);
        look.midtones = knob(travel[1]);
        look.shadows = knob(travel[2]);
        look.highlights = knob(travel[3]);
    })
}

fn tone_with(travel: Travel, model: &ToneShapeSettings) -> LookTone {
    LookTone::resolve(&settings(travel), model).unwrap()
}

fn tone(travel: Travel) -> LookTone {
    tone_with(travel, &ToneShapeSettings::default())
}

/// Все сочетания краёв и нуля, кроме нейтрали.
fn corners() -> Vec<Travel> {
    let edges = [-1.0_f32, 0.0, 1.0];
    let mut travels = Vec::new();
    for contrast in edges {
        for midtones in edges {
            for shadows in edges {
                for highlights in edges {
                    travels.push([contrast, midtones, shadows, highlights]);
                }
            }
        }
    }
    travels.retain(|travel| travel.iter().any(|value| *value != 0.0));
    travels
}

/// Наибольший сдвиг кода экрана из 255 по серому клину: `(вверх, вниз)`.
fn code_shift(tone: &LookTone) -> (f32, f32) {
    let mut shift = (0.0_f32, 0.0_f32);
    for step in 0..=4096 {
        let light = step as f32 / 4096.0;
        let moved = 255.0 * (srgb_encode(tone.curve(light)) - srgb_encode(light));
        shift = (shift.0.max(moved), shift.1.min(moved));
    }
    shift
}

#[test]
fn black_stays_black_and_white_stays_white_at_every_corner() {
    for travel in corners() {
        let tone = tone(travel);
        assert_eq!(tone.apply([0.0; 3]), [0.0; 3], "{travel:?}");
        assert_eq!(tone.apply([1.0; 3]), [1.0; 3], "{travel:?}");
        // И подход к концам без скачка: рядом с чёрным — чёрное, рядом с белым — белое.
        assert!(tone.curve(1e-9) < 1e-3, "{travel:?}: {}", tone.curve(1e-9));
        assert!(tone.curve(1.0 - 1e-6) > 0.97, "{travel:?}: {}", tone.curve(1.0 - 1e-6));
    }
}

#[test]
fn the_curve_grows_at_every_corner() {
    for travel in corners() {
        let tone = tone(travel);
        let mut previous = 0.0_f32;
        for step in 1..=20_000 {
            // Шаг по √света: тени разрешены так же мелко, как света.
            let light = (step as f32 / 20_000.0).powi(2);
            let value = tone.curve(light);
            assert!(
                value >= previous - 2e-7 && (0.0..=1.0).contains(&value),
                "{travel:?}: при {light} стало {value} после {previous}"
            );
            previous = value;
        }
    }
}

#[test]
fn the_strongest_allowed_edges_still_give_a_growing_curve() {
    let widest = KnobRange::symmetric(6.0);
    let model = ToneShapeSettings {
        contrast: widest,
        midtones: widest,
        shadows: widest,
        highlights: widest,
        ..ToneShapeSettings::default()
    };
    assert_eq!(model.validate(), Ok(()));
    for travel in corners() {
        let tone = tone_with(travel, &model);
        let mut previous = 0.0_f32;
        for step in 1..=4000 {
            let value = tone.curve((step as f32 / 4000.0).powi(2));
            assert!(
                value >= previous - 1e-6,
                "{travel:?}, шаг {step}: {value} после {previous}"
            );
            previous = value;
        }
    }
}

#[test]
fn only_the_midtones_knob_moves_the_display_grey() {
    let grey = ToneShapeSettings::default().pivot_display_gray;
    for travel in corners().into_iter().filter(|travel| travel[1] == 0.0) {
        let moved = tone(travel).curve(grey);
        assert!((moved - grey).abs() < 1e-6, "{travel:?}: серый {grey} стал {moved}");
    }
    assert!(tone([0.0, 1.0, 0.0, 0.0]).curve(grey) > 0.6);
    assert!(tone([0.0, -1.0, 0.0, 0.0]).curve(grey) < 0.02);
}

#[test]
fn the_curve_has_no_kink_at_the_grey_where_shadows_meet_highlights() {
    let model = ToneShapeSettings::default();
    let q = f64::from(model.pivot_display_gray.sqrt());
    for travel in corners() {
        let tone = tone(travel);
        // Наклоны слева и справа от серого в координате t = √света.
        let at = |t: f64| f64::from(tone.curve((t * t) as f32)).sqrt();
        // Односторонние разности отличаются на вторую производную × шаг даже у гладкой кривой, а перелом не
        // уменьшается с шагом. Поэтому смотрим на два шага: разрыв наклона держался бы одинаковым, а у гладкой
        // кривой он убывает вместе с шагом.
        let mismatch = |step: f64| {
            let (left, right) = ((at(q) - at(q - step)) / step, (at(q + step) - at(q)) / step);
            (left / right - 1.0).abs()
        };
        let (wide, narrow) = (mismatch(2e-3), mismatch(5e-4));
        assert!(
            narrow < 0.025 && narrow < 0.6 * wide.max(0.01),
            "{travel:?}: расхождение наклонов {wide} на шаге 2e-3 и {narrow} на 5e-4 — перелом, а не кривизна"
        );
    }
}

#[test]
fn shadows_change_only_the_dark_part_and_highlights_only_the_bright_part() {
    let grey = ToneShapeSettings::default().pivot_display_gray;
    for side in [-1.0, 1.0] {
        let (shadows, highlights) = (tone([0.0, 0.0, side, 0.0]), tone([0.0, 0.0, 0.0, side]));
        for step in 1..100 {
            let light = step as f32 / 100.0;
            let (by_shadows, by_highlights) = (shadows.curve(light) - light, highlights.curve(light) - light);
            if light > grey {
                assert!(by_shadows.abs() < 1e-6, "тени {side} тронули {light}: {by_shadows}");
                assert!(
                    by_highlights * side > 0.0 || light > 0.995,
                    "света {side} не тронули {light}"
                );
            } else if light < grey - 1e-3 {
                assert!(
                    by_highlights.abs() < 1e-6,
                    "света {side} тронули {light}: {by_highlights}"
                );
                assert!(by_shadows * side > 0.0, "тени {side} не тронули {light}");
            }
        }
    }
}

/// Сила краёв стадии тона на умолчаниях, замер по серому клину (коды из 255). Решение 242 выбрало края по снимкам
/// («контраст ≈ ±51, средние ≈ ±125»): числа умолчаний (2,32 и 3,0) дают 50,0 и 122,0 — в пределах 2,5 % от
/// выбранного. Сквозную силу на всех основах и пороги 90 % от выбранного держит `tests/look_knob_strength.rs`; этот
/// тест ловит случайное изменение самой формулы стадии (допуск — 1 код).
#[test]
fn every_tone_edge_has_the_strength_the_owner_chose() {
    for (name, travel, up, down) in [
        ("контраст +", [1.0, 0.0, 0.0, 0.0], 50.0, -49.7),
        ("контраст −", [-1.0, 0.0, 0.0, 0.0], 49.7, -50.0),
        ("средние +", [0.0, 1.0, 0.0, 0.0], 122.0, 0.0),
        ("средние −", [0.0, -1.0, 0.0, 0.0], 0.0, -122.0),
        ("тени +", [0.0, 0.0, 1.0, 0.0], 66.1, 0.0),
        ("тени −", [0.0, 0.0, -1.0, 0.0], 0.0, -25.6),
        ("света +", [0.0, 0.0, 0.0, 1.0], 22.5, 0.0),
        ("света −", [0.0, 0.0, 0.0, -1.0], 0.0, -65.0),
    ] {
        let shift = code_shift(&tone(travel));
        assert!(
            (shift.0 - up).abs() < 1.0 && (shift.1 - down).abs() < 1.0,
            "{name}: сдвиг {shift:?}, ждали ({up}, {down})"
        );
    }
}

#[test]
fn a_grey_pixel_stays_grey_at_every_corner() {
    for travel in corners() {
        let tone = tone(travel);
        for light in [0.003, 0.02, 0.1, 0.2, 0.5, 0.9] {
            let [red, green, blue] = tone.apply([light; 3]);
            assert!(
                red == green && green == blue,
                "{travel:?}: {light} → {red} {green} {blue}"
            );
        }
    }
}

/// Каналы цвета внутри `0…1` остаются внутри: в отличие от растяжки светлоты OKLab, яркому насыщенному цвету не
/// нужна обрезка (её сдвиг оттенка на коже доходил до 18–21° — замер 14l).
#[test]
fn colours_inside_the_gamut_never_leave_it() {
    for travel in corners() {
        let tone = tone(travel);
        for pixel in [
            [1.0, 0.3, 0.05],
            [0.9, 0.62, 0.5],
            [0.02, 0.0, 0.3],
            [0.0, 1.0, 0.0],
            [0.4, 0.41, 0.39],
        ] {
            let mapped = tone.apply(pixel);
            assert!(
                mapped.iter().all(|channel| (-1e-6..=1.0 + 1e-6).contains(channel)),
                "{travel:?}: {pixel:?} → {mapped:?}"
            );
        }
    }
}

/// Доля цвета: 0 — цвет не меняется вовсе (отношения каналов прежние), 1 — насыщенность тянется за контрастом,
/// выбранная половина — ровно посередине.
#[test]
fn colour_follow_mixes_the_unchanged_and_the_stretched_colour() {
    let pixel = [0.5, 0.2, 0.1];
    let with_follow = |color_follow: f32| {
        let model = ToneShapeSettings {
            color_follow,
            ..ToneShapeSettings::default()
        };
        tone_with([0.6, 0.0, 0.0, 0.0], &model).apply(pixel)
    };
    let (similar, half, stretched) = (with_follow(0.0), with_follow(0.5), with_follow(1.0));
    for channel in 1..3 {
        let (before, after) = (pixel[channel] / pixel[0], similar[channel] / similar[0]);
        assert!(
            (before - after).abs() < 1e-6,
            "подобный цвет сменил отношение: {similar:?}"
        );
    }
    let chroma = |rgb: [f32; 3]| (rgb[0] - rgb[2]) / rgb[0];
    assert!(
        chroma(stretched) > chroma(similar) + 0.05,
        "{stretched:?} против {similar:?}"
    );
    for channel in 0..3 {
        let middle = 0.5 * (similar[channel] + stretched[channel]);
        assert!((half[channel] - middle).abs() < 1e-6, "{half:?}, ждали {middle}");
    }
}

#[test]
fn hue_of_skin_sky_and_foliage_holds_on_the_contrast_edges() {
    for (name, pixel) in [
        ("кожа", [0.55, 0.33, 0.22]),
        ("небо", [0.25, 0.4, 0.7]),
        ("листва", [0.12, 0.22, 0.05]),
    ] {
        let (_, _, before) = lch(pixel);
        for side in [-1.0, 1.0] {
            let (_, chroma, after) = lch(tone([side, 0.0, 0.0, 0.0]).apply(pixel));
            assert!(chroma > 0.02, "{name}: цвет пропал ({chroma})");
            let shift = hue_distance(before, after);
            assert!(shift < 6.0, "{name}, контраст {side}: сдвиг оттенка {shift}°");
        }
    }
}

/// За охватом (AgX отдаёт и −0,008, и 1,067) стадия не взрывается: выход ограничен на любом краю.
#[test]
fn slightly_out_of_range_channels_stay_bounded_and_finite() {
    for travel in corners() {
        let tone = tone(travel);
        for pixel in [
            [0.3, 0.5, -0.008],
            [1e-6, -0.008, -0.004],
            [1.067, 0.9, 0.2],
            [-0.01, -0.02, -0.03],
            [1.2, 1.1, 1.05],
        ] {
            let mapped = tone.apply(pixel);
            assert!(
                mapped
                    .iter()
                    .all(|channel| channel.is_finite() && (-0.2..=1.3).contains(channel)),
                "{travel:?}: {pixel:?} → {mapped:?}"
            );
        }
    }
}

#[test]
fn neutral_tone_knobs_skip_the_stage_and_colour_knobs_do_not_enable_it() {
    let model = ToneShapeSettings::default();
    assert_eq!(LookTone::resolve(&LookSettings::NEUTRAL, &model), None);
    assert_eq!(
        LookTone::resolve(&settings_with(|look| look.saturation = knob(1.0)), &model),
        None
    );
    let working = crate::color::chromaticity::Primaries::REC_2020;
    let resolve = |look: LookSettings| {
        LookParams::resolve(
            &LookControls::new(look, std::sync::Arc::new(LookModelSettings::default())),
            &working,
        )
        .unwrap()
    };
    assert!(resolve(LookSettings::NEUTRAL).is_neutral());
    let touched = resolve(settings([0.01, 0.0, 0.0, 0.0]));
    assert!(touched.tone.is_some() && !touched.is_neutral() && !touched.has_no_display_stages());
    let pixel = [0.3, 0.1, 0.05];
    assert_ne!(
        touched.apply_to_pixel(pixel),
        pixel,
        "даже одна единица шкалы доходит до пикселя"
    );
}

/// Тон — первая стадия вида: колёса и насыщенность видят уже сдвинутый тоном свет (что на экране, то и красят).
#[test]
fn tone_runs_before_the_colour_stage() {
    let model = std::sync::Arc::new(LookModelSettings::default());
    let working = crate::color::chromaticity::Primaries::REC_2020;
    let params = LookParams::resolve(
        &LookControls::new(
            settings_with(|look| {
                look.midtones = knob(-0.5);
                look.saturation = knob(0.5);
            }),
            model,
        ),
        &working,
    )
    .unwrap();
    let pixel = [0.4, 0.2, 0.1];
    let toned = params.tone.unwrap().apply(pixel);
    assert_eq!(params.apply_to_pixel(pixel), params.color.unwrap().apply(toned));
}

#[test]
fn every_bad_number_of_the_tone_model_is_rejected_by_its_own_name() {
    let default = ToneShapeSettings::default;
    let bad = KnobRange {
        minus: f32::NAN,
        plus: 1.0,
    };
    let cases = [
        (
            "tone.contrast",
            ToneShapeSettings {
                contrast: bad,
                ..default()
            },
        ),
        (
            "tone.midtones",
            ToneShapeSettings {
                midtones: KnobRange::symmetric(6.5),
                ..default()
            },
        ),
        (
            "tone.shadows",
            ToneShapeSettings {
                shadows: KnobRange::symmetric(-0.1),
                ..default()
            },
        ),
        (
            "tone.highlights",
            ToneShapeSettings {
                highlights: bad,
                ..default()
            },
        ),
        (
            "tone.color_follow",
            ToneShapeSettings {
                color_follow: 1.01,
                ..default()
            },
        ),
        (
            "tone.pivot_display_gray",
            ToneShapeSettings {
                pivot_display_gray: 0.9,
                ..default()
            },
        ),
    ];
    for (name, tone) in cases {
        let error = LookModelSettings {
            tone,
            ..LookModelSettings::default()
        }
        .validate();
        assert!(
            matches!(error, Err(InvalidLookModel::OutOfRange { field, .. }) if field == name),
            "{name}: {error:?}"
        );
    }
}
