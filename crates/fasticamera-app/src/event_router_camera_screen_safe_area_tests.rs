//! Сквозной тест безопасной зоны (14k): событие оболочки `SafeArea` идёт через настоящую очередь оболочки и
//! маршрутизатор в настоящий экран и доходит до его состояния. Что экран с ней делает (раскладка, палец по AGX) —
//! `camera-screen/src/screen/screen_calibration_safe_area_tests.rs` и `tests/calibration_safe_area_end_to_end.rs`.

use shell_contract::edge_insets::RawEdgeInsetsPx;
use shell_contract::safe_area::SafeAreaInsets;

use super::*;

fn insets(top: i32) -> SafeAreaInsets {
    SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top,
        right: 0,
        bottom: 176,
    })
    .expect("отступы")
}

fn shown_safe_area(harness: &Harness) -> SafeAreaInsets {
    harness.router.camera_screen().report().expect("отчёт").state.safe_area
}

#[test]
fn the_safe_area_reaches_the_installed_screen_and_every_change_replaces_the_previous_one() {
    let (mut harness, mut clock) = ready_screen();
    assert_eq!(
        shown_safe_area(&harness),
        SafeAreaInsets::NONE,
        "до сообщения отступов нет"
    );
    for top in [208, 90] {
        harness.deliver(ShellEvent::SafeArea(insets(top)));
        clock.frames(&harness, 2);
        assert_eq!(shown_safe_area(&harness), insets(top));
    }
}

#[test]
fn a_safe_area_reported_before_the_screen_exists_is_remembered_and_given_at_install() {
    // Платформа шлёт зону один раз и обычно до установки экрана: маршрутизатор обязан её запомнить.
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::new(instant_capture_config());
    harness.deliver(ShellEvent::SafeArea(insets(208)));
    assert_eq!(
        harness.router.camera_screen().dropped_input().actions,
        0,
        "зона без экрана запомнена, а не потеряна"
    );
    harness.router.install_camera_screen();
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);
    assert_eq!(shown_safe_area(&harness), insets(208));
}
