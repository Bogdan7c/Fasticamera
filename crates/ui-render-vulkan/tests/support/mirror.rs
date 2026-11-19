//! Тонкая обвязка над эталоном `ui-draw` для тестов паритета: формулы фигур и стекла (с тенью, нажатием
//! группы) живут в `ui_draw::draw_list::{shade_shape_pixel, shade_glass_group_pixel}`, здесь только
//! перевод в `[f32; 4]`, «поверх», CPU-рендер списка фигур и сравнение картинок.

use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, ShapeItem, shade_glass_group_pixel, shade_shape_pixel};
use ui_draw::geometry::PxPoint;
use ui_draw::light::LightDirection;

/// «Поверх» в premultiplied.
pub fn over(destination: [f32; 4], source: [f32; 4]) -> [f32; 4] {
    let keep = 1.0 - source[3];
    [
        source[0] + destination[0] * keep,
        source[1] + destination[1] * keep,
        source[2] + destination[2] * keep,
        source[3] + destination[3] * keep,
    ]
}

/// Цвет пикселя фигуры в точке экрана по эталону `ui_draw::draw_list::shade_shape_pixel` (ширина сглаживания 1 px, как в шейдере).
pub fn shape_pixel(item: &ShapeItem, point: PxPoint) -> [f32; 4] {
    let color = shade_shape_pixel(item, point, 1.0).unwrap();
    [color.r, color.g, color.b, color.a]
}

/// Пиксель стекла группы вместе с её тенью по эталону `ui_draw::draw_list::shade_glass_group_pixel`.
/// `backdrop` — цвет фона в экранной точке.
pub fn glass_pixel(
    group: &GlassGroupItem,
    light: LightDirection,
    point: PxPoint,
    mean_luma: f32,
    backdrop: impl Fn(PxPoint) -> DisplayColor,
) -> [f32; 4] {
    let color = shade_glass_group_pixel(group, light, point, mean_luma, backdrop, 1.0).unwrap();
    [color.r, color.g, color.b, color.a]
}

/// CPU-рендер списка из одних фигур поверх `clear` (прямая ориентация, без поворота).
pub fn render_shapes(list: &DrawList, width: u32, height: u32, clear: [f32; 4]) -> Vec<[f32; 4]> {
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let point = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
            let mut color = clear;
            for item in &list.items {
                if let DrawItem::Shape(shape) = item {
                    color = over(color, shape_pixel(shape, point));
                }
            }
            pixels.push(color);
        }
    }
    pixels
}

/// Наибольшее расхождение по каналам RGB и его позиция.
pub fn worst_difference(actual: &[[f32; 4]], expected: &[[f32; 4]], width: u32) -> (f32, (u32, u32)) {
    let mut worst = (0.0_f32, (0, 0));
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..3 {
            let difference = (a[channel] - e[channel]).abs();
            if difference > worst.0 {
                worst = (difference, (index as u32 % width, index as u32 / width));
            }
        }
    }
    worst
}
