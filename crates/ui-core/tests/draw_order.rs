#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Обход в порядке рисования: порядок, скрытые, прозрачность и согласие преобразования с
//! хит-тестом и `visual_bounds`.

mod common;

use common::anchored_rect;
use std::f64::consts::FRAC_PI_6;
use ui_core::geometry::{Insets, Point, Rect, Size, Vector};
use ui_core::id::NodeId;
use ui_core::tree::{ElementScale, UiTree, Visibility};

fn add(tree: &mut UiTree, parent: NodeId, label: &'static str, frame: Rect) -> NodeId {
    let node = tree.insert_child(parent, label).unwrap();
    tree.set_placement(node, anchored_rect(frame)).unwrap();
    node
}

fn labels(tree: &UiTree) -> Vec<&'static str> {
    tree.draw_order().iter().map(|visual| visual.label).collect()
}

fn assert_close(actual: Point, expected: Point) {
    assert!(
        (actual.x - expected.x).abs() < 1e-9 && (actual.y - expected.y).abs() < 1e-9,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn order_is_parent_first_children_in_order_last_on_top() {
    let mut tree = UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let first = add(&mut tree, root, "first", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    add(&mut tree, first, "first.a", Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
    add(&mut tree, first, "first.b", Rect::from_xywh(20.0, 0.0, 10.0, 10.0));
    add(&mut tree, root, "second", Rect::from_xywh(200.0, 0.0, 100.0, 100.0));
    tree.layout().unwrap();
    assert_eq!(labels(&tree), vec!["root", "first", "first.a", "first.b", "second"]);
    let depths: Vec<usize> = tree.draw_order().iter().map(|visual| visual.depth).collect();
    assert_eq!(depths, vec![0, 1, 2, 2, 1]);
}

#[test]
fn hidden_subtree_is_skipped_but_keeps_its_place_in_layout() {
    let mut tree = UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let hidden = add(&mut tree, root, "hidden", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    add(&mut tree, hidden, "hidden.child", Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
    add(&mut tree, root, "shown", Rect::from_xywh(200.0, 0.0, 100.0, 100.0));
    tree.layout().unwrap();
    tree.set_visibility(hidden, Visibility::Hidden).unwrap();
    assert_eq!(labels(&tree), vec!["root", "shown"]);
    // Вёрстка не тронута: рамка скрытого на месте.
    assert_eq!(tree.frame(hidden).unwrap(), Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    tree.set_visibility(hidden, Visibility::Visible).unwrap();
    assert_eq!(labels(&tree), vec!["root", "hidden", "hidden.child", "shown"]);
}

#[test]
fn opacity_is_multiplied_down_the_chain() {
    let mut tree = UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let panel = add(&mut tree, root, "panel", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    let child = add(&mut tree, panel, "child", Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
    tree.layout().unwrap();
    tree.set_opacity(panel, 0.5).unwrap();
    tree.set_opacity(child, 0.4).unwrap();
    let order = tree.draw_order();
    let opacity_of = |node: NodeId| order.iter().find(|v| v.node == node).unwrap().opacity;
    assert_eq!(opacity_of(root), 1.0);
    assert_eq!(opacity_of(panel), 0.5);
    assert!((opacity_of(child) - 0.2).abs() < 1e-12);
}

/// Предок и ребёнок повёрнуты, масштабированы и сдвинуты: `to_screen` ребёнка согласован
/// с `visual_bounds` и с хит-тестом.
#[test]
fn composed_transform_agrees_with_visual_bounds_and_hit_test() {
    let mut tree = UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let panel = add(&mut tree, root, "panel", Rect::from_xywh(50.0, 60.0, 200.0, 120.0));
    let child = add(&mut tree, panel, "child", Rect::from_xywh(80.0, 40.0, 40.0, 30.0));
    tree.layout().unwrap();
    tree.set_rotation_radians(panel, FRAC_PI_6).unwrap();
    tree.set_scale(panel, ElementScale { x: 1.2, y: 0.8 }).unwrap();
    tree.set_translation(panel, Vector::new(15.0, -7.0)).unwrap();
    tree.set_rotation_radians(child, -0.4).unwrap();
    tree.set_scale(child, ElementScale::uniform(1.5)).unwrap();
    tree.set_translation(child, Vector::new(-3.0, 9.0)).unwrap();

    let order = tree.draw_order();
    let visual = order.iter().find(|v| v.node == child).unwrap();
    assert_eq!(visual.frame, tree.frame(child).unwrap());
    assert_eq!(visual.to_screen, tree.to_screen_transform(child).unwrap());

    // 1. Образы углов рамки дают ровно ту же коробку, что `visual_bounds`.
    let frame = visual.frame;
    let corners = [
        Point::new(frame.min_x(), frame.min_y()),
        Point::new(frame.max_x(), frame.min_y()),
        Point::new(frame.max_x(), frame.max_y()),
        Point::new(frame.min_x(), frame.max_y()),
    ]
    .map(|corner| visual.to_screen.apply(corner));
    let bounds = tree.visual_bounds(child).unwrap();
    let min_x = corners.iter().map(|c| c.x).fold(f64::INFINITY, f64::min);
    let max_y = corners.iter().map(|c| c.y).fold(f64::NEG_INFINITY, f64::max);
    assert!((min_x - bounds.min_x()).abs() < 1e-9 && (max_y - bounds.max_y()).abs() < 1e-9);

    // 2. Точки внутри рамки ребёнка → на экране попадают в хит-тест ребёнка, обратный
    //    `to_screen` возвращает их в рамку.
    let inverse = visual.to_screen.inverse().unwrap();
    for (fx, fy) in [(0.1, 0.1), (0.5, 0.5), (0.9, 0.2), (0.3, 0.8)] {
        let local = Point::new(
            frame.min_x() + frame.size.width * fx,
            frame.min_y() + frame.size.height * fy,
        );
        let on_screen = visual.to_screen.apply(local);
        assert_eq!(tree.hit_path(on_screen)[0], child, "точка {fx},{fy}");
        assert_close(inverse.apply(on_screen), local);
        assert!(frame.contains(inverse.apply(on_screen)));
    }

    // 3. Точка чуть за рамкой (на экране) хит-тест ребёнку не отдаёт, и обратное
    //    преобразование выводит её за рамку.
    let outside_local = Point::new(frame.max_x() + 5.0, frame.max_y() + 5.0);
    let outside_screen = visual.to_screen.apply(outside_local);
    assert_ne!(tree.hit_path(outside_screen).first(), Some(&child));
    assert!(!frame.contains(inverse.apply(outside_screen)));
}

#[test]
fn identity_chain_gives_identity_transform() {
    let mut tree = UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let child = add(&mut tree, root, "child", Rect::from_xywh(10.0, 20.0, 30.0, 40.0));
    tree.layout().unwrap();
    let order = tree.draw_order();
    let visual = order.iter().find(|v| v.node == child).unwrap();
    assert_eq!(visual.to_screen.apply(Point::new(10.0, 20.0)), Point::new(10.0, 20.0));
    assert_eq!(visual.frame, Rect::from_xywh(10.0, 20.0, 30.0, 40.0));
}
