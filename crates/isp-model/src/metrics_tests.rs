use super::{Lab, MetricsError, Psnr, delta_e_2000, display_linear_to_lab, max_abs_difference, psnr, xyz_to_lab};
use crate::color::chromaticity::Chromaticity;
use crate::image::DisplayGamut;

/// Тестовые данные Sharma, Wu, Dalal (2005), «The CIEDE2000 color-difference formula:
/// implementation notes, supplementary test data, and mathematical observations», таблица 1:
/// 34 пары (эталон, образец, ΔE00). Числа взяты из `colour/difference/tests/test_delta_e.py`
/// (colour-science 0.4.7), где они перенесены из статьи; в colour пара № 14 закомментирована из-за
/// платформенной разницы atan2, здесь она возвращена (ΔE00 = 4,8045, как в статье).
const SHARMA_PAIRS: [(Lab, Lab, f64); 34] = [
    (
        Lab {
            l: 50.0000,
            a: 2.6772,
            b: -79.7751,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -82.7485,
        },
        2.0425,
    ),
    (
        Lab {
            l: 50.0000,
            a: 3.1571,
            b: -77.2803,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -82.7485,
        },
        2.8615,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.8361,
            b: -74.0200,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -82.7485,
        },
        3.4412,
    ),
    (
        Lab {
            l: 50.0000,
            a: -1.3802,
            b: -84.2814,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -82.7485,
        },
        1.0000,
    ),
    (
        Lab {
            l: 50.0000,
            a: -1.1848,
            b: -84.8006,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -82.7485,
        },
        1.0000,
    ),
    (
        Lab {
            l: 50.0000,
            a: -0.9009,
            b: -85.5211,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -82.7485,
        },
        1.0000,
    ),
    (
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: 0.0000,
        },
        Lab {
            l: 50.0000,
            a: -1.0000,
            b: 2.0000,
        },
        2.3669,
    ),
    (
        Lab {
            l: 50.0000,
            a: -1.0000,
            b: 2.0000,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: 0.0000,
        },
        2.3669,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.4900,
            b: -0.0010,
        },
        Lab {
            l: 50.0000,
            a: -2.4900,
            b: 0.0009,
        },
        7.1792,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.4900,
            b: -0.0010,
        },
        Lab {
            l: 50.0000,
            a: -2.4900,
            b: 0.0010,
        },
        7.1792,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.4900,
            b: -0.0010,
        },
        Lab {
            l: 50.0000,
            a: -2.4900,
            b: 0.0011,
        },
        7.2195,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.4900,
            b: -0.0010,
        },
        Lab {
            l: 50.0000,
            a: -2.4900,
            b: 0.0012,
        },
        7.2195,
    ),
    (
        Lab {
            l: 50.0000,
            a: -0.0010,
            b: 2.4900,
        },
        Lab {
            l: 50.0000,
            a: 0.0009,
            b: -2.4900,
        },
        4.8045,
    ),
    (
        Lab {
            l: 50.0000,
            a: -0.0010,
            b: 2.4900,
        },
        Lab {
            l: 50.0000,
            a: 0.0010,
            b: -2.4900,
        },
        4.8045,
    ),
    (
        Lab {
            l: 50.0000,
            a: -0.0010,
            b: 2.4900,
        },
        Lab {
            l: 50.0000,
            a: 0.0011,
            b: -2.4900,
        },
        4.7461,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 50.0000,
            a: 0.0000,
            b: -2.5000,
        },
        4.3065,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 73.0000,
            a: 25.0000,
            b: -18.0000,
        },
        27.1492,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 61.0000,
            a: -5.0000,
            b: 29.0000,
        },
        22.8977,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 56.0000,
            a: -27.0000,
            b: -3.0000,
        },
        31.9030,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 58.0000,
            a: 24.0000,
            b: 15.0000,
        },
        19.4535,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 50.0000,
            a: 3.1736,
            b: 0.5854,
        },
        1.0000,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 50.0000,
            a: 3.2972,
            b: 0.0000,
        },
        1.0000,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 50.0000,
            a: 1.8634,
            b: 0.5757,
        },
        1.0000,
    ),
    (
        Lab {
            l: 50.0000,
            a: 2.5000,
            b: 0.0000,
        },
        Lab {
            l: 50.0000,
            a: 3.2592,
            b: 0.3350,
        },
        1.0000,
    ),
    (
        Lab {
            l: 60.2574,
            a: -34.0099,
            b: 36.2677,
        },
        Lab {
            l: 60.4626,
            a: -34.1751,
            b: 39.4387,
        },
        1.2644,
    ),
    (
        Lab {
            l: 63.0109,
            a: -31.0961,
            b: -5.8663,
        },
        Lab {
            l: 62.8187,
            a: -29.7946,
            b: -4.0864,
        },
        1.2630,
    ),
    (
        Lab {
            l: 61.2901,
            a: 3.7196,
            b: -5.3901,
        },
        Lab {
            l: 61.4292,
            a: 2.2480,
            b: -4.9620,
        },
        1.8731,
    ),
    (
        Lab {
            l: 35.0831,
            a: -44.1164,
            b: 3.7933,
        },
        Lab {
            l: 35.0232,
            a: -40.0716,
            b: 1.5901,
        },
        1.8645,
    ),
    (
        Lab {
            l: 22.7233,
            a: 20.0904,
            b: -46.6940,
        },
        Lab {
            l: 23.0331,
            a: 14.9730,
            b: -42.5619,
        },
        2.0373,
    ),
    (
        Lab {
            l: 36.4612,
            a: 47.8580,
            b: 18.3852,
        },
        Lab {
            l: 36.2715,
            a: 50.5065,
            b: 21.2231,
        },
        1.4146,
    ),
    (
        Lab {
            l: 90.8027,
            a: -2.0831,
            b: 1.4410,
        },
        Lab {
            l: 91.1528,
            a: -1.6435,
            b: 0.0447,
        },
        1.4441,
    ),
    (
        Lab {
            l: 90.9257,
            a: -0.5406,
            b: -0.9208,
        },
        Lab {
            l: 88.6381,
            a: -0.8985,
            b: -0.7239,
        },
        1.5381,
    ),
    (
        Lab {
            l: 6.7747,
            a: -0.2908,
            b: -2.4247,
        },
        Lab {
            l: 5.8714,
            a: -0.0985,
            b: -2.2286,
        },
        0.6377,
    ),
    (
        Lab {
            l: 2.0776,
            a: 0.0795,
            b: -1.1350,
        },
        Lab {
            l: 0.9033,
            a: -0.0636,
            b: -0.5514,
        },
        0.9082,
    ),
];

#[test]
fn ciede2000_matches_all_34_sharma_pairs() {
    for (index, (reference, sample, expected)) in SHARMA_PAIRS.iter().enumerate() {
        let forward = delta_e_2000(*reference, *sample);
        assert!(
            (forward - expected).abs() < 1e-4,
            "пара {}: {forward} против {expected}",
            index + 1
        );
        // Формула симметрична: статья проверяет и обратный порядок.
        let backward = delta_e_2000(*sample, *reference);
        assert!(
            (backward - expected).abs() < 1e-4,
            "пара {} (обратно): {backward} против {expected}",
            index + 1
        );
    }
}

#[test]
fn identical_colors_have_zero_difference() {
    let color = Lab {
        l: 55.0,
        a: 12.0,
        b: -30.0,
    };
    assert_eq!(delta_e_2000(color, color), 0.0);
}

/// Эталон colour-science 0.4.7: `XYZ_to_Lab(xyz, illuminant=(0.3127, 0.3290))`.
#[test]
fn xyz_to_lab_matches_colour_science() {
    let white = Chromaticity::D65.to_xyz();
    let cases = [
        (
            [0.2, 0.3, 0.4],
            [61.65422220953167, -37.31986864758225, -9.343090335101522],
        ),
        // Ниже порога ε: линейный участок кривой.
        (
            [0.005, 0.004, 0.003],
            [3.613185185185184, 4.908298688839144, 1.9394786078291537],
        ),
        (
            [0.9, 0.95, 1.0],
            [98.03351840902079, -0.5328070560313147, 2.216930715579135],
        ),
    ];
    for (xyz, expected) in cases {
        let lab = xyz_to_lab(xyz, white);
        for (actual, wanted) in [lab.l, lab.a, lab.b].into_iter().zip(expected) {
            assert!((actual - wanted).abs() < 1e-9, "{xyz:?}: {lab:?} против {expected:?}");
        }
    }
}

#[test]
fn white_is_l100_and_black_is_l0() {
    let white = Chromaticity::D65.to_xyz();
    let lab_white = xyz_to_lab(white, white);
    assert!((lab_white.l - 100.0).abs() < 1e-12 && lab_white.a.abs() < 1e-12 && lab_white.b.abs() < 1e-12);
    let lab_black = xyz_to_lab([0.0; 3], white);
    assert!(lab_black.l.abs() < 1e-12);
}

/// Эталон colour-science: `XYZ_to_Lab(RGB_to_XYZ_matrix · rgb, D65)` для RGB (0,2; 0,5; 0,7).
#[test]
fn display_linear_to_lab_matches_colour_science_in_both_gamuts() {
    let rgb = [0.2_f32, 0.5, 0.7];
    let srgb = display_linear_to_lab(rgb, DisplayGamut::Srgb).unwrap();
    let expected_srgb = [72.93533118805284, -12.54870836433769, -21.60134675322094];
    let p3 = display_linear_to_lab(rgb, DisplayGamut::DisplayP3).unwrap();
    let expected_p3 = [72.70482868797276, -17.626129969637418, -23.93807273358288];
    // Display P3: colour выводит матрицу из основных цветов, как и мы, — совпадение до 1e-6.
    for (actual, wanted) in [p3.l, p3.a, p3.b].into_iter().zip(expected_p3) {
        assert!((actual - wanted).abs() < 1e-6, "{p3:?}");
    }
    // sRGB: colour берёт округлённую до 4 знаков матрицу IEC 61966-2-1, мы выводим её из основных
    // цветов (разница матриц до 5e-5), отсюда расхождение до 4e-3 в Lab (ΔE << 0,01, незаметно).
    for (actual, wanted) in [srgb.l, srgb.a, srgb.b].into_iter().zip(expected_srgb) {
        assert!((actual - wanted).abs() < 1e-2, "{srgb:?}");
    }
}

#[test]
fn display_white_and_black_in_lab() {
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        let white = display_linear_to_lab([1.0; 3], gamut).unwrap();
        assert!(
            (white.l - 100.0).abs() < 1e-9 && white.a.abs() < 1e-9 && white.b.abs() < 1e-9,
            "{white:?}"
        );
        let black = display_linear_to_lab([0.0; 3], gamut).unwrap();
        assert!(black.l.abs() < 1e-12);
    }
}

#[test]
fn psnr_matches_hand_computation() {
    // Один из четырёх отсчётов отличается на 0,1: MSE = 0,01/4 = 0,0025, PSNR = 10·log10(400).
    let reference = [0.0_f32, 0.5, 0.25, 1.0];
    let sample = [0.0_f32, 0.5, 0.25, 0.9];
    let Psnr::Finite(db) = psnr(&reference, &sample, 1.0).unwrap() else {
        panic!("ошибка ненулевая, PSNR конечен");
    };
    assert!((db - 26.020599913).abs() < 1e-5, "{db}");
    // Пик 2 при той же ошибке даёт на 20·log10(2) дБ больше.
    let Psnr::Finite(db_peak2) = psnr(&reference, &sample, 2.0).unwrap() else {
        panic!("PSNR конечен");
    };
    assert!((db_peak2 - db - 6.0205999).abs() < 1e-5);
}

#[test]
fn psnr_of_identical_arrays_is_identical_not_infinity() {
    let values = [0.1_f32, 0.2, 0.3];
    assert_eq!(psnr(&values, &values, 1.0), Ok(Psnr::Identical));
}

#[test]
fn metrics_reject_bad_inputs() {
    assert_eq!(
        psnr(&[1.0], &[1.0, 2.0], 1.0),
        Err(MetricsError::LengthMismatch {
            reference: 1,
            sample: 2
        })
    );
    assert_eq!(psnr(&[], &[], 1.0), Err(MetricsError::Empty));
    assert_eq!(psnr(&[1.0], &[1.0], 0.0), Err(MetricsError::InvalidPeak));
    assert_eq!(psnr(&[1.0], &[f32::NAN], 1.0), Err(MetricsError::NonFiniteSample));
    assert_eq!(
        max_abs_difference(&[1.0], &[1.0, 2.0]),
        Err(MetricsError::LengthMismatch {
            reference: 1,
            sample: 2
        })
    );
    assert_eq!(max_abs_difference(&[], &[]), Err(MetricsError::Empty));
    assert_eq!(
        max_abs_difference(&[0.0, 1.0], &[0.0, f32::NAN]),
        Err(MetricsError::NonFiniteSample)
    );
}

#[test]
fn max_abs_difference_finds_largest_in_either_direction() {
    let reference = [0.5_f32, 0.5, 0.5];
    assert_eq!(max_abs_difference(&reference, &[0.5, 0.75, 0.5]), Ok(0.25));
    assert_eq!(max_abs_difference(&reference, &[0.0, 0.75, 0.5]), Ok(0.5));
    assert_eq!(max_abs_difference(&reference, &reference), Ok(0.0));
}
