use camera_contract::capabilities::{Matrix3x3, Rational};

use super::{BrokenRational, Matrix3};

fn assert_matrix_close(actual: &Matrix3, expected: &Matrix3, tolerance: f64) {
    for row in 0..3 {
        for column in 0..3 {
            let difference = (actual.rows[row][column] - expected.rows[row][column]).abs();
            assert!(
                difference <= tolerance,
                "[{row}][{column}]: {actual:?} против {expected:?}"
            );
        }
    }
}

/// ColorMatrix1 основной камеры OnePlus 11 (паспорт C7 «лампа»).
fn oneplus_color_matrix_d65() -> Matrix3 {
    Matrix3::from_rows([
        [0.9848, -0.3684, -0.1011],
        [-0.2682, 1.1295, 0.1152],
        [0.0588, 0.0403, 0.5348],
    ])
}

#[test]
fn inverse_times_matrix_is_identity() {
    let matrix = oneplus_color_matrix_d65();
    let inverse = matrix.inverse().unwrap();
    assert_matrix_close(&matrix.mul(&inverse), &Matrix3::IDENTITY, 1e-12);
    assert_matrix_close(&inverse.mul(&matrix), &Matrix3::IDENTITY, 1e-12);
}

#[test]
fn singular_matrix_is_an_error_not_infinity() {
    let singular = Matrix3::from_rows([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]]);
    let error = singular.inverse().unwrap_err();
    assert!(error.determinant.abs() < 1e-12);
}

#[test]
fn multiplication_order_applies_right_operand_first() {
    let scale = Matrix3::diagonal([2.0, 3.0, 4.0]);
    let swap_red_green = Matrix3::from_rows([[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    // Сначала масштаб, потом перестановка: [1,1,1] → [2,3,4] → [3,2,4].
    assert_eq!(swap_red_green.mul(&scale).apply([1.0, 1.0, 1.0]), [3.0, 2.0, 4.0]);
}

#[test]
fn blend_weight_one_returns_first_and_zero_returns_second() {
    let first = Matrix3::diagonal([1.0, 1.0, 1.0]);
    let second = Matrix3::diagonal([3.0, 5.0, 7.0]);
    assert_eq!(first.blend(&second, 1.0), first);
    assert_eq!(first.blend(&second, 0.0), second);
    assert_eq!(first.blend(&second, 0.5), Matrix3::diagonal([2.0, 3.0, 4.0]));
}

#[test]
fn rationals_are_read_in_row_order_and_zero_denominator_is_reported() {
    let mut elements = [Rational {
        numerator: 0,
        denominator: 1,
    }; 9];
    elements[1] = Rational {
        numerator: 3,
        denominator: 4,
    };
    let matrix = Matrix3::from_rationals(&Matrix3x3(elements)).unwrap();
    assert_eq!(matrix.rows[0][1], 0.75);
    elements[5] = Rational {
        numerator: 1,
        denominator: 0,
    };
    assert_eq!(
        Matrix3::from_rationals(&Matrix3x3(elements)),
        Err(BrokenRational { element: 5 })
    );
}

#[test]
fn pixel_matrix_matches_f64_application() {
    let matrix = oneplus_color_matrix_d65();
    let pixel = [0.25_f32, 0.5, 0.75];
    let narrow = matrix.to_f32().apply(pixel);
    let wide = matrix.apply(pixel.map(f64::from));
    for channel in 0..3 {
        assert!((f64::from(narrow[channel]) - wide[channel]).abs() < 1e-6);
    }
}
