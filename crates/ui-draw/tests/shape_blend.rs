#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Сглаживание края и слияние форм.

use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::shape::{ShapeGeometry, coverage, signed_distance, smooth_union, smooth_union_with_weights};

#[test]
fn coverage_is_half_on_the_edge_and_saturates_on_both_sides() {
    assert!((coverage(0.0, 1.0) - 0.5).abs() < 1e-6);
    assert_eq!(coverage(-2.0, 1.0), 1.0);
    assert_eq!(coverage(2.0, 1.0), 0.0);
    let mut previous = 1.0_f32;
    for step in 0..=20 {
        let value = coverage(-1.0 + step as f32 * 0.1, 2.0);
        assert!(value <= previous + 1e-6, "покрытие должно убывать с расстоянием");
        previous = value;
    }
    let partial = coverage(0.25, 1.0);
    assert!(partial > 0.0 && partial < 0.5, "{partial}");
}

#[test]
fn coverage_without_antialiasing_is_a_hard_step() {
    assert_eq!(coverage(-0.001, 0.0), 1.0);
    assert_eq!(coverage(0.001, 0.0), 0.0);
}

#[test]
fn smooth_union_with_zero_radius_is_plain_min() {
    assert_eq!(smooth_union(3.0, -2.0, 0.0), -2.0);
    assert_eq!(smooth_union(1.0, 1.5, 0.0), 1.0);
    let weights = smooth_union_with_weights(3.0, -2.0, 0.0);
    assert_eq!((weights.weight_first, weights.weight_second), (0.0, 1.0));
}

#[test]
fn smooth_union_deepens_the_field_only_within_the_merge_radius() {
    // Равные поля 2 и радиус 8: провал k/4 = 2 → ровно 0.
    assert!(smooth_union(2.0, 2.0, 8.0).abs() < 1e-6);
    // Разница больше радиуса — обычный min.
    assert_eq!(smooth_union(1.0, 20.0, 8.0), 1.0);
    let merged = smooth_union(3.0, 4.0, 8.0);
    assert!((3.0 - 8.0 / 4.0..3.0).contains(&merged), "{merged}");
}

#[test]
fn smooth_union_weights_sum_to_one_and_match_numerical_derivative() {
    let (first, second, radius) = (3.0_f32, 4.5_f32, 8.0_f32);
    let weights = smooth_union_with_weights(first, second, radius);
    assert!((weights.weight_first + weights.weight_second - 1.0).abs() < 1e-6);
    let step = 1e-2;
    let numeric_first =
        (smooth_union(first + step, second, radius) - smooth_union(first - step, second, radius)) / (2.0 * step);
    assert!(
        (weights.weight_first - numeric_first).abs() < 1e-2,
        "{} против {numeric_first}",
        weights.weight_first
    );
}

/// Поле двух капсул 100×40 с зазором `gap` между краями.
fn two_capsules_field(point: PxPoint, gap: f32, merge_radius: f32) -> f32 {
    let capsule = ShapeGeometry::Capsule {
        size: PxSize::new(100.0, 40.0),
    };
    let offset = 50.0 + gap / 2.0;
    let left = signed_distance(&capsule, PxPoint::new(point.x + offset, point.y));
    let right = signed_distance(&capsule, PxPoint::new(point.x - offset, point.y));
    smooth_union(left, right, merge_radius)
}

#[test]
fn close_capsules_merge_and_distant_ones_do_not() {
    let middle = PxPoint::ORIGIN;
    // Зазор 6 px, радиус слияния 24 > 2·зазор: между ними поле отрицательно (формы слились).
    assert!(two_capsules_field(middle, 6.0, 24.0) < 0.0);
    // Тот же зазор без слияния: между капсулами пустота.
    assert!((two_capsules_field(middle, 6.0, 0.0) - 3.0).abs() < 1e-4);
    // Далёкие капсулы (зазор 60) даже с большим радиусом не сливаются.
    assert!(two_capsules_field(middle, 60.0, 24.0) > 0.0);
}
