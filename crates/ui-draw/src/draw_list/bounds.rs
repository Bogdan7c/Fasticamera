//! Границы элементов: охватывающий прямоугольник преобразованной локальной рамки.

use crate::draw_list::items::{IconItem, ShapeItem, TextItem};
use crate::geometry::{PxPoint, PxRect};
use crate::transform::Affine2;

/// Охватывающий прямоугольник локального `rect` после преобразования (четыре угла).
pub(crate) fn transformed_bounds(transform: &Affine2, local: &PxRect) -> Option<PxRect> {
    if !transform.is_finite() {
        return None;
    }
    let corners = [
        PxPoint::new(local.origin.x, local.origin.y),
        PxPoint::new(local.max_x(), local.origin.y),
        PxPoint::new(local.max_x(), local.max_y()),
        PxPoint::new(local.origin.x, local.max_y()),
    ];
    PxRect::bounding(corners.map(|corner| transform.apply_point(corner)))
}

/// Наибольшее растяжение линейной частью (наибольшее сингулярное число): во сколько раз при самом
/// невыгодном направлении преобразование удлиняет единичный вектор. Нужно, чтобы запас в локальных
/// пикселях (обводка) перевести в экранные. `σ² = (S + √(S² − det²)) / 2`, где `S = a²+b²+c²+d²`.
pub(crate) fn linear_stretch(transform: &Affine2) -> f32 {
    let frobenius_sq =
        transform.a * transform.a + transform.b * transform.b + transform.c * transform.c + transform.d * transform.d;
    let determinant = transform.determinant();
    let discriminant = (frobenius_sq * frobenius_sq - 4.0 * determinant * determinant)
        .max(0.0)
        .sqrt();
    ((frobenius_sq + discriminant) * 0.5).sqrt()
}

pub(crate) fn shape_bounds(item: &ShapeItem) -> Option<PxRect> {
    let extent = item.geometry.extent();
    let local = PxRect::from_center(PxPoint::ORIGIN, extent);
    let mut bounds = transformed_bounds(&item.transform, &local)?;
    if let Some(stroke) = item.stroke {
        bounds = bounds.expanded(stroke.width_px * 0.5 * linear_stretch(&item.transform));
    }
    if let Some(shadow) = item.shadow {
        // Тень — размытая копия, сдвинутая вниз: запас на радиус и на смещение.
        bounds = bounds.expanded(shadow.radius_px + shadow.offset_px);
    }
    Some(bounds)
}

pub(crate) fn text_bounds(item: &TextItem) -> Option<PxRect> {
    let mut quads = item.run.glyphs.iter().map(|glyph| glyph.quad);
    let first = quads.next()?;
    let local = quads.fold(first, |acc, quad| acc.union(&quad));
    transformed_bounds(&item.transform, &local)
}

pub(crate) fn icon_bounds(item: &IconItem) -> Option<PxRect> {
    let local = PxRect::from_center(PxPoint::ORIGIN, item.size_px);
    transformed_bounds(&item.transform, &local)
}
