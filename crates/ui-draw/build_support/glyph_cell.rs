//! Ячейка атласа для одного глифа: уплощение контура и SDF-заливка (решение 88).

use super::font_bake::{GlyphShape, PathCommand};
use super::sdf::{GridSize, Point, encode_field, fill_distance_field};

/// Допуск уплощения кривых, текселей: ошибка формы много меньше шага квантования SDF.
pub const FLATTEN_TOLERANCE_TEXELS: f64 = 0.02;

/// Готовая ячейка глифа и её положение относительно начала глифа.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphCell {
    pub width_texels: u32,
    pub height_texels: u32,
    /// Левый край ячейки от начала глифа (по x), в em.
    pub left_em: f64,
    /// Верхний край ячейки от базовой линии, em; ось вниз, поэтому над базовой линией — отрицательное.
    pub top_em: f64,
    /// SDF-байты построчно, `width_texels × height_texels`.
    pub texels: Vec<u8>,
}

/// Строит ячейку глифа; `None`, если у глифа нет контура (пробел).
///
/// Рамка ячейки — рамка контура плюс `spread_texels` со всех сторон, ширина и высота округляются вверх
/// до целых текселей. Масштаб ячейки совпадает с масштабом em (`em_texels` текселей на em), поэтому
/// прямоугольник на экране получается обычным умножением на кегль.
pub fn render_glyph_cell(
    shape: &GlyphShape,
    units_per_em: u16,
    em_texels: u32,
    spread_texels: u32,
) -> Option<GlyphCell> {
    let scale = f64::from(em_texels) / f64::from(units_per_em);
    let contours_up = flatten_commands(shape, scale);
    let (min_x, max_x, min_y, max_y) = bounds_of(&contours_up)?;
    let spread = f64::from(spread_texels);
    let cell_left = min_x - spread;
    let cell_top = max_y + spread;
    let width_texels = (max_x - min_x).ceil() as u32 + 2 * spread_texels;
    let height_texels = (max_y - min_y).ceil() as u32 + 2 * spread_texels;

    // Переходим из системы «y вверх от базовой линии» в систему ячейки «y вниз от верхнего края».
    let contours: Vec<Vec<Point>> = contours_up
        .iter()
        .map(|contour| {
            contour
                .iter()
                .map(|point| Point::new(point.x - cell_left, cell_top - point.y))
                .collect()
        })
        .collect();
    let grid = GridSize {
        width: width_texels as usize,
        height: height_texels as usize,
    };
    let field = fill_distance_field(grid, &contours, spread);
    Some(GlyphCell {
        width_texels,
        height_texels,
        left_em: cell_left / f64::from(em_texels),
        top_em: -cell_top / f64::from(em_texels),
        texels: encode_field(&field, spread),
    })
}

/// Уплощает контур глифа в замкнутые ломаные; единицы шрифта умножаются на `scale`, ось y остаётся вверх.
pub fn flatten_commands(shape: &GlyphShape, scale: f64) -> Vec<Vec<Point>> {
    let mut contours: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let to_point = |raw: [f64; 2]| Point::new(raw[0] * scale, raw[1] * scale);
    for command in shape {
        match *command {
            PathCommand::MoveTo(target) => {
                finish_contour(&mut contours, &mut current);
                current.push(to_point(target));
            }
            PathCommand::LineTo(target) => current.push(to_point(target)),
            PathCommand::QuadTo(control, target) => {
                if let Some(&start) = current.last() {
                    flatten_quad(&mut current, start, to_point(control), to_point(target));
                }
            }
            PathCommand::CubicTo(first_control, second_control, target) => {
                if let Some(&start) = current.last() {
                    flatten_cubic(
                        &mut current,
                        start,
                        to_point(first_control),
                        to_point(second_control),
                        to_point(target),
                    );
                }
            }
            PathCommand::Close => finish_contour(&mut contours, &mut current),
        }
    }
    finish_contour(&mut contours, &mut current);
    contours
}

/// Завершает текущий контур: контуры короче трёх точек площади не имеют и отбрасываются.
fn finish_contour(contours: &mut Vec<Vec<Point>>, current: &mut Vec<Point>) {
    let finished = std::mem::take(current);
    if finished.len() >= 3 {
        contours.push(finished);
    }
}

/// Равномерное деление квадратичной кривой. Ошибка хорды при `n` частях не больше `|p0−2p1+p2| / (4n²)`.
fn flatten_quad(output: &mut Vec<Point>, start: Point, control: Point, end: Point) {
    let second_difference = (start.x - 2.0 * control.x + end.x).hypot(start.y - 2.0 * control.y + end.y);
    let parts = ((second_difference / (4.0 * FLATTEN_TOLERANCE_TEXELS)).sqrt().ceil() as usize).max(1);
    for step in 1..=parts {
        let t = step as f64 / parts as f64;
        let inverse = 1.0 - t;
        output.push(Point::new(
            inverse * inverse * start.x + 2.0 * inverse * t * control.x + t * t * end.x,
            inverse * inverse * start.y + 2.0 * inverse * t * control.y + t * t * end.y,
        ));
    }
}

/// Равномерное деление кубической кривой. Ошибка хорды при `n` частях не больше `0.75 · max|Δ²| / n²`.
fn flatten_cubic(output: &mut Vec<Point>, start: Point, first: Point, second: Point, end: Point) {
    let first_difference = (start.x - 2.0 * first.x + second.x).hypot(start.y - 2.0 * first.y + second.y);
    let second_difference = (first.x - 2.0 * second.x + end.x).hypot(first.y - 2.0 * second.y + end.y);
    let worst = first_difference.max(second_difference);
    let parts = ((0.75 * worst / FLATTEN_TOLERANCE_TEXELS).sqrt().ceil() as usize).max(1);
    for step in 1..=parts {
        let t = step as f64 / parts as f64;
        let inverse = 1.0 - t;
        let weights = [
            inverse * inverse * inverse,
            3.0 * inverse * inverse * t,
            3.0 * inverse * t * t,
            t * t * t,
        ];
        output.push(Point::new(
            weights[0] * start.x + weights[1] * first.x + weights[2] * second.x + weights[3] * end.x,
            weights[0] * start.y + weights[1] * first.y + weights[2] * second.y + weights[3] * end.y,
        ));
    }
}

/// Рамка всех точек: (min x, max x, min y, max y); `None` для пустого набора.
fn bounds_of(contours: &[Vec<Point>]) -> Option<(f64, f64, f64, f64)> {
    let mut points = contours.iter().flatten();
    let first = points.next()?;
    let mut bounds = (first.x, first.x, first.y, first.y);
    for point in points {
        bounds.0 = bounds.0.min(point.x);
        bounds.1 = bounds.1.max(point.x);
        bounds.2 = bounds.2.min(point.y);
        bounds.3 = bounds.3.max(point.y);
    }
    Some(bounds)
}
