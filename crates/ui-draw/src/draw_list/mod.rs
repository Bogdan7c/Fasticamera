//! Список рисования кадра: что и в каком порядке нарисовать поверх видоискателя.
//!
//! Экран камеры (12) собирает [`DrawList`] из сцены; рендер (`ui-render-vulkan`) его исполняет.
//! Элементы идут в порядке рисования: последний — сверху. Стекло группы рисуется одним проходом
//! над одним снимком фона, поэтому всё, что не стекло, но должно оказаться под стеклом, стоит в
//! списке раньше группы.
//!
//! * `items`  — типы элементов и их границы [`DrawItem::bounds_px`];
//! * `error`  — [`DrawListError`] и [`DrawList::validate`];
//! * `glass_group`  — группа стекла, поле слитой формы, нажатие группы;
//! * `pixel_reference` — эталонный цвет пикселя фигуры и группы стекла (для паритета рендера);
//! * `image_reference` — эталонный цвет пикселя картинки (обрезка кругом, `Cover`, перевод цвета).

mod bounds;
mod error;
mod glass_group;
mod image_reference;
mod items;
mod pixel_reference;
mod validate;

use crate::light::LightDirection;

pub use error::DrawListError;
pub use glass_group::{GlassGroupItem, GlassMember, GroupField, MAX_GLASS_GROUP_MEMBERS, PressState};
pub use image_reference::{clip_geometry, cover_uv_scale, shade_image_pixel};
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
}
