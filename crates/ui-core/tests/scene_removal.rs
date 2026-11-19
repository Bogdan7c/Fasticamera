#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Удаление элемента через сцену: жесты отменяются, анимации снимаются, соседи живут.

mod common;

use common::*;
use shell_contract::touch::TouchPhase;
use ui_core::animator::{AnimatedProperty, AnimationRequest, PropertyKey, StartVelocity};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureState, PanSpec, RecognizerSpec, TapSpec};
use ui_core::spring::Spring;

/// Элемент удалён посреди пана: его жест получает `Cancelled` со своим намерением и последними
/// данными, его анимации сняты, анимация соседа продолжается, дальнейшие касания его не будят.
#[test]
fn removing_element_mid_pan_cancels_gesture_and_drops_its_animations() {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let panel = add_rect(&mut scene, root, "panel", Rect::from_xywh(0.0, 400.0, 440.0, 300.0));
    let panel_button = add_rect(
        &mut scene,
        panel,
        "panel_button",
        Rect::from_xywh(10.0, 10.0, 80.0, 80.0),
    );
    let neighbor = add_rect(&mut scene, root, "neighbor", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    scene
        .add_recognizer(panel, RecognizerSpec::Pan(PanSpec::default()), "panel_pan")
        .unwrap();
    scene
        .add_recognizer(
            panel_button,
            RecognizerSpec::Tap(TapSpec::default()),
            "panel_button_tap",
        )
        .unwrap();
    scene
        .add_recognizer(neighbor, RecognizerSpec::Tap(TapSpec::default()), "neighbor_tap")
        .unwrap();

    let panel_opacity = PropertyKey {
        node: panel,
        property: AnimatedProperty::Opacity,
    };
    let button_rotation = PropertyKey {
        node: panel_button,
        property: AnimatedProperty::Rotation,
    };
    let neighbor_opacity = PropertyKey {
        node: neighbor,
        property: AnimatedProperty::Opacity,
    };
    for (key, target) in [(panel_opacity, 0.2), (button_rotation, 1.0), (neighbor_opacity, 0.5)] {
        scene
            .animate(AnimationRequest {
                key,
                target,
                spring: Spring::smooth(),
                velocity: StartVelocity::Zero,
                now: at_ms(0),
            })
            .unwrap();
    }
    scene.advance_frame(at_ms(16));
    let neighbor_before = scene.tree().opacity(neighbor).unwrap();
    assert!(neighbor_before < 1.0, "контроль: сосед уже едет");

    // Пан по панели (мимо её кнопки) идёт.
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Began, 300.0, 600.0, 20));
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Moved, 330.0, 600.0, 36));
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Moved, 360.0, 600.0, 52));
    let running = scene.drain_gesture_events();
    assert!(
        running
            .iter()
            .any(|e| e.intent == "panel_pan" && e.state == GestureState::Began)
    );
    assert!(
        running
            .iter()
            .any(|e| e.intent == "panel_pan" && e.state == GestureState::Changed)
    );

    let removal = scene.remove_subtree(panel).unwrap();
    assert_eq!(removal.removed, vec![panel, panel_button]);
    assert_eq!(removal.recognizers_removed, 2);
    assert_eq!(removal.animations_removed, 2);

    let events = scene.drain_gesture_events();
    let cancelled: Vec<_> = events.iter().filter(|e| e.state == GestureState::Cancelled).collect();
    assert_eq!(cancelled.len(), 1, "{events:?}");
    assert_eq!(cancelled[0].intent, "panel_pan");
    assert_eq!(cancelled[0].node, panel);
    assert!(
        matches!(cancelled[0].data, GestureData::Pan { .. }),
        "Cancelled несёт последние данные пана"
    );

    assert!(!scene.animator().is_animating(panel_opacity));
    assert!(!scene.animator().is_animating(button_rotation));
    assert!(scene.animator().is_animating(neighbor_opacity));
    assert!(!scene.tree().contains(panel));

    // Дальше: палец ещё на экране — движения и отрыв не рождают событий удалённого элемента.
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Moved, 400.0, 600.0, 68));
    scene.handle_touch(&touch_at_pt(0, TouchPhase::Ended, 400.0, 600.0, 84));
    assert!(scene.drain_gesture_events().is_empty());

    // Сосед продолжил ехать, и ничьё чужое значение не записано.
    let report = scene.advance_frame(at_ms(100)).animations;
    assert_eq!(report.applied, 1);
    assert_eq!(
        report.dropped_dead_nodes, 0,
        "сцена сама сняла анимации, мёртвых не осталось"
    );
    assert!(scene.tree().opacity(neighbor).unwrap() < neighbor_before);

    // Тап по соседу работает: арена не сломана удалением.
    scene.handle_touch(&touch_at_pt(1, TouchPhase::Began, 50.0, 50.0, 120));
    scene.handle_touch(&touch_at_pt(1, TouchPhase::Ended, 50.0, 50.0, 150));
    assert!(
        scene
            .drain_gesture_events()
            .iter()
            .any(|e| e.intent == "neighbor_tap" && e.state == GestureState::Ended)
    );
}

/// Удаление элемента без жестов и анимаций ничего лишнего не трогает и честно сообщает нули.
#[test]
fn removing_plain_element_reports_zero_side_effects() {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let plain = add_rect(&mut scene, root, "plain", Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
    let other = add_rect(&mut scene, root, "other", Rect::from_xywh(20.0, 0.0, 10.0, 10.0));
    scene
        .add_recognizer(other, RecognizerSpec::Tap(TapSpec::default()), "other_tap")
        .unwrap();
    let removal = scene.remove_subtree(plain).unwrap();
    assert_eq!(removal.removed, vec![plain]);
    assert_eq!(removal.recognizers_removed, 0);
    assert_eq!(removal.animations_removed, 0);
    assert!(scene.drain_gesture_events().is_empty());
    // Повторное удаление того же идентификатора — ошибка, а не тихий успех.
    assert!(scene.remove_subtree(plain).is_err());
    assert!(scene.tree().contains(other));
}

/// `set_property` снимает анимацию свойства: следующий кадр не перезаписывает ручное значение.
#[test]
fn set_property_cancels_animation_of_that_property_only() {
    let mut scene = empty_scene();
    let root = scene.tree().root();
    let node = add_rect(&mut scene, root, "node", Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
    let opacity = PropertyKey {
        node,
        property: AnimatedProperty::Opacity,
    };
    let rotation = PropertyKey {
        node,
        property: AnimatedProperty::Rotation,
    };
    for (key, target) in [(opacity, 0.0), (rotation, 2.0)] {
        scene
            .animate(AnimationRequest {
                key,
                target,
                spring: Spring::smooth(),
                velocity: StartVelocity::Zero,
                now: at_ms(0),
            })
            .unwrap();
    }
    scene.set_property(node, AnimatedProperty::Opacity, 0.75).unwrap();
    scene.advance_frame(at_ms(100));
    assert_eq!(scene.tree().opacity(node).unwrap(), 0.75);
    assert!(
        scene.tree().rotation_radians(node).unwrap() > 0.0,
        "поворот продолжает ехать"
    );
}
