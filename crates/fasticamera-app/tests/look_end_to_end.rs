//! Сквозной тест вида картинки на ПК (сессия 14a, решение 178): намерение экрана → редьюсер `camera-screen` →
//! исполнитель вида приложения (`LookEffects` + библиотека пресетов) → команда рендеру → НАСТОЯЩИЙ бэкенд показа
//! (RADV, headless, полная validation, подставная камера) → пиксели видоискателя совпадают с эталоном `isp-model`
//! с теми значениями, которые держит библиотека пресетов.
//!
//! Стенд подставной камеры и эталона — тот же файл, что у тестов `render-loop` (`viewfinder_rig`), подключён по пути:
//! второй копии фальшивой камеры не заводим. «Ящик» рендера здесь — запись команд: их применяет сам тест тем же
//! `apply_viewfinder_command`, каким пользуется поток рендера, когда забирает команду из своего ящика.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/viewfinder_rig/mod.rs"]
mod viewfinder_rig;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;

use std::sync::Mutex;

use camera_screen::{Effect, ScreenAction, ScreenConfig, ScreenState, reduce};
use fasticamera::look_effects::LookEffects;
use fasticamera::look_service::LookHandle;
use fasticamera::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};
use look_contract::{LookIntent, LookKnob, LookSettings};
use look_presets::PresetStoreConfig;
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig};
use viewfinder_rig::{TOLERANCE, ViewfinderBench, assert_image_color, neutral_gray, reference_color_with_look};

/// Ящик рендера теста: запоминает команды, как поток рендера запоминает их до применения.
#[derive(Default)]
struct RecordingRenderInbox {
    adjustments: Mutex<Vec<RenderAdjustment>>,
}

impl RenderControl for RecordingRenderInbox {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery {
        self.adjustments.lock().expect("замок").push(adjustment);
        ControlDelivery::Delivered
    }

    fn ui_overlay(&self, _command: render_loop::ui_overlay::UiOverlayCommand) -> ControlDelivery {
        ControlDelivery::Delivered
    }
}

fn warm_look() -> LookSettings {
    LookSettings {
        warmth: LookKnob::new(0.7).expect("ручка в диапазоне"),
        saturation: LookKnob::new(0.4).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    }
}

#[test]
fn a_screen_set_look_intent_changes_the_picture_the_viewfinder_shows_to_the_look_the_library_holds() {
    let config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        ..RenderLoopConfig::default()
    };
    let Some(mut bench) = ViewfinderBench::start(&config, neutral_gray(), 4) else {
        return;
    };
    let tone = config.viewfinder.tone;

    // Библиотека пресетов приложения; до намерения активен её стартовый вид («Как глаз»).
    let handle = LookHandle::in_memory(&PresetStoreConfig::default()).expect("библиотека пресетов");
    let before = handle.active_settings();
    let mut look_effects = LookEffects::new(Some(handle.clone()));
    let inbox = RecordingRenderInbox::default();

    // Экран получает действие и выдаёт эффект-намерение; исполнитель вида применяет его и шлёт команду рендеру.
    let screen_config = ScreenConfig::default();
    let state = ScreenState::new(&screen_config).expect("состояние экрана");
    let transition = reduce(
        &state,
        ScreenAction::Look(LookIntent::SetLook(warm_look())),
        &screen_config,
    );
    for effect in transition.effects {
        match effect {
            Effect::Look(intent) => look_effects.run_intent(intent, &inbox),
            other => panic!("действие вида породило посторонний эффект: {other:?}"),
        }
    }
    let held = handle.active_settings();
    assert_eq!(held, warm_look(), "библиотека держит значения намерения");
    assert_ne!(held, before, "вид действительно сменился");

    // Кадр ДО доставки команды: рендер ещё рисует стартовый (нейтральный у бэкенда) вид.
    let neutral_reference = reference_color_with_look(&bench.passport, neutral_gray(), &tone, &LookSettings::NEUTRAL);
    let styled_reference = reference_color_with_look(&bench.passport, neutral_gray(), &tone, &held);
    let separation = (0..3)
        .map(|channel| (styled_reference[channel] - neutral_reference[channel]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        separation > 8.0 * TOLERANCE,
        "эталоны почти не различаются: тест ничего не докажет"
    );
    bench.show_next_frame();
    assert_image_color(&bench.backend, neutral_reference, "до команды");

    // Поток рендера забирает команду из ящика и применяет её; следующие кадры рисуются с видом библиотеки.
    let delivered: Vec<RenderAdjustment> = inbox.adjustments.lock().expect("замок").drain(..).collect();
    assert_eq!(
        delivered.len(),
        1,
        "исполнитель вида послал ровно одну команду: {delivered:?}"
    );
    for adjustment in delivered {
        let RenderAdjustment::Viewfinder(command) = adjustment else {
            panic!("ожидалась команда видоискателю: {adjustment:?}");
        };
        assert_eq!(
            bench.backend.apply_viewfinder_command(command),
            render_loop::viewfinder::ViewfinderCommandOutcome::Applied
        );
    }
    bench.show_next_frame();
    assert_image_color(&bench.backend, styled_reference, "после намерения экрана");
    bench.finish();
}
