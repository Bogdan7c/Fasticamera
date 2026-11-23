//! Снимок берёт вид МОМЕНТА НАЖАТИЯ (инвариант 3, решения 98, 178) на пикселях: настоящий бэкенд (RADV, headless,
//! полная validation), стенд `still_rig`. Нажатие с видом A, сразу после него видоискатель получает вид B; проявленный
//! снимок обязан совпасть с эталоном `isp-model` для вида A и заметно отличаться от эталонов вида B и нейтрали.
//!
//! Кадр со вспышкой на пикселях здесь НЕ сверяется: у него своя экспозиция (доля света вспышки, 14d), эталон потребовал
//! бы повторить расчёт плана вспышки. Вид у него берётся тем же `StillPlan.look` в том же `prepare_frame`
//! (путь один), а сам кадр со вспышкой доказывает `vulkan_still_flash.rs`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod still_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use continuity_rig::{SIZE_A, finish};
use look_contract::{ColorWheel, ColorWheels, LookKnob, LookSettings, WheelPush, WheelZone};
use render_loop::backend::DisplayBackend;
use render_loop::config::StillDevelopConfig;
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use still_contract::develop::DevelopEvent;
use still_rig::{
    JPEG, StillBench, expected_params_with_look, offer_readback, plan, press_during, reference_rgba, setup, shown_frame,
};

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Вид A: тёплая насыщенная картинка с контрастом.
fn look_a() -> LookSettings {
    LookSettings {
        warmth: knob(0.7),
        saturation: knob(0.5),
        contrast: knob(0.3),
        ..LookSettings::NEUTRAL
    }
}

/// Вид B: холодная, приглушённая — противоположность A.
fn look_b() -> LookSettings {
    LookSettings {
        warmth: knob(-0.7),
        saturation: knob(-0.5),
        contrast: knob(-0.3),
        ..LookSettings::NEUTRAL
    }
}

/// Наибольшая разность байтов двух RGBA8-картинок.
fn worst_difference(first: &[u8], second: &[u8]) -> u8 {
    assert_eq!(first.len(), second.len(), "размеры картинок");
    first
        .iter()
        .zip(second)
        .map(|(left, right)| left.abs_diff(*right))
        .max()
        .expect("пиксели")
}

/// Делает снимок кадра на экране в показе `shown` с видом `look_at_press`, а сразу после нажатия отдаёт видоискателю
/// вид `look_after_press`. Возвращает RGBA8 проявленной картинки и ячейку кадра.
fn take_still(
    bench: &mut StillBench,
    id: u64,
    shown: usize,
    look_at_press: &LookSettings,
    look_after_press: &LookSettings,
) -> (Vec<u8>, continuity_rig::CellValues) {
    let (_, frame_index) = shown_frame(bench, shown);
    let mut still = plan(id, press_during(shown), JPEG);
    still.look = look_at_press.clone();
    let sink = bench.begin(still);
    // Видоискатель получает другой вид СРАЗУ после нажатия, до того как снимок проявился.
    let outcome = bench
        .rig
        .backend
        .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::new(look_after_press.clone())));
    assert!(
        matches!(
            outcome,
            ViewfinderCommandOutcome::Applied | ViewfinderCommandOutcome::AlreadyInEffect
        ),
        "{outcome:?}"
    );
    assert!(bench.frames_until(30, || sink.has_secured()), "Secured не пришёл");
    offer_readback(bench, id);
    assert!(bench.frames_until(60, || sink.finals() == 1), "итог снимка не пришёл");
    let image = match sink.events().last() {
        Some(DevelopEvent::PrimaryReadback { image, .. }) => image.clone(),
        other => panic!("итог должен быть PrimaryReadback: {other:?}"),
    };
    assert_eq!(image.size(), SIZE_A);
    let bytes = image.rgba_bytes().to_vec();
    drop(image);
    // События держат буферы GPU картинки; отпускаем до проверки утечек.
    sink.events().clear();
    (bytes, still_rig::cell(frame_index))
}

#[test]
fn a_still_is_developed_with_the_look_at_the_press_even_if_the_viewfinder_look_changes_right_after() {
    let Some(mut bench) = setup(StillDevelopConfig::default(), 12) else {
        return;
    };
    bench.frames(10);
    assert!(bench.inject_timings() >= 6);
    let (look_a, look_b) = (look_a(), look_b());

    // Снимок 1: нажатие при виде A, потом видоискатель получает B.
    let (picture, cell) = take_still(&mut bench, 1, 3, &look_a, &look_b);
    assert!(
        bench.rig.backend.diagnostics().viewfinder.expect("видоискатель").look == Arc::new(look_b.clone()),
        "видоискатель после нажатия действительно переключился на вид B"
    );
    let (params_a, _) = expected_params_with_look(&bench.passport, &look_a);
    let (params_b, _) = expected_params_with_look(&bench.passport, &look_b);
    let (params_neutral, _) = expected_params_with_look(&bench.passport, &LookSettings::NEUTRAL);
    let reference_a = reference_rgba(&params_a, cell, SIZE_A);
    let reference_b = reference_rgba(&params_b, cell, SIZE_A);
    let reference_neutral = reference_rgba(&params_neutral, cell, SIZE_A);
    // Эталоны должны различаться сильнее допуска (1 LSB), иначе тест не отличил бы вид A от B.
    assert!(
        worst_difference(&reference_a, &reference_b) > 8 && worst_difference(&reference_a, &reference_neutral) > 8,
        "эталоны видов почти не различаются: тест ничего не докажет"
    );
    assert!(
        worst_difference(&picture, &reference_a) <= 1,
        "снимок совпадает с эталоном вида A (момент нажатия): {} LSB",
        worst_difference(&picture, &reference_a)
    );
    assert!(
        worst_difference(&picture, &reference_b) > 8,
        "снимок не взял текущий вид видоискателя B"
    );

    // Снимок 2: теперь на видоискателе B; нажатие с нейтралью, видоискатель после него получает A. Снимок — нейтральный.
    let (picture_two, cell_two) = take_still(&mut bench, 2, 5, &LookSettings::NEUTRAL, &look_a);
    let reference_two = reference_rgba(&params_neutral, cell_two, SIZE_A);
    assert!(
        worst_difference(&picture_two, &reference_two) <= 1,
        "второй снимок с нейтралью момента нажатия: {} LSB",
        worst_difference(&picture_two, &reference_two)
    );
    assert!(
        worst_difference(&picture_two, &reference_rgba(&params_a, cell_two, SIZE_A)) > 8,
        "второй снимок не взял вид A видоискателя"
    );

    finish(&mut bench.rig, &[bench.epoch]);
}

/// Вид только с колёсами: тени и середина толкаются к синему, света и весь кадр — к красному, кольцо Gain светлит.
fn wheels_look() -> LookSettings {
    let push = |hue_degrees: f32, ring: f32| ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, 1.0).expect("точка в круге"),
        ring: knob(ring),
    };
    LookSettings {
        wheels: ColorWheels::NEUTRAL
            .with_wheel(WheelZone::Lift, push(265.0, 0.0))
            .with_wheel(WheelZone::Gamma, push(265.0, 0.0))
            .with_wheel(WheelZone::Gain, push(30.0, 1.0))
            .with_wheel(WheelZone::Offset, push(30.0, 0.0)),
        ..LookSettings::NEUTRAL
    }
}

#[test]
fn a_still_carries_the_color_wheels_of_the_press_not_those_of_the_viewfinder() {
    let Some(mut bench) = setup(StillDevelopConfig::default(), 12) else {
        return;
    };
    bench.frames(10);
    assert!(bench.inject_timings() >= 6);
    let wheels = wheels_look();
    assert!(!wheels.wheels.is_neutral(), "вид теста действительно с колёсами");

    // Нажатие с колёсами, сразу после него видоискатель получает нейтральный вид (без колёс).
    let (picture, cell) = take_still(&mut bench, 1, 3, &wheels, &LookSettings::NEUTRAL);
    let (params_wheels, _) = expected_params_with_look(&bench.passport, &wheels);
    let (params_neutral, _) = expected_params_with_look(&bench.passport, &LookSettings::NEUTRAL);
    let reference_wheels = reference_rgba(&params_wheels, cell, SIZE_A);
    let reference_neutral = reference_rgba(&params_neutral, cell, SIZE_A);
    assert!(
        worst_difference(&reference_wheels, &reference_neutral) > 8,
        "эталон с колёсами почти не отличается от нейтрального: тест ничего не докажет"
    );
    assert!(
        worst_difference(&picture, &reference_wheels) <= 1,
        "снимок совпадает с эталоном колёс момента нажатия: {} LSB",
        worst_difference(&picture, &reference_wheels)
    );
    assert!(
        worst_difference(&picture, &reference_neutral) > 8,
        "снимок не взял нейтральный вид видоискателя"
    );

    // Обратное: нажатие без колёс, видоискатель после него получает колёса, — снимок остаётся нейтральным.
    let (picture_two, cell_two) = take_still(&mut bench, 2, 5, &LookSettings::NEUTRAL, &wheels);
    assert!(
        worst_difference(&picture_two, &reference_rgba(&params_neutral, cell_two, SIZE_A)) <= 1,
        "снимок без колёс момента нажатия остаётся нейтральным"
    );
    assert!(
        worst_difference(&picture_two, &reference_rgba(&params_wheels, cell_two, SIZE_A)) > 8,
        "снимок не взял колёса видоискателя"
    );

    finish(&mut bench.rig, &[bench.epoch]);
}
