//! Знаковое расстояние до фигуры в локальных координатах (центр фигуры в (0,0), y вниз).
//!
//! Схема «rounded box» с нормой `L_n` в угловой зоне. Пусть `half` — половина размера, `r` — радиус,
//! `n` — показатель (2 для дуги, 5 для непрерывного скругления):
//!
//! ```text
//! q = |p| − (half − r)
//! d = ‖max(q, 0)‖ₙ + min(max(q.x, q.y), 0) − r,      ‖v‖ₙ = (|v.x|ⁿ + |v.y|ⁿ)^(1/n)
//! ```
//!
//! Внутри `d < 0`, на границе `d = 0`. В угловой зоне нулевая линия — ровно суперэллипс
//! `|qx/r|ⁿ + |qy/r|ⁿ = 1`. Для `n ≠ 2` `d` — НЕ евклидово расстояние (вдали от границы и по
//! диагонали угла оно завышает/занижает), поэтому рядом даны градиент и «исправленное» расстояние
//! `d / |∇d|` — первый порядок точности около границы; шейдер сглаживает край именно им.

use crate::geometry::{PxPoint, PxVector};
use crate::shape::types::{SdfParameters, ShapeGeometry};

/// Расстояние со знаком (внутри — отрицательное), в единицах локальных координат.
pub fn signed_distance(geometry: &ShapeGeometry, local: PxPoint) -> f32 {
    field(&geometry.sdf_parameters(), local).0
}

/// Аналитический градиент [`signed_distance`]. Вне угловой зоны — единичный вектор наружу;
/// в углу при `n > 2` его длина меньше 1. На осях симметрии (например, в самом центре) направление не
/// определено, и берётся одна из осей. Нулевым градиент не бывает.
pub fn distance_gradient(geometry: &ShapeGeometry, local: PxPoint) -> PxVector {
    field(&geometry.sdf_parameters(), local).1
}

/// Расстояние, исправленное до первого порядка к евклидову, и единичная нормаль наружу.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorrectedDistance {
    /// `d / |∇d|`: у границы равно евклидову расстоянию с точностью до второго порядка.
    pub distance: f32,
    /// Единичная нормаль наружу (нулевой вектор не возникает: градиент не бывает нулевым).
    pub normal_out: PxVector,
}

/// Исправленное расстояние `d / |∇d|` и нормаль — ровно то, что шейдер использует для сглаживания
/// края, линзы и блика.
pub fn corrected_distance(geometry: &ShapeGeometry, local: PxPoint) -> CorrectedDistance {
    let (distance, gradient) = field(&geometry.sdf_parameters(), local);
    match gradient.normalized() {
        Some(normal_out) => CorrectedDistance {
            distance: distance / gradient.length(),
            normal_out,
        },
        None => CorrectedDistance {
            distance,
            normal_out: PxVector::ZERO,
        },
    }
}

/// Знак с нулём как «плюс»: у оси симметрии выбираем сторону детерминированно.
fn sign(value: f32) -> f32 {
    if value >= 0.0 { 1.0 } else { -1.0 }
}

/// Значение поля и градиент за один проход.
fn field(shape: &SdfParameters, p: PxPoint) -> (f32, PxVector) {
    let sign_x = sign(p.x);
    let sign_y = sign(p.y);
    // q — положение относительно центра угловой зоны (угловая зона — квадрат r×r у угла).
    let qx = p.x.abs() - (shape.half_size.width - shape.radius);
    let qy = p.y.abs() - (shape.half_size.height - shape.radius);

    if qx > 0.0 && qy > 0.0 {
        // Угловая зона: расстояние — L_n-норма вектора q.
        let n = shape.exponent;
        let norm = (qx.powf(n) + qy.powf(n)).powf(1.0 / n);
        // ∂‖q‖ₙ/∂qᵢ = qᵢ^(n−1) / ‖q‖ₙ^(n−1); знаки возвращают градиент из «|p|» в p.
        let scale = norm.powf(1.0 - n);
        let gradient = PxVector::new(sign_x * qx.powf(n - 1.0) * scale, sign_y * qy.powf(n - 1.0) * scale);
        return (norm - shape.radius, gradient);
    }

    // Вне угловой зоны (прямая сторона либо внутренность) расстояние линейно по одной оси:
    // при qx > 0, qy ≤ 0 это qx − r; внутри — max(qx, qy) − r.
    if qx > qy {
        (qx - shape.radius, PxVector::new(sign_x, 0.0))
    } else {
        (qy - shape.radius, PxVector::new(0.0, sign_y))
    }
}
