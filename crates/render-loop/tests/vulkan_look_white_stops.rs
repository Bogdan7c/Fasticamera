//! Упоры ручек теплоты и оттенка через НАСТОЯЩИЙ бэкенд показа (решения 288, 293): до первого кадра на доске
//! «неизвестно», после кадра на лампе — посчитанные упоры, команда `SetLook` с другой теплотой меняет упор на
//! доске, а смена потока снова даёт «неизвестно». Стенд тот же, что у `vulkan_white_held.rs` (RADV, headless).

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
use look_contract::{KnobStop, LookKnob, LookSettings, LookWhiteStops, LookWhiteStopsBoard};
use render_loop::backend::DisplayBackend;
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome, WhiteLinks};
use white_engine::{held_setting, setting_of};
use white_rig::{rig_or_skip, scene_set, second_lens_passport};

fn quiet(_: i64) -> CaptureColorHints {
    CaptureColorHints::default()
}

fn warmth(value: f32) -> Arc<LookSettings> {
    Arc::new(LookSettings {
        warmth: LookKnob::new(value).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    })
}

/// Упор оттенка «+» на доске: падает, если упоров нет.
fn tint_plus(board: &LookWhiteStopsBoard) -> f32 {
    match board.latest() {
        LookWhiteStops::Known(stops) => match stops.tint.plus {
            KnobStop::At(fraction) => fraction.get(),
            other => panic!("ждали упор оттенка «+» на тёплой лампе, получили {other:?}"),
        },
        other => panic!("ждали посчитанные упоры, получили {other:?}"),
    }
}

#[test]
fn the_board_follows_the_real_session_frames_look_commands_and_stream_changes() {
    // Лампа 2000 К названа указанием: белая сцены тёплая, оттенок «+» упирается (см. тесты `isp-model`).
    let links = held_setting(setting_of(2000.0));
    let board = links.look_white_stops.clone();
    assert_eq!(board.latest(), LookWhiteStops::Unknown, "до кадров упоров нет");
    let Some((mut rig, journal)) = rig_or_skip(
        scene_set(false),
        links,
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    ) else {
        return;
    };
    assert_eq!(board.latest(), LookWhiteStops::Unknown, "поток есть, кадров ещё нет");

    rig.drive(0..6, &quiet, &mut |_| {});
    let neutral_stop = tint_plus(&board);
    assert!(neutral_stop > 0.0 && neutral_stop < 1.0, "{neutral_stop}");

    // Теплота −1 приближает упор оттенка: он считается ТЕМИ ЖЕ входами, что и кадр, и доезжает до доски.
    assert_eq!(
        rig.backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(warmth(-1.0))),
        ViewfinderCommandOutcome::Applied
    );
    rig.drive(6..12, &quiet, &mut |_| {});
    let warm_stop = tint_plus(&board);
    assert!(
        warm_stop < neutral_stop - 0.05,
        "теплота −1 сдвигает упор оттенка к нулю: {neutral_stop} → {warm_stop}"
    );

    // Поток закончился: упоры прежнего потока чужие любому следующему и с доски уходят.
    rig.stream.producer().end_stream(rig.epoch);
    rig.tick();
    assert_eq!(board.latest(), LookWhiteStops::Unknown, "упоры прежнего потока чужие");
    // Новый поток (другая линза): упоры считаются заново по его калибровке.
    rig.epoch = rig
        .stream
        .producer()
        .begin_stream(Arc::new(second_lens_passport()))
        .epoch;
    rig.drive(12..18, &quiet, &mut |_| {});
    assert!(
        matches!(board.latest(), LookWhiteStops::Known(_)),
        "в новом потоке посчитаны заново"
    );

    rig.finish(&journal);
}

#[test]
fn default_links_carry_an_unknown_board() {
    assert_eq!(WhiteLinks::default().look_white_stops.latest(), LookWhiteStops::Unknown);
}
