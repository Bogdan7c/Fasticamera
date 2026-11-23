//! Колёса цвета по зонам (сессия 14i, решения 186, 191–193) на ПИКСЕЛЯХ через НАСТОЯЩИЙ бэкенд показа: RADV,
//! headless-показ, полная validation, подставная камера (стенд `viewfinder_rig`, как у `vulkan_look_picture.rs`).
//!
//! Доказывается: `SetLook` с колёсами меняет то, что нарисовал видоискатель, и картинка совпадает с эталоном
//! `isp-model` ЭТОГО вида; колесо Lift красит тени и не трогает света; вид, отличающийся от текущего ТОЛЬКО
//! колёсами, даёт новую картинку (ключ кэша параметров ISP видит колёса); возврат к нейтрали возвращает картинку.
//! Снимок с колёсами момента нажатия — в `vulkan_still_look.rs`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod viewfinder_rig;
mod vulkan_support;

use std::sync::Arc;

use look_contract::{ColorWheel, ColorWheels, LookKnob, LookSettings, WheelPush, WheelZone};
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig};
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use viewfinder_rig::{
    BLACK_LEVEL, CellValues, TOLERANCE, ViewfinderBench, WHITE_LEVEL, assert_image_color, reference_color_with_look,
    scene_neutral,
};

/// Угол оттенка синего в OKLab (градусы, 0° = `+a`, 90° = `+b`).
const BLUE_HUE_DEGREES: f32 = 265.0;
/// Угол оттенка красно-оранжевого в OKLab.
const RED_HUE_DEGREES: f32 = 30.0;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

fn wheel(hue_degrees: f32, strength: f32, ring: f32) -> ColorWheel {
    ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, strength).expect("точка в круге"),
        ring: knob(ring),
    }
}

/// Вид только с колесом Lift: тени сильно к синему, света не трогает.
fn lift_blue_look() -> LookSettings {
    LookSettings {
        wheels: ColorWheels::NEUTRAL.with_wheel(WheelZone::Lift, wheel(BLUE_HUE_DEGREES, 1.0, 0.0)),
        ..LookSettings::NEUTRAL
    }
}

/// Вид, который отличается от `lift_blue_look` ТОЛЬКО колёсами: Lift толкает к красному.
fn lift_red_look() -> LookSettings {
    LookSettings {
        wheels: ColorWheels::NEUTRAL.with_wheel(WheelZone::Lift, wheel(RED_HUE_DEGREES, 1.0, 0.0)),
        ..LookSettings::NEUTRAL
    }
}

/// Вид «Lift к синему + кольцо Gain»: колёса двух зон сразу.
fn lift_blue_and_gain_ring_look() -> LookSettings {
    LookSettings {
        wheels: ColorWheels::NEUTRAL
            .with_wheel(WheelZone::Lift, wheel(BLUE_HUE_DEGREES, 1.0, 0.0))
            .with_wheel(WheelZone::Gain, wheel(RED_HUE_DEGREES, 0.0, 1.0)),
        ..LookSettings::NEUTRAL
    }
}

/// Серый под нейтраль сцены на доле `fraction` размаха между чёрным и белым сенсора.
fn gray_at(fraction: f64) -> CellValues {
    let neutral = scene_neutral();
    let span = f64::from(WHITE_LEVEL - BLACK_LEVEL) * fraction;
    let channel = |weight: f64| BLACK_LEVEL + (span * weight).round() as u16;
    CellValues {
        red: channel(neutral[0]),
        green: channel(neutral[1]),
        blue: channel(neutral[2]),
    }
}

/// Тёмная ячейка: глубоко в зоне теней.
fn dark_cell() -> CellValues {
    gray_at(0.03)
}

/// Светлая ячейка: выше серого экрана, где вес теней равен нулю.
fn bright_cell() -> CellValues {
    gray_at(0.9)
}

fn config() -> RenderLoopConfig {
    RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        ..RenderLoopConfig::default()
    }
}

fn color_distance(first: [f32; 3], second: [f32; 3]) -> f32 {
    (0..3)
        .map(|channel| (first[channel] - second[channel]).abs())
        .fold(0.0_f32, f32::max)
}

fn set_look(bench: &mut ViewfinderBench, look: &LookSettings) {
    assert_eq!(
        bench
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::new(look.clone()))),
        ViewfinderCommandOutcome::Applied
    );
}

/// Эталон обязан отличаться от другого сильнее допуска, иначе тест не отличил бы «колёса дошли» от «не дошли».
fn assert_distinguishable(first: [f32; 3], second: [f32; 3], what: &str) {
    let distance = color_distance(first, second);
    assert!(
        distance > 8.0 * TOLERANCE,
        "{what}: эталоны почти не различаются ({first:?} против {second:?}): тест ничего не докажет"
    );
}

fn params_resolved(bench: &mut ViewfinderBench) -> u64 {
    bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("видоискатель")
        .counters
        .params_resolved
}

#[test]
fn a_wheels_look_reaches_the_shown_picture_like_the_reference_and_neutral_restores_it() {
    let config = config();
    let Some(mut bench) = ViewfinderBench::start(&config, dark_cell(), 8) else {
        return;
    };
    let tone = config.viewfinder.tone;
    let look = lift_blue_and_gain_ring_look();
    let neutral_reference = reference_color_with_look(&bench.passport, dark_cell(), &tone, &LookSettings::NEUTRAL);
    let styled_reference = reference_color_with_look(&bench.passport, dark_cell(), &tone, &look);
    assert_distinguishable(styled_reference, neutral_reference, "Lift к синему в тени");
    assert!(
        styled_reference[2] > neutral_reference[2],
        "Lift к синему добавляет синего: {styled_reference:?} против {neutral_reference:?}"
    );

    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "нейтральный вид");

    set_look(&mut bench, &look);
    bench.show_next_frame();
    assert_image_color(&bench.backend, styled_reference, "вид с колёсами Lift и Gain");

    set_look(&mut bench, &LookSettings::NEUTRAL);
    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "возврат к нейтрали");
    bench.finish();
}

#[test]
fn a_lift_only_look_paints_the_shadows_and_leaves_the_highlights_alone() {
    let config = config();
    let tone = config.viewfinder.tone;
    let look = lift_blue_look();

    // Тёмная сцена: колесо Lift красит её, картинка совпадает с эталоном вида.
    let Some(mut dark) = ViewfinderBench::start(&config, dark_cell(), 4) else {
        return;
    };
    let dark_neutral = reference_color_with_look(&dark.passport, dark_cell(), &tone, &LookSettings::NEUTRAL);
    let dark_styled = reference_color_with_look(&dark.passport, dark_cell(), &tone, &look);
    assert_distinguishable(dark_styled, dark_neutral, "Lift в тёмной зоне");
    set_look(&mut dark, &look);
    dark.show_next_frame();
    assert_image_color(&dark.backend, dark_styled, "тёмная сцена с Lift");
    dark.finish();

    // Светлая сцена: вес теней там ровно 0, картинка остаётся нейтральной.
    let Some(mut bright) = ViewfinderBench::start(&config, bright_cell(), 4) else {
        return;
    };
    let bright_neutral = reference_color_with_look(&bright.passport, bright_cell(), &tone, &LookSettings::NEUTRAL);
    let bright_styled = reference_color_with_look(&bright.passport, bright_cell(), &tone, &look);
    assert!(
        color_distance(bright_styled, bright_neutral) <= TOLERANCE,
        "эталон: Lift не трогает света ({bright_styled:?} против {bright_neutral:?})"
    );
    set_look(&mut bright, &look);
    bright.show_next_frame();
    assert_image_color(
        &bright.backend,
        bright_neutral,
        "светлая сцена с Lift остаётся нейтральной",
    );
    bright.finish();
}

#[test]
fn a_look_that_differs_only_in_wheels_produces_a_new_picture() {
    let config = config();
    let Some(mut bench) = ViewfinderBench::start(&config, dark_cell(), 8) else {
        return;
    };
    let tone = config.viewfinder.tone;
    let (blue, red) = (lift_blue_look(), lift_red_look());
    let blue_reference = reference_color_with_look(&bench.passport, dark_cell(), &tone, &blue);
    let red_reference = reference_color_with_look(&bench.passport, dark_cell(), &tone, &red);
    assert_distinguishable(blue_reference, red_reference, "Lift синий против красного");

    set_look(&mut bench, &blue);
    bench.show_next_frame();
    assert_image_color(&bench.backend, blue_reference, "Lift к синему");
    let resolved_blue = params_resolved(&mut bench);

    // Все ручки те же, отличаются только колёса: ключ кэша обязан это увидеть, иначе осталась бы прежняя картинка.
    set_look(&mut bench, &red);
    bench.show_next_frame();
    assert_image_color(&bench.backend, red_reference, "Lift к красному");
    assert_eq!(
        params_resolved(&mut bench),
        resolved_blue + 1,
        "вид, отличающийся только колёсами, пересчитал параметры ровно один раз"
    );
    bench.finish();
}
