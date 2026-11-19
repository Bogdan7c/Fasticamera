//! Жизненный цикл дерева: вставка, удаление поддерева, поколения, свойства.

// Вспомогательные функции тестов не помечены `#[test]`, но падение на `unwrap` в них — это провал теста.
#![allow(clippy::unwrap_used)]

use ui_core::geometry::{Insets, Size};
use ui_core::tree::{ElementScale, HitTestPolicy, TreeError, UiTree, Visibility};

fn screen_tree() -> UiTree {
    UiTree::new(Size::new(440.0, 956.0), Insets::new(62.0, 0.0, 34.0, 0.0)).unwrap()
}

#[test]
fn children_keep_draw_order_and_insert_at_index() {
    let mut tree = screen_tree();
    let root = tree.root();
    let first = tree.insert_child(root, "first").unwrap();
    let third = tree.insert_child(root, "third").unwrap();
    let second = tree.insert_child_at(root, 1, "second").unwrap();
    let bottom = tree.insert_child_at(root, 0, "bottom").unwrap();
    assert_eq!(tree.children(root).unwrap(), &[bottom, first, second, third]);
    assert_eq!(tree.parent(second).unwrap(), Some(root));
    assert_eq!(tree.parent(root).unwrap(), None);
    assert_eq!(tree.label(second).unwrap(), "second");
    // Граница: индекс == числу детей — это конец, больше — ошибка без изменений.
    let last = tree.insert_child_at(root, 4, "last").unwrap();
    assert_eq!(tree.children(root).unwrap().last(), Some(&last));
    let error = tree.insert_child_at(root, 9, "bad").unwrap_err();
    assert!(matches!(
        error,
        TreeError::ChildIndexOutOfRange {
            index: 9,
            child_count: 5,
            ..
        }
    ));
    assert_eq!(tree.children(root).unwrap().len(), 5);
}

#[test]
fn remove_subtree_returns_every_removed_id() {
    let mut tree = screen_tree();
    let root = tree.root();
    let panel = tree.insert_child(root, "panel").unwrap();
    let icon = tree.insert_child(panel, "icon").unwrap();
    let badge = tree.insert_child(icon, "badge").unwrap();
    let label = tree.insert_child(panel, "label").unwrap();
    let sibling = tree.insert_child(root, "sibling").unwrap();

    let mut removed = tree.remove_subtree(panel).unwrap();
    assert_eq!(removed[0], panel, "сначала сам элемент");
    removed.sort();
    let mut expected = vec![panel, icon, badge, label];
    expected.sort();
    assert_eq!(removed, expected);
    for id in expected {
        assert!(!tree.contains(id));
        assert_eq!(tree.frame(id).unwrap_err(), TreeError::UnknownNode(id));
    }
    assert!(tree.contains(sibling));
    assert_eq!(tree.children(root).unwrap(), &[sibling]);
    assert_eq!(tree.len(), 2);
}

#[test]
fn stale_id_is_error_after_slot_reuse_and_new_id_works() {
    let mut tree = screen_tree();
    let root = tree.root();
    let old = tree.insert_child(root, "old").unwrap();
    tree.remove_subtree(old).unwrap();
    let reborn = tree.insert_child(root, "reborn").unwrap();
    assert_ne!(old, reborn, "поколение обязано отличаться");
    assert_eq!(tree.label(reborn).unwrap(), "reborn");
    // Старый id не должен ни читать, ни менять новый элемент.
    assert_eq!(tree.label(old).unwrap_err(), TreeError::UnknownNode(old));
    assert_eq!(tree.set_opacity(old, 0.2).unwrap_err(), TreeError::UnknownNode(old));
    assert_eq!(tree.opacity(reborn).unwrap(), 1.0);
    assert_eq!(tree.remove_subtree(old).unwrap_err(), TreeError::UnknownNode(old));
    assert!(tree.contains(reborn));
    assert!(matches!(tree.insert_child(old, "x"), Err(TreeError::UnknownNode(_))));
}

#[test]
fn root_cannot_be_removed() {
    let mut tree = screen_tree();
    let root = tree.root();
    assert_eq!(tree.remove_subtree(root).unwrap_err(), TreeError::CannotRemoveRoot);
    assert!(tree.contains(root));
}

#[test]
fn invalid_screen_is_rejected() {
    assert_eq!(
        UiTree::new(Size::new(-1.0, 10.0), Insets::ZERO).unwrap_err(),
        TreeError::InvalidScreen
    );
    assert_eq!(
        UiTree::new(Size::new(f64::NAN, 10.0), Insets::ZERO).unwrap_err(),
        TreeError::InvalidScreen
    );
}

#[test]
fn invalid_property_values_are_errors_and_keep_old_value() {
    let mut tree = screen_tree();
    let node = tree.insert_child(tree.root(), "node").unwrap();
    for bad in [-0.1, 1.1, f64::NAN] {
        assert!(matches!(tree.set_opacity(node, bad), Err(TreeError::InvalidOpacity(_))));
    }
    assert_eq!(tree.opacity(node).unwrap(), 1.0);
    assert!(matches!(
        tree.set_scale(node, ElementScale { x: 0.0, y: 1.0 }),
        Err(TreeError::InvalidScale { .. })
    ));
    assert!(matches!(
        tree.set_scale(node, ElementScale::uniform(f64::INFINITY)),
        Err(TreeError::InvalidScale { .. })
    ));
    assert_eq!(tree.scale(node).unwrap(), ElementScale::IDENTITY);
    assert!(matches!(
        tree.set_rotation_radians(node, f64::NAN),
        Err(TreeError::InvalidRotation(_))
    ));
    assert_eq!(tree.rotation_radians(node).unwrap(), 0.0);
    let bad_outset = Insets::new(f64::NAN, 0.0, 0.0, 0.0);
    assert!(matches!(
        tree.set_touch_outset(node, bad_outset),
        Err(TreeError::InvalidTouchOutset(_))
    ));
    assert_eq!(tree.touch_outset(node).unwrap(), Insets::ZERO);
}

#[test]
fn property_change_does_not_touch_neighbours() {
    let mut tree = screen_tree();
    let root = tree.root();
    let target = tree.insert_child(root, "target").unwrap();
    let neighbour = tree.insert_child(root, "neighbour").unwrap();
    let child = tree.insert_child(target, "child").unwrap();

    tree.set_opacity(target, 0.5).unwrap();
    tree.set_scale(target, ElementScale { x: 2.0, y: 3.0 }).unwrap();
    tree.set_rotation_radians(target, 1.0).unwrap();
    tree.set_visibility(target, Visibility::Hidden).unwrap();
    tree.set_hit_test_policy(target, HitTestPolicy::PassThrough).unwrap();
    tree.set_touch_outset(target, Insets::uniform(5.0)).unwrap();

    assert_eq!(tree.opacity(target).unwrap(), 0.5);
    assert_eq!(tree.scale(target).unwrap(), ElementScale { x: 2.0, y: 3.0 });
    assert_eq!(tree.rotation_radians(target).unwrap(), 1.0);
    assert_eq!(tree.visibility(target).unwrap(), Visibility::Hidden);
    assert_eq!(tree.hit_test_policy(target).unwrap(), HitTestPolicy::PassThrough);
    assert_eq!(tree.touch_outset(target).unwrap(), Insets::uniform(5.0));
    for other in [neighbour, child, root] {
        assert_eq!(tree.opacity(other).unwrap(), 1.0);
        assert_eq!(tree.scale(other).unwrap(), ElementScale::IDENTITY);
        assert_eq!(tree.rotation_radians(other).unwrap(), 0.0);
        assert_eq!(tree.visibility(other).unwrap(), Visibility::Visible);
        assert_eq!(tree.hit_test_policy(other).unwrap(), HitTestPolicy::Normal);
        assert_eq!(tree.touch_outset(other).unwrap(), Insets::ZERO);
    }
}
