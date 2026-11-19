#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Непрерывное скругление iOS: угол — суперэллипс n = 5, а не дуга окружности.

use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::shape::{CornerSpec, ShapeGeometry, signed_distance};

const RADIUS: f32 = 40.0;
const WIDTH: f32 = 200.0;
const HEIGHT: f32 = 120.0;
/// Центр угловой зоны правого нижнего угла (+x, +y) в локальных координатах; поле симметрично.
const CORNER_CENTER: PxPoint = PxPoint::new(WIDTH / 2.0 - RADIUS, HEIGHT / 2.0 - RADIUS);

fn rect(corner: CornerSpec) -> ShapeGeometry {
    ShapeGeometry::RoundedRect {
        size: PxSize::new(WIDTH, HEIGHT),
        corner,
    }
}

/// Ищет нулевую точку поля на луче из `origin` в направлении `direction` делением пополам.
fn zero_on_ray(geometry: &ShapeGeometry, origin: PxPoint, direction: (f32, f32), max_t: f32) -> PxPoint {
    let at = |t: f32| PxPoint::new(origin.x + direction.0 * t, origin.y + direction.1 * t);
    assert!(
        signed_distance(geometry, at(0.0)) < 0.0,
        "начало луча должно быть внутри"
    );
    assert!(
        signed_distance(geometry, at(max_t)) > 0.0,
        "конец луча должен быть снаружи"
    );
    let (mut inside, mut outside) = (0.0_f32, max_t);
    for _ in 0..60 {
        let middle = 0.5 * (inside + outside);
        if signed_distance(geometry, at(middle)) < 0.0 {
            inside = middle;
        } else {
            outside = middle;
        }
    }
    at(0.5 * (inside + outside))
}

#[test]
fn continuous_corner_zero_level_lies_on_superellipse_n5() {
    let shape = rect(CornerSpec::continuous(RADIUS));
    for degrees in [5.0_f32, 20.0, 35.0, 45.0, 60.0, 75.0, 85.0] {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let point = zero_on_ray(&shape, CORNER_CENTER, (cos, sin), 2.0 * RADIUS);
        let (qx, qy) = (point.x - CORNER_CENTER.x, point.y - CORNER_CENTER.y);
        // Суперэллипс |qx/r|^5 + |qy/r|^5 = 1 → L5-норма вектора q равна r.
        let norm = (qx.abs().powf(5.0) + qy.abs().powf(5.0)).powf(0.2);
        assert!(
            (norm - RADIUS).abs() < 0.05,
            "угол {degrees}°: L5-норма {norm}, ожидали {RADIUS}"
        );
    }
}

#[test]
fn continuous_corner_is_far_from_circular_arc_on_the_diagonal() {
    let continuous = rect(CornerSpec::continuous(RADIUS));
    let circular = rect(CornerSpec::circular(RADIUS));
    let diagonal = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
    let on_squircle = zero_on_ray(&continuous, CORNER_CENTER, diagonal, 2.0 * RADIUS);
    let on_arc = zero_on_ray(&circular, CORNER_CENTER, diagonal, 2.0 * RADIUS);
    // Дуга на диагонали ровно в r от центра угла; суперэллипс n=5 — в r·2^(1/2−1/5) = 1,23·r.
    assert!(((on_arc - CORNER_CENTER).length() - RADIUS).abs() < 1e-3);
    let gap = (on_squircle - on_arc).length();
    assert!(
        gap > 5.0,
        "суперэллипс отстоит от дуги на диагонали лишь на {gap} px — это n≈2, а не 5"
    );
}

/// Кривизна окружности через три точки: `4·Площадь / (a·b·c)`.
fn curvature(p0: PxPoint, p1: PxPoint, p2: PxPoint) -> f32 {
    let (a, b, c) = ((p1 - p0).length(), (p2 - p1).length(), (p2 - p0).length());
    let cross = (p1 - p0).dx * (p2 - p0).dy - (p1 - p0).dy * (p2 - p0).dx;
    2.0 * cross.abs() / (a * b * c)
}

/// Нулевая точка поля на вертикали x = CORNER_CENTER.x + offset_x (идём вниз от центра фигуры).
fn zero_above(geometry: &ShapeGeometry, offset_x: f32) -> PxPoint {
    zero_on_ray(
        geometry,
        PxPoint::new(CORNER_CENTER.x + offset_x, 0.0),
        (0.0, 1.0),
        HEIGHT,
    )
}

/// Кривизна нулевой линии около `center_offset` по трём точкам с шагом 2 px.
fn boundary_curvature(geometry: &ShapeGeometry, center_offset: f32) -> f32 {
    let step = 2.0;
    curvature(
        zero_above(geometry, center_offset - step),
        zero_above(geometry, center_offset),
        zero_above(geometry, center_offset + step),
    )
}

#[test]
fn continuous_curvature_vanishes_at_the_seam_while_arc_jumps_to_one_over_r() {
    let continuous = rect(CornerSpec::continuous(RADIUS));
    let circular = rect(CornerSpec::circular(RADIUS));
    let seam_offset = 8.0; // 0,2 радиуса от стыка с прямой стороной
    let continuous_curvature = boundary_curvature(&continuous, seam_offset);
    let circular_curvature = boundary_curvature(&circular, seam_offset);
    assert!(
        continuous_curvature < 0.1 / RADIUS,
        "у стыка кривизна непрерывного угла {continuous_curvature}, ожидали ≈ 0 (< {})",
        0.1 / RADIUS
    );
    assert!(
        (circular_curvature - 1.0 / RADIUS).abs() < 0.05 / RADIUS,
        "у дуги кривизна {circular_curvature}, ожидали 1/r = {}",
        1.0 / RADIUS
    );
    // В середине угла кривизна непрерывного скругления растёт и превышает 1/r.
    let middle_curvature = boundary_curvature(&continuous, 30.0);
    assert!(
        middle_curvature > 1.0 / RADIUS,
        "в середине угла кривизна {middle_curvature} должна быть выше 1/r"
    );
}
