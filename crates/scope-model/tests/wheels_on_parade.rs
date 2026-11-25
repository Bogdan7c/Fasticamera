//! Приборы видят колёса цвета: RGB parade серой рампы через основу AgX, Lift в синеву против нейтрали.
//!
//! Цепочка как в кадре: свет сцены → основа тона (рабочее пространство Rec. 2020) → колёса цвета (OKLab, первый шаг
//! стадии цвета вида) → `to_output` → кривая sRGB → коды экрана → parade (решение 190: прибор считает то, что на
//! экране). Колёса идут тем же путём, что в кадре: значения словаря `look-contract` (`LookSettings.wheels`) →
//! `LookColorParams::resolve` с моделью вида по умолчанию → стадия цвета вида.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use isp_model::color::chromaticity::Primaries;
use isp_model::color::working_space::WORKING_PRIMARIES;
use isp_model::image::DisplayGamut;
use isp_model::look::color::LookColorParams;
use isp_model::look::model::LookModelSettings;
use isp_model::tone::agx::AgxBase;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::transfer::srgb_encode;
use look_contract::{ColorWheel, ColorWheels, LookSettings, WheelPush, WheelZone};
use scope_model::trace_rules::ParadeRule;
use scope_model::{HorizontalOrder, SampleStride, ScopeCounters, ShownCodes, accumulate};

/// Столбцов в картинке и в parade (один столбец картинки — один столбец прибора), строк в картинке.
const COLUMNS: u32 = 32;
const ROWS: u32 = 4;
/// Уровней по вертикали parade: 256 → код 10 бит делится на 4.
const LEVELS: u32 = 256;
/// Рампа серого сцены: от −6 до +4 ступеней слева направо; серый экрана (пивот колёс) лежит на 0 EV.
const FIRST_EV: f64 = -6.0;
const LAST_EV: f64 = 4.0;
/// Серый сцены, который AgX по умолчанию переводит в серый экрана 0,20.
const SCENE_MIDDLE_GRAY: f64 = 0.075;
/// Сколько крайних столбцов считается тенями (слева) и светами (справа).
const EDGE_COLUMNS: u32 = 8;
/// Оттенок синего на круге OKLCh, градусы.
const BLUE_HUE_DEGREES: f32 = 265.0;

/// Стадия цвета вида с одним колесом Lift на полную силу в синеву (остальные ручки нейтральны).
fn lift_toward_blue() -> LookColorParams {
    lift_toward_hue(BLUE_HUE_DEGREES)
}

/// Сила толчка в тестах: четверть хода. С 14l край толчка — 0,20 цветности OKLab (решение 246), четверть — это
/// прежний край 0,05: заметный тон теней, ещё внутри охвата. На полном крае цвет теней уходит за охват, и мягкое
/// сжатие охвата выхода поднимает соседние каналы (R′ в тенях растёт), поэтому «R′ и G′ ниже нейтральных» там
/// не выполняется по построению; это проверяет отдельный тест.
const PUSH_STRENGTH: f32 = 0.25;

/// Стадия цвета вида с одним колесом Lift силой `strength` к оттенку `hue_degrees`.
fn lift_with_strength(hue_degrees: f32, strength: f32) -> LookColorParams {
    let lift = ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, strength).unwrap(),
        ..ColorWheel::NEUTRAL
    };
    let settings = LookSettings {
        wheels: ColorWheels::NEUTRAL.with_wheel(WheelZone::Lift, lift),
        ..LookSettings::NEUTRAL
    };
    LookColorParams::resolve(&settings, &LookModelSettings::default(), &WORKING_PRIMARIES)
        .unwrap()
        .params
        .expect("тронутое колесо включает стадию цвета вида")
}

/// Колесо Lift к оттенку `hue_degrees` силой [`PUSH_STRENGTH`].
fn lift_toward_hue(hue_degrees: f32) -> LookColorParams {
    lift_with_strength(hue_degrees, PUSH_STRENGTH)
}

/// Коды экрана серой рампы: `look` — стадия цвета вида между основой и выходом (`None` — без вида).
fn ramp_codes(look: Option<&LookColorParams>) -> ShownCodes {
    let base = AgxBase::resolve(&AgxSettings::default(), DisplayGamut::Srgb).unwrap();
    let srgb_to_working = WORKING_PRIMARIES
        .rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&Primaries::SRGB.rgb_to_xyz().unwrap());
    let column_pixels: Vec<[f32; 3]> = (0..COLUMNS)
        .map(|column| {
            let ev = FIRST_EV + (LAST_EV - FIRST_EV) * f64::from(column) / f64::from(COLUMNS - 1);
            let scene = srgb_to_working
                .apply([SCENE_MIDDLE_GRAY * ev.exp2(); 3])
                .map(|channel| channel as f32);
            let mut display = base.map_scene_pixel(scene, 1.0);
            if let Some(look) = look {
                display = look.apply(display);
            }
            base.to_output(display).map(srgb_encode)
        })
        .collect();
    let pixels: Vec<[f32; 3]> = (0..ROWS).flat_map(|_| column_pixels.iter().copied()).collect();
    ShownCodes::from_display_rgb(COLUMNS, ROWS, &pixels, HorizontalOrder::AsStored).unwrap()
}

/// Средний уровень следа плоскости `plane` по столбцам `columns` (в ячейках сетки, 0 — низ, `LEVELS − 1` — верх).
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

#[test]
fn parade_shows_blue_shadows_for_lift_toward_blue_and_unchanged_highlights() {
    let rule = ParadeRule::new(COLUMNS, LEVELS).unwrap();
    let wheels = lift_toward_blue();
    let neutral = accumulate(&ramp_codes(None), SampleStride::EVERY_PIXEL, &rule).unwrap();
    let tinted = accumulate(&ramp_codes(Some(&wheels)), SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(tinted.total(), neutral.total(), "колёса потеряли или добавили отсчёты");

    let shadows = 0..EDGE_COLUMNS;
    let highlights = COLUMNS - EDGE_COLUMNS..COLUMNS;
    let level = |counters: &ScopeCounters, plane: u32, columns: &std::ops::Range<u32>| {
        mean_trace_level(counters, plane, columns.clone())
    };
    let (red, green, blue) = (0, 1, 2);
    eprintln!(
        "parade тени   | нейтраль R/G/B {:.1}/{:.1}/{:.1} | Lift в синеву R/G/B {:.1}/{:.1}/{:.1} (из {LEVELS})",
        level(&neutral, red, &shadows),
        level(&neutral, green, &shadows),
        level(&neutral, blue, &shadows),
        level(&tinted, red, &shadows),
        level(&tinted, green, &shadows),
        level(&tinted, blue, &shadows),
    );
    eprintln!(
        "parade света  | нейтраль R/G/B {:.1}/{:.1}/{:.1} | Lift в синеву R/G/B {:.1}/{:.1}/{:.1}",
        level(&neutral, red, &highlights),
        level(&neutral, green, &highlights),
        level(&neutral, blue, &highlights),
        level(&tinted, red, &highlights),
        level(&tinted, green, &highlights),
        level(&tinted, blue, &highlights),
    );

    // Нейтральная рампа: три плоскости совпадают (серое серо), в тенях и в светах.
    for columns in [&shadows, &highlights] {
        let reference = level(&neutral, red, columns);
        assert!((level(&neutral, green, columns) - reference).abs() < 1e-9);
        assert!((level(&neutral, blue, columns) - reference).abs() < 1e-9);
    }
    // Тени при Lift в синеву: синий след выше красного и выше своего нейтрального, красный и зелёный ниже нейтральных.
    const VISIBLE_SHIFT_LEVELS: f64 = 3.0;
    let blue_over_red = level(&tinted, blue, &shadows) - level(&tinted, red, &shadows);
    assert!(
        blue_over_red > VISIBLE_SHIFT_LEVELS,
        "B' над R' всего на {blue_over_red} уровней"
    );
    assert!(level(&tinted, blue, &shadows) > level(&neutral, blue, &shadows));
    assert!(level(&tinted, red, &shadows) < level(&neutral, red, &shadows) - 1.0);
    assert!(level(&tinted, green, &shadows) < level(&neutral, green, &shadows) - 0.5);
    // Света: Lift ровно ноль выше серого — все три плоскости как у нейтральной рампы (допуск — округление кодов
    // от прохода через OKLab туда и обратно: доли уровня).
    for plane in [red, green, blue] {
        let moved = (level(&tinted, plane, &highlights) - level(&neutral, plane, &highlights)).abs();
        assert!(moved < 0.25, "плоскость {plane}: света сдвинулись на {moved} уровней");
    }
}

#[test]
fn parade_tracks_the_lift_push_direction() {
    // Противоположный оттенок (жёлтый, 85°) красит тени в обратную сторону: B' ниже R'.
    let warm = lift_toward_hue(85.0);
    let rule = ParadeRule::new(COLUMNS, LEVELS).unwrap();
    let counters = accumulate(&ramp_codes(Some(&warm)), SampleStride::EVERY_PIXEL, &rule).unwrap();
    let blue_over_red =
        mean_trace_level(&counters, 2, 0..EDGE_COLUMNS) - mean_trace_level(&counters, 0, 0..EDGE_COLUMNS);
    assert!(
        blue_over_red < -3.0,
        "жёлтый Lift не опустил B' под R': {blue_over_red}"
    );
}

/// На полном крае толчка синева теней только сильнее: синий след ещё выше над красным, света по-прежнему не тронуты.
#[test]
fn the_full_push_edge_paints_the_shadows_bluer_than_the_quarter() {
    let rule = ParadeRule::new(COLUMNS, LEVELS).unwrap();
    let shadows = 0..EDGE_COLUMNS;
    let highlights = COLUMNS - EDGE_COLUMNS..COLUMNS;
    let blue_over_red = |strength: f32, columns: &std::ops::Range<u32>| {
        let wheels = lift_with_strength(BLUE_HUE_DEGREES, strength);
        let counters = accumulate(&ramp_codes(Some(&wheels)), SampleStride::EVERY_PIXEL, &rule).unwrap();
        mean_trace_level(&counters, 2, columns.clone()) - mean_trace_level(&counters, 0, columns.clone())
    };
    let (quarter, full) = (blue_over_red(PUSH_STRENGTH, &shadows), blue_over_red(1.0, &shadows));
    assert!(
        full > quarter + 10.0,
        "полный край {full} против четверти {quarter} уровней"
    );
    assert!(blue_over_red(1.0, &highlights).abs() < 0.25, "света окрасились");
}
