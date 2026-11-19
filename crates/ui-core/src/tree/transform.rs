//! Визуальное преобразование элемента (масштаб + поворот вокруг центра рамки + сдвиг).
//!
//! Как `transform` в UIKit: вёрстку не затрагивает, но влияет на то, где элемент виден и куда
//! попадает касание. Порядок композиции (как `translate ∘ rotate ∘ scale` у `CGAffineTransform`
//! вокруг центра): сначала масштаб, затем поворот, затем сдвиг — `p' = c + T + R·S·(p − c)`.
//! Сдвиг `T` в pt системы родителя и от масштаба/поворота самого элемента не зависит.

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
    /// Из локальной (до преобразования) точки в точку родительской системы.
    pub fn apply(&self, point: Point) -> Point {
        let (sin, cos) = self.rotation_radians.sin_cos();
        let scaled_x = (point.x - self.center.x) * self.scale.x;
        let scaled_y = (point.y - self.center.y) * self.scale.y;
        Point::new(
            self.center.x + self.translation.dx + scaled_x * cos - scaled_y * sin,
            self.center.y + self.translation.dy + scaled_x * sin + scaled_y * cos,
        )
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
