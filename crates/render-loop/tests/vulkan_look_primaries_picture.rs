//! Полоски Lift/Gamma/Gain/Offset по R, G, B (сессия 14t, решения 339–343) на ПИКСЕЛЯХ через НАСТОЯЩИЙ бэкенд
//! показа: RADV, headless-показ, полная validation, подставная камера (стенд `viewfinder_rig`, как у
//! `vulkan_look_wheels_picture.rs`).
//!
//! Доказывается: `SetLook` с полосками меняет то, что нарисовал видоискатель, и картинка совпадает с эталоном
//! `isp-model` ЭТОГО вида; Lift B «+» синит тени заметно сильнее светов; вид, отличающийся от текущего ТОЛЬКО
//! полосками, даёт новую картинку и ровно одно новое разрешение параметров (ключ кэша ISP видит полоски); возврат к
//! нейтрали возвращает картинку. Снимок с полосками момента нажатия — в `vulkan_still_look.rs`.

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

use look_contract::{ChannelPrimaries, LookKnob, LookSettings, PrimaryChannel, WheelZone};
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig};
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use viewfinder_rig::{
    BLACK_LEVEL, CellValues, TOLERANCE, ViewfinderBench, WHITE_LEVEL, assert_image_color, reference_color_with_look,
    scene_neutral,
};

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Вид только с одной полоской.
fn single_bar_look(zone: WheelZone, channel: PrimaryChannel, value: f32) -> LookSettings {
    LookSettings {
        primaries: ChannelPrimaries::NEUTRAL.with_value(zone, channel, knob(value)),
        ..LookSettings::NEUTRAL
    }
}

/// Lift B «+»: тени к синему.
fn lift_blue_look() -> LookSettings {
    single_bar_look(WheelZone::Lift, PrimaryChannel::Blue, 1.0)
}

/// Вид, который отличается от `lift_blue_look` ТОЛЬКО полосками: Lift R «+» вместо Lift B «+».
fn lift_red_look() -> LookSettings {
    single_bar_look(WheelZone::Lift, PrimaryChannel::Red, 1.0)
}

/// Вид «Lift B «+» и Gain R «+»»: полоски двух зон и двух каналов сразу.
fn lift_blue_and_gain_red_look() -> LookSettings {
    LookSettings {
        primaries: ChannelPrimaries::NEUTRAL
            .with_value(WheelZone::Lift, PrimaryChannel::Blue, knob(1.0))
            .with_value(WheelZone::Gain, PrimaryChannel::Red, knob(1.0)),
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

/// Тёмная ячейка: глубоко в тенях.
fn dark_cell() -> CellValues {
    gray_at(0.03)
}

/// Средняя ячейка: Lift и Gain действуют оба.
fn middle_cell() -> CellValues {
    gray_at(0.25)
}

/// Светлая ячейка: света.
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

/// Сдвиг КОДА экрана в канале `channel` между двумя эталонами.
fn code_shift(styled: [f32; 3], neutral: [f32; 3], channel: usize) -> f32 {
    styled[channel] - neutral[channel]
}

#[test]
fn a_bars_look_reaches_the_shown_picture_like_the_reference_and_neutral_restores_it() {
    let config = config();
    let Some(mut bench) = ViewfinderBench::start(&config, middle_cell(), 8) else {
        return;
    };
    let tone = config.viewfinder.tone;
    let look = lift_blue_and_gain_red_look();
    let neutral_reference = reference_color_with_look(&bench.passport, middle_cell(), &tone, &LookSettings::NEUTRAL);
    let styled_reference = reference_color_with_look(&bench.passport, middle_cell(), &tone, &look);
    assert_distinguishable(styled_reference, neutral_reference, "Lift B и Gain R на среднем сером");
    assert!(
        code_shift(styled_reference, neutral_reference, 0) > 0.0
            && code_shift(styled_reference, neutral_reference, 2) > 0.0,
        "Gain R «+» поднимает красный, Lift B «+» поднимает синий: {styled_reference:?} против {neutral_reference:?}"
    );

    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "нейтральный вид");

    set_look(&mut bench, &look);
    bench.show_next_frame();
    assert_image_color(&bench.backend, styled_reference, "вид с полосками Lift B и Gain R");

    set_look(&mut bench, &LookSettings::NEUTRAL);
    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "возврат к нейтрали");
    bench.finish();
}

#[test]
fn lift_blue_paints_the_shadows_clearly_more_than_the_highlights_and_the_gpu_matches_both_references() {
    let config = config();
    let tone = config.viewfinder.tone;
    let look = lift_blue_look();

    // Тёмная сцена: Lift B красит её, картинка совпадает с эталоном вида.
    let Some(mut dark) = ViewfinderBench::start(&config, dark_cell(), 4) else {
        return;
    };
    let dark_neutral = reference_color_with_look(&dark.passport, dark_cell(), &tone, &LookSettings::NEUTRAL);
    let dark_styled = reference_color_with_look(&dark.passport, dark_cell(), &tone, &look);
    assert_distinguishable(dark_styled, dark_neutral, "Lift B в тени");
    set_look(&mut dark, &look);
    dark.show_next_frame();
    assert_image_color(&dark.backend, dark_styled, "тёмная сцена с Lift B");
    dark.finish();

    // Светлая сцена: Lift сходит на нет к белому, синий сдвиг в светах заметно меньше (но не нулевой: зоны
    // полосок перекрываются, решение 340); картинка совпадает с эталоном.
    let Some(mut bright) = ViewfinderBench::start(&config, bright_cell(), 4) else {
        return;
    };
    let bright_neutral = reference_color_with_look(&bright.passport, bright_cell(), &tone, &LookSettings::NEUTRAL);
    let bright_styled = reference_color_with_look(&bright.passport, bright_cell(), &tone, &look);
    let (dark_shift, bright_shift) = (
        code_shift(dark_styled, dark_neutral, 2),
        code_shift(bright_styled, bright_neutral, 2),
    );
    assert!(
        dark_shift > 2.0 * bright_shift.max(0.0) && dark_shift > 8.0 * TOLERANCE,
        "Lift B синит тени ({dark_shift}) заметно сильнее светов ({bright_shift})"
    );
    set_look(&mut bright, &look);
    bright.show_next_frame();
    assert_image_color(&bright.backend, bright_styled, "светлая сцена с Lift B");
    bright.finish();
}

#[test]
fn a_look_that_differs_only_in_bars_produces_a_new_picture_and_one_new_params_resolve() {
    let config = config();
    let Some(mut bench) = ViewfinderBench::start(&config, dark_cell(), 8) else {
        return;
    };
    let tone = config.viewfinder.tone;
    let (blue, red) = (lift_blue_look(), lift_red_look());
    let blue_reference = reference_color_with_look(&bench.passport, dark_cell(), &tone, &blue);
    let red_reference = reference_color_with_look(&bench.passport, dark_cell(), &tone, &red);
    assert_distinguishable(blue_reference, red_reference, "Lift B против Lift R");

    set_look(&mut bench, &blue);
    bench.show_next_frame();
    assert_image_color(&bench.backend, blue_reference, "Lift B");
    let resolved_blue = params_resolved(&mut bench);

    // Все ручки, колёса те же, отличаются только полоски: ключ кэша обязан это увидеть, иначе осталась бы прежняя картинка.
    set_look(&mut bench, &red);
    bench.show_next_frame();
    assert_image_color(&bench.backend, red_reference, "Lift R");
    assert_eq!(
        params_resolved(&mut bench),
        resolved_blue + 1,
        "вид, отличающийся только полосками, пересчитал параметры ровно один раз"
    );
    bench.finish();
}
