#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Арбитраж жестов видоискателя (`docs/ios-camera/behavior.md`, таблица 1.2) на настоящей сцене:
//! касания в пикселях → хит-тест дерева → арена. Точные последовательности состояний.
//!
//! Видоискатель — родитель с `Tap` (фокус), `LongPress` (AE/AF Lock), `Pinch` (зум), горизонтальным
//! `Pan` (режимы) и `Pan` вверх (меню Controls). Дочерний элемент «солнце» у рамки фокуса — свой
//! вертикальный `Pan` (экспозиция).

mod common;

use common::*;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::scene::UiScene;

const VIEWFINDER: Rect = Rect::from_xywh(0.0, 100.0, 440.0, 700.0);
/// «Солнце» рядом с рамкой фокуса: узкая вертикальная полоса внутри видоискателя.
const SUN: Rect = Rect::from_xywh(300.0, 350.0, 40.0, 200.0);

fn pan(direction: PanDirection) -> RecognizerSpec {
    RecognizerSpec::Pan(PanSpec {
        direction,
        ..PanSpec::default()
    })
}

fn viewfinder_scene() -> UiScene<&'static str> {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let viewfinder = add_rect(&mut scene, root, "viewfinder", VIEWFINDER);
    // Порядок добавления — второй ключ приоритета; он не должен решать исход (решает глубина и направление).
    for (spec, name) in [
        (RecognizerSpec::Tap(Default::default()), "tap_focus"),
        (RecognizerSpec::LongPress(Default::default()), "long_press_lock"),
        (RecognizerSpec::Pinch(Default::default()), "pinch_zoom"),
        (pan(PanDirection::Horizontal), "pan_modes"),
        (pan(PanDirection::Up), "pan_menu"),
    ] {
        scene.add_recognizer(viewfinder, spec, name).unwrap();
    }
    let sun = add_rect(&mut scene, viewfinder, "sun", SUN);
    scene
        .add_recognizer(sun, pan(PanDirection::Vertical), "pan_sun")
        .unwrap();
    scene
}

/// События как «имя:состояние» в порядке возникновения.
fn trace(events: Vec<GestureEvent<&'static str>>) -> Vec<String> {
    events
        .into_iter()
        .map(|event| format!("{}:{:?}", event.intent, event.state))
        .collect()
}

fn touch(scene: &mut UiScene<&'static str>, pointer: i32, phase: TouchPhase, x: f64, y: f64, ms: i64) -> Vec<String> {
    scene.handle_touch(&touch_at_pt(pointer, phase, x, y, ms));
    trace(scene.drain_gesture_events())
}

/// Вертикальный жест по «солнцу»: побеждает «солнце» (оно глубже), меню не открывается.
#[test]
fn vertical_drag_on_the_sun_goes_to_the_sun_and_never_opens_the_menu() {
    let mut scene = viewfinder_scene();
    assert!(touch(&mut scene, 0, TouchPhase::Began, 320.0, 500.0, 0).is_empty());
    // 8 pt вверх: гистерезис (10 pt) ещё не пройден — ничего не решено.
    assert!(touch(&mut scene, 0, TouchPhase::Moved, 320.0, 492.0, 16).is_empty());
    // 14 pt вверх: ось вертикальная; «солнце» и меню готовы одновременно, глубже — «солнце».
    let began = touch(&mut scene, 0, TouchPhase::Moved, 320.0, 486.0, 32);
    assert_eq!(
        began,
        [
            "tap_focus:Failed",
            "long_press_lock:Failed",
            "pan_modes:Failed",
            "pan_sun:Began",
            "pinch_zoom:Failed",
            "pan_menu:Failed",
        ]
    );
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Moved, 320.0, 440.0, 48),
        ["pan_sun:Changed"]
    );
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Ended, 320.0, 440.0, 64),
        ["pan_sun:Ended"]
    );
}

/// Вертикаль вверх вне «солнца» — меню Controls; «солнце» в хит-тест не попало и молчит.
#[test]
fn upward_drag_outside_the_sun_opens_the_menu() {
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 100.0, 500.0, 0);
    let began = touch(&mut scene, 0, TouchPhase::Moved, 100.0, 486.0, 16);
    assert_eq!(
        began,
        [
            "tap_focus:Failed",
            "long_press_lock:Failed",
            "pan_modes:Failed",
            "pan_menu:Began",
            "pinch_zoom:Failed"
        ]
    );
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Ended, 100.0, 450.0, 40),
        ["pan_menu:Ended"]
    );
}

/// Вертикаль вниз вне «солнца»: меню требует «вверх», режимы — горизонталь, значит жест никому
/// не достаётся (проваливаются все).
#[test]
fn downward_drag_outside_the_sun_belongs_to_nobody() {
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 100.0, 500.0, 0);
    let moved = touch(&mut scene, 0, TouchPhase::Moved, 100.0, 514.0, 16);
    assert_eq!(
        moved,
        [
            "tap_focus:Failed",
            "long_press_lock:Failed",
            "pan_modes:Failed",
            "pan_menu:Failed"
        ]
    );
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Ended, 100.0, 514.0, 40),
        ["pinch_zoom:Failed"]
    );
}

/// Горизонталь — режимы; над «солнцем» тоже: его вертикальный пан ось не принимает.
#[test]
fn horizontal_drag_changes_modes_even_when_it_starts_on_the_sun() {
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 100.0, 500.0, 0);
    let began = touch(&mut scene, 0, TouchPhase::Moved, 86.0, 500.0, 16);
    assert_eq!(
        began,
        [
            "tap_focus:Failed",
            "long_press_lock:Failed",
            "pan_menu:Failed",
            "pan_modes:Began",
            "pinch_zoom:Failed"
        ]
    );
    touch(&mut scene, 0, TouchPhase::Ended, 86.0, 500.0, 32);

    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 320.0, 500.0, 0);
    let began = touch(&mut scene, 0, TouchPhase::Moved, 306.0, 500.0, 16);
    assert_eq!(
        began,
        [
            "pan_sun:Failed",
            "tap_focus:Failed",
            "long_press_lock:Failed",
            "pan_menu:Failed",
            "pan_modes:Began",
            "pinch_zoom:Failed"
        ]
    );
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Moved, 250.0, 500.0, 32),
        ["pan_modes:Changed"]
    );
}

/// Два пальца — щипок; однопальцевые жесты проваливаются, как только лёг второй палец.
#[test]
fn two_fingers_start_a_pinch_and_all_one_finger_gestures_fail() {
    let mut scene = viewfinder_scene();
    assert!(touch(&mut scene, 0, TouchPhase::Began, 200.0, 500.0, 0).is_empty());
    let second = touch(&mut scene, 1, TouchPhase::Began, 240.0, 500.0, 20);
    assert_eq!(
        second,
        [
            "tap_focus:Failed",
            "long_press_lock:Failed",
            "pan_modes:Failed",
            "pan_menu:Failed"
        ]
    );
    // Расстояние между пальцами выросло на 20 pt (порог 10): щипок начался.
    let pinch = touch(&mut scene, 1, TouchPhase::Moved, 260.0, 500.0, 40);
    assert_eq!(pinch, ["pinch_zoom:Began"]);
    assert_eq!(
        touch(&mut scene, 1, TouchPhase::Moved, 280.0, 500.0, 56),
        ["pinch_zoom:Changed"]
    );
}

/// Щипок, начавшийся при уже идущем одном пальце над «солнцем»: «солнце» тоже уступает.
#[test]
fn second_finger_cancels_an_active_sun_drag() {
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 320.0, 500.0, 0);
    touch(&mut scene, 0, TouchPhase::Moved, 320.0, 486.0, 16);
    let second = touch(&mut scene, 1, TouchPhase::Began, 100.0, 500.0, 32);
    assert_eq!(second, ["pan_sun:Cancelled"]);
}

/// Тап — фокус; остальные проваливаются; долгий тап не срабатывает.
#[test]
fn quick_tap_is_a_focus_tap_and_everything_else_fails() {
    let mut scene = viewfinder_scene();
    assert!(touch(&mut scene, 0, TouchPhase::Began, 100.0, 500.0, 0).is_empty());
    let lifted = touch(&mut scene, 0, TouchPhase::Ended, 100.0, 500.0, 80);
    assert_eq!(
        lifted,
        [
            "long_press_lock:Failed",
            "pinch_zoom:Failed",
            "pan_modes:Failed",
            "pan_menu:Failed",
            "tap_focus:Ended"
        ]
    );
    // Тап по «солнцу» без сдвига — тоже фокус: «солнце» проваливается, без жеста.
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 320.0, 500.0, 0);
    let lifted = touch(&mut scene, 0, TouchPhase::Ended, 320.0, 500.0, 80);
    assert_eq!(
        lifted,
        [
            "pan_sun:Failed",
            "long_press_lock:Failed",
            "pinch_zoom:Failed",
            "pan_modes:Failed",
            "pan_menu:Failed",
            "tap_focus:Ended"
        ]
    );
}

/// Удержание 0,5 с без сдвига — долгий тап (AE/AF Lock): срабатывает по таймеру кадра, а не по касанию.
#[test]
fn holding_for_half_a_second_is_a_long_press_and_the_tap_fails() {
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 100.0, 500.0, 0);
    scene.advance_frame(at_ms(400));
    assert!(
        trace(scene.drain_gesture_events()).is_empty(),
        "до 0,5 с ничего не решено"
    );
    scene.advance_frame(at_ms(520));
    assert_eq!(
        trace(scene.drain_gesture_events()),
        [
            "long_press_lock:Began",
            "tap_focus:Failed",
            "pinch_zoom:Failed",
            "pan_modes:Failed",
            "pan_menu:Failed"
        ]
    );
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Ended, 100.0, 500.0, 600),
        ["long_press_lock:Ended"]
    );
}

/// Порог щипка: изменение расстояния между пальцами 9 pt — щипка ещё нет, 11 pt — начался
/// (`behavior.md` раздел 1, порог 10 pt `[estimate]`). Касания идут в пикселях, поэтому точную
/// границу «ровно 10» проверяют тесты арены; здесь — по обе стороны от неё.
#[test]
fn pinch_starts_only_after_the_finger_distance_changes_by_about_ten_points() {
    let mut scene = viewfinder_scene();
    touch(&mut scene, 0, TouchPhase::Began, 200.0, 500.0, 0);
    touch(&mut scene, 1, TouchPhase::Began, 240.0, 500.0, 10);
    assert!(touch(&mut scene, 1, TouchPhase::Moved, 249.0, 500.0, 20).is_empty());
    assert_eq!(
        touch(&mut scene, 1, TouchPhase::Moved, 251.0, 500.0, 30),
        ["pinch_zoom:Began"]
    );
}

/// Капсула режимов: тап по подписи, горизонтальный свайп (карусель), свайп вверх (меню) — то же
/// правило направления, что у видоискателя (`behavior.md` 1.2, строка «Капсула режимов»).
#[test]
fn mode_capsule_separates_tap_horizontal_swipe_and_swipe_up() {
    let build = || {
        let mut scene: UiScene<&'static str> = empty_scene();
        let root = scene.tree().root();
        let capsule = add_rect(&mut scene, root, "capsule", Rect::from_xywh(120.0, 780.0, 200.0, 40.0));
        scene
            .add_recognizer(capsule, RecognizerSpec::Tap(Default::default()), "capsule_tap")
            .unwrap();
        scene
            .add_recognizer(capsule, pan(PanDirection::Horizontal), "capsule_carousel")
            .unwrap();
        scene
            .add_recognizer(capsule, pan(PanDirection::Up), "capsule_menu")
            .unwrap();
        scene
    };
    let mut scene = build();
    touch(&mut scene, 0, TouchPhase::Began, 220.0, 800.0, 0);
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Ended, 220.0, 800.0, 60),
        ["capsule_carousel:Failed", "capsule_menu:Failed", "capsule_tap:Ended"]
    );
    let mut scene = build();
    touch(&mut scene, 0, TouchPhase::Began, 220.0, 800.0, 0);
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Moved, 200.0, 800.0, 16),
        ["capsule_tap:Failed", "capsule_menu:Failed", "capsule_carousel:Began"]
    );
    let mut scene = build();
    touch(&mut scene, 0, TouchPhase::Began, 220.0, 800.0, 0);
    assert_eq!(
        touch(&mut scene, 0, TouchPhase::Moved, 220.0, 780.0, 16),
        ["capsule_tap:Failed", "capsule_carousel:Failed", "capsule_menu:Began"]
    );
}
