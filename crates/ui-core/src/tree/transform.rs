//! Визуальное преобразование элемента (масштаб + поворот вокруг центра рамки + сдвиг).
//!
//! Как `transform` в UIKit: вёрстку не затрагивает, но влияет на то, где элемент виден и куда
//! попадает касание. Порядок композиции (как `translate ∘ rotate ∘ scale` у `CGAffineTransform`
//! вокруг центра): сначала масштаб, затем поворот, затем сдвиг — `p' = c + T + R·S·(p − c)`.
//! Сдвиг `T` в pt системы родителя и от масштаба/поворота самого элемента не зависит.

use super::node_transform::NodeTransform;
use crate::geometry::{Point, Vector};

/// Масштаб элемента по осям (1,0 — без изменений).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementScale {
    pub x: f64,
    pub y: f64,
}

impl ElementScale {
    /// Без масштабирования.
    pub const IDENTITY: ElementScale = ElementScale { x: 1.0, y: 1.0 };

    /// Одинаковый масштаб по обеим осям.
    pub const fn uniform(factor: f64) -> Self {
        Self { x: factor, y: factor }
    }
}

/// Преобразование вокруг заданного центра.
#[derive(Debug, Clone, Copy)]
pub(super) struct CenteredTransform {
    pub center: Point,
    pub scale: ElementScale,
    pub rotation_radians: f64,
    /// Визуальный сдвиг в системе родителя, pt.
    pub translation: Vector,
}

impl CenteredTransform {
    /// То же преобразование в виде аффинной матрицы: единственное место, где формула
    /// `p' = c + T + R·S·(p − c)` раскрывается в числа (рисование и `visual_bounds` берут её отсюда).
    pub fn to_affine(self) -> NodeTransform {
        let (sin, cos) = self.rotation_radians.sin_cos();
        // Линейная часть R·S: столбцы — образы осей x и y.
        let m_a = cos * self.scale.x;
        let m_b = sin * self.scale.x;
        let m_c = -sin * self.scale.y;
        let m_d = cos * self.scale.y;
        NodeTransform {
            a: m_a,
            b: m_b,
            c: m_c,
            d: m_d,
            // c + T − M·c: сдвиг, который оставляет центр рамки на месте до применения T.
            tx: self.center.x + self.translation.dx - m_a * self.center.x - m_c * self.center.y,
            ty: self.center.y + self.translation.dy - m_b * self.center.x - m_d * self.center.y,
        }
    }

    /// Из точки родительской системы в локальную. `None`, если масштаб вырожден (защита:
    /// публичные сеттеры нулевой масштаб не принимают).
    pub fn invert(&self, point: Point) -> Option<Point> {
        if self.scale.x == 0.0 || self.scale.y == 0.0 {
            return None;
        }
        let (sin, cos) = self.rotation_radians.sin_cos();
        // Сначала убираем сдвиг (он применялся последним).
        let delta_x = point.x - self.center.x - self.translation.dx;
        let delta_y = point.y - self.center.y - self.translation.dy;
        // Обратный поворот на −θ, затем деление на масштаб.
        let unrotated_x = delta_x * cos + delta_y * sin;
        let unrotated_y = -delta_x * sin + delta_y * cos;
        Some(Point::new(
            self.center.x + unrotated_x / self.scale.x,
            self.center.y + unrotated_y / self.scale.y,
        ))
    }
}
