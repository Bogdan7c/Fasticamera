//! Продуктовая связка: AE авто И AWB авто на реалистичном холодном старте (RADV, полная validation).
//!
//! В `vulkan_ae_scene.rs` AWB зафиксирован ручным указанием; в продукте автоматические оба. Здесь камера
//! стартует с экспозиции из конфига (4 с·ISO), сцена — дневной свет (D65) либо тёмная комната, сенсор
//! отвечает через 8 кадров, шаг замера вниз ограничен. Тест печатает числа холодного старта (`--nocapture`)
//! и падает, если AWB «голодает» дольше лимита скрытия, показывается пересвеченный или цветной кадр.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod ae_rig;
mod awb_support;
mod support;
mod vulkan_support;

use ae_rig::{AeRig, RigSetup, Shot, display_middle_gray, ev_error, rig_with};
use render_loop::config::{AutoExposureConfig, WhiteBalanceConfig};

/// Стартовая экспозиция камеры из конфига, с·ISO.
const START_PRODUCT: f64 = 4.0;
/// Допуск нейтральности плашки: 4 шага 10-битного выхода.
const NEUTRAL_TOLERANCE: f32 = 4.0 / 1023.0;
const FRAMES: usize = 140;

/// Числа холодного старта.
#[derive(Debug)]
#[expect(dead_code, reason = "часть полей только печатается в отчёт сценария")]
struct ColdStart {
    hidden_by_awb: u64,
    forced_provisional: u64,
    frames_with_provisional_white: u64,
    shown: usize,
    shown_clipped: usize,
    first_shown: Option<i64>,
    first_neutral: Option<i64>,
    first_on_target: Option<i64>,
    first_neutral_and_on_target: Option<i64>,
    final_luminance: f64,
    final_spread: f32,
}

fn spread(pixel: [f32; 3]) -> f32 {
    let max = pixel.iter().copied().fold(f32::MIN, f32::max);
    let min = pixel.iter().copied().fold(f32::MAX, f32::min);
    max - min
}

fn run_scenario(name: &str, scene: &'static str, scene_scale: f64) -> Option<ColdStart> {
    let (mut rig, journal) = rig_with(RigSetup {
        auto_exposure: Some(AutoExposureConfig::default()),
        white_balance: None,
        scene,
        start_product: START_PRODUCT,
        scene_scale,
        allow_clipping: true,
    })?;
    rig.run(FRAMES);
    let numbers = summarize(&mut rig, scene_scale);
    let diagnostics = rig.viewfinder();
    eprintln!(
        "[{name}] {numbers:#?}\n[{name}] WB: {:?}\n[{name}] AE: {:?}",
        diagnostics.white_balance, diagnostics.auto_exposure
    );
    rig.finish(&journal);
    Some(numbers)
}

fn summarize(rig: &mut AeRig, scene_scale: f64) -> ColdStart {
    let wanted = display_middle_gray();
    let diagnostics = rig.viewfinder();
    let wb = &diagnostics.white_balance;
    let shown: Vec<&Shot> = rig.history.iter().filter(|shot| shot.pixel.is_some()).collect();
    // Плашка в клипе, если хоть один канал сенсора на белом.
    let clipped = |shot: &Shot| scene_scale * shot.sensor_product * 1.0 >= 1.0;
    let neutral = |shot: &Shot| shot.pixel.is_some_and(|pixel| spread(pixel) <= NEUTRAL_TOLERANCE);
    let on_target = |shot: &Shot| {
        shot.display_luminance
            .is_some_and(|luminance| ev_error(luminance, wanted).abs() < 0.12)
    };
    let first = |predicate: &dyn Fn(&Shot) -> bool| shown.iter().find(|shot| predicate(shot)).map(|shot| shot.frame);
    let last = shown.last().expect("кадры показаны");
    ColdStart {
        hidden_by_awb: wb.hidden_frames_cold_start,
        forced_provisional: wb.cold_start_forced,
        frames_with_provisional_white: wb.frames_with_provisional_white,
        shown: shown.len(),
        shown_clipped: shown.iter().filter(|shot| clipped(shot)).count(),
        first_shown: shown.first().map(|shot| shot.frame),
        first_neutral: first(&neutral),
        first_on_target: first(&on_target),
        first_neutral_and_on_target: first(&|shot| neutral(shot) && on_target(shot)),
        final_luminance: last.display_luminance.expect("показан"),
        final_spread: spread(last.pixel.expect("показан")),
    }
}

fn assert_cold_start_is_clean(numbers: &ColdStart) {
    let limit = u64::from(WhiteBalanceConfig::default().max_hidden_frames);
    assert!(
        numbers.hidden_by_awb <= limit && numbers.forced_provisional == 0,
        "AWB голодал: скрыто {} (лимит {limit}), показано с провизорной белой после лимита {}; {numbers:?}",
        numbers.hidden_by_awb,
        numbers.forced_provisional
    );
    assert_eq!(numbers.shown_clipped, 0, "показан пересвеченный кадр: {numbers:?}");
    assert_eq!(
        numbers.first_neutral, numbers.first_shown,
        "первый показанный кадр не нейтрален: {numbers:?}"
    );
    assert!(
        numbers.first_on_target.is_some()
            && (numbers.final_luminance / display_middle_gray()).log2().abs() < 0.12
            && numbers.final_spread <= NEUTRAL_TOLERANCE,
        "плашка не пришла к нейтральной 0,20: {numbers:?}"
    );
}

#[test]
fn daylight_with_the_camera_starting_four_stops_over_exposed() {
    // Масштаб подобран так, что нужная экспозиция ≈ 0,25 с·ISO: старт 4 с·ISO — это +4 EV пересвета.
    let Some(numbers) = run_scenario("daylight", "checker_d65", 0.30) else {
        return;
    };
    assert_cold_start_is_clean(&numbers);
}

#[test]
fn dark_room_four_stops_under_the_start_exposure() {
    // Нужная экспозиция ≈ 64 с·ISO (на 4 EV больше стартовой).
    let Some(numbers) = run_scenario("dark_room", "checker_d65", 0.0748 / 64.0) else {
        return;
    };
    assert_cold_start_is_clean(&numbers);
}

#[test]
fn tungsten_lamp_gets_the_right_white_before_the_first_shown_frame() {
    // Та же связка под лампой 2800 К: провизорная белая D65 тут цветная, поэтому показ допустим только с оценкой.
    let Some(numbers) = run_scenario("lamp_2800", "checker_bb2800", 0.30) else {
        return;
    };
    assert_cold_start_is_clean(&numbers);
}

#[test]
fn bright_sun_seven_stops_over_the_start_exposure() {
    // Улица днём: нужная экспозиция ≈ 1/32 с·ISO, то есть старт 4 с·ISO пересвечен на +7 EV.
    let Some(numbers) = run_scenario("sun", "checker_d65", 2.4) else {
        return;
    };
    assert_cold_start_is_clean(&numbers);
}
