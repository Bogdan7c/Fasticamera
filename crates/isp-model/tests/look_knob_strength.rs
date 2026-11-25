//! Сила ручек вида на краю шкалы (сессия 14l, решения 242–246): каждая ручка обязана давать не меньше
//! выбранного владельцем сдвига на КАЖДОЙ основе тона, а сила на разных основах — почти одна.
//!
//! * Тон (CONTRAST, MIDTONES, SHADOWS, HIGHLIGHTS): серая рампа сцены (−10…+6 EV шагом 1/16 и белый точки) идёт через
//!   полную цепочку кадра (`IspParams::resolve` → основа → стадия вида → выход), мера — наибольший сдвиг кода
//!   экрана из 255 вверх/вниз относительно нейтрали той же основы.
//! * Цвет (насыщенность, сочность, теплота, оттенок, полосы, колёса): мишень из 24 патчей ColorChecker идёт через ВЕСЬ
//!   эталонный конвейер от RAW; мера — наибольший сдвиг кода канала (`support::color_chart`).
//!
//! Пороги — 90 % от выбранного владельцем (тон) или от измеренного минимума по основам (цвет): запас на округления,
//! но не на ослабление. Таблицы печатаются (`--nocapture`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "интеграционный тест: падение — способ сообщить о провале, таблица печатается для владельца"
)]

mod support;

use isp_model::look::model::LookModelSettings;
use isp_model::look::range::KnobRange;
use isp_model::look::tone_shape::ToneShapeSettings;
use isp_model::tone::base::ToneBase;
use look_contract::{
    BandAdjustment, BandAdjustments, ColorWheel, ColorWheels, HueBandId, LookKnob, LookSettings, WheelPush, WheelZone,
};
use support::color_chart::{ChartCodes, ChartShift};
use support::knob_strength::{EdgeShift, KnobBench, all_tone_bases, knob};

/// Шаги хода ручки при проверке «нет мёртвых участков»: 25 %, 50 %, 75 %, 100 %.
const TRAVEL_STEPS: [f32; 4] = [0.25, 0.5, 0.75, 1.0];
/// Наименьший прирост сдвига между соседними шагами хода, коды из 255: меньше — участок мёртвый.
const MIN_STEP_GROWTH_CODES: f32 = 0.05;

// ---------- тон ----------

/// Тон-ручка словаря: имя и как задать значение.
#[derive(Clone, Copy)]
struct ToneKnob {
    name: &'static str,
    set: fn(&mut LookSettings, LookKnob),
}

const TONE_KNOBS: [ToneKnob; 4] = [
    ToneKnob {
        name: "CONTRAST",
        set: |settings, value| settings.contrast = value,
    },
    ToneKnob {
        name: "MIDTONES",
        set: |settings, value| settings.midtones = value,
    },
    ToneKnob {
        name: "SHADOWS",
        set: |settings, value| settings.shadows = value,
    },
    ToneKnob {
        name: "HIGHLIGHTS",
        set: |settings, value| settings.highlights = value,
    },
];

/// Порог силы на краю, коды из 255: (ручка, на −1, на +1). Это 90 % выбранного владельцем (решение 242):
/// контраст ±51, средние ±125, тени +66 / −26, света +22 / −64.
const TONE_THRESHOLDS: [(&str, f32, f32); 4] = [
    ("CONTRAST", 46.0, 46.0),
    ("MIDTONES", 112.0, 112.0),
    ("SHADOWS", 23.0, 59.0),
    ("HIGHLIGHTS", 58.0, 20.0),
];

/// Наибольшая допустимая разница между основами в силе одного края, доля от наибольшей. Мерка владельца — «около
/// 10 %», но стадия тона работает над светом экрана, который отдала основа, и рампа сцены проходит через все его
/// значения у каждой основы: фактический разброс — доли процента (шаг рампы). Допуск 2 % ловит возврат к силе,
/// зависящей от основы (до 14l разброс был в разы), и не требует нуля от дискретной рампы.
const MAX_SPREAD_BETWEEN_BASES: f32 = 0.02;

fn threshold(table: &[(&str, f32, f32)], name: &str, sign: f32) -> f32 {
    let (_, minus, plus) = table.iter().find(|row| row.0 == name).copied().unwrap();
    if sign > 0.0 { plus } else { minus }
}

/// Только эта ручка стоит в `value`.
fn only(knob_under_test: ToneKnob, value: f32) -> LookSettings {
    let mut settings = LookSettings::NEUTRAL;
    (knob_under_test.set)(&mut settings, knob(value));
    settings
}

/// Сила ручки на ходу `travel` (со знаком): «в сторону хода» — вверх при плюсе, вниз при минусе; у контраста — обе
/// стороны кривой разом, берётся меньшая (раскрыть надо и света, и тени).
fn strength_of(shift: &EdgeShift, knob_name: &str, travel: f32) -> f32 {
    match (knob_name, travel > 0.0) {
        ("CONTRAST", _) => shift.up_codes.min(-shift.down_codes),
        (_, true) => shift.up_codes,
        (_, false) => -shift.down_codes,
    }
}

fn tone_strength(
    bench: &KnobBench,
    base: &ToneBase,
    knob_under_test: ToneKnob,
    travel: f32,
    model: &LookModelSettings,
) -> f32 {
    let shift = bench
        .measure(base, &only(knob_under_test, travel), model)
        .unwrap_or_else(|reason| panic!("{} {travel:+}: {reason}", knob_under_test.name));
    strength_of(&shift, knob_under_test.name, travel)
}

/// Сила на краю ±1 по основам: (имя основы, сила) для каждой из четырёх.
fn edge_strengths(
    bench: &KnobBench,
    knob_under_test: ToneKnob,
    sign: f32,
    model: &LookModelSettings,
) -> Vec<(&'static str, f32)> {
    all_tone_bases()
        .into_iter()
        .map(|(name, base)| (name, tone_strength(bench, &base, knob_under_test, sign, model)))
        .collect()
}

#[test]
fn every_tone_knob_reaches_its_threshold_on_every_base() {
    let bench = KnobBench::new();
    let model = LookModelSettings::default();
    println!("\nСила тон-ручек на краю (умолчания, P3, без подтяжки AE), коды из 255: порог | AgX ACES filmic Legacy");
    let mut failures = Vec::new();
    for knob_under_test in TONE_KNOBS {
        for sign in [-1.0_f32, 1.0] {
            let needed = threshold(&TONE_THRESHOLDS, knob_under_test.name, sign);
            let strengths = edge_strengths(&bench, knob_under_test, sign, &model);
            let cells: Vec<String> = strengths.iter().map(|(_, value)| format!("{value:6.1}")).collect();
            println!(
                "{:<11}{sign:+} {needed:>6.0} | {}",
                knob_under_test.name,
                cells.join(" ")
            );
            for (base_name, strength) in strengths {
                if strength < needed {
                    failures.push(format!(
                        "{} {sign:+} на {base_name}: {strength:.1} < порога {needed}",
                        knob_under_test.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "ручка слабее порога: {failures:#?}");
}

#[test]
fn tone_strength_is_nearly_the_same_on_every_base() {
    let bench = KnobBench::new();
    let model = LookModelSettings::default();
    let mut failures = Vec::new();
    println!("\nРазброс силы тон-ручек между основами (наибольший − наименьший) / наибольший");
    for knob_under_test in TONE_KNOBS {
        for sign in [-1.0_f32, 1.0] {
            let strengths = edge_strengths(&bench, knob_under_test, sign, &model);
            let strongest = strengths.iter().map(|(_, value)| *value).fold(f32::MIN, f32::max);
            let weakest = strengths.iter().map(|(_, value)| *value).fold(f32::MAX, f32::min);
            let spread = (strongest - weakest) / strongest;
            println!(
                "{:<11}{sign:+} {:.1} % ({weakest:.1}…{strongest:.1})",
                knob_under_test.name,
                spread * 100.0
            );
            if spread > MAX_SPREAD_BETWEEN_BASES {
                failures.push(format!(
                    "{} {sign:+}: разброс {:.1} %",
                    knob_under_test.name,
                    spread * 100.0
                ));
            }
        }
    }
    assert!(failures.is_empty(), "основы расходятся по силе: {failures:#?}");
}

#[test]
fn every_tone_knob_grows_on_every_step_of_its_travel_on_every_base() {
    let bench = KnobBench::new();
    let model = LookModelSettings::default();
    for (base_name, base) in all_tone_bases() {
        for knob_under_test in TONE_KNOBS {
            for sign in [-1.0_f32, 1.0] {
                let mut previous = 0.0_f32;
                for step in TRAVEL_STEPS {
                    let strength = tone_strength(&bench, &base, knob_under_test, sign * step, &model);
                    assert!(
                        strength > previous + MIN_STEP_GROWTH_CODES,
                        "{} {base_name}: ход {:+.0} % дал {strength:.2}, на шаге раньше было {previous:.2} — мёртвый участок",
                        knob_under_test.name,
                        sign * step * 100.0
                    );
                    previous = strength;
                }
            }
        }
    }
}

/// Прежние края (до 14l: контраст ±0,1 log2, средние ±0,3, тени и света ±0,6) обязаны НЕ проходить пороги: иначе
/// пороги ничего не защищают. Контрольная пара — те же замеры на сегодняшних краях проходят.
#[test]
fn the_former_narrow_edges_fail_the_thresholds_that_the_current_edges_pass() {
    let bench = KnobBench::new();
    let former = LookModelSettings {
        tone: ToneShapeSettings {
            contrast: KnobRange::symmetric(0.1),
            midtones: KnobRange::symmetric(0.3),
            shadows: KnobRange::symmetric(0.6),
            highlights: KnobRange::symmetric(0.6),
            ..ToneShapeSettings::default()
        },
        ..LookModelSettings::default()
    };
    former.validate().expect("прежние края допустимы для модели");
    let current = LookModelSettings::default();
    for knob_under_test in TONE_KNOBS {
        for sign in [-1.0_f32, 1.0] {
            let needed = threshold(&TONE_THRESHOLDS, knob_under_test.name, sign);
            for (base_name, base) in all_tone_bases() {
                let old = tone_strength(&bench, &base, knob_under_test, sign, &former);
                let new = tone_strength(&bench, &base, knob_under_test, sign, &current);
                assert!(
                    old < needed,
                    "{} {sign:+} на {base_name}: прежний край дал {old:.1}, порог {needed} не защищает",
                    knob_under_test.name
                );
                assert!(
                    new >= needed,
                    "{} {sign:+} на {base_name}: сегодняшний край {new:.1} < {needed}",
                    knob_under_test.name
                );
            }
        }
    }
}

// ---------- цвет ----------

/// Цветная ручка: как собрать `LookSettings` по ходу `travel` ∈ [−1, 1].
#[derive(Clone, Copy)]
struct ColorKnob {
    name: &'static str,
    settings_at: fn(f32) -> LookSettings,
}

fn with(edit: impl FnOnce(&mut LookSettings)) -> LookSettings {
    let mut settings = LookSettings::NEUTRAL;
    edit(&mut settings);
    settings
}

/// Правка одной полосы: ось 0 — оттенок, 1 — насыщенность, 2 — яркость.
fn band(id: &str, axis: usize, travel: f32) -> LookSettings {
    let mut axes = [0.0_f32; 3];
    axes[axis] = travel;
    with(|settings| {
        settings.bands = BandAdjustments::new(vec![BandAdjustment {
            band: HueBandId::new(id).unwrap(),
            hue: knob(axes[0]),
            saturation: knob(axes[1]),
            luminance: knob(axes[2]),
        }])
        .unwrap();
    })
}

/// Колесо зоны: толчок цвета на `travel` по оттенку 0° (плюс) или 180° (минус) с силой `|travel|`.
fn wheel_push(zone: WheelZone, travel: f32) -> LookSettings {
    let hue_degrees = if travel >= 0.0 { 0.0 } else { 180.0 };
    let push = WheelPush::from_hue_degrees(hue_degrees, travel.abs()).unwrap();
    with(|settings| {
        settings.wheels = ColorWheels::NEUTRAL.with_wheel(
            zone,
            ColorWheel {
                push,
                ring: LookKnob::NEUTRAL,
            },
        );
    })
}

/// Колесо зоны: кольцо яркости.
fn wheel_ring(zone: WheelZone, travel: f32) -> LookSettings {
    with(|settings| {
        settings.wheels = ColorWheels::NEUTRAL.with_wheel(
            zone,
            ColorWheel {
                push: WheelPush::NEUTRAL,
                ring: knob(travel),
            },
        );
    })
}

const COLOR_KNOBS: [ColorKnob; 18] = [
    ColorKnob {
        name: "SATURATION",
        settings_at: |t| with(|s| s.saturation = knob(t)),
    },
    ColorKnob {
        name: "VIBRANCE",
        settings_at: |t| with(|s| s.vibrance = knob(t)),
    },
    ColorKnob {
        name: "WARMTH",
        settings_at: |t| with(|s| s.warmth = knob(t)),
    },
    ColorKnob {
        name: "TINT",
        settings_at: |t| with(|s| s.tint = knob(t)),
    },
    ColorKnob {
        name: "BAND HUE red",
        settings_at: |t| band("red", 0, t),
    },
    ColorKnob {
        name: "BAND HUE blue",
        settings_at: |t| band("blue", 0, t),
    },
    ColorKnob {
        name: "BAND SAT red",
        settings_at: |t| band("red", 1, t),
    },
    ColorKnob {
        name: "BAND SAT blue",
        settings_at: |t| band("blue", 1, t),
    },
    ColorKnob {
        name: "BAND LUM red",
        settings_at: |t| band("red", 2, t),
    },
    ColorKnob {
        name: "BAND LUM blue",
        settings_at: |t| band("blue", 2, t),
    },
    ColorKnob {
        name: "PUSH lift",
        settings_at: |t| wheel_push(WheelZone::Lift, t),
    },
    ColorKnob {
        name: "PUSH gamma",
        settings_at: |t| wheel_push(WheelZone::Gamma, t),
    },
    ColorKnob {
        name: "PUSH gain",
        settings_at: |t| wheel_push(WheelZone::Gain, t),
    },
    ColorKnob {
        name: "PUSH offset",
        settings_at: |t| wheel_push(WheelZone::Offset, t),
    },
    ColorKnob {
        name: "RING lift",
        settings_at: |t| wheel_ring(WheelZone::Lift, t),
    },
    ColorKnob {
        name: "RING gamma",
        settings_at: |t| wheel_ring(WheelZone::Gamma, t),
    },
    ColorKnob {
        name: "RING gain",
        settings_at: |t| wheel_ring(WheelZone::Gain, t),
    },
    ColorKnob {
        name: "RING offset",
        settings_at: |t| wheel_ring(WheelZone::Offset, t),
    },
];

/// Пороги цветных ручек на краю, коды из 255: (ручка, на −1, на +1). Каждый — 90 % измеренного (14l, умолчания)
/// минимума по четырём основам. У толчка колеса «−» — противоположный оттенок (180°), у остальных — ход ручки в минус.
const COLOR_THRESHOLDS: [(&str, f32, f32); 18] = [
    ("SATURATION", 57.4, 119.5),
    ("VIBRANCE", 30.2, 29.7),
    ("WARMTH", 43.1, 36.2),
    ("TINT", 25.6, 38.0),
    ("BAND HUE red", 60.1, 46.0),
    ("BAND HUE blue", 41.5, 39.6),
    ("BAND SAT red", 43.0, 38.4),
    ("BAND SAT blue", 47.7, 30.5),
    ("BAND LUM red", 126.2, 93.9),
    ("BAND LUM blue", 131.7, 104.4),
    ("PUSH lift", 13.9, 11.8),
    ("PUSH gamma", 119.6, 66.0),
    ("PUSH gain", 122.9, 50.5),
    ("PUSH offset", 144.8, 82.6),
    ("RING lift", 5.7, 5.8),
    ("RING gamma", 37.5, 39.1),
    ("RING gain", 40.0, 32.8),
    ("RING offset", 40.5, 40.2),
];

fn color_shift(
    bench: &KnobBench,
    baseline: &ChartCodes,
    base: &ToneBase,
    knob_under_test: ColorKnob,
    travel: f32,
    model: &LookModelSettings,
) -> ChartShift {
    bench
        .measure_chart_against(baseline, base, &(knob_under_test.settings_at)(travel), model)
        .unwrap_or_else(|reason| panic!("{} {travel:+}: {reason}", knob_under_test.name))
}

/// Наибольший сдвиг кода канала на мишени (мера порогов).
fn color_strength(
    bench: &KnobBench,
    baseline: &ChartCodes,
    base: &ToneBase,
    knob_under_test: ColorKnob,
    travel: f32,
    model: &LookModelSettings,
) -> f32 {
    color_shift(bench, baseline, base, knob_under_test, travel, model).codes
}

#[test]
fn every_colour_knob_reaches_its_threshold_on_every_base() {
    let bench = KnobBench::new();
    let model = LookModelSettings::default();
    println!("\nЦвет: наибольший сдвиг кода канала на мишени, коды из 255: порог | AgX ACES filmic Legacy");
    let mut failures = Vec::new();
    for (base_name, base) in all_tone_bases() {
        let baseline = bench.chart_baseline(&base, &model).unwrap();
        for knob_under_test in COLOR_KNOBS {
            for sign in [-1.0_f32, 1.0] {
                let needed = threshold(&COLOR_THRESHOLDS, knob_under_test.name, sign);
                let strength = color_strength(&bench, &baseline, &base, knob_under_test, sign, &model);
                println!(
                    "{base_name:<7}{:<16}{sign:+} {needed:>6.1} | {strength:6.1}",
                    knob_under_test.name
                );
                if strength < needed {
                    failures.push(format!(
                        "{} {sign:+} на {base_name}: {strength:.1} < порога {needed}",
                        knob_under_test.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "цветная ручка слабее порога: {failures:#?}");
}

#[test]
fn every_colour_knob_grows_on_every_step_of_its_travel_on_every_base() {
    let bench = KnobBench::new();
    let model = LookModelSettings::default();
    for (base_name, base) in all_tone_bases() {
        let baseline = bench.chart_baseline(&base, &model).unwrap();
        for knob_under_test in COLOR_KNOBS {
            for sign in [-1.0_f32, 1.0] {
                let mut previous = 0.0_f32;
                for step in TRAVEL_STEPS {
                    // Средний сдвиг, а не наибольший: наибольший упирается в край охвата экрана (см. `ChartShift`).
                    let strength =
                        color_shift(&bench, &baseline, &base, knob_under_test, sign * step, &model).mean_codes;
                    assert!(
                        strength > previous + MIN_STEP_GROWTH_CODES,
                        "{} {base_name}: ход {:+.0} % дал {strength:.2}, на шаге раньше было {previous:.2} — мёртвый участок",
                        knob_under_test.name,
                        sign * step * 100.0
                    );
                    previous = strength;
                }
            }
        }
    }
}

/// Модель с краями ДО 14l (диапазоны, симметричные в обе стороны; колёса — прежние силы).
fn former_colour_model() -> LookModelSettings {
    use isp_model::look::wheels::WheelModelSettings;
    LookModelSettings {
        saturation: KnobRange::symmetric(1.0),
        vibrance: KnobRange::symmetric(0.8),
        warmth_mired: KnobRange::symmetric(50.0),
        tint_duv: KnobRange::symmetric(0.012),
        band_hue_degrees: KnobRange::symmetric(30.0),
        band_saturation: KnobRange::symmetric(0.8),
        band_luminance: KnobRange::symmetric(0.35),
        wheels: WheelModelSettings {
            push_chroma_range: 0.05,
            ring_lightness_range: 0.10,
            ..WheelModelSettings::default()
        },
        ..LookModelSettings::default()
    }
}

/// Прежние края цветных ручек не проходят пороги: хотя бы на одной основе каждая ручка (где край вырос) оказывается
/// ниже порога. Исключения — в коде (край не менялся или метрика упирается в охват экрана).
#[test]
fn the_former_narrow_colour_edges_fail_the_thresholds() {
    let bench = KnobBench::new();
    let former = former_colour_model();
    former.validate().expect("прежние края допустимы для модели");
    let baselines: Vec<_> = all_tone_bases()
        .into_iter()
        .map(|(name, base)| (name, base, bench.chart_baseline(&base, &former).unwrap()))
        .collect();
    let mut not_protected = Vec::new();
    for knob_under_test in COLOR_KNOBS {
        for sign in [-1.0_f32, 1.0] {
            // Насыщенность в минус не менялась (край — серый). «BAND SAT blue +»: самый синий патч уже у границы
            // охвата экрана, наибольший сдвиг один и тот же при 0,8 и 1,5 — порог там защищает только от обнуления.
            let unchanged_or_clip_limited = matches!(
                (knob_under_test.name, sign < 0.0),
                ("SATURATION", true) | ("BAND SAT blue", false)
            );
            if unchanged_or_clip_limited {
                continue;
            }
            let needed = threshold(&COLOR_THRESHOLDS, knob_under_test.name, sign);
            let weakest = baselines
                .iter()
                .map(|(name, base, baseline)| {
                    (
                        *name,
                        color_strength(&bench, baseline, base, knob_under_test, sign, &former),
                    )
                })
                .min_by(|left, right| left.1.total_cmp(&right.1))
                .unwrap();
            println!(
                "{:<16}{sign:+} порог {needed:>6.1}, прежний край на {} дал {:.1}",
                knob_under_test.name, weakest.0, weakest.1
            );
            if weakest.1 >= needed {
                not_protected.push(format!("{} {sign:+}", knob_under_test.name));
            }
        }
    }
    assert!(
        not_protected.is_empty(),
        "прежние края проходят порог на всех основах: {not_protected:?}"
    );
}
