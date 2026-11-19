//! Аффинное преобразование 2×3 из системы координат элемента в экранную (pt).

use crate::geometry::Point;

/// Аффинное преобразование плоскости: `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`
/// (матрица `[[a, c, tx], [b, d, ty]]`, как `CGAffineTransform`).
///
/// В [`crate::tree::NodeVisual`] — полный путь «точка в рамке элемента (pt, после вёрстки) →
/// точка экрана (pt)» с учётом масштаба, поворота и сдвига самого элемента и всех предков.
/// Рамки в дереве абсолютные, поэтому система «родителя» у ребёнка — та же экранная система,
/// а преобразования предков просто накладываются сверху.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeTransform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl NodeTransform {
    /// Тождественное преобразование.
    pub const IDENTITY: NodeTransform = NodeTransform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Переводит точку.
    pub fn apply(&self, point: Point) -> Point {
        Point::new(
            self.a * point.x + self.c * point.y + self.tx,
            self.b * point.x + self.d * point.y + self.ty,
        )
    }

    /// Композиция: сначала `self`, затем `outer` (`result.apply(p) == outer.apply(self.apply(p))`).
    pub fn then(&self, outer: &NodeTransform) -> NodeTransform {
        NodeTransform {
            a: outer.a * self.a + outer.c * self.b,
            b: outer.b * self.a + outer.d * self.b,
            c: outer.a * self.c + outer.c * self.d,
            d: outer.b * self.c + outer.d * self.d,
            tx: outer.a * self.tx + outer.c * self.ty + outer.tx,
            ty: outer.b * self.tx + outer.d * self.ty + outer.ty,
        }
    }

    /// Обратное преобразование; `None`, если матрица вырождена (определитель 0 или не число).
    pub fn inverse(&self) -> Option<NodeTransform> {
        let determinant = self.a * self.d - self.b * self.c;
        if determinant == 0.0 || !determinant.is_finite() {
            return None;
        }
        let a = self.d / determinant;
        let b = -self.b / determinant;
        let c = -self.c / determinant;
        let d = self.a / determinant;
        Some(NodeTransform {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROTATE_90_THEN_SHIFT: NodeTransform = NodeTransform {
        a: 0.0,
        b: 1.0,
        c: -1.0,
        d: 0.0,
        tx: 10.0,
        ty: 20.0,
    };

    #[test]
    fn apply_rotates_then_shifts() {
        let moved = ROTATE_90_THEN_SHIFT.apply(Point::new(1.0, 0.0));
        assert_eq!(moved, Point::new(10.0, 21.0));
    }

    #[test]
    fn then_applies_self_first() {
        let scale = NodeTransform {
            a: 2.0,
            d: 2.0,
            ..NodeTransform::IDENTITY
        };
        let composed = ROTATE_90_THEN_SHIFT.then(&scale);
        let point = Point::new(3.0, 4.0);
        assert_eq!(composed.apply(point), scale.apply(ROTATE_90_THEN_SHIFT.apply(point)));
        assert_eq!(composed.apply(Point::ORIGIN), Point::new(20.0, 40.0));
    }

    #[test]
    fn inverse_round_trips_and_rejects_degenerate() {
        let inverse = ROTATE_90_THEN_SHIFT.inverse().unwrap();
        let point = Point::new(7.0, -2.0);
        let back = inverse.apply(ROTATE_90_THEN_SHIFT.apply(point));
        assert!((back.x - point.x).abs() < 1e-12 && (back.y - point.y).abs() < 1e-12);
        let flat = NodeTransform {
            a: 0.0,
            d: 0.0,
            ..NodeTransform::IDENTITY
        };
        assert_eq!(flat.inverse(), None);
    }
}
