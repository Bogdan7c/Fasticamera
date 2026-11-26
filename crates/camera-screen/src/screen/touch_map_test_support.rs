//! Карта областей касания живых элементов сцены — опора тестов соседства (пункт 13 сессии 14n).
//!
//! «Живой» элемент — узел с распознавателем жестов, видимый и не выключенный вместе с поддеревом. Для каждого
//! считаются две области в pt экрана:
//! * **заявленная** — рамка узла с расширением зоны касания (то, что виджет попросил);
//! * **действующая** — заявленная, урезанная предками по тем же правилам, что хит-тест `ui-core`: обычный узел
//!   режет край родителя, узел с `TouchReach::BeyondParent` — нет, но его родителя по-прежнему режет дед.
//!
//! Растровая проверка спрашивает настоящий хит-тест сцены в точках сетки внутри действующей области: так тест
//! не пересказывает формулы раскладки, а проверяет то, что поймает палец.

use std::collections::HashMap;

use ui_core::geometry::{Point, Rect};
use ui_core::id::NodeId;
use ui_core::scene::UiScene;
use ui_core::tree::{HitTestPolicy, NodeVisual, TouchReach, UiTree};

use crate::widget::intent::ScreenGesture;

/// Шаг сетки проб, pt: мельче самой узкой цели раздела (точки и «↻» — 24–44 pt).
const PROBE_STEP_PT: f64 = 1.5;
/// Отступ проб от края области, pt: точка ровно на границе принадлежит соседу по правилу «полуоткрытый край».
const PROBE_EDGE_INSET_PT: f64 = 0.25;
/// Допуск сравнения рамок, pt.
const RECT_TOLERANCE_PT: f64 = 1e-6;

/// Живой элемент и его области на экране.
#[derive(Debug, Clone)]
pub(crate) struct LiveArea {
    pub node: NodeId,
    pub label: &'static str,
    /// Рамка с расширением зоны касания, pt экрана.
    pub declared: Rect,
    /// То, что реально достаётся касанию после урезания предками (`None` — урезано целиком).
    pub effective: Option<Rect>,
    /// Узлу расширили зону касания (маленькая кнопка): её расширение обязано быть живым целиком.
    pub expanded: bool,
    /// Рамка узла (без расширения) видна целиком — не уехала за окно прокрутки и не срезана краем предка.
    /// Расширение срезанной так кнопки законно урезано вместе с ней.
    pub frame_fully_live: bool,
    /// `PassThrough`: сам касание не ловит, только как предок пойманного ребёнка.
    pub pass_through: bool,
}

/// Нарушение соседства: в точке `at` области `area` касание поймал не этот элемент и не его потомок.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TouchConflict {
    pub area: &'static str,
    pub at: Point,
    /// Кто поймал (`None` — никто из живых: мёртвое место).
    pub caught_by: Option<&'static str>,
}

/// Карта живых областей сцены.
pub(crate) struct TouchMap<'scene> {
    tree: &'scene UiTree,
    live: Vec<LiveArea>,
}

impl<'scene> TouchMap<'scene> {
    /// Снимает карту живых элементов поддерева первого видимого узла с подписью `subtree_label` (например,
    /// корня раздела калибровки: видоискатель PHOTO под ним жестов раздела не получает — их гасит маршрутизация
    /// экрана, — поэтому в соседство раздела не входит).
    pub(crate) fn of_subtree(scene: &'scene UiScene<ScreenGesture>, subtree_label: &str) -> Self {
        let tree = scene.tree();
        let visuals = visuals_of(tree);
        let subtree_root = tree
            .draw_order()
            .into_iter()
            .find(|visual| visual.label == subtree_label)
            .map(|visual| visual.node);
        let live = scene
            .nodes_with_recognizers()
            .into_iter()
            .filter(|node| subtree_root.is_some_and(|root| *node == root || is_descendant_in(tree, *node, root)))
            .filter_map(|node| live_area(tree, &visuals, node))
            .collect();
        Self { tree, live }
    }

    /// Снимает карту с текущей вёрстки сцены.
    #[allow(
        dead_code,
        reason = "карта всей сцены — для будущих проверок PHOTO; раздел берёт поддерево"
    )]
    pub(crate) fn of(scene: &'scene UiScene<ScreenGesture>) -> Self {
        let tree = scene.tree();
        let visuals = visuals_of(tree);
        let live = scene
            .nodes_with_recognizers()
            .into_iter()
            .filter_map(|node| live_area(tree, &visuals, node))
            .collect();
        Self { tree, live }
    }

    /// Живые элементы (в порядке добавления распознавателей).
    pub(crate) fn live(&self) -> &[LiveArea] {
        &self.live
    }

    /// Расширенные зоны, урезанные предками: «мёртвые края» маленьких кнопок.
    pub(crate) fn cut_expansions(&self) -> Vec<&LiveArea> {
        self.live
            .iter()
            .filter(|area| area.expanded && area.frame_fully_live)
            .filter(|area| {
                !area
                    .effective
                    .is_some_and(|effective| same_rect(effective, area.declared))
            })
            .collect()
    }

    /// Точки, где действующая область элемента достаётся чужому элементу (не потомку) или никому.
    pub(crate) fn conflicts(&self) -> Vec<TouchConflict> {
        let mut conflicts = Vec::new();
        for area in self.live.iter().filter(|area| !area.pass_through) {
            let Some(effective) = area.effective else {
                continue;
            };
            for point in probe_points(effective) {
                let owner = self.owner_at(point);
                let owned = owner.is_some_and(|owner| owner == area.node || self.is_descendant(owner, area.node));
                if !owned {
                    conflicts.push(TouchConflict {
                        area: area.label,
                        at: point,
                        caught_by: owner.and_then(|node| self.tree.label(node).ok()),
                    });
                }
            }
        }
        conflicts
    }

    /// Самый глубокий живой элемент на пути хит-теста — тот, чей распознаватель первым получит касание.
    pub(crate) fn owner_at(&self, point: Point) -> Option<NodeId> {
        self.tree
            .hit_path(point)
            .into_iter()
            .find(|node| self.live.iter().any(|area| area.node == *node))
    }

    fn is_descendant(&self, node: NodeId, ancestor: NodeId) -> bool {
        is_descendant_in(self.tree, node, ancestor)
    }
}

fn visuals_of(tree: &UiTree) -> HashMap<NodeId, NodeVisual> {
    tree.draw_order()
        .into_iter()
        .map(|visual| (visual.node, visual))
        .collect()
}

fn is_descendant_in(tree: &UiTree, node: NodeId, ancestor: NodeId) -> bool {
    let mut current = tree.parent(node).ok().flatten();
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        current = tree.parent(parent).ok().flatten();
    }
    false
}

/// Сводка конфликтов «чья область → кто поймал → сколько точек, первая точка» для понятного сообщения теста.
pub(crate) fn describe_conflicts(conflicts: &[TouchConflict]) -> String {
    let mut groups: Vec<(&'static str, Option<&'static str>, usize, Point)> = Vec::new();
    for conflict in conflicts {
        match groups
            .iter_mut()
            .find(|(area, caught, _, _)| *area == conflict.area && *caught == conflict.caught_by)
        {
            Some(group) => group.2 += 1,
            None => groups.push((conflict.area, conflict.caught_by, 1, conflict.at)),
        }
    }
    groups
        .iter()
        .map(|(area, caught, count, first)| {
            format!(
                "{area}: {count} точек ловит {} (первая {:.1}, {:.1})",
                caught.unwrap_or("никто"),
                first.x,
                first.y
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn live_area(tree: &UiTree, visuals: &HashMap<NodeId, NodeVisual>, node: NodeId) -> Option<LiveArea> {
    let visual = visuals.get(&node)?;
    if visual.opacity < ui_core::tree::HitTestConfig::default().min_hittable_opacity || disabled_in_chain(tree, node) {
        return None;
    }
    let declared = area_on_screen(tree, visuals, node)?;
    let outset = tree.touch_outset(node).ok()?;
    let expanded = [outset.top, outset.left, outset.bottom, outset.right]
        .iter()
        .any(|side| *side > RECT_TOLERANCE_PT);
    let pass_through = tree.hit_test_policy(node).ok()? == HitTestPolicy::PassThrough;
    let limit = limit_of(tree, visuals, node);
    let effective = limit.map_or(Some(declared), |limit| intersect(declared, limit));
    let frame = rect_on_screen(visual, visual.frame);
    let frame_fully_live = limit.is_none_or(|limit| intersect(frame, limit).is_some_and(|live| same_rect(live, frame)));
    Some(LiveArea {
        node,
        label: visual.label,
        declared,
        effective,
        expanded,
        frame_fully_live,
        pass_through,
    })
}

/// Предел, которым предки режут область узла (`None` — не режут: дошли до корня). Повторяет правило хит-теста:
/// обычный узел ограничен областью родителя и пределом родителя; `BeyondParent` — только пределом родителя.
fn limit_of(tree: &UiTree, visuals: &HashMap<NodeId, NodeVisual>, node: NodeId) -> Option<Rect> {
    let parent = tree.parent(node).ok().flatten()?;
    let parent_limit = limit_of(tree, visuals, parent);
    let reach = tree.touch_reach(node).unwrap_or(TouchReach::WithinParent);
    match reach {
        TouchReach::BeyondParent => parent_limit,
        TouchReach::WithinParent => {
            let parent_area = area_on_screen(tree, visuals, parent)?;
            match parent_limit {
                Some(limit) => intersect(parent_area, limit).or(Some(Rect::from_xywh(0.0, 0.0, 0.0, 0.0))),
                None => Some(parent_area),
            }
        }
    }
}

/// Рамка узла с расширением зоны касания в pt экрана (охватывающий прямоугольник: при поворотах на 90° — точный).
fn area_on_screen(tree: &UiTree, visuals: &HashMap<NodeId, NodeVisual>, node: NodeId) -> Option<Rect> {
    let visual = visuals.get(&node)?;
    let outset = tree.touch_outset(node).ok()?;
    Some(rect_on_screen(visual, visual.frame.outset_by(outset)))
}

/// Прямоугольник `local` (система рамок дерева) на экране: охватывающий прямоугольник углов.
fn rect_on_screen(visual: &NodeVisual, local: Rect) -> Rect {
    let corners = [
        Point::new(local.min_x(), local.min_y()),
        Point::new(local.max_x(), local.min_y()),
        Point::new(local.min_x(), local.max_y()),
        Point::new(local.max_x(), local.max_y()),
    ]
    .map(|corner| visual.to_screen.apply(corner));
    let min_x = corners.iter().map(|point| point.x).fold(f64::INFINITY, f64::min);
    let max_x = corners.iter().map(|point| point.x).fold(f64::NEG_INFINITY, f64::max);
    let min_y = corners.iter().map(|point| point.y).fold(f64::INFINITY, f64::min);
    let max_y = corners.iter().map(|point| point.y).fold(f64::NEG_INFINITY, f64::max);
    Rect::from_xywh(min_x, min_y, max_x - min_x, max_y - min_y)
}

fn disabled_in_chain(tree: &UiTree, node: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if tree.hit_test_policy(id).ok() == Some(HitTestPolicy::DisabledWithSubtree) {
            return true;
        }
        current = tree.parent(id).ok().flatten();
    }
    false
}

fn intersect(first: Rect, second: Rect) -> Option<Rect> {
    let min_x = first.min_x().max(second.min_x());
    let min_y = first.min_y().max(second.min_y());
    let max_x = first.max_x().min(second.max_x());
    let max_y = first.max_y().min(second.max_y());
    (max_x - min_x > RECT_TOLERANCE_PT && max_y - min_y > RECT_TOLERANCE_PT)
        .then(|| Rect::from_xywh(min_x, min_y, max_x - min_x, max_y - min_y))
}

fn same_rect(first: Rect, second: Rect) -> bool {
    (first.min_x() - second.min_x()).abs() < 1e-3
        && (first.min_y() - second.min_y()).abs() < 1e-3
        && (first.max_x() - second.max_x()).abs() < 1e-3
        && (first.max_y() - second.max_y()).abs() < 1e-3
}

fn probe_points(area: Rect) -> Vec<Point> {
    let axis = |min: f64, max: f64| -> Vec<f64> {
        let start = min + PROBE_EDGE_INSET_PT;
        let end = max - PROBE_EDGE_INSET_PT;
        if end <= start {
            return vec![(min + max) * 0.5];
        }
        let steps = ((end - start) / PROBE_STEP_PT).ceil().max(1.0) as usize;
        (0..=steps)
            .map(|index| start + (end - start) * index as f64 / steps as f64)
            .collect()
    };
    let xs = axis(area.min_x(), area.max_x());
    let ys = axis(area.min_y(), area.max_y());
    ys.iter()
        .flat_map(|y| xs.iter().map(move |x| Point::new(*x, *y)))
        .collect()
}
