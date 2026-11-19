//! Расчёт рамок: обход дерева сверху вниз, якоря и стеки.

use super::types::{
    AnchorRule, AxisPlacement, ContainerLayout, CrossAlignment, LayoutError, MainDistribution, Placement,
    ReferenceArea, StackAxis, StackSpec,
};
use crate::geometry::{Rect, Size};
use crate::id::NodeId;
use crate::tree::UiTree;

impl UiTree {
    /// Пересчитывает абсолютные рамки всех элементов (pt). Детерминированно: результат зависит
    /// только от дерева и правил. Атомарно: при ошибке ни одна рамка не меняется.
    pub fn layout(&mut self) -> Result<(), LayoutError> {
        let frames = solve_frames(self)?;
        self.apply_frames(&frames);
        Ok(())
    }
}

/// Считает рамки всех элементов, не изменяя дерево.
fn solve_frames(tree: &UiTree) -> Result<Vec<(NodeId, Rect)>, LayoutError> {
    let root = tree.root();
    let mut solved = vec![(root, tree.frame(root)?)];
    // Рабочий стек вместо рекурсии: глубина дерева не ограничивает стек вызовов.
    let mut pending = vec![(root, tree.frame(root)?)];
    while let Some((parent, parent_frame)) = pending.pop() {
        for (child, child_frame) in solve_children(tree, parent, parent_frame)? {
            solved.push((child, child_frame));
            pending.push((child, child_frame));
        }
    }
    Ok(solved)
}

/// Рамки детей одного родителя по его способу раскладки.
fn solve_children(tree: &UiTree, parent: NodeId, parent_frame: Rect) -> Result<Vec<(NodeId, Rect)>, LayoutError> {
    let children = tree.children(parent)?;
    let safe_insets = tree.safe_insets(parent)?;
    let area_of = |area: ReferenceArea| match area {
        ReferenceArea::Frame => parent_frame,
        ReferenceArea::SafeArea => parent_frame.inset_by(safe_insets),
    };
    match tree.container_layout(parent)? {
        ContainerLayout::Free => children
            .iter()
            .map(|&child| match tree.placement(child)? {
                Placement::Anchored(rule) => Ok((child, solve_anchor(child, rule, area_of(rule.area))?)),
                Placement::InStack(_) => Err(LayoutError::PlacementMismatch { node: child }),
            })
            .collect(),
        ContainerLayout::Stack(spec) => {
            let sizes = children
                .iter()
                .map(|&child| match tree.placement(child)? {
                    Placement::InStack(size) => Ok((child, size)),
                    Placement::Anchored(_) => Err(LayoutError::PlacementMismatch { node: child }),
                })
                .collect::<Result<Vec<_>, LayoutError>>()?;
            solve_stack(spec, area_of(spec.area), parent, &sizes)
        }
    }
}

/// Рамка элемента с якорем в опорной области.
fn solve_anchor(node: NodeId, rule: AnchorRule, area: Rect) -> Result<Rect, LayoutError> {
    let (x, width) = solve_axis(node, rule.horizontal, area.min_x(), area.size.width)?;
    let (y, height) = solve_axis(node, rule.vertical, area.min_y(), area.size.height)?;
    Ok(Rect::from_xywh(x, y, width, height))
}

/// Начало и длина по одной оси: `area_start`, `area_length` — область по этой оси.
fn solve_axis(
    node: NodeId,
    placement: AxisPlacement,
    area_start: f64,
    area_length: f64,
) -> Result<(f64, f64), LayoutError> {
    match placement {
        AxisPlacement::Start { inset, length } => {
            check_offset(node, inset)?;
            check_length(node, length)?;
            Ok((area_start + inset, length))
        }
        AxisPlacement::End { inset, length } => {
            check_offset(node, inset)?;
            check_length(node, length)?;
            Ok((area_start + area_length - inset - length, length))
        }
        AxisPlacement::Center { offset, length } => {
            check_offset(node, offset)?;
            check_length(node, length)?;
            Ok((area_start + (area_length - length) * 0.5 + offset, length))
        }
        AxisPlacement::Stretch { start_inset, end_inset } => {
            check_offset(node, start_inset)?;
            check_offset(node, end_inset)?;
            let length = area_length - start_inset - end_inset;
            if length < 0.0 {
                return Err(LayoutError::NegativeStretch {
                    node,
                    resulting_length: length,
                });
            }
            Ok((area_start + start_inset, length))
        }
    }
}

/// Раскладка детей стека вдоль главной оси.
fn solve_stack(
    spec: StackSpec,
    area: Rect,
    container: NodeId,
    children: &[(NodeId, Size)],
) -> Result<Vec<(NodeId, Rect)>, LayoutError> {
    if !(spec.spacing.is_finite() && spec.spacing >= 0.0) {
        return Err(LayoutError::InvalidOffset {
            node: container,
            value: spec.spacing,
        });
    }
    let (main_start, main_length, cross_start, cross_length) = match spec.axis {
        StackAxis::Horizontal => (area.min_x(), area.size.width, area.min_y(), area.size.height),
        StackAxis::Vertical => (area.min_y(), area.size.height, area.min_x(), area.size.width),
    };
    let mut main_total = 0.0;
    for &(child, size) in children {
        check_length(child, size.width)?;
        check_length(child, size.height)?;
        main_total += match spec.axis {
            StackAxis::Horizontal => size.width,
            StackAxis::Vertical => size.height,
        };
    }
    let gap_count = children.len().saturating_sub(1);
    main_total += spec.spacing * gap_count as f64;
    // Свободное место может быть отрицательным (содержимое больше области) — это не ошибка.
    let free_main = main_length - main_total;
    let mut cursor = main_start
        + match spec.distribution {
            MainDistribution::Start => 0.0,
            MainDistribution::Center => free_main * 0.5,
            MainDistribution::End => free_main,
        };

    let mut frames = Vec::with_capacity(children.len());
    for &(child, size) in children {
        let (child_main, child_cross) = match spec.axis {
            StackAxis::Horizontal => (size.width, size.height),
            StackAxis::Vertical => (size.height, size.width),
        };
        let cross_offset = match spec.cross_alignment {
            CrossAlignment::Start => 0.0,
            CrossAlignment::Center => (cross_length - child_cross) * 0.5,
            CrossAlignment::End => cross_length - child_cross,
        };
        let cross_position = cross_start + cross_offset;
        let frame = match spec.axis {
            StackAxis::Horizontal => Rect::from_xywh(cursor, cross_position, size.width, size.height),
            StackAxis::Vertical => Rect::from_xywh(cross_position, cursor, size.width, size.height),
        };
        frames.push((child, frame));
        cursor += child_main + spec.spacing;
    }
    Ok(frames)
}

fn check_length(node: NodeId, value: f64) -> Result<(), LayoutError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(LayoutError::InvalidLength { node, value })
    }
}

fn check_offset(node: NodeId, value: f64) -> Result<(), LayoutError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(LayoutError::InvalidOffset { node, value })
    }
}
