//! Хит-тест: правила UIKit на конкретных числах.

// Вспомогательные функции тестов не помечены `#[test]`, но падение на `unwrap` в них — это провал теста.
#![allow(clippy::unwrap_used)]

use std::f64::consts::FRAC_PI_4;

use ui_core::geometry::{Insets, Point, Rect, Size};
use ui_core::id::NodeId;
use ui_core::layout::{AnchorRule, AxisPlacement, Placement, ReferenceArea};
use ui_core::tree::{ElementScale, HitTestConfig, HitTestPolicy, TreeError, UiTree, Visibility};

/// Дерево 400x400 с корнем в (0,0).
fn tree() -> UiTree {
    UiTree::new(Size::new(400.0, 400.0), Insets::ZERO).unwrap()
}

/// Добавляет ребёнка с абсолютной рамкой `rect` (якоря от левого верхнего угла родителя —
/// родитель в тестах стоит в (0,0), так что координаты совпадают с абсолютными).
fn add(tree: &mut UiTree, parent: NodeId, label: &'static str, rect: Rect) -> NodeId {
    let node = tree.insert_child(parent, label).unwrap();
    tree.set_placement(
        node,
        Placement::Anchored(AnchorRule {
            area: ReferenceArea::Frame,
            horizontal: AxisPlacement::Start {
                inset: rect.origin.x,
                length: rect.size.width,
            },
            vertical: AxisPlacement::Start {
                inset: rect.origin.y,
                length: rect.size.height,
            },
        }),
    )
    .unwrap();
    node
}

fn labels(tree: &UiTree, path: &[NodeId]) -> Vec<&'static str> {
    path.iter().map(|&id| tree.label(id).unwrap()).collect()
}

#[test]
fn deepest_first_path_ends_at_root() {
    let mut tree = tree();
    let root = tree.root();
    let panel = add(&mut tree, root, "panel", Rect::from_xywh(0.0, 0.0, 200.0, 200.0));
    let button = add(&mut tree, panel, "button", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    tree.layout().unwrap();
    assert_eq!(
        labels(&tree, &tree.hit_path(Point::new(50.0, 50.0))),
        ["button", "panel", "root"]
    );
    assert_eq!(tree.hit_path(Point::new(150.0, 150.0)), vec![panel, root]);
    assert_eq!(tree.hit_path(Point::new(300.0, 300.0)), vec![root]);
    assert_eq!(tree.hit_path(Point::new(50.0, 50.0))[0], button);
    // Вне корня и нечисловая точка — пустой путь.
    assert!(tree.hit_path(Point::new(-1.0, 10.0)).is_empty());
    assert!(tree.hit_path(Point::new(f64::NAN, 10.0)).is_empty());
}

#[test]
fn topmost_sibling_wins_in_overlap() {
    let mut tree = tree();
    let root = tree.root();
    let lower = add(&mut tree, root, "lower", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    let upper = add(&mut tree, root, "upper", Rect::from_xywh(50.0, 50.0, 100.0, 100.0));
    tree.layout().unwrap();
    assert_eq!(tree.hit_path(Point::new(75.0, 75.0))[0], upper);
    assert_eq!(tree.hit_path(Point::new(25.0, 25.0))[0], lower);
    // Верхний скрыт — касание проходит к нижнему.
    tree.set_visibility(upper, Visibility::Hidden).unwrap();
    assert_eq!(tree.hit_path(Point::new(75.0, 75.0))[0], lower);
}

#[test]
fn hidden_disabled_and_transparent_do_not_catch_even_with_children() {
    let mut tree = tree();
    let root = tree.root();
    let base = add(&mut tree, root, "base", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    let cover = add(&mut tree, root, "cover", Rect::from_xywh(0.0, 0.0, 100.0, 100.0));
    let inner = add(&mut tree, cover, "inner", Rect::from_xywh(0.0, 0.0, 50.0, 50.0));
    tree.layout().unwrap();
    let probe = Point::new(10.0, 10.0);
    assert_eq!(tree.hit_path(probe)[0], inner);

    tree.set_visibility(cover, Visibility::Hidden).unwrap();
    assert_eq!(tree.hit_path(probe)[0], base, "скрытый скрывает и детей");
    tree.set_visibility(cover, Visibility::Visible).unwrap();

    tree.set_hit_test_policy(cover, HitTestPolicy::DisabledWithSubtree)
        .unwrap();
    assert_eq!(tree.hit_path(probe)[0], base, "выключенный выключает и детей");
    tree.set_hit_test_policy(cover, HitTestPolicy::Normal).unwrap();

    tree.set_opacity(cover, 0.009).unwrap();
    assert_eq!(tree.hit_path(probe)[0], base, "прозрачность < 0,01 не ловит");
    tree.set_opacity(cover, 0.01).unwrap();
    assert_eq!(tree.hit_path(probe)[0], inner, "ровно порог — ловит");
    // Порог настраивается.
    let strict = HitTestConfig {
        min_hittable_opacity: 0.5,
    };
    tree.set_opacity(cover, 0.4).unwrap();
    assert_eq!(tree.hit_path_with(probe, &strict)[0], base);
    assert_eq!(
        HitTestConfig {
            min_hittable_opacity: 2.0
        }
        .validate(),
        Err(TreeError::InvalidHitTestThreshold(2.0))
    );
}

#[test]
fn pass_through_lets_children_catch_but_not_itself() {
    let mut tree = tree();
    let root = tree.root();
    let below = add(&mut tree, root, "below", Rect::from_xywh(0.0, 0.0, 200.0, 200.0));
    let overlay = add(&mut tree, root, "overlay", Rect::from_xywh(0.0, 0.0, 200.0, 200.0));
    let button = add(&mut tree, overlay, "button", Rect::from_xywh(0.0, 0.0, 50.0, 50.0));
    tree.set_hit_test_policy(overlay, HitTestPolicy::PassThrough).unwrap();
    tree.layout().unwrap();
    // Мимо кнопки — касание проваливается под оверлей.
    assert_eq!(tree.hit_path(Point::new(100.0, 100.0)), vec![below, root]);
    // На кнопке — путь идёт через оверлей-предка.
    assert_eq!(tree.hit_path(Point::new(10.0, 10.0)), vec![button, overlay, root]);
}

#[test]
fn rotated_and_scaled_element_uses_visual_shape() {
    let mut tree = tree();
    let root = tree.root();
    // Квадрат 100x100 с центром (200, 200); поворот 45° делает его ромбом с «радиусом» 70,7.
    let square = add(&mut tree, root, "square", Rect::from_xywh(150.0, 150.0, 100.0, 100.0));
    tree.layout().unwrap();
    tree.set_rotation_radians(square, FRAC_PI_4).unwrap();
    // Точка (152, 152) лежит у угла исходной рамки (в 67,9 pt от центра по диагонали). Ромб после
    // поворота вытянут вдоль осей x/y (вершины в 70,7 pt), по диагоналям он узкий (50 pt) —
    // поэтому эта точка теперь мимо.
    let original_corner_zone = Point::new(200.0 - 48.0, 200.0 - 48.0);
    assert!(
        tree.hit_path(original_corner_zone).iter().all(|&id| id != square),
        "угол исходной рамки — промах"
    );
    // Точка на оси x в 65 pt от центра: вне исходной рамки (50), но внутри ромба (70,7).
    let on_axis = Point::new(200.0 + 65.0, 200.0);
    assert_eq!(tree.hit_path(on_axis)[0], square, "вершина ромба — попадание");
    // Масштаб 2x на повёрнутом: вершина уходит на 141 pt по оси.
    tree.set_scale(square, ElementScale::uniform(2.0)).unwrap();
    assert_eq!(tree.hit_path(Point::new(200.0 + 135.0, 200.0))[0], square);
    assert!(
        tree.hit_path(Point::new(200.0 + 145.0, 200.0))
            .iter()
            .all(|&id| id != square)
    );
}

#[test]
fn ancestor_scale_affects_child_hit_area() {
    let mut tree = tree();
    let root = tree.root();
    let parent = add(&mut tree, root, "parent", Rect::from_xywh(100.0, 100.0, 100.0, 100.0));
    let child = add(&mut tree, parent, "child", Rect::from_xywh(0.0, 0.0, 20.0, 20.0));
    tree.layout().unwrap();
    // Родитель (100..200)^2, центр (150, 150); ребёнок в его углу: абсолютно (100..120)^2.
    // Без масштаба точка (95, 95) левее и выше родителя — попадает только в корень.
    let probe = Point::new(95.0, 95.0);
    assert_eq!(tree.hit_path(probe), vec![root]);
    // Масштаб родителя x2 растёт от его центра: экранная точка p соответствует локальной
    // 150 + (p - 150) / 2.
    tree.set_scale(parent, ElementScale::uniform(2.0)).unwrap();
    // Экранная (55, 55) -> локальная 102,5 — внутри ребёнка (100..120).
    assert_eq!(tree.hit_path(Point::new(55.0, 55.0)), vec![child, parent, root]);
    // Экранная (95, 95) -> локальная 122,5 — вне ребёнка (> 120), но внутри родителя.
    assert_eq!(tree.hit_path(probe), vec![parent, root]);
}

#[test]
fn touch_outset_enlarges_small_icon() {
    let mut tree = tree();
    let root = tree.root();
    let icon = add(&mut tree, root, "icon", Rect::from_xywh(100.0, 100.0, 20.0, 20.0));
    tree.layout().unwrap();
    let near_miss = Point::new(95.0, 110.0);
    assert_eq!(tree.hit_path(near_miss), vec![root]);
    tree.set_touch_outset(icon, Insets::uniform(12.0)).unwrap();
    assert_eq!(tree.hit_path(near_miss)[0], icon);
    // Расширение не бесконечно, и рамка (вёрстка) не изменилась.
    assert_eq!(tree.hit_path(Point::new(85.0, 110.0)), vec![root]);
    assert_eq!(tree.frame(icon).unwrap(), Rect::from_xywh(100.0, 100.0, 20.0, 20.0));
}

#[test]
fn children_outside_parent_are_not_found_unless_parent_outset_covers() {
    let mut tree = tree();
    let root = tree.root();
    let parent = add(&mut tree, root, "parent", Rect::from_xywh(100.0, 100.0, 50.0, 50.0));
    // Ребёнок торчит за правую границу родителя (x 140..190).
    let child = add(&mut tree, parent, "child", Rect::from_xywh(40.0, 0.0, 50.0, 50.0));
    tree.layout().unwrap();
    // Ребёнок в абсолютных координатах: (140,100)-(190,150); точка (170,120) за границей родителя.
    let outside_parent = Point::new(170.0, 120.0);
    assert_eq!(tree.hit_path(outside_parent), vec![root]);
    // Расширили область родителя вправо — ребёнок достижим.
    tree.set_touch_outset(parent, Insets::new(0.0, 0.0, 0.0, 40.0)).unwrap();
    assert_eq!(tree.hit_path(outside_parent), vec![child, parent, root]);
}

#[test]
fn visual_bounds_follow_transform_and_leave_frame_untouched() {
    let mut tree = tree();
    let root = tree.root();
    let node = add(&mut tree, root, "node", Rect::from_xywh(150.0, 150.0, 100.0, 100.0));
    tree.layout().unwrap();
    tree.set_scale(node, ElementScale::uniform(2.0)).unwrap();
    assert_eq!(
        tree.visual_bounds(node).unwrap(),
        Rect::from_xywh(100.0, 100.0, 200.0, 200.0)
    );
    assert_eq!(tree.frame(node).unwrap(), Rect::from_xywh(150.0, 150.0, 100.0, 100.0));
}
