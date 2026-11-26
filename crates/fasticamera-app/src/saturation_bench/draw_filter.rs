//! Фильтр списка рисования для абляций прохода UI: что стоит каждый вид элемента экрана.
//!
//! # Зачем (простыми словами)
//! Кадр показа CALIBRATE стоит 7,6 мс на проход UI, но неизвестно, ЧТО внутри дорого: стекло, фигуры, текст или
//! живая картинка прибора. Абляция — тот же настоящий список рисования экрана, из которого оставлен один вид
//! элементов (или убран один вид). Порядок оставшихся элементов не меняется: стекло видит под собой то же, что
//! видело в кадре продукта, а цена каждого вида получается вычитанием.
//!
//! Модуль чистый (GPU не нужен): [`DrawFilter::apply`] принимает список и возвращает новый; ничего лишнего он не
//! выкидывает (проверяют тесты) и не меняет сами элементы, поэтому [`DrawList::validate`] остаётся верным.

use serde::Serialize;
use ui_draw::draw_list::{ClipGroup, DrawItem, DrawList};

/// Какие элементы списка рисования остаются в кадре работы показа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawFilter {
    /// Весь список (продукт).
    All,
    /// Только группы стекла.
    GlassOnly,
    /// Только фигуры (скруглённые прямоугольники, линии, обводки).
    ShapesOnly,
    /// Только текст и значки: оба рисуются глифами атласа.
    TextOnly,
    /// Только картинки (в CALIBRATE это живая картинка прибора).
    ImagesOnly,
    /// Всё, кроме стекла.
    NoGlass,
}

impl DrawFilter {
    /// Остаётся ли элемент в кадре. Группа обрезки остаётся, если остаётся хоть один её лист.
    pub fn keeps(self, item: &DrawItem) -> bool {
        if let DrawItem::Clipped(group) = item {
            return group.items.iter().any(|child| self.keeps(child));
        }
        match self {
            Self::All => true,
            Self::GlassOnly => matches!(item, DrawItem::Glass(_)),
            Self::ShapesOnly => matches!(item, DrawItem::Shape(_)),
            Self::TextOnly => matches!(item, DrawItem::Text(_) | DrawItem::Icon(_)),
            Self::ImagesOnly => matches!(item, DrawItem::Image(_)),
            Self::NoGlass => !matches!(item, DrawItem::Glass(_)),
        }
    }

    /// Новый список: оставленные элементы в прежнем порядке, направление света то же.
    pub fn apply(self, list: &DrawList) -> DrawList {
        DrawList {
            items: self.filter_items(&list.items),
            light: list.light,
        }
    }

    /// Оставленные элементы в прежнем порядке; внутри групп обрезки фильтр применяется к детям, а сама
    /// группа (её прямоугольник) сохраняется, чтобы оставшееся рисовалось с той же обрезкой, что в продукте.
    fn filter_items(self, items: &[DrawItem]) -> Vec<DrawItem> {
        items
            .iter()
            .filter(|item| self.keeps(item))
            .map(|item| match item {
                DrawItem::Clipped(group) => DrawItem::Clipped(ClipGroup {
                    rect: group.rect,
                    items: self.filter_items(&group.items),
                }),
                other => other.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "draw_filter_tests.rs"]
mod tests;
