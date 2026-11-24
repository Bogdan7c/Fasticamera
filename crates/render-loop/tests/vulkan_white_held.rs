//! Держимая белая на настоящем драйвере (RADV, полная validation): указание сильнее нашего оценщика, смена
//! линзы и новая сессия показа белую держат и источник заново не спрашивают, переход `Over` идёт кадрами между
//! и заканчивается ровно на цели.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod awb_support;
mod support;
mod vulkan_support;
mod white_engine;
mod white_rig;

use std::sync::Arc;

use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{WhiteDirective, WhiteTransition};
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::WhiteLinks;
use scene_estimation::white_source::WhiteInputKind;
use white_engine::{SCRIPTED_ID, Script, WhiteTestEngine, held_setting, lamp_outcome, scripted_registry, setting_of};
use white_rig::{FrameImage, GRAY_PIXEL, Rig, rig_or_skip, scene_set, second_lens_passport, spread};

fn quiet(_: i64) -> CaptureColorHints {
    CaptureColorHints::default()
}

fn start(links: WhiteLinks, config: WhiteBalanceConfig) -> Option<(Rig, Arc<support::Journal>)> {
    rig_or_skip(scene_set(false), links, CaptureColorHints::default(), config)
}

/// Серая плитка кадра (центр) из пикселей изображения 32×24.
fn gray_of(image: &FrameImage) -> [f32; 3] {
    let pixels = image.as_ref().expect("кадр показан");
    let pixel = pixels[GRAY_PIXEL.1 * 32 + GRAY_PIXEL.0];
    [pixel[0], pixel[1], pixel[2]]
}

#[test]
fn the_directive_wins_over_what_our_old_estimator_hears_and_the_estimator_keeps_feeding_statistics() {
    // Сцена — лампа 2800 К, указание — D65-подобные 6500 К: старый оценщик слышит лампу, картинка идёт по указанию.
    let setting = setting_of(6500.0);
    let Some((mut rig, journal)) = start(held_setting(setting), WhiteBalanceConfig::default()) else {
        return;
    };
    let images = rig.drive(0..30, &quiet, &mut |_| {});
    let diagnostics = rig.viewfinder();
    let wb = &diagnostics.white_balance;
    assert!(wb.estimates >= 1, "статистика идёт (AE на ней стоит): {wb:?}");
    let heard = wb.last_estimate.expect("оценка").white.temperature_kelvin;
    assert!((heard - 2800.0).abs() < 100.0, "оценщик слышит лампу: {heard}");
    assert!((wb.applied_white.expect("белая").temperature_kelvin - 6500.0).abs() < 1.0);
    let gray = gray_of(&images[29]);
    assert!(
        spread(gray) > 0.1,
        "лампа под белой 6500 К оранжевая, а не «исправлена» оценщиком: {gray:?}"
    );
    let last = rig.pixels();
    rig.finish(&journal);

    // Тот же прогон с другим расписанием статистики и дикими подсказками: оценщик ведёт себя иначе, картинка прежняя.
    let config = WhiteBalanceConfig {
        statistics_every_n_frames: 1,
        ..WhiteBalanceConfig::default()
    };
    let Some((mut other, journal)) = start(held_setting(setting), config) else {
        return;
    };
    let wild = |frame: i64| {
        awb_support::phone_hints_for_white(awb_support::planckian_white(2000.0 + 500.0 * (frame % 7) as f64))
    };
    let other_images = other.drive(0..30, &wild, &mut |_| {});
    let other_diagnostics = other.viewfinder().white_balance;
    assert!(other_diagnostics.estimates > wb.estimates, "оценщик работал по-другому");
    assert_eq!(other_images[29], images[29], "пиксели побитово те же");
    assert_eq!(other.pixels(), last);
    other.finish(&journal);
}

#[test]
fn a_new_lens_and_a_new_surface_keep_the_white_and_never_ask_the_source_again() {
    let frames_before = 12;
    let total = 30;
    let answers_lamp = Script::AnswerAfter {
        observed_frames: 3,
        outcome: lamp_outcome(2800.0),
    };
    // Эталон: сразу вторая линза, белая лампы названа с начала.
    let Some((mut reference, ref_journal)) = start(held_setting(setting_of(2800.0)), WhiteBalanceConfig::default())
    else {
        return;
    };
    reference.begin_stream(second_lens_passport());
    let ref_images = reference.drive(0..total, &quiet, &mut |_| {});
    reference.finish(&ref_journal);

    let (registry, log) = scripted_registry(answers_lamp, WhiteInputKind::ColorHints);
    let links = WhiteLinks {
        sources: registry,
        ..WhiteLinks::default()
    };
    let mut engine = WhiteTestEngine::open(&links, 5600.0, SCRIPTED_ID);
    let Some((mut rig, journal)) = start(links, WhiteBalanceConfig::default()) else {
        return;
    };
    let before = rig.drive(0..frames_before, &quiet, &mut |_| engine.react());
    assert!(before.iter().any(Option::is_some), "открытие закончилось");
    let first_lens = rig.pixels();
    // Линза сменилась: другой поток, другая калибровка.
    rig.begin_stream(second_lens_passport());
    let after_lens = rig.drive(frames_before..20, &quiet, &mut |_| engine.react());
    assert!(
        after_lens.iter().all(Option::is_some),
        "смена линзы картинку не прячет: белая известна"
    );
    assert_eq!(
        after_lens[7], ref_images[19],
        "нейтральный предмет совпадает с эталоном второй линзы"
    );
    assert_ne!(rig.pixels(), first_lens, "линза в самом деле другая");
    // Новая сессия показа: surface снята и присоединена заново.
    rig.reattach_surface(&journal, 2);
    let after_surface = rig.drive(20..total, &quiet, &mut |_| engine.react());
    assert!(
        after_surface.iter().all(Option::is_some),
        "новая сессия показывает сразу: белая известна"
    );
    assert_eq!(
        after_surface.last().expect("кадры"),
        ref_images.last().expect("кадры"),
        "белая та же и после новой surface"
    );
    let diagnostics = rig.viewfinder().white_balance;
    assert_eq!(diagnostics.answers_published, 0, "новая сессия ничего не спрашивала");
    let log = log.lock().unwrap();
    assert_eq!(
        log.requests.len(),
        1,
        "источник спрошен один раз за всё время: {:?}",
        log.requests
    );
    assert!(
        log.streams_begun >= 3,
        "источники узнали о новых потоках: {}",
        log.streams_begun
    );
    drop(log);
    assert_eq!(engine.answers_read.len(), 1);
    rig.finish(&journal);
}

#[test]
fn an_over_transition_passes_through_in_between_frames_and_ends_bit_exactly_on_the_target() {
    let links = held_setting(setting_of(6500.0));
    let Some((mut rig, journal)) = start(links.clone(), WhiteBalanceConfig::default()) else {
        return;
    };
    let from = rig.drive(0..6, &quiet, &mut |_| {});
    links.directives.publish(WhiteDirective {
        white: setting_of(2800.0),
        transition: WhiteTransition::Over(std::time::Duration::from_millis(300)),
        request: None,
    });
    let moving = rig.drive(6..50, &quiet, &mut |_| {});
    rig.finish(&journal);

    let reference = |kelvin: f64| {
        let (mut rig, journal) =
            start(held_setting(setting_of(kelvin)), WhiteBalanceConfig::default()).expect("GPU есть");
        let images = rig.drive(0..50, &quiet, &mut |_| {});
        rig.finish(&journal);
        images
    };
    let start_images = reference(6500.0);
    let target_images = reference(2800.0);
    assert_eq!(from[5], start_images[5], "до указания картинка прежняя");
    // Красный минус синий серой плитки лампы: под белой 6500 К сильно положителен, под 2800 К ≈ 0.
    let warmth = |image: &FrameImage| {
        let gray = gray_of(image);
        gray[0] - gray[2]
    };
    let series: Vec<f32> = moving.iter().map(warmth).collect();
    assert!(
        series.windows(2).all(|pair| pair[1] <= pair[0] + 1e-6),
        "переход монотонен: {series:?}"
    );
    let settled = moving
        .iter()
        .enumerate()
        .position(|(index, image)| *image == target_images[index + 6])
        .expect("цель достигнута");
    let in_between = moving[..settled]
        .iter()
        .enumerate()
        .filter(|(index, image)| **image != start_images[index + 6] && **image != target_images[index + 6])
        .count();
    assert!(
        in_between >= 8,
        "промежуточных кадров между: {in_between} (0,3 с ≈ 18 кадров)"
    );
    for (index, image) in moving.iter().enumerate().skip(settled) {
        assert_eq!(
            *image,
            target_images[index + 6],
            "кадр {}: после цели побитово цель",
            index + 6
        );
    }
}
