//! Вид картинки через НАСТОЯЩИЙ бэкенд показа (решение 178): стартовый вид действует с первого кадра, команда
//! `SetLook` применяется один раз, повтор тех же значений — `AlreadyInEffect`, а кадры идут дальше без сбоя.
//! Стенд тот же, что у `vulkan_screen_placement.rs` (RADV, headless-показ, полная validation).
//!
//! Пиксели вида здесь не сверяются: формулы ручек и их паритет с `isp-model` доказывают тесты `isp-vulkan`.
//! Здесь — путь «команда → настройки бэкенда → сессия показа → следующий кадр ISP».

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod camera_rig;
mod support;
mod ui_overlay_rig;
mod vulkan_support;

use std::sync::Arc;

use look_contract::{LookKnob, LookSettings};
use render_loop::backend::DisplayBackend;
use render_loop::config::UiOverlayConfig;
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use ui_overlay_rig::*;

fn warm_look() -> Arc<LookSettings> {
    Arc::new(LookSettings {
        warmth: LookKnob::new(0.5).expect("ручка в диапазоне"),
        contrast: LookKnob::new(-0.25).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    })
}

#[test]
fn the_initial_look_is_in_effect_before_the_first_frame_and_survives_it() {
    // Вид передан при сборке бэкенда, а не командой после старта.
    let Some(mut harness) = Harness::start_with_initial_look(UiOverlayConfig::default(), Some(warm_look())) else {
        return;
    };
    harness.attach(1, SURFACE);
    assert_eq!(harness.viewfinder().look, warm_look(), "вид действует до первого кадра");

    harness.show_new_camera_frame();

    assert_eq!(harness.viewfinder().look, warm_look(), "первый кадр не сбросил вид");
    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(warm_look())),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "стартовые значения и есть действующие"
    );
    harness.finish();
}

#[test]
fn set_look_is_applied_once_and_frames_keep_flowing() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    assert!(harness.viewfinder().look.is_neutral(), "без команды вид нейтральный");
    let processed = harness.viewfinder().counters.frames_processed;

    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(warm_look())),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(harness.viewfinder().look, warm_look());
    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::new((*warm_look()).clone()))),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "те же значения в другом Arc — без изменений"
    );

    harness.show_new_camera_frame();
    assert!(
        harness.viewfinder().counters.frames_processed > processed,
        "после смены вида ISP обработал следующий кадр"
    );

    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetLook(Arc::new(LookSettings::NEUTRAL))),
        ViewfinderCommandOutcome::Applied
    );
    assert!(harness.viewfinder().look.is_neutral());
    harness.finish();
}
