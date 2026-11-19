//! Ошибки проверки списка рисования.

use thiserror::Error;

use crate::draw_list::glass_group::MAX_GLASS_GROUP_MEMBERS;
use crate::shape::ShapeError;
use crate::transform::AffineError;

/// Что не так с элементом списка. Везде `item` — индекс элемента в [`super::DrawList::items`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DrawListError {
    /// В числе элемента NaN или бесконечность.
    #[error("элемент {item}: поле «{field}» не конечное число")]
    NotFinite { item: usize, field: &'static str },
    /// Размер, ширина обводки и т.п. отрицательны (или иконка нулевого размера).
    #[error("элемент {item}: «{field}» должно быть неотрицательным и конечным, получено {value}")]
    InvalidSize {
        item: usize,
        field: &'static str,
        value: f32,
    },
    /// Значение, которое обязано быть в 0..1, вышло за диапазон (материализация, сила нажатия, цвет).
    #[error("элемент {item}: «{field}» вне диапазона 0…1: {value}")]
    OutOfUnitRange {
        item: usize,
        field: &'static str,
        value: f32,
    },
    /// Фигура задана неверно.
    #[error("элемент {item}: фигура некорректна: {source}")]
    InvalidShape { item: usize, source: ShapeError },
    /// Преобразование нельзя обратить (вырождено или не число).
    #[error("элемент {item}: преобразование непригодно: {source}")]
    DegenerateTransform { item: usize, source: AffineError },
    /// В группе стекла больше форм, чем влезает в шейдер.
    #[error("элемент {item}: в группе стекла {count} форм, предел {limit}")]
    TooManyGlassMembers { item: usize, count: usize, limit: usize },
    /// Группа стекла без единой формы.
    #[error("элемент {item}: группа стекла пуста")]
    EmptyGlassGroup { item: usize },
}

impl DrawListError {
    pub(crate) fn too_many_members(item: usize, count: usize) -> Self {
        Self::TooManyGlassMembers {
            item,
            count,
            limit: MAX_GLASS_GROUP_MEMBERS,
        }
    }
}
