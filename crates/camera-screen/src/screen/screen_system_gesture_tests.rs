//! Касания в зонах обязательных системных жестов: свайп от нижнего края (вытягивание панели навигации Android)
//! не должен открывать меню Controls. Сквозные тесты: пиксели surface → фильтр → жесты → состояние и эффекты.

use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};

use super::ScreenTouch;
use super::test_rig::*;
use crate::state::PanelState;

/// Зоны телефона владельца: сверху 156 px, снизу 132 px (нижняя полоса y ≥ 2280).
fn owner_zones() -> SystemGestureZones {
    SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 156,
        right: 0,
        bottom: 132,
    })
    .expect("зоны владельца")
}

fn rig_with_owner_zones() -> Rig {
    let mut rig = Rig::new();
    rig.screen.set_system_gesture_zones(owner_zones());
    rig
}

/// Образец касания в пикселях surface (а не в pt, как у `Rig::touch`).
fn px_touch(rig: &mut Rig, pointer: i32, phase: TouchPhase, x_px: f32, y_px: f32, at_ms: i64) {
    rig.screen.handle_touch(ScreenTouch::Sample(TouchSample {
        pointer: PointerId::new(pointer),
        phase,
        position: SurfacePoint { x_px, y_px },
        time: ms(at_ms),
    }));
}

/// Свайп пальцем `pointer` по вертикали из `from_y_px` в `to_y_px` за 160 мс, отрыв и кадр.
fn vertical_swipe(rig: &mut Rig, pointer: i32, x_px: f32, from_y_px: f32, to_y_px: f32) {
    let start = rig.now_ms;
    px_touch(rig, pointer, TouchPhase::Began, x_px, from_y_px, start);
    let steps = 10;
    for step in 1..=steps {
        let y = from_y_px + (to_y_px - from_y_px) * step as f32 / steps as f32;
        px_touch(rig, pointer, TouchPhase::Moved, x_px, y, start + step * FRAME_MS);
    }
    px_touch(
        rig,
        pointer,
        TouchPhase::Ended,
        x_px,
        to_y_px,
        start + steps * FRAME_MS + 1,
    );
    rig.now_ms = start + steps * FRAME_MS + 10;
    rig.frame();
}

#[test]
fn swipe_up_that_starts_in_the_bottom_zone_belongs_to_the_system_and_opens_nothing() {
    let mut rig = rig_with_owner_zones();
    vertical_swipe(&mut rig, 0, 540.0, 2400.0, 1800.0);
    rig.run(1000);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed, "меню не открылось");
    assert!(rig.effects().is_empty(), "эффектов нет");
    let diagnostics = rig.screen.diagnostics();
    assert_eq!(diagnostics.touches_ignored_in_system_gesture_zone, 1);
    assert_eq!(diagnostics.touches_received, 0, "до сцены не дошёл ни один образец");
}

#[test]
fn the_same_swipe_starting_above_the_zone_still_opens_the_menu() {
    let mut rig = rig_with_owner_zones();
    vertical_swipe(&mut rig, 0, 540.0, 2200.0, 1600.0);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 0);
}

#[test]
fn a_touch_that_began_outside_the_zone_keeps_working_after_it_enters_the_zone() {
    // Затвор лежит выше зоны; палец опускается на него и до отрыва уходит в нижнюю зону.
    let mut rig = rig_with_owner_zones();
    let shutter = rig.shutter_center();
    let (x_px, y_px) = (
        (shutter.x * f64::from(PX_PER_PT)) as f32,
        (shutter.y * f64::from(PX_PER_PT)) as f32,
    );
    assert!(y_px < 2280.0, "затвор лежит выше зоны: {y_px}");
    let start = rig.now_ms;
    px_touch(&mut rig, 0, TouchPhase::Began, x_px, y_px, start);
    px_touch(&mut rig, 0, TouchPhase::Moved, x_px, 2350.0, start + 20);
    px_touch(&mut rig, 0, TouchPhase::Ended, x_px, 2350.0, start + 40);
    rig.now_ms = start + 60;
    rig.frame();
    let diagnostics = rig.screen.diagnostics();
    assert_eq!(diagnostics.touches_ignored_in_system_gesture_zone, 0);
    assert_eq!(diagnostics.touches_received, 3, "все образцы дошли до сцены");
}

#[test]
fn without_zones_the_bottom_swipe_behaves_as_before() {
    let mut rig = Rig::new();
    assert!(rig.screen.system_gesture_zones().is_none(), "по умолчанию зон нет");
    vertical_swipe(&mut rig, 0, 540.0, 2200.0, 1600.0);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    let diagnostics = rig.screen.diagnostics();
    assert_eq!(diagnostics.touches_ignored_in_system_gesture_zone, 0);
    assert_eq!(diagnostics.touches_received, 12);
}

#[test]
fn the_bottom_edge_swipe_is_a_real_menu_gesture_when_no_zones_are_known() {
    // Контроль к первому тесту: тот же свайп (от y = 2400) без зон ОТКРЫВАЕТ меню, значит, в тесте с зонами меню
    // не открылось именно из-за фильтра, а не потому что жест сам по себе не распознаётся.
    let mut rig = Rig::new();
    vertical_swipe(&mut rig, 0, 540.0, 2400.0, 1800.0);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
}

#[test]
fn a_second_finger_outside_the_zone_works_while_the_first_belongs_to_the_system() {
    let mut rig = rig_with_owner_zones();
    let start = rig.now_ms;
    // Первый палец лёг в нижнюю зону и остаётся там.
    px_touch(&mut rig, 0, TouchPhase::Began, 100.0, 2400.0, start);
    // Второй палец тапает по затвору вне зоны.
    let shutter = rig.shutter_center();
    let (x_px, y_px) = (
        (shutter.x * f64::from(PX_PER_PT)) as f32,
        (shutter.y * f64::from(PX_PER_PT)) as f32,
    );
    px_touch(&mut rig, 1, TouchPhase::Began, x_px, y_px, start + 10);
    px_touch(&mut rig, 1, TouchPhase::Ended, x_px, y_px, start + 60);
    px_touch(&mut rig, 0, TouchPhase::Ended, 100.0, 2400.0, start + 80);
    rig.now_ms = start + 100;
    rig.frame();
    let diagnostics = rig.screen.diagnostics();
    assert_eq!(diagnostics.touches_ignored_in_system_gesture_zone, 1);
    assert_eq!(diagnostics.touches_received, 2, "дошёл только второй палец");
    assert!(
        rig.effects()
            .iter()
            .any(|effect| matches!(effect, crate::effect::Effect::Capture(_))),
        "тап вторым пальцем нажал затвор"
    );
}

#[test]
fn gap_forgets_system_owned_pointers_so_a_new_touch_is_judged_afresh() {
    let mut rig = rig_with_owner_zones();
    let start = rig.now_ms;
    // Палец в зоне, затем `Ended` потерян: разрыв потока.
    px_touch(&mut rig, 0, TouchPhase::Began, 540.0, 2400.0, start);
    rig.screen.handle_touch(ScreenTouch::Gap);
    // Продолжение того же указателя после разрыва больше не считается системным: до сцены доходит.
    px_touch(&mut rig, 0, TouchPhase::Moved, 540.0, 2300.0, start + 20);
    assert_eq!(rig.screen.diagnostics().touches_received, 1, "учёт сброшен разрывом");
    px_touch(&mut rig, 0, TouchPhase::Ended, 540.0, 2300.0, start + 40);
    // Новое касание того же указателя вне зоны обрабатывается.
    vertical_swipe(&mut rig, 0, 540.0, 2200.0, 1600.0);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
}

#[test]
fn cancelled_ends_system_ownership_and_the_next_touch_of_that_pointer_is_free() {
    let mut rig = rig_with_owner_zones();
    let start = rig.now_ms;
    px_touch(&mut rig, 0, TouchPhase::Began, 540.0, 2400.0, start);
    px_touch(&mut rig, 0, TouchPhase::Cancelled, 540.0, 2400.0, start + 30);
    assert_eq!(
        rig.screen.diagnostics().touches_received,
        0,
        "Cancelled системного касания не дошёл"
    );
    rig.now_ms = start + 40;
    vertical_swipe(&mut rig, 0, 540.0, 2200.0, 1600.0);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 1);
}

#[test]
fn setting_zones_does_not_touch_screen_state_or_effects() {
    let mut rig = Rig::new();
    let before = rig.screen.state().clone();
    rig.screen.set_system_gesture_zones(owner_zones());
    assert_eq!(rig.screen.state(), &before);
    assert!(rig.effects().is_empty());
    assert_eq!(rig.screen.system_gesture_zones(), owner_zones());
}
