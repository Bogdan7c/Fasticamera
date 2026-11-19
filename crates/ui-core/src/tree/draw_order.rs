//! Обход дерева в порядке рисования: итоговая геометрия и прозрачность каждого элемента.
//!
//! Рендер UI (`ui-render-vulkan`) не должен пересчитывать чужую математику — он получает готовые
//! [`NodeVisual`]. Преобразование собирается той же композицией, что у `visual_bounds` и хит-теста
//! (`CenteredTransform`): одна правда о том, где элемент на экране.

use super::arena::UiTree;
use super::error::TreeError;
use super::node_transform::NodeTransform;
use super::props::Visibility;
use crate::geometry::Rect;
use crate::id::NodeId;

/// Что нужно нарисовать для одного элемента.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeVisual {
    pub node: NodeId,
    pub label: &'static str,
    /// Рамка после вёрстки, pt, без визуального преобразования.
    pub frame: Rect,
    /// Точка в координатах рамки (pt) → точка экрана (pt), с учётом всех предков.
    pub to_screen: NodeTransform,
    /// Прозрачность элемента, перемноженная с прозрачностью всех предков (простое произведение:
    /// рендер рисует элементы по одному, группового слоя нет).
    pub opacity: f64,
    /// Глубина: у корня 0.
    pub depth: usize,
}

impl UiTree {
    /// Преобразование «рамка элемента → экран» с учётом самого элемента и всех предков.
    pub fn to_screen_transform(&self, node: NodeId) -> Result<NodeTransform, TreeError> {
        let mut composed = NodeTransform::IDENTITY;
        let mut current = Some(node);
        while let Some(id) = current {
            let data = self.require(id)?;
            // Сначала то, что накоплено (ниже по дереву), затем уровень выше.
            composed = composed.then(&data.transform().to_affine());
            current = data.parent;
        }
        Ok(composed)
    }

    /// Элементы в порядке рисования: родитель раньше детей, дети по порядку, последний — сверху.
    ///
    /// Поддерево скрытого элемента (`Visibility::Hidden`) пропускается целиком; место в вёрстке
    /// при этом остаётся. Прозрачность 0 элемент не убирает: решать, рисовать ли невидимое,
    /// рендеру (но значение перемножено, так что он видит итог).
    pub fn draw_order(&self) -> Vec<NodeVisual> {
        let mut visuals = Vec::with_capacity(self.len());
        // Стек вместо рекурсии: глубина дерева не ограничена. Хранит данные родителя.
        let mut pending = vec![PendingNode {
            node: self.root(),
            parent_to_screen: NodeTransform::IDENTITY,
            parent_opacity: 1.0,
            depth: 0,
        }];
        while let Some(entry) = pending.pop() {
            let Some(data) = self.node(entry.node) else { continue };
            if data.props.visibility == Visibility::Hidden {
                continue;
            }
            let to_screen = data.transform().to_affine().then(&entry.parent_to_screen);
            let opacity = entry.parent_opacity * data.props.opacity;
            visuals.push(NodeVisual {
                node: entry.node,
                label: data.label,
                frame: data.frame,
                to_screen,
                opacity,
                depth: entry.depth,
            });
            // Детей кладём в обратном порядке: со стека первым выйдет первый ребёнок.
            for &child in data.children.iter().rev() {
                pending.push(PendingNode {
                    node: child,
                    parent_to_screen: to_screen,
                    parent_opacity: opacity,
                    depth: entry.depth + 1,
                });
            }
        }
        visuals
    }
}

struct PendingNode {
    node: NodeId,
    parent_to_screen: NodeTransform,
    parent_opacity: f64,
    depth: usize,
}
