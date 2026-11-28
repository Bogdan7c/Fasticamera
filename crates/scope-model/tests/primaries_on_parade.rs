//! Приборы видят полоски R, G, B (решения 339–343): RGB parade серой рампы на экране Display P3 через НАСТОЯЩИЙ шаг
//! выхода основы AgX (`to_output`: перевод в цвета экрана, мягкое сжатие охвата, обрезка 0…1) и кодирование.
//!
//! Цепочка как в кадре: свет сцены → основа тона (Rec. 2020) → вид целиком (`LookParams::apply_to_pixel`: тон,
//! полоски, цвет) → `to_output` → кривая передачи → коды экрана → parade (решение 190: прибор считает то, что на
//! экране). Каналы полосок — основные цвета Display P3 (решение 342), экран — тоже P3: полоска R двигает только
//! след R, зелёный и синий следы остаются на месте (в пределах одного уровня из 256).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::sync::Arc;

use isp_model::color::chromaticity::Primaries;
use isp_model::color::working_space::WORKING_PRIMARIES;
use isp_model::image::DisplayGamut;
use isp_model::look::model::LookModelSettings;
use isp_model::look::{LookControls, LookParams};
use isp_model::tone::agx::AgxBase;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::transfer::srgb_encode;
use look_contract::{ChannelPrimaries, LookKnob, LookSettings, PrimaryChannel, WheelZone};
use scope_model::trace_rules::ParadeRule;
use scope_model::{HorizontalOrder, SampleStride, ScopeCounters, ShownCodes, accumulate};

/// Столбцов в картинке и в parade (один столбец картинки — один столбец прибора), строк в картинке.
const COLUMNS: u32 = 32;
const ROWS: u32 = 4;
/// Уровней по вертикали parade: 256 → код 10 бит делится на 4.
const LEVELS: u32 = 256;
/// Рампа серого сцены: от −6 до +4 ступеней слева направо.
const FIRST_EV: f64 = -6.0;
const LAST_EV: f64 = 4.0;
/// Серый сцены, который AgX по умолчанию переводит в серый экрана 0,20.
const SCENE_MIDDLE_GRAY: f64 = 0.075;
/// Наибольшая доля сдвига «своего» следа, на которую сжатие охвата выхода может подтянуть чужие следы в тени.
/// Замер: при силе Lift 0,10 — ≈ 8 % (3–4 уровня против 43), при силе владельца 0,20 (14t, «сильно ×2») — ≈ 17 %
/// (R 10, G 12 против B 72): синий у края P3 сжимается к серому сильнее. Предел 0,25 — запас над замером; этот тест
/// проверяет направление (B двигается намного сильнее R и G). Утечку самой полоски в чужой канал ловят строгий тест
/// Gain R ниже (≤ 1 уровня на светах, где сжатия нет) и проверка основы P3 в `isp-model` (`primaries_tests`).
const MAX_COMPRESSION_SPILL: f64 = 0.25;
/// Допустимый сдвиг «чужого» следа, уровни из 256: «в пределах одного кода».
const OTHER_TRACE_TOLERANCE_LEVELS: f64 = 1.0;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Вид только с одной полоской.
fn single_bar(zone: WheelZone, channel: PrimaryChannel, value: f32) -> LookSettings {
    LookSettings {
        primaries: ChannelPrimaries::NEUTRAL.with_value(zone, channel, knob(value)),
        ..LookSettings::NEUTRAL
    }
}

/// Столбец рампы, в котором сцена лежит на `ev` ступенях от среднего серого.
fn column_at(ev: f64) -> u32 {
    ((ev - FIRST_EV) / (LAST_EV - FIRST_EV) * f64::from(COLUMNS - 1)).round() as u32
}

/// Свет экрана P3 после основы AgX и вида `settings` для серого сцены `ev` ступеней от среднего серого.
fn display_light(base: &AgxBase, look: &LookParams, ev: f64) -> [f32; 3] {
    let srgb_to_working = WORKING_PRIMARIES
        .rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&Primaries::SRGB.rgb_to_xyz().unwrap());
    let scene = srgb_to_working
        .apply([SCENE_MIDDLE_GRAY * ev.exp2(); 3])
        .map(|channel| channel as f32);
    let working_display = base.map_scene_pixel(scene, 1.0);
    // Настоящий шаг выхода: перевод в P3, сжатие охвата и обрезка (как `working_display_to_output` кадра).
    base.to_output(look.apply_to_pixel(working_display))
}

/// Коды экрана P3 серой рампы с видом `settings`.
fn ramp_codes(settings: &LookSettings) -> ShownCodes {
    let base = AgxBase::resolve(&AgxSettings::default(), DisplayGamut::DisplayP3).unwrap();
    let controls = LookControls::new(settings.clone(), Arc::new(LookModelSettings::default()));
    let look = LookParams::resolve(&controls, &WORKING_PRIMARIES).unwrap();
    let column_pixels: Vec<[f32; 3]> = (0..COLUMNS)
        .map(|column| {
            let ev = FIRST_EV + (LAST_EV - FIRST_EV) * f64::from(column) / f64::from(COLUMNS - 1);
            let light = display_light(&base, &look, ev);
            assert!(light.iter().all(|channel| channel.is_finite()), "ev {ev}: {light:?}");
            light.map(srgb_encode)
        })
        .collect();
    let pixels: Vec<[f32; 3]> = (0..ROWS).flat_map(|_| column_pixels.iter().copied()).collect();
    ShownCodes::from_display_rgb(COLUMNS, ROWS, &pixels, HorizontalOrder::AsStored).unwrap()
}

/// Средний уровень следа плоскости `plane` по столбцам `columns` (0 — низ, `LEVELS − 1` — верх).
fn mean_trace_level(counters: &ScopeCounters, plane: u32, columns: std::ops::Range<u32>) -> f64 {
    let (mut weighted_levels, mut samples) = (0.0, 0.0);
    for column in columns {
        for row in 0..LEVELS {
            let count = f64::from(counters.count(plane, row, column).unwrap());
            weighted_levels += count * f64::from(LEVELS - 1 - row);
            samples += count;
        }
    }
    assert!(samples > 0.0, "в столбцах нет отсчётов");
    weighted_levels / samples
}

/// Средние уровни следов R, G, B по столбцам `columns` для вида `settings`.
fn trace_levels(settings: &LookSettings, columns: std::ops::Range<u32>) -> [f64; 3] {
    let rule = ParadeRule::new(COLUMNS, LEVELS).unwrap();
    let counters = accumulate(&ramp_codes(settings), SampleStride::EVERY_PIXEL, &rule).unwrap();
    [0, 1, 2].map(|plane| mean_trace_level(&counters, plane, columns.clone()))
}

/// Сдвиги следов R, G, B против нейтрали (уровни из 256).
fn trace_shift(settings: &LookSettings, columns: std::ops::Range<u32>) -> [f64; 3] {
    let neutral = trace_levels(&LookSettings::NEUTRAL, columns.clone());
    let touched = trace_levels(settings, columns);
    [0, 1, 2].map(|plane| touched[plane] - neutral[plane])
}

#[test]
fn gain_red_lifts_only_the_red_trace_of_the_parade_in_the_highlights() {
    // Света, но не белое: у самого белого выход обрезается, и сдвиг прячется в обрезке.
    let highlights = column_at(0.0)..column_at(2.0);
    let shift = trace_shift(
        &single_bar(WheelZone::Gain, PrimaryChannel::Red, 1.0),
        highlights.clone(),
    );
    eprintln!("Gain R +1, света: сдвиг следов R/G/B = {shift:.2?} уровней из {LEVELS}");
    assert!(shift[0] > 4.0, "след R поднялся всего на {} уровней", shift[0]);
    assert!(
        shift[1].abs() <= OTHER_TRACE_TOLERANCE_LEVELS && shift[2].abs() <= OTHER_TRACE_TOLERANCE_LEVELS,
        "Gain R задел чужие следы: G {}, B {}",
        shift[1],
        shift[2]
    );
    // Тени: Gain почти ничего не делает (чёрное неподвижно), след R там сдвинут заметно слабее, чем в светах.
    let shadow_shift = trace_shift(
        &single_bar(WheelZone::Gain, PrimaryChannel::Red, 1.0),
        column_at(-6.0)..column_at(-4.0),
    );
    assert!(
        shadow_shift[0] < 0.5 * shift[0],
        "Gain R в тенях {shadow_shift:?}, в светах {shift:?}"
    );
}

#[test]
fn lift_blue_lifts_the_blue_trace_in_the_shadows_and_leaves_red_and_green() {
    let shadows = column_at(-6.0)..column_at(-3.0);
    let shift = trace_shift(&single_bar(WheelZone::Lift, PrimaryChannel::Blue, 1.0), shadows.clone());
    eprintln!("Lift B +1, тени: сдвиг следов R/G/B = {shift:.2?} уровней из {LEVELS}");
    assert!(shift[2] > 4.0, "след B в тенях поднялся всего на {}", shift[2]);
    // Красный и зелёный следы в глубокой тени всё же чуть поднимаются (≈ 10–12 уровней против 72 у синего): это не
    // полоска, а мягкое сжатие охвата выхода — сильно синий пиксель у края P3 оно подтягивает к серому.
    // Именно поэтому тест идёт через настоящий шаг выхода, а не через одну матрицу. Предел — доля сдвига синего.
    assert!(
        shift[0].abs() < MAX_COMPRESSION_SPILL * shift[2] && shift[1].abs() < MAX_COMPRESSION_SPILL * shift[2],
        "Lift B задел чужие следы сильнее сжатия охвата: R {}, G {}, B {}",
        shift[0],
        shift[1],
        shift[2]
    );
    // Света: Lift сходит на нет к белому — сдвиг B там слабее, чем в тенях.
    let highlight_shift = trace_shift(
        &single_bar(WheelZone::Lift, PrimaryChannel::Blue, 1.0),
        column_at(0.0)..column_at(2.0),
    );
    assert!(
        highlight_shift[2] < shift[2],
        "света {highlight_shift:?}, тени {shift:?}"
    );
}

#[test]
fn every_other_bar_moves_its_own_trace_too_and_the_neighbours_stay() {
    // Offset G «+»: равный сдвиг по всей рампе, только зелёный след.
    let all = column_at(-5.0)..column_at(1.0);
    let offset = trace_shift(&single_bar(WheelZone::Offset, PrimaryChannel::Green, 1.0), all.clone());
    assert!(offset[1] > 4.0, "Offset G: {offset:?}");
    assert!(offset[0].abs() <= OTHER_TRACE_TOLERANCE_LEVELS && offset[2].abs() <= OTHER_TRACE_TOLERANCE_LEVELS);
    // Gamma B «+»: середина, только синий след.
    let middle = column_at(-2.0)..column_at(1.0);
    let gamma = trace_shift(&single_bar(WheelZone::Gamma, PrimaryChannel::Blue, 1.0), middle);
    assert!(gamma[2] > 2.0, "Gamma B: {gamma:?}");
    assert!(gamma[0].abs() <= OTHER_TRACE_TOLERANCE_LEVELS && gamma[1].abs() <= OTHER_TRACE_TOLERANCE_LEVELS);
}

#[test]
fn the_most_negative_lift_survives_the_real_output_step_without_nan_and_darkens_the_trace() {
    // Lift −1 на всех трёх каналах: чёрное уходит в минус, выход обрезает его в 0. Ни NaN, ни потерянных отсчётов.
    let darkest = ChannelPrimaries::NEUTRAL
        .with_value(WheelZone::Lift, PrimaryChannel::Red, knob(-1.0))
        .with_value(WheelZone::Lift, PrimaryChannel::Green, knob(-1.0))
        .with_value(WheelZone::Lift, PrimaryChannel::Blue, knob(-1.0));
    let settings = LookSettings {
        primaries: darkest,
        ..LookSettings::NEUTRAL
    };
    let rule = ParadeRule::new(COLUMNS, LEVELS).unwrap();
    let neutral = accumulate(&ramp_codes(&LookSettings::NEUTRAL), SampleStride::EVERY_PIXEL, &rule).unwrap();
    let lowered = accumulate(&ramp_codes(&settings), SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(lowered.total(), neutral.total(), "отсчёты потеряны или добавлены");
    let shadows = 0..column_at(-3.0);
    for plane in [0, 1, 2] {
        let before = mean_trace_level(&neutral, plane, shadows.clone());
        let after = mean_trace_level(&lowered, plane, shadows.clone());
        assert!(
            after < before,
            "плоскость {plane}: Lift −1 не опустил тени ({before} -> {after})"
        );
    }
}
