//! Сквозной тест зон системных жестов (пакет P8): событие оболочки `SystemGestureZones` и касания идут через
//! настоящую очередь оболочки и маршрутизатор в настоящий экран. Свайп вверх от нижнего края (так владелец вытягивает
//! скрытую панель навигации) не должен открыть меню Controls. Помощники — из родительского модуля `camera_screen_e2e`.

use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};

use super::*;
use crate::camera_screen_source::PanelKind;

/// Зоны, пропорциональные телефону владельца (снизу 132 px и сверху 156 px при ширине 1080) для surface 1440 px.
fn scaled_owner_zones() -> SystemGestureZones {
    SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 208,
        right: 0,
        bottom: 176,
    })
    .expect("зоны")
}

fn touch_px(harness: &mut Harness, phase: TouchPhase, y_px: f32, at_ms: i64) {
    harness.deliver(ShellEvent::Touch(TouchSample {
        pointer: PointerId::new(0),
        phase,
        position: SurfacePoint {
            x_px: SURFACE_WIDTH_PX as f32 / 2.0,
            y_px,
        },
        time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
    }));
}

/// Свайп вверх по вертикали `from_y_px → to_y_px`, отрыв, несколько кадров.
fn swipe_up(harness: &mut Harness, clock: &mut RenderClock, from_y_px: f32, to_y_px: f32) {
    let start_ms = clock.now_ms;
    touch_px(harness, TouchPhase::Began, from_y_px, start_ms);
    for step in 1..=10 {
        let y = from_y_px + (to_y_px - from_y_px) * step as f32 / 10.0;
        clock.frame(harness);
        touch_px(harness, TouchPhase::Moved, y, clock.now_ms);
    }
    touch_px(harness, TouchPhase::Ended, to_y_px, clock.now_ms + 1);
    clock.frames(harness, 40);
}

fn panel(harness: &Harness) -> PanelKind {
    harness.router.camera_screen().report().expect("отчёт").state.panel
}

#[test]
fn a_bottom_edge_swipe_does_not_open_the_menu_once_the_shell_reported_the_zones() {
    let (mut harness, mut clock) = ready_screen();
    harness.deliver(ShellEvent::SystemGestureZones(scaled_owner_zones()));
    clock.frames(&harness, 2);

    swipe_up(&mut harness, &mut clock, 3190.0, 2400.0);

    assert_eq!(panel(&harness), PanelKind::Closed, "меню не открылось");
    let report = harness.router.camera_screen().report().expect("отчёт");
    assert_eq!(report.diagnostics.touches_ignored_in_system_gesture_zone, 1);
    assert_eq!(
        report.effects.camera.sent, 0,
        "камере ничего не ушло: касание принадлежало системе"
    );
}

#[test]
fn the_same_swipe_opens_the_menu_when_the_shell_reported_no_zones() {
    // Контроль: без события зон тот же свайп открывает меню — значит, в тесте выше его остановили именно зоны.
    let (mut harness, mut clock) = ready_screen();

    swipe_up(&mut harness, &mut clock, 3190.0, 2400.0);

    assert_eq!(panel(&harness), PanelKind::ControlsMenu);
    let report = harness.router.camera_screen().report().expect("отчёт");
    assert_eq!(report.diagnostics.touches_ignored_in_system_gesture_zone, 0);
}

#[test]
fn zones_reported_before_the_screen_exists_are_applied_when_it_is_installed() {
    // Платформа шлёт зоны один раз и обычно до установки экрана: сервис обязан их запомнить.
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::new(instant_capture_config());
    harness.deliver(ShellEvent::SystemGestureZones(scaled_owner_zones()));
    assert_eq!(
        harness.router.camera_screen().dropped_input().actions,
        0,
        "зоны без экрана запомнены, а не потеряны"
    );
    harness.router.install_camera_screen();
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);

    swipe_up(&mut harness, &mut clock, 3190.0, 2400.0);

    assert_eq!(panel(&harness), PanelKind::Closed);
    assert_eq!(
        harness
            .router
            .camera_screen()
            .report()
            .expect("отчёт")
            .diagnostics
            .touches_ignored_in_system_gesture_zone,
        1
    );
}
