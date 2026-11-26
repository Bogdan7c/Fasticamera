//! Доступ к дереву через сцену: единственный путь изменения структуры и свойств.

use super::{SceneError, UiScene};
use crate::animator::{AnimatedProperty, AnimationRequest, AnimationStart, FreezeOutcome, PropertyKey, SnapOutcome};
use crate::geometry::Insets;
use crate::id::NodeId;
use crate::layout::{ContainerLayout, Placement};
use crate::tree::{HitTestPolicy, TouchReach, Visibility};

/// Что убрала [`UiScene::remove_subtree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeRemoval {
    /// Удалённые элементы: сам элемент, затем потомки в порядке обхода в глубину.
    pub removed: Vec<NodeId>,
    /// Сколько распознавателей удалено (идущие жесты получили `Cancelled`).
    pub recognizers_removed: usize,
    /// Сколько анимаций свойств снято.
    pub animations_removed: usize,
}

impl<I: Clone> UiScene<I> {
    /// Добавляет ребёнка в конец (рисуется поверх остальных).
    pub fn insert_child(&mut self, parent: NodeId, label: &'static str) -> Result<NodeId, SceneError> {
        Ok(self.tree.insert_child(parent, label)?)
    }

    /// Добавляет ребёнка в позицию порядка рисования.
    pub fn insert_child_at(&mut self, parent: NodeId, index: usize, label: &'static str) -> Result<NodeId, SceneError> {
        Ok(self.tree.insert_child_at(parent, index, label)?)
    }

    /// Удаляет поддерево **вместе с его жестами и анимациями**: идущие жесты удалённых элементов
    /// получают `Cancelled` со своим намерением, анимации снимаются. Корень удалить нельзя.
    ///
    /// Это единственный способ удалить элемент: `&mut UiTree` сцена не выдаёт.
    pub fn remove_subtree(&mut self, node: NodeId) -> Result<SubtreeRemoval, SceneError> {
        let removed = self.tree.remove_subtree(node)?;
        // Анимации снимаются первыми (это не может завершиться ошибкой): жесты удаляются после,
        // и если нарушен инвариант арены, ошибка возвращается, а не теряется.
        let animations_removed = self.animator.forget(&removed);
        let mut recognizers_removed = 0;
        for &gone in &removed {
            recognizers_removed += self.gestures.remove_node_recognizers(gone)?;
        }
        Ok(SubtreeRemoval {
            removed,
            recognizers_removed,
            animations_removed,
        })
    }

    pub fn set_placement(&mut self, node: NodeId, placement: Placement) -> Result<(), SceneError> {
        Ok(self.tree.set_placement(node, placement)?)
    }

    pub fn set_container_layout(&mut self, node: NodeId, container: ContainerLayout) -> Result<(), SceneError> {
        Ok(self.tree.set_container_layout(node, container)?)
    }

    pub fn set_safe_insets(&mut self, node: NodeId, insets: Insets) -> Result<(), SceneError> {
        Ok(self.tree.set_safe_insets(node, insets)?)
    }

    pub fn set_visibility(&mut self, node: NodeId, visibility: Visibility) -> Result<(), SceneError> {
        Ok(self.tree.set_visibility(node, visibility)?)
    }

    pub fn set_hit_test_policy(&mut self, node: NodeId, policy: HitTestPolicy) -> Result<(), SceneError> {
        Ok(self.tree.set_hit_test_policy(node, policy)?)
    }

    pub fn set_touch_outset(&mut self, node: NodeId, outset: Insets) -> Result<(), SceneError> {
        Ok(self.tree.set_touch_outset(node, outset)?)
    }

    pub fn set_touch_reach(&mut self, node: NodeId, reach: TouchReach) -> Result<(), SceneError> {
        Ok(self.tree.set_touch_reach(node, reach)?)
    }

    /// Пересчитывает рамки. Хит-тест жестов работает по последней вёрстке.
    pub fn layout(&mut self) -> Result<(), SceneError> {
        Ok(self.tree.layout()?)
    }

    /// Останавливает едущее свойство там, где его видно (захват пальцем, начало перетаскивания).
    /// См. [`crate::animator::PropertyAnimator::freeze`].
    pub fn freeze_property(&mut self, node: NodeId, property: AnimatedProperty) -> Result<FreezeOutcome, SceneError> {
        Ok(self.animator.freeze(&self.tree, PropertyKey { node, property })?)
    }

    /// Устанавливает анимируемое свойство сразу (ведение за пальцем 1:1) и снимает его анимацию.
    pub fn set_property(
        &mut self,
        node: NodeId,
        property: AnimatedProperty,
        value: f64,
    ) -> Result<SnapOutcome, SceneError> {
        Ok(self
            .animator
            .snap(&mut self.tree, PropertyKey { node, property }, value)?)
    }

    /// Запускает или перенацеливает анимацию свойства (см. [`crate::animator::PropertyAnimator::animate`]).
    pub fn animate(&mut self, request: AnimationRequest) -> Result<AnimationStart, SceneError> {
        Ok(self.animator.animate(&self.tree, request)?)
    }
}
