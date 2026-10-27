use camera_contract::capabilities::{ColorCalibrationSet, Matrix3x3, Rational, ReferenceIlluminant};

use super::{
    CameraNeutral, ColorProfileError, Convergence, DngCalibration, DngColorProfile, InvalidCameraNeutral,
    find_fixed_point_white,
};
use crate::color::chromaticity::Chromaticity;
use crate::color::matrix::Matrix3;
use crate::color::temperature::Kelvin;

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

fn assert_vector_close(actual: [f64; 3], expected: [f64; 3], tolerance: f64) {
    for index in 0..3 {
        assert!(
            (actual[index] - expected[index]).abs() <= tolerance,
            "{actual:?} против {expected:?}"
        );
    }
}

// --- Калибровка основной камеры OnePlus 11 (паспорт C7). ---

fn matrix_from_row_major(values: [f64; 9]) -> Matrix3 {
    Matrix3::from_rows([
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ])
}

const ONEPLUS_CM_D65: [f64; 9] = [
    0.9848, -0.3684, -0.1011, -0.2682, 1.1295, 0.1152, 0.0588, 0.0403, 0.5348,
];
const ONEPLUS_CM_STD_A: [f64; 9] = [1.9084, -1.3277, 0.1113, -0.2221, 1.1645, 0.2236, 0.141, -0.0884, 0.8508];
const ONEPLUS_FORWARD: [f64; 9] = [
    0.4375, 0.3828125, 0.140625, 0.21875, 0.71875, 0.0625, 0.015625, 0.09375, 0.7109375,
];
const ONEPLUS_NEUTRAL: [f64; 3] = [0.515625, 1.0, 0.4990234375];

fn oneplus_d65(with_forward: bool) -> DngCalibration {
    DngCalibration {
        temperature: Kelvin(6500.0),
        color_matrix: matrix_from_row_major(ONEPLUS_CM_D65),
        forward_matrix: with_forward.then(|| matrix_from_row_major(ONEPLUS_FORWARD)),
        camera_calibration: Matrix3::diagonal([1.0078125, 1.0, 1.0078125]),
    }
}

fn oneplus_std_a(with_forward: bool) -> DngCalibration {
    DngCalibration {
        temperature: Kelvin(2850.0),
        color_matrix: matrix_from_row_major(ONEPLUS_CM_STD_A),
        forward_matrix: with_forward.then(|| matrix_from_row_major(ONEPLUS_FORWARD)),
        camera_calibration: Matrix3::diagonal([0.9921875, 1.0, 0.9765625]),
    }
}

/// Порядок как у телефона: первая калибровка D65 (выше по температуре).
fn oneplus_profile(with_forward: bool) -> DngColorProfile {
    DngColorProfile::new(oneplus_d65(with_forward), Some(oneplus_std_a(with_forward)), [1.0; 3]).unwrap()
}

fn oneplus_neutral() -> CameraNeutral {
    CameraNeutral::new(ONEPLUS_NEUTRAL).unwrap()
}

// --- CameraNeutral ---

#[test]
fn camera_neutral_rejects_non_positive_and_non_finite_values() {
    for bad in [
        [0.0, 1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, f64::NAN],
        [f64::INFINITY, 1.0, 1.0],
        // Положительно и конечно, но усиление 1/1e-40 в f32 бесконечно — баланс дал бы NaN.
        [1e-40, 1.0, 1.0],
    ] {
        assert!(
            matches!(CameraNeutral::new(bad), Err(InvalidCameraNeutral { .. })),
            "{bad:?}"
        );
    }
    assert_eq!(CameraNeutral::new([0.5, 1.0, 0.6]).unwrap().values(), [0.5, 1.0, 0.6]);
}

// --- Интерполяция ---

#[test]
fn interpolation_is_linear_in_mired_and_clamped() {
    let profile = oneplus_profile(false);
    let d65 = matrix_from_row_major(ONEPLUS_CM_D65);
    let std_a = matrix_from_row_major(ONEPLUS_CM_STD_A);
    // Выше 6500 К и ниже 2850 К — крайние калибровки без экстраполяции.
    assert_eq!(profile.interpolate(Kelvin(9000.0)).color_matrix, d65);
    assert_eq!(profile.interpolate(Kelvin(2000.0)).color_matrix, std_a);
    // Середина по майредам: (1e6/6500 + 1e6/2850)/2 майред — ровно среднее матриц, а не по кельвинам.
    let mid_mired = (Kelvin(6500.0).mired() + Kelvin(2850.0).mired()) * 0.5;
    let middle = profile.interpolate(Kelvin(1.0e6 / mid_mired)).color_matrix;
    assert_matrix_close(&middle, &d65.blend(&std_a, 0.5), 1e-12);
    // Среднее по кельвинам (4675 К) дало бы другой вес — тест отличает майреды от кельвинов.
    let by_kelvin = profile.interpolate(Kelvin(4675.0)).color_matrix;
    assert!((by_kelvin.rows[0][0] - middle.rows[0][0]).abs() > 0.01);
}

#[test]
fn interpolation_does_not_depend_on_calibration_order() {
    let phone_order = oneplus_profile(true);
    let swapped = DngColorProfile::new(oneplus_std_a(true), Some(oneplus_d65(true)), [1.0; 3]).unwrap();
    let a = phone_order.camera_to_xyz_d50(oneplus_neutral()).unwrap();
    let b = swapped.camera_to_xyz_d50(oneplus_neutral()).unwrap();
    assert_matrix_close(&a.matrix, &b.matrix, 1e-12);
    assert!((a.white.white.x - b.white.white.x).abs() < 1e-12);
}

#[test]
fn single_calibration_is_not_interpolated() {
    let profile = DngColorProfile::new(oneplus_d65(true), None, [1.0; 3]).unwrap();
    let at_any_temperature = profile.interpolate(Kelvin(3000.0));
    assert_eq!(at_any_temperature.color_matrix, matrix_from_row_major(ONEPLUS_CM_D65));
    assert_eq!(
        at_any_temperature.camera_calibration,
        Matrix3::diagonal([1.0078125, 1.0, 1.0078125])
    );
}

// --- Поиск белой точки: сверка с colour-hdri ---

/// Эталон: colour-hdri 0.2.6 `camera_neutral_to_xy` с калибровками по возрастанию температуры
/// (A 2850 первой — colour-hdri требует `CCT_1 < CCT_2`; у нас D65 первая, результат от порядка не
/// зависит, это проверено тестом выше), venv `~/.cache/fasticamera/isp-reference-venv`,
/// `python -I -W ignore`, нейтраль и матрицы C7 OnePlus 11:
/// xy = (0.363269853831, 0.383835531643), CCT по `uv_to_CCT_Robertson1968` = 4532.0951 К.
/// Допуск 1e-6 по xy и 0,1 К: colour-hdri стартует с (1/3, 1/3) и останавливается по `|Σ Δ| ≤ ε`,
/// мы стартуем с D50 и по `|Δx| + |Δy| < 1e-7`; оба останавливаются вблизи одной неподвижной точки
/// (отображение сжимающее), положение остановки отличается на порядок порога.
#[test]
fn white_for_oneplus_neutral_matches_colour_hdri() {
    let estimate = oneplus_profile(true).white_for_neutral(oneplus_neutral()).unwrap();
    assert!((estimate.white.x - 0.363269853831).abs() < 1e-6, "{estimate:?}");
    assert!((estimate.white.y - 0.383835531643).abs() < 1e-6, "{estimate:?}");
    assert!((estimate.temperature.0 - 4532.095144).abs() < 0.1, "{estimate:?}");
    assert!(matches!(estimate.convergence, Convergence::Converged { passes } if (1..30).contains(&passes)));
}

/// Эталон: colour-hdri `matrix_camera_space_to_XYZ` с теми же данными (ветка ForwardMatrix).
/// Допуск 1e-5: матрица зависит от T только через CC(T) (FM1 = FM2), CC почти не меняется на
/// разнице T порядка 0,01 К между нашей и их белой точкой.
#[test]
fn camera_to_xyz_with_forward_matrix_matches_colour_hdri() {
    let result = oneplus_profile(true).camera_to_xyz_d50(oneplus_neutral()).unwrap();
    let expected = Matrix3::from_rows([
        [0.8484848484848488, 0.3828125, 0.2818003913894326],
        [0.4242424242424244, 0.71875, 0.12524461839530338],
        [0.030303030303030314, 0.09375, 1.4246575342465757],
    ]);
    // colour-hdri не нормирует FM, а SDK (и мы) нормируем: `diag(D50) · diag(FM·1)⁻¹` слева.
    // FM·(1,1,1) у округлённой FM камеры = (0.9609375, 1, 0.8203125).
    let sdk_normalization = Matrix3::diagonal(Chromaticity::D50.to_xyz())
        .mul(&Matrix3::diagonal([0.9609375, 1.0, 0.8203125]).inverse().unwrap());
    assert_matrix_close(&result.matrix, &sdk_normalization.mul(&expected), 1e-5);
    // «Серое остаётся серым»: после нормировки нейтраль уходит ровно в D50.
    let gray = result.matrix.apply(ONEPLUS_NEUTRAL);
    assert_vector_close(gray, Chromaticity::D50.to_xyz(), 1e-9);
}

/// Ветка без ForwardMatrix. Эталон colour-hdri (identity FM) = `reference`; он не нормирует
/// матрицу, мы делим её на Y образа нейтрали (0.997561192699 у colour-hdri), поэтому сравниваем
/// `ours · k` с `reference`. Допуск 1e-5 — как выше.
#[test]
fn camera_to_xyz_without_forward_matches_colour_hdri_up_to_luminance_scale() {
    let result = oneplus_profile(false).camera_to_xyz_d50(oneplus_neutral()).unwrap();
    let reference = Matrix3::from_rows([
        [0.8643302502690197, 0.5231041727313952, -0.013687759770811735],
        [0.20573789089074768, 0.9934893264506078, -0.20442273062946859],
        [-0.14333337204661303, -0.0973741960710391, 1.9926374648777638],
    ]);
    let colour_hdri_neutral_luminance = 0.997561192699;
    let mut rescaled = result.matrix;
    for value in rescaled.rows.iter_mut().flatten() {
        *value *= colour_hdri_neutral_luminance;
    }
    assert_matrix_close(&rescaled, &reference, 1e-5);
    // Главное свойство нормировки: нейтраль даёт D50 с Y = 1.
    assert_vector_close(result.matrix.apply(ONEPLUS_NEUTRAL), Chromaticity::D50.to_xyz(), 1e-9);
}

/// FM, нормированная так, чтобы (1,1,1) давало ровно D50: тогда свойство «серое — серое» точное.
#[test]
fn gray_maps_to_exact_d50_for_normalized_forward_matrix_and_any_neutral() {
    let d50 = Chromaticity::D50.to_xyz();
    let raw = matrix_from_row_major(ONEPLUS_FORWARD);
    let ones_image = raw.apply([1.0; 3]);
    let mut rows = raw.rows;
    for (row, scale) in rows
        .iter_mut()
        .zip(d50.iter().zip(ones_image).map(|(want, have)| want / have))
    {
        for value in row.iter_mut() {
            *value *= scale;
        }
    }
    let normalized = Matrix3 { rows };
    let calibration = |base: DngCalibration| DngCalibration {
        forward_matrix: Some(normalized),
        ..base
    };
    let profile = DngColorProfile::new(
        calibration(oneplus_d65(true)),
        Some(calibration(oneplus_std_a(true))),
        [1.0; 3],
    )
    .unwrap();
    for values in [ONEPLUS_NEUTRAL, [0.8, 1.0, 0.45], [0.42, 1.0, 0.7]] {
        let neutral = CameraNeutral::new(values).unwrap();
        let result = profile.camera_to_xyz_d50(neutral).unwrap();
        assert_vector_close(result.matrix.apply(values), d50, 1e-9);
    }
}

#[test]
fn gray_maps_to_exact_d50_without_forward_matrix_for_any_neutral() {
    let profile = oneplus_profile(false);
    for values in [ONEPLUS_NEUTRAL, [0.8, 1.0, 0.45], [0.42, 1.0, 0.7]] {
        let result = profile.camera_to_xyz_d50(CameraNeutral::new(values).unwrap()).unwrap();
        assert_vector_close(result.matrix.apply(values), Chromaticity::D50.to_xyz(), 1e-9);
    }
}

/// Матрица ForwardMatrix-пути обратно пропорциональна масштабу нейтрали (`D = diag(RN)⁻¹`),
/// поэтому образ самой нейтрали от масштаба не зависит — это и есть «серое остаётся серым».
#[test]
fn neutral_scale_cancels_out_in_the_gray_image() {
    let profile = oneplus_profile(true);
    let base = profile.camera_to_xyz_d50(oneplus_neutral()).unwrap();
    let scaled_values = ONEPLUS_NEUTRAL.map(|value| value * 3.0);
    let scaled = profile
        .camera_to_xyz_d50(CameraNeutral::new(scaled_values).unwrap())
        .unwrap();
    assert_matrix_close(&scaled.matrix.mul(&Matrix3::diagonal([3.0; 3])), &base.matrix, 1e-9);
    assert_vector_close(
        scaled.matrix.apply(scaled_values),
        base.matrix.apply(ONEPLUS_NEUTRAL),
        1e-9,
    );
}

// --- Поиск неподвижной точки ---

#[test]
fn oscillation_is_averaged_after_thirty_passes() {
    let low = Chromaticity { x: 0.30, y: 0.30 };
    let high = Chromaticity { x: 0.40, y: 0.40 };
    let mut calls = 0;
    let (white, convergence) = find_fixed_point_white(low, |guess| {
        calls += 1;
        Ok(if guess == low { high } else { low })
    })
    .unwrap();
    assert_eq!(calls, 30);
    assert_eq!(convergence, Convergence::AveragedAfterOscillation);
    assert!(
        (white.x - 0.35).abs() < 1e-12 && (white.y - 0.35).abs() < 1e-12,
        "{white:?}"
    );
}

#[test]
fn fixed_point_reports_number_of_passes() {
    let target = Chromaticity { x: 0.33, y: 0.34 };
    // Сжимающее отображение к цели: каждый проход приближает вдвое.
    let (white, convergence) = find_fixed_point_white(Chromaticity::D50, |guess| {
        Ok(Chromaticity {
            x: (guess.x + target.x) * 0.5,
            y: (guess.y + target.y) * 0.5,
        })
    })
    .unwrap();
    assert!((white.x - target.x).abs() < 1e-6);
    assert!(matches!(convergence, Convergence::Converged { passes } if passes > 5 && passes < 30));
}

#[test]
fn error_from_a_pass_is_propagated() {
    let result = find_fixed_point_white(Chromaticity::D50, |_| Err(ColorProfileError::UndefinedWhitePoint));
    assert_eq!(result.unwrap_err(), ColorProfileError::UndefinedWhitePoint);
}

// --- Сборка профиля из HAL ---

fn rational_matrix(values: [f64; 9]) -> Matrix3x3 {
    // Знаменатель 2^20: все значения FM представимы точно, десятичные CM — с ошибкой < 1e-6.
    const DENOMINATOR: i32 = 1 << 20;
    Matrix3x3(values.map(|value| Rational {
        numerator: (value * f64::from(DENOMINATOR)).round() as i32,
        denominator: DENOMINATOR,
    }))
}

fn oneplus_sets() -> [ColorCalibrationSet; 2] {
    [
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::D65,
            color_matrix: rational_matrix(ONEPLUS_CM_D65),
            forward_matrix: rational_matrix(ONEPLUS_FORWARD),
            calibration_transform: rational_matrix([1.0078125, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0078125]),
        },
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::StandardA,
            color_matrix: rational_matrix(ONEPLUS_CM_STD_A),
            forward_matrix: rational_matrix(ONEPLUS_FORWARD),
            calibration_transform: rational_matrix([0.9921875, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.9765625]),
        },
    ]
}

#[test]
fn calibration_sets_from_hal_give_the_same_result_as_direct_profile() {
    let sets = oneplus_sets();
    let from_hal = DngColorProfile::from_calibration_sets(&sets).unwrap();
    let direct = oneplus_profile(true);
    let a = from_hal.camera_to_xyz_d50(oneplus_neutral()).unwrap();
    let b = direct.camera_to_xyz_d50(oneplus_neutral()).unwrap();
    assert_matrix_close(&a.matrix, &b.matrix, 1e-5);
    assert!((a.white.temperature.0 - b.white.temperature.0).abs() < 1.0);
}

#[test]
fn zero_forward_matrix_means_absent() {
    let mut sets = oneplus_sets();
    for set in &mut sets {
        set.forward_matrix = rational_matrix([0.0; 9]);
    }
    let from_hal = DngColorProfile::from_calibration_sets(&sets).unwrap();
    let a = from_hal.camera_to_xyz_d50(oneplus_neutral()).unwrap();
    let b = oneplus_profile(false).camera_to_xyz_d50(oneplus_neutral()).unwrap();
    assert_matrix_close(&a.matrix, &b.matrix, 1e-5);
}

#[test]
fn camera_contract_fixture_calibration_gives_identity_behaviour() {
    use camera_contract::capabilities::RawSupport;
    let camera = camera_contract::fixtures::main_wide_camera();
    let RawSupport::Supported(info) = camera.capabilities.raw else {
        panic!("в образце камеры должен быть RAW");
    };
    let profile = DngColorProfile::from_calibration_sets(&info.color_calibration).unwrap();
    // Все матрицы единичные: XYZtoCamera = I, ForwardMatrix = I. После нормировки SDK
    // FM = diag(XYZ D50), поэтому матрица = diag(XYZ D50) · diag(1/нейтраль).
    let values = [0.5, 1.0, 0.8];
    let result = profile.camera_to_xyz_d50(CameraNeutral::new(values).unwrap()).unwrap();
    let d50 = Chromaticity::D50.to_xyz();
    assert_matrix_close(
        &result.matrix,
        &Matrix3::diagonal([2.0 * d50[0], d50[1], 1.25 * d50[2]]),
        1e-9,
    );
    assert!((result.white.white.x - 0.5 / 2.3).abs() < 1e-6, "{:?}", result.white);
}

#[test]
fn profile_construction_errors_are_distinguished() {
    assert_eq!(
        DngColorProfile::from_calibration_sets(&[]).unwrap_err(),
        ColorProfileError::NoCalibration
    );
    let sets = oneplus_sets();
    let three = [sets[0], sets[1], sets[0]];
    assert_eq!(
        DngColorProfile::from_calibration_sets(&three).unwrap_err(),
        ColorProfileError::TooManyCalibrations { count: 3 }
    );

    let mut broken = sets;
    broken[1].color_matrix.0[4] = Rational {
        numerator: 1,
        denominator: 0,
    };
    assert!(matches!(
        DngColorProfile::from_calibration_sets(&broken).unwrap_err(),
        ColorProfileError::BrokenMatrixElement {
            set_index: 1,
            matrix: "ColorMatrix",
            ..
        }
    ));

    let mut unknown = sets;
    unknown[0].illuminant = ReferenceIlluminant::Other(0);
    assert!(matches!(
        DngColorProfile::from_calibration_sets(&unknown).unwrap_err(),
        ColorProfileError::UnknownIlluminant { set_index: 0, .. }
    ));

    assert_eq!(
        DngColorProfile::new(oneplus_d65(true), Some(oneplus_std_a(false)), [1.0; 3]).unwrap_err(),
        ColorProfileError::MixedForwardMatrices
    );
    assert!(matches!(
        DngColorProfile::new(oneplus_d65(true), None, [1.0, 0.0, 1.0]).unwrap_err(),
        ColorProfileError::InvalidAnalogBalance { .. }
    ));
}

#[test]
fn singular_color_matrix_is_reported_not_nan() {
    let broken = DngCalibration {
        color_matrix: Matrix3::from_rows([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]]),
        ..oneplus_d65(true)
    };
    let profile = DngColorProfile::new(broken, None, [1.0; 3]).unwrap();
    assert!(matches!(
        profile.white_for_neutral(oneplus_neutral()).unwrap_err(),
        ColorProfileError::Singular {
            context: "XYZtoCamera",
            ..
        }
    ));
}
