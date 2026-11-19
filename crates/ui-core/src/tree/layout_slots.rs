//! Правила вёрстки, хранимые в элементе: сеттеры и геттеры.
//!
//! Правила не проверяются при записи — их целостность (длины, растяжение, соответствие родителю)
//! проверяет [`UiTree::layout`] целиком и атомарно.

use super::arena::UiTree;
use super::error::TreeError;
use crate::geometry::Insets;
use crate::id::NodeId;
use crate::layout::{ContainerLayout, Placement};

impl UiTree {
    pub fn placement(&self, node: NodeId) -> Result<Placement, TreeError> {
        Ok(self.require(node)?.placement)
    }

    /// Задаёт правило размещения элемента у родителя. У корня правила нет.
    pub fn set_placement(&mut self, node: NodeId, placement: Placement) -> Result<(), TreeError> {
        if node == self.root() {
            return Err(TreeError::RootHasNoPlacement);
        }
        self.require_mut(node)?.placement = placement;
        Ok(())
    }

    pub fn container_layout(&self, node: NodeId) -> Result<ContainerLayout, TreeError> {
        Ok(self.require(node)?.container)
    }

    /// Задаёт, как элемент размещает своих детей.
    pub fn set_container_layout(&mut self, node: NodeId, container: ContainerLayout) -> Result<(), TreeError> {
        self.require_mut(node)?.container = container;
        Ok(())
    }

    /// Безопасная зона элемента (отступы внутрь рамки) для детей с [`crate::layout::ReferenceArea::SafeArea`].
    pub fn safe_insets(&self, node: NodeId) -> Result<Insets, TreeError> {
        Ok(self.require(node)?.safe_insets)
    }

    /// Задаёт безопасную зону элемента. Для корня используйте [`UiTree::resize_screen`], но и это
    /// допустимо: значение просто заменяется.
    pub fn set_safe_insets(&mut self, node: NodeId, insets: Insets) -> Result<(), TreeError> {
        let all_finite = [insets.top, insets.left, insets.bottom, insets.right]
            .iter()
            .all(|v| v.is_finite());
        if !all_finite {
            return Err(TreeError::InvalidScreen);
        }
        self.require_mut(node)?.safe_insets = insets;
        Ok(())
    }
}
