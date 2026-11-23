//! Элементы списка рисования, кроме стекла.

use crate::color::{DisplayColor, Opacity};
use crate::draw_list::glass_group::GlassGroupItem;
use crate::draw_list::image_source::ImageSource;
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

/// Как картинка ложится в рамку элемента.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFit {
    /// Заполнить рамку целиком, сохранив пропорции: картинка масштабируется так, чтобы закрыть рамку,
    /// лишнее по длинной стороне обрезается поровну с двух краёв (центр картинки = центр рамки).
    Cover,
    /// Растянуть источник на рамку целиком, пропорции НЕ сохраняются (источник и рамка совпадают по
    /// краям). Для картинок, посчитанных под рамку (приборы цвета): обрезки нет.
    Stretch,
}

/// Форма, которой картинка обрезается внутри рамки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageClip {
    /// Круг, вписанный в рамку: диаметр = меньшая сторона `size_px`, центр = центр рамки.
    /// Край сглаживается тем же покрытием, что у фигур.
    Circle,
    /// Без формы: прямоугольник рамки. Край сглаживается тем же покрытием, что у фигур (SDF
    /// прямоугольника без скругления), поэтому при рамке на границе пикселей он остаётся резким, а при
    /// дробном сдвиге получает ту же мягкость, что любая фигура.
    None,
}

/// Картинка (14g: миниатюра снимка; 14h: прибор цвета). Локальная рамка размера `size_px` с центром в
/// (0,0); пиксели источника — [`ImageSource`] (из памяти или живые с GPU), перевод кодировки источника
/// в цвет экрана — забота рендера и его эталона.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageItem {
    pub source: ImageSource,
    /// Локальные координаты (центр рамки = 0) → пиксели экрана. Поворот картинки (значки по ориентации
    /// телефона) задаётся здесь же.
    pub transform: Affine2,
    pub size_px: PxSize,
    pub fit: ImageFit,
    pub clip: ImageClip,
    pub opacity: Opacity,
}

/// Элемент списка рисования.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawItem {
    Shape(ShapeItem),
    Glass(GlassGroupItem),
    Text(TextItem),
    Icon(IconItem),
    Image(ImageItem),
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
            DrawItem::Image(item) => crate::draw_list::bounds::image_bounds(item),
        }
    }
}
