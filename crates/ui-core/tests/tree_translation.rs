#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Визуальный сдвиг: хит-тест, `visual_bounds`, `frame` не меняется, валидация.

mod common;

use common::anchored_rect;
use std::f64::consts::FRAC_PI_2;
use ui_core::geometry::{Insets, Point, Rect, Size, Vector};
use ui_core::tree::{TreeError, UiTree};

fn tree_with_panel_and_child() -> (UiTree, ui_core::id::NodeId, ui_core::id::NodeId) {
    let mut tree = UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let panel = tree.insert_child(root, "panel").unwrap();
    tree.set_placement(panel, anchored_rect(Rect::from_xywh(0.0, 0.0, 100.0, 100.0)))
        .unwrap();
    let child = tree.insert_child(panel, "child").unwrap();
    tree.set_placement(child, anchored_rect(Rect::from_xywh(10.0, 10.0, 20.0, 20.0)))
        .unwrap();
    tree.layout().unwrap();
    (tree, panel, child)
}

#[test]
fn translation_moves_hit_area_but_not_frame() {
    let (mut tree, panel, child) = tree_with_panel_and_child();
    let frame_before = tree.frame(panel).unwrap();
    tree.set_translation(panel, Vector::new(200.0, 50.0)).unwrap();
    assert_eq!(tree.translation(panel).unwrap(), Vector::new(200.0, 50.0));
    assert_eq!(tree.frame(panel).unwrap(), frame_before, "вёрстка не затронута");
    // Старое место — промах (остался корень), новое — попадание.
    assert_eq!(tree.hit_path(Point::new(50.0, 50.0)), vec![tree.root()]);
    assert_eq!(tree.hit_path(Point::new(250.0, 100.0)), vec![panel, tree.root()]);
    // Ребёнок едет вместе с родителем: его рамка (10..30) + сдвиг (200, 50) = (210..230, 60..80).
    assert_eq!(tree.hit_path(Point::new(220.0, 70.0))[0], child);
    assert_eq!(tree.hit_path(Point::new(20.0, 20.0)), vec![tree.root()]);
    let bounds = tree.visual_bounds(child).unwrap();
    assert_eq!(bounds, Rect::from_xywh(210.0, 60.0, 20.0, 20.0));
}

/// Порядок композиции `p' = c + T + R·S·(p − c)`: поворот на 90° вокруг центра (50,50), затем сдвиг.
#[test]
fn translation_composes_after_rotation_and_scale() {
    let (mut tree, panel, _) = tree_with_panel_and_child();
    tree.set_rotation_radians(panel, FRAC_PI_2).unwrap();
    tree.set_translation(panel, Vector::new(200.0, 0.0)).unwrap();
    // Квадрат 100x100 при повороте на 90° занимает то же место; сдвиг уводит его в x 200..300.
    let bounds = tree.visual_bounds(panel).unwrap();
    assert!(
        (bounds.min_x() - 200.0).abs() < 1e-9 && (bounds.max_x() - 300.0).abs() < 1e-9,
        "{bounds:?}"
    );
    // Вытянутый элемент: ширина 100 -> после масштаба x2 и поворота 90° он вытянут по вертикали,
    // а центр уехал ровно на сдвиг, не на повёрнутый сдвиг.
    tree.set_scale(panel, ui_core::tree::ElementScale { x: 2.0, y: 1.0 })
        .unwrap();
    let bounds = tree.visual_bounds(panel).unwrap();
    assert!(
        (bounds.center().x - 250.0).abs() < 1e-9 && (bounds.center().y - 50.0).abs() < 1e-9,
        "{bounds:?}"
    );
    assert!((bounds.size.height - 200.0).abs() < 1e-9 && (bounds.size.width - 100.0).abs() < 1e-9);
    // Хит-тест согласован: центр и точка, куда уехал вытянутый край.
    assert_eq!(tree.hit_path(Point::new(250.0, 50.0))[0], panel);
    assert_eq!(tree.hit_path(Point::new(250.0, 140.0))[0], panel);
    assert_eq!(
        tree.hit_path(Point::new(150.0, 50.0)),
        vec![tree.root()],
        "левее сдвинутой формы — промах"
    );
}

#[test]
fn invalid_translation_is_rejected_and_keeps_old_value() {
    let (mut tree, panel, _) = tree_with_panel_and_child();
    tree.set_translation(panel, Vector::new(5.0, 6.0)).unwrap();
    for bad in [Vector::new(f64::NAN, 0.0), Vector::new(0.0, f64::INFINITY)] {
        assert!(matches!(
            tree.set_translation(panel, bad),
            Err(TreeError::InvalidTranslation { .. })
        ));
    }
    assert_eq!(tree.translation(panel).unwrap(), Vector::new(5.0, 6.0));
    tree.remove_subtree(panel).unwrap();
    assert_eq!(tree.translation(panel), Err(TreeError::UnknownNode(panel)));
}
