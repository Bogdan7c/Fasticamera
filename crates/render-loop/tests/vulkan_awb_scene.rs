//! Белая из указания владельца на настоящем драйвере (RADV, полная validation): синтетическая сцена ColorChecker
//! под лампой 2800 К (камерные значения плиток — из независимой фикстуры `test-assets/scene-estimation`) идёт через
//! боевой путь: импорт → ISP на GPU → параметры ISP из указания → изображение видоискателя. Статистика кадра
//! нужна только AE: стенд без AE её не считает вовсе (ток стадии не тратится), белая от неё не зависит.
//!
//! Доказывается:
//! * держимая белая лампы: **каждый показанный кадр сбалансирован с первого**, ничего не скрыто;
//! * без указаний белая временная D65: кадры показываются и заметно цветные (контроль чувствительности);
//! * подсказки HAL (намеренно неверные) на изображение не влияют: два прогона с разными подсказками дают
//!   одинаковые пиксели и одинаковую белую;
//! * кадры серии с брекетингом не показываются и камере возвращаются;
//! * без потребителя (AE выключен) статистика кадра не считается и не публикуется;
//! * «ровно один раз»: обработано = показано.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod awb_support;
mod support;
mod vulkan_support;
mod white_rig;

use std::sync::PoisonError;

use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::three_a::{BurstId, RequestOrigin};
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::WhiteLinks;
use vulkan_support::held_white;
use white_rig::{
    BRACKET_TS_BASE, FIRST_TS, FIXTURE_LIGHT_KELVIN, FRAME_NANOS, NEUTRAL_TOLERANCE, rig_or_skip, scene_set, spread,
    wild_hal_hints,
};

/// Прогон сцены лампы под держимой белой лампы: каждый показанный кадр проверяется на нейтральность серой плитки.
/// Возвращает финальные пиксели изображения и диагностику.
fn run_lamp_scene(hints: CaptureColorHints) -> Option<(Vec<[f32; 4]>, render_loop::viewfinder::ViewfinderDiagnostics)> {
    let (mut rig, journal) = rig_or_skip(
        scene_set(false),
        held_white(FIXTURE_LIGHT_KELVIN),
        hints,
        WhiteBalanceConfig::default(),
    )?;
    for index in 0..30 {
        let shown = rig.present_new_frame(FIRST_TS + index * FRAME_NANOS, RequestOrigin::Repeating);
        let gray = shown.gray.expect("белая известна с первого кадра: ничего не скрыто");
        assert!(
            spread(gray) <= NEUTRAL_TOLERANCE,
            "кадр {index}: показанный кадр не сбалансирован, серая плитка {gray:?}"
        );
    }
    let diagnostics = rig.viewfinder();
    let pixels = rig.readback().pixels;
    rig.finish(&journal);
    Some((pixels, diagnostics))
}

#[test]
fn every_frame_is_balanced_from_the_first_and_without_ae_no_statistics_are_computed() {
    let Some((pixels, diagnostics)) = run_lamp_scene(CaptureColorHints::default()) else {
        return;
    };
    let counters = &diagnostics.counters;
    let wb = &diagnostics.white_balance;
    assert_eq!(wb.hidden_frames_cold_start, 0, "заявки нет — ничего не скрыто: {wb:?}");
    assert_eq!(wb.cold_start_forced, 0);
    assert_eq!(counters.frames_processed, 30, "обработано = показано: {counters:?}");
    assert_eq!(counters.frames_hidden_cold_start, 0);
    let applied = wb.applied_white.expect("белая применена");
    assert!(
        (applied.temperature_kelvin - FIXTURE_LIGHT_KELVIN).abs() < 1.0,
        "применена белая указания: {applied:?}"
    );
    assert_eq!(wb.directive_generation, Some(1));
    assert_eq!(wb.frames_with_provisional_white, 0);
    // Потребителей статистики нет (стенд без AE): стадия не считается, публикаций и расписания нет.
    assert_eq!(diagnostics.statistics, None, "{diagnostics:?}");
    assert_eq!(counters.statistics_published, 0, "{counters:?}");
    assert!(!pixels.is_empty());
}

#[test]
fn without_any_directive_the_provisional_white_shows_off_colour_and_nothing_is_hidden() {
    // Контроль чувствительности на боевом пути: стенд без движка — белая временная D65, серая плитка лампы 2800 К
    // заметно цветная. Значит, нейтральность кадров в основном тесте обеспечивает именно указание.
    let Some((mut rig, journal)) = rig_or_skip(
        scene_set(false),
        WhiteLinks::default(),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    ) else {
        return;
    };
    let first = rig.present_new_frame(FIRST_TS, RequestOrigin::Repeating);
    let gray = first.gray.expect("без движка кадры не прячутся");
    assert!(
        spread(gray) > 0.1,
        "серая плитка под временной белой должна быть цветной: {gray:?}"
    );
    let diagnostics = rig.viewfinder().white_balance;
    assert_eq!(
        (diagnostics.hidden_frames_cold_start, diagnostics.cold_start_forced),
        (0, 0)
    );
    assert_eq!(diagnostics.frames_with_provisional_white, 1);
    assert_eq!(diagnostics.applied_white, None);
    rig.finish(&journal);
}

#[test]
fn wild_hal_colour_hints_change_neither_the_picture_nor_the_white() {
    let Some((quiet_pixels, quiet)) = run_lamp_scene(CaptureColorHints::default()) else {
        return;
    };
    let Some((wild_pixels, wild)) = run_lamp_scene(wild_hal_hints()) else {
        return;
    };
    assert_eq!(
        quiet_pixels, wild_pixels,
        "изображение побитово то же при любых подсказках HAL"
    );
    assert_eq!(quiet.white_balance.applied_white, wild.white_balance.applied_white);
    assert_eq!(quiet.counters.params_resolved, wild.counters.params_resolved);
    assert_eq!(quiet.white_balance.hal_white, None, "у тихого прогона белой HAL нет");
    let hal = wild
        .white_balance
        .hal_white
        .expect("коэффициенты HAL дали белую для сравнения");
    let applied = wild.white_balance.applied_white.expect("применённая белая");
    assert!(
        (hal.temperature_kelvin - applied.temperature_kelvin).abs() > 500.0,
        "разница с HAL видна в диагностике: {hal:?} против {applied:?}"
    );
    assert!(wild.white_balance.hal_delta.is_some());
}

#[test]
fn bracket_frames_are_not_shown_not_processed_and_returned_to_the_camera() {
    let Some((mut rig, journal)) = rig_or_skip(
        scene_set(true),
        held_white(FIXTURE_LIGHT_KELVIN),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    ) else {
        return;
    };
    for index in 0..20 {
        rig.present_new_frame(FIRST_TS + index * FRAME_NANOS, RequestOrigin::Repeating);
    }
    let settled = rig.viewfinder();
    let settled_pixels = rig.readback().pixels;
    let settled_white = settled.white_balance.applied_white;
    assert!(settled_white.is_some());

    let burst = BurstId(1);
    for index in 0..3u32 {
        let shown = rig.present_new_frame(
            BRACKET_TS_BASE + i64::from(index) * FRAME_NANOS,
            RequestOrigin::Burst { burst, index },
        );
        assert_eq!(
            shown.presented_after, shown.presented_before,
            "кадр серии {index} не показан"
        );
        assert_eq!(shown.gray, None);
    }
    // Ещё несколько кадров без новых: обработка кадров серии (если бы она была) успела бы проявиться.
    for _ in 0..4 {
        rig.tick();
    }
    let after = rig.viewfinder();
    assert_eq!(after.counters.bracket_frames_skipped, 3);
    assert_eq!(
        after.counters.frames_processed, settled.counters.frames_processed,
        "ISP кадры серии не обрабатывал"
    );
    assert_eq!(after.white_balance.applied_white, settled_white, "белая не сдвинулась");
    assert_eq!(rig.readback().pixels, settled_pixels, "изображение осталось прежним");
    let returned = rig.returned.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for index in 0..3 {
        assert!(
            returned.contains(&(BRACKET_TS_BASE + index * FRAME_NANOS)),
            "кадр серии {index} возвращён камере"
        );
    }
    // Обычные кадры после серии снова показываются.
    let shown = rig.present_new_frame(FIRST_TS + 20 * FRAME_NANOS, RequestOrigin::Repeating);
    assert!(shown.presented_after > shown.presented_before);
    rig.finish(&journal);
}
