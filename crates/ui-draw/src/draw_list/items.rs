//! Элементы списка рисования, кроме стекла.

use crate::color::{DisplayColor, Opacity};
use crate::draw_list::glass_group::GlassGroupItem;
use crate::geometry::{PxRect, PxSize};
use crate::glass::ShadowParamsPx;
use crate::icons::IconId;
use crate::shape::ShapeGeometry;
use crate::text::TextRun;
use crate::transform::Affine2;

/// Обводка фигуры по её границе (центрирована на границе).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    pub width_px: f32,
    pub color: DisplayColor,
}

/// Закрашенная фигура.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeItem {
    pub geometry: ShapeGeometry,
    /// Локальные координаты (центр фигуры = 0) → пиксели экрана.
    pub transform: Affine2,
    pub fill: DisplayColor,
    pub stroke: Option<StrokeStyle>,
    pub shadow: Option<ShadowParamsPx>,
    pub opacity: Opacity,
}

/// Свёрстанная строка. Локальное начало — левый конец базовой линии (см. `TextRun`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextItem {
    pub run: TextRun,
    pub transform: Affine2,
    pub color: DisplayColor,
    pub opacity: Opacity,
}

/// Иконка из атласа. Локальная рамка размера `size_px` с центром в (0,0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconItem {
    pub icon: IconId,
    pub transform: Affine2,
    pub size_px: PxSize,
    pub color: DisplayColor,
    pub opacity: Opacity,
}

/// Элемент списка рисования.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawItem {
    Shape(ShapeItem),
    Glass(GlassGroupItem),
    Text(TextItem),
    Icon(IconItem),
}

impl DrawItem {
    /// Границы области, которую элемент может закрасить, в пикселях экрана (охватывающий
    /// прямоугольник вдоль осей, с запасом под тень, обводку, свечение и вынос линзы).
    /// `None` — нечего рисовать (нет глифов, пустая группа) или преобразование не конечно.
    pub fn bounds_px(&self) -> Option<PxRect> {
        match self {
            DrawItem::Shape(item) => crate::draw_list::bounds::shape_bounds(item),
            DrawItem::Glass(item) => item.bounds_px(),
            DrawItem::Text(item) => crate::draw_list::bounds::text_bounds(item),
            DrawItem::Icon(item) => crate::draw_list::bounds::icon_bounds(item),
        }
    }
}
