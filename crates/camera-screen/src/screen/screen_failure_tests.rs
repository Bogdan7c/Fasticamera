//! Ошибки кадра и смена размера: сцена сохраняется там, где это безопасно, жесты не пропадают молча,
//! устойчивая ошибка не пересобирает сцену на каждом кадре.

use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;

use super::ScreenTouch;
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::facts::{CameraFacts, CameraPresence, UnavailableReason};
use crate::state::PanelState;

/// Конфиг, при котором плашка «камера недоступна» не раскладывается (символ вне шрифта): кадр с недоступной
/// камерой падает на сборке вида, а не на изменении сцены.
fn config_with_unrenderable_notice() -> ScreenConfig {
    let mut config = ScreenConfig::default();
    config.widgets.overlay.unavailable_text = "日本語".to_owned();
    config
}

fn camera_unavailable() -> ScreenAction {
    ScreenAction::CameraFactsChanged(CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: UnavailableReason::InUse,
        },
        ..CameraFacts::STARTING
    })
}

#[test]
fn a_view_error_keeps_the_scene_and_the_running_gesture() {
    let mut rig = Rig::with(config_with_unrenderable_notice(), MotionPreference::Full);
    let center = rig.frame_center();
    let rebuilds = rig.screen.diagnostics().stage_rebuilds;
    // Палец пошёл вверх по видоискателю (пан меню) и ещё не оторван.
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, center, start);
    rig.touch(TouchPhase::Moved, Point::new(center.x, center.y - 20.0), start + 16);
    // Камера пропала: вид плашки не собирается, кадр падает.
    rig.screen.handle_action(camera_unavailable());
    rig.now_ms = start + 32;
    assert!(
        rig.screen
            .produce_frame(&request(rig.now_ms, (WIDTH_PX, HEIGHT_PX)))
            .is_err(),
        "кадр с нераскладываемой плашкой — ошибка источника"
    );
    let after_failure = rig.screen.diagnostics();
    assert_eq!(after_failure.frame_failures_scene_kept, 1);
    assert_eq!(after_failure.stage_rebuilds, rebuilds, "сцена не пересобиралась");
    // Касания идут в ту же сцену, а не в пустоту.
    let received = after_failure.touches_received;
    rig.touch(TouchPhase::Moved, Point::new(center.x, center.y - 140.0), start + 48);
    assert_eq!(rig.screen.diagnostics().touches_received, received + 1);
    assert_eq!(rig.screen.diagnostics().touches_dropped_while_stage_missing, 0);
    // Камера вернулась, палец оторван: начатый до ошибки жест доводится до результата (меню открыто).
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(crate::test_support::live_facts()));
    rig.touch(TouchPhase::Ended, Point::new(center.x, center.y - 140.0), start + 64);
    rig.now_ms = start + 80;
    rig.frame();
    rig.run(100);
    assert_eq!(
        rig.screen.state().panel(),
        PanelState::ControlsMenu,
        "жест, идущий во время ошибки вида, не потерян"
    );
    assert_eq!(rig.screen.diagnostics().stage_rebuilds, rebuilds);
}

#[test]
fn a_persistent_build_error_rebuilds_rarely_and_counts_dropped_touches() {
    let mut rig = Rig::new();
    let base = rig.screen.diagnostics();
    let broken_size = (1.0, 1.0);
    // 20 кадров по 16 мс (320 мс) с непригодной раскладкой: пересобирать каждый кадр нельзя.
    for step in 1..=20 {
        rig.now_ms += FRAME_MS;
        assert!(rig.screen.produce_frame(&request(rig.now_ms, broken_size)).is_err());
        if step == 2 {
            rig.screen.handle_touch(ScreenTouch::Gap);
        }
    }
    let during = rig.screen.diagnostics();
    assert!(
        during.stage_rebuilds - base.stage_rebuilds <= 2,
        "пересборок за 320 мс: {}",
        during.stage_rebuilds - base.stage_rebuilds
    );
    assert!(during.frames_skipped_rebuild_backoff >= 10, "{during:?}");
    assert_eq!(
        during.touches_dropped_while_stage_missing, 1,
        "касание без сцены посчитано"
    );
    assert_eq!(during.touches_dropped_before_first_frame, 0, "первый кадр уже был");
    assert_eq!(during.frame_failures - base.frame_failures, 20);
    // Пауза кончилась, размер снова нормальный: экран восстанавливается сам.
    rig.now_ms += 300;
    rig.frame();
    rig.run(50);
    assert!(
        rig.screen
            .produce_frame(&request(rig.now_ms, (WIDTH_PX, HEIGHT_PX)))
            .is_ok()
    );
}

#[test]
fn resizing_in_the_middle_of_a_gesture_cancels_it_explicitly() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    let before = rig.screen.diagnostics();
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, center, start);
    rig.touch(TouchPhase::Moved, Point::new(center.x, center.y - 40.0), start + 16);
    // Размер surface поменялся, пока палец на экране.
    rig.now_ms = start + 32;
    rig.last = rig
        .screen
        .produce_frame(&request(rig.now_ms, (720.0, 1608.0)))
        .expect("кадр на новом размере");
    let after = rig.screen.diagnostics();
    assert!(
        after.gestures_cancelled_by_stage_change >= 1,
        "жест отменён явно: {after:?}"
    );
    assert!(
        after.gestures.cancelled > before.gestures.cancelled,
        "фаза Cancelled учтена"
    );
    // Хвост старого жеста не открывает меню на новой геометрии.
    rig.touch(TouchPhase::Ended, center, start + 48);
    rig.now_ms = start + 64;
    rig.last = rig
        .screen
        .produce_frame(&request(rig.now_ms, (720.0, 1608.0)))
        .expect("кадр");
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
}
