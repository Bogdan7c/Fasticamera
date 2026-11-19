#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Знаковое расстояние, градиент и исправленное расстояние для всех фигур.

use ui_draw::geometry::{PxPoint, PxSize, PxVector};
use ui_draw::shape::{
    CornerCurve, CornerSpec, ShapeError, ShapeGeometry, corrected_distance, distance_gradient, signed_distance,
};

fn rounded(corner: CornerSpec) -> ShapeGeometry {
    ShapeGeometry::RoundedRect {
        size: PxSize::new(200.0, 120.0),
        corner,
    }
}

fn capsule() -> ShapeGeometry {
    ShapeGeometry::Capsule {
        size: PxSize::new(200.0, 80.0),
    }
}

fn circle() -> ShapeGeometry {
    ShapeGeometry::Circle { diameter: 100.0 }
}

#[test]
fn boundary_points_are_zero_and_sign_flips_across_it() {
    // (имя, фигура, точка на границе, единичное направление наружу)
    let cases: [(&str, ShapeGeometry, PxPoint, PxVector); 4] = [
        (
            "прямоугольник, правая сторона",
            rounded(CornerSpec::continuous(30.0)),
            PxPoint::new(100.0, 0.0),
            PxVector::new(1.0, 0.0),
        ),
        (
            "капсула, кончик",
            capsule(),
            PxPoint::new(100.0, 0.0),
            PxVector::new(1.0, 0.0),
        ),
        (
            "капсула, верх",
            capsule(),
            PxPoint::new(0.0, -40.0),
            PxVector::new(0.0, -1.0),
        ),
        ("круг", circle(), PxPoint::new(0.0, 50.0), PxVector::new(0.0, 1.0)),
    ];
    for (name, shape, boundary, outward) in cases {
        assert!(
            signed_distance(&shape, boundary).abs() < 1e-4,
            "{name}: на границе не ноль"
        );
        let inside = boundary + outward * -5.0;
        let outside = boundary + outward * 5.0;
        assert!(
            (signed_distance(&shape, inside) + 5.0).abs() < 1e-3,
            "{name}: внутри ожидали −5"
        );
        assert!(
            (signed_distance(&shape, outside) - 5.0).abs() < 1e-3,
            "{name}: снаружи ожидали +5"
        );
    }
}

#[test]
fn centers_are_deepest_points() {
    assert!((signed_distance(&circle(), PxPoint::ORIGIN) + 50.0).abs() < 1e-4);
    assert!((signed_distance(&capsule(), PxPoint::ORIGIN) + 40.0).abs() < 1e-4);
    assert!((signed_distance(&rounded(CornerSpec::circular(30.0)), PxPoint::ORIGIN) + 60.0).abs() < 1e-4);
}

#[test]
fn corner_radius_is_clamped_to_half_of_the_smaller_side() {
    // Радиус 500 на 200×120 ограничен до 60 — получается капсула с теми же значениями поля.
    let huge = rounded(CornerSpec::circular(500.0));
    let as_capsule = ShapeGeometry::Capsule {
        size: PxSize::new(200.0, 120.0),
    };
    for point in [
        PxPoint::new(95.0, 10.0),
        PxPoint::new(0.0, 70.0),
        PxPoint::new(-40.0, -30.0),
        PxPoint::new(130.0, 20.0),
    ] {
        let (a, b) = (signed_distance(&huge, point), signed_distance(&as_capsule, point));
        assert!((a - b).abs() < 1e-4, "{point:?}: {a} против {b}");
    }
}

#[test]
fn gradient_is_unit_away_from_the_corner_and_shorter_inside_the_corner() {
    let shape = rounded(CornerSpec::continuous(40.0));
    for point in [
        PxPoint::new(110.0, 10.0),
        PxPoint::new(20.0, 70.0),
        PxPoint::new(10.0, 0.0),
        PxPoint::new(-103.0, -20.0),
    ] {
        let gradient = distance_gradient(&shape, point);
        assert!(
            (gradient.length() - 1.0).abs() < 1e-5,
            "{point:?}: |∇d| = {}",
            gradient.length()
        );
    }
    // На диагонали угла с L5-нормой |∇d| = 2^(1/5 − 1/2) ≈ 0,812 для любого расстояния.
    let diagonal_point = PxPoint::new(60.0 + 35.0, 20.0 + 35.0);
    let length = distance_gradient(&shape, diagonal_point).length();
    assert!(
        (length - 2.0_f32.powf(0.2 - 0.5)).abs() < 1e-4,
        "|∇d| на диагонали {length}"
    );
    // Дуга окружности: везде единичный.
    let arc = rounded(CornerSpec::circular(40.0));
    assert!((distance_gradient(&arc, diagonal_point).length() - 1.0).abs() < 1e-5);
}

#[test]
fn gradient_matches_numerical_derivative_everywhere() {
    let shapes = [
        rounded(CornerSpec::continuous(40.0)),
        rounded(CornerSpec::circular(25.0)),
        capsule(),
        circle(),
    ];
    let points = [
        PxPoint::new(95.0, 50.0),
        PxPoint::new(80.0, 40.0),
        PxPoint::new(-97.0, -33.0),
        PxPoint::new(50.0, 20.0),
        PxPoint::new(30.0, -70.0),
        PxPoint::new(-60.0, 10.0),
    ];
    let step = 0.05;
    for shape in shapes {
        for point in points {
            let analytic = distance_gradient(&shape, point);
            let along = |dx: f32, dy: f32| signed_distance(&shape, PxPoint::new(point.x + dx, point.y + dy));
            let numeric = PxVector::new(
                (along(step, 0.0) - along(-step, 0.0)) / (2.0 * step),
                (along(0.0, step) - along(0.0, -step)) / (2.0 * step),
            );
            assert!(
                (analytic - numeric).length() < 2e-2,
                "{shape:?} в {point:?}: аналитический {analytic:?}, численный {numeric:?}"
            );
        }
    }
}

#[test]
fn corrected_distance_approximates_euclidean_distance_in_the_corner() {
    let shape = rounded(CornerSpec::continuous(40.0));
    // Граница на диагонали угла: qx = qy = r/2^(1/5); нормаль по симметрии (1,1)/√2.
    let on_boundary = 40.0 / 2.0_f32.powf(0.2);
    let normal = std::f32::consts::FRAC_1_SQRT_2;
    let boundary = PxPoint::new(60.0 + on_boundary, 20.0 + on_boundary);
    let probe = PxPoint::new(boundary.x + normal, boundary.y + normal); // ровно 1 px наружу
    let raw = signed_distance(&shape, probe);
    let corrected = corrected_distance(&shape, probe);
    assert!(
        (raw - 1.0).abs() > 0.1,
        "сырое L5-расстояние {raw} должно заметно врать (≈ 0,79)"
    );
    assert!(
        (corrected.distance - 1.0).abs() < 0.03,
        "исправленное {} должно быть ≈ 1 px",
        corrected.distance
    );
    assert!((corrected.normal_out.length() - 1.0).abs() < 1e-5, "нормаль единичная");
    assert!((corrected.normal_out.dx - normal).abs() < 1e-3 && (corrected.normal_out.dy - normal).abs() < 1e-3);
}

#[test]
fn circle_center_gradient_is_finite_and_validation_rejects_bad_shapes() {
    // В самом центре круга направления нет; поле всё равно отдаёт конечную единичную ось (не NaN).
    let center_normal = corrected_distance(&circle(), PxPoint::ORIGIN).normal_out;
    assert!(center_normal.is_finite() && (center_normal.length() - 1.0).abs() < 1e-6);
    // Рядом с центром нормаль радиальна.
    let off_center = corrected_distance(&circle(), PxPoint::new(3.0, 4.0)).normal_out;
    assert!((off_center.dx - 0.6).abs() < 1e-5 && (off_center.dy - 0.8).abs() < 1e-5);
    assert_eq!(
        ShapeGeometry::Circle { diameter: -1.0 }.validate(),
        Err(ShapeError::InvalidDimension {
            field: "width",
            value: -1.0
        })
    );
    let bad_exponent = ShapeGeometry::RoundedRect {
        size: PxSize::new(10.0, 10.0),
        corner: CornerSpec {
            radius_px: 2.0,
            curve: CornerCurve::Continuous { exponent: 1.5 },
        },
    };
    assert_eq!(bad_exponent.validate(), Err(ShapeError::InvalidExponent(1.5)));
    assert!(rounded(CornerSpec::continuous(30.0)).validate().is_ok());
}
