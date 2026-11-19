//! Идентификаторы элементов дерева.
//!
//! [`NodeId`] — «номер места + поколение»: после удаления элемента место может достаться новому,
//! но старый идентификатор с прежним поколением больше ни на что не указывает. Так жест или
//! анимация, пережившие свой элемент, не попадут молча в чужой (тест жизненного цикла дерева).

use serde::Serialize;

/// Идентификатор элемента дерева [`crate::tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct NodeId {
    slot: u32,
    generation: u32,
}

impl NodeId {
    /// Создаётся только деревом: снаружи идентификаторы приходят из его методов.
    pub(crate) const fn new(slot: u32, generation: u32) -> Self {
        Self { slot, generation }
    }

    /// Номер места в хранилище дерева.
    pub(crate) const fn slot(self) -> u32 {
        self.slot
    }

    /// Поколение места.
    pub(crate) const fn generation(self) -> u32 {
        self.generation
    }
}
