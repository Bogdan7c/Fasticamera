#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Аффинное преобразование: порядок композиции, обратное, вырожденность.

use ui_draw::geometry::{PxPoint, PxVector};
use ui_draw::transform::{Affine2, AffineError};

fn close(a: PxPoint, b: PxPoint) -> bool {
    (a - b).length() < 1e-4
}

#[test]
fn then_applies_self_first_and_next_second() {
    let scale_then_move = Affine2::uniform_scale(2.0).then(Affine2::translate(10.0, 0.0));
    assert!(close(
        scale_then_move.apply_point(PxPoint::new(1.0, 1.0)),
        PxPoint::new(12.0, 2.0)
    ));
    let move_then_scale = Affine2::translate(10.0, 0.0).then(Affine2::uniform_scale(2.0));
    assert!(close(
        move_then_scale.apply_point(PxPoint::new(1.0, 1.0)),
        PxPoint::new(22.0, 2.0)
    ));
}

#[test]
fn rotation_is_clockwise_on_a_y_down_screen() {
    // Точка справа от центра при повороте на 90° по часовой уходит вниз (y вниз).
    let turned = Affine2::rotate(std::f32::consts::FRAC_PI_2).apply_point(PxPoint::new(1.0, 0.0));
    assert!(close(turned, PxPoint::new(0.0, 1.0)));
}

#[test]
fn inverse_undoes_the_transform() {
    let transform = Affine2::scale(2.0, 3.0)
        .then(Affine2::rotate(0.7))
        .then(Affine2::translate(5.0, -9.0));
    let inverse = transform.inverse().unwrap();
    let point = PxPoint::new(13.0, -4.0);
    assert!(close(inverse.apply_point(transform.apply_point(point)), point));
    assert!(close(transform.then(inverse).apply_point(point), point));
}

#[test]
fn vectors_ignore_translation_and_gradients_use_the_transposed_linear_part() {
    let transform = Affine2::scale(2.0, 4.0).then(Affine2::translate(100.0, 100.0));
    assert_eq!(transform.apply_vector(PxVector::new(1.0, 1.0)), PxVector::new(2.0, 4.0));
    // Градиент поля f(x) = x_local при x_screen = 2·x_local равен 1/2 по экрану: (L⁻¹)ᵀ(1,0) = (0,5; 0).
    let to_local = transform.inverse().unwrap();
    assert_eq!(
        to_local.apply_gradient(PxVector::new(1.0, 0.0)),
        PxVector::new(0.5, 0.0)
    );
}

#[test]
fn degenerate_and_nonfinite_matrices_have_distinct_errors() {
    assert!(matches!(
        Affine2::scale(0.0, 1.0).inverse(),
        Err(AffineError::Degenerate { .. })
    ));
    assert_eq!(
        Affine2::translate(f32::INFINITY, 0.0).inverse(),
        Err(AffineError::NotFinite)
    );
    // Сдвиг вырожденным не бывает; поворот на любой угол обратим.
    assert!(Affine2::translate(5.0, 5.0).check_invertible().is_ok());
    assert!(Affine2::rotate(2.0).check_invertible().is_ok());
}
