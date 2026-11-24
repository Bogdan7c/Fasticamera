//! Список рисования кадра: что и в каком порядке нарисовать поверх видоискателя.
//!
//! Экран камеры (12) собирает [`DrawList`] из сцены; рендер (`ui-render-vulkan`) его исполняет.
//! Элементы идут в порядке рисования: последний — сверху. Стекло группы рисуется одним проходом
//! над одним снимком фона, поэтому всё, что не стекло, но должно оказаться под стеклом, стоит в
//! списке раньше группы.
//!
//! * `items`  — типы элементов и их границы [`DrawItem::bounds_px`];
//! * `clip`   — группа прямоугольной обрезки [`ClipGroup`] и правило центра пикселя;
//! * `error`  — [`DrawListError`] и [`DrawList::validate`];
//! * `glass_group`  — группа стекла, поле слитой формы, нажатие группы;
//! * `pixel_reference` — эталонный цвет пикселя фигуры и группы стекла (для паритета рендера);
//! * `image_reference` — эталонный цвет пикселя картинки (обрезка кругом или прямоугольником, `Cover`/`Stretch`, перевод цвета);
//! * `image_source` — источник пикселей: картинка из памяти или живая с GPU.

mod bounds;
mod clip;
mod error;
mod glass_group;
mod image_reference;
mod image_source;
mod items;
mod pixel_reference;
mod validate;

use crate::light::LightDirection;

pub use clip::{
    ClipGroup, ClippedLeaf, ClippedLeaves, EffectiveClip, clip_keeps_pixel, clip_pixel_rect, clipped_leaves,
};
pub use error::DrawListError;
pub use glass_group::{GlassGroupItem, GlassMember, GroupField, MAX_GLASS_GROUP_MEMBERS, PressState};
pub use image_reference::{clip_geometry, fit_uv_scale, shade_image_pixel, shade_image_pixel_with_live};
pub use image_source::{ImageColorSpace, ImageSource, LiveImage, LiveImageTexels, LiveImageTexelsError};
pub use items::{DrawItem, IconItem, ImageClip, ImageFit, ImageItem, ShapeItem, StrokeStyle, TextItem};
pub use pixel_reference::{shade_glass_group_pixel, shade_shape_pixel};

/// Список рисования одного кадра.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawList {
    /// Элементы в порядке рисования (последний сверху).
    pub items: Vec<DrawItem>,
    /// Направление света для бликов всех групп стекла кадра.
    pub light: LightDirection,
}

impl DrawList {
    /// Пустой список со светом в покое.
    pub fn new(light: LightDirection) -> Self {
        Self {
            items: Vec::new(),
            light,
        }
    }

    /// Листья списка в порядке рисования (в глубину): элементы без групп обрезки. Для диагностики и
    /// поиска элементов в тестах; рендеру нужна ещё и действующая обрезка — [`clipped_leaves`].
    pub fn leaf_items(&self) -> impl Iterator<Item = &DrawItem> {
        clipped_leaves(&self.items).map(|leaf| leaf.item)
    }
}

#[cfg(test)]
mod equality_tests;
