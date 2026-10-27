use super::{Chromaticity, Primaries, bradford_adaptation, xyz_d50_to_scene_linear};
use crate::color::matrix::Matrix3;
use crate::image::DisplayGamut;

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

#[test]
fn xyz_roundtrip_returns_same_chromaticity() {
    let xyz = Chromaticity::D50.to_xyz();
    // Y = 1 по определению, X = x/y.
    assert!((xyz[1] - 1.0).abs() < 1e-15);
    assert!((xyz[0] - 0.3457 / 0.3585).abs() < 1e-15);
    // Масштаб яркости не влияет на цветность.
    let scaled = [xyz[0] * 7.0, xyz[1] * 7.0, xyz[2] * 7.0];
    let back = Chromaticity::from_xyz(scaled).unwrap();
    assert!((back.x - 0.3457).abs() < 1e-12 && (back.y - 0.3585).abs() < 1e-12);
}

#[test]
fn black_or_broken_xyz_has_no_chromaticity() {
    assert_eq!(Chromaticity::from_xyz([0.0, 0.0, 0.0]), None);
    assert_eq!(Chromaticity::from_xyz([f64::NAN, 1.0, 1.0]), None);
    assert_eq!(Chromaticity::from_xyz([f64::INFINITY, 1.0, 1.0]), None);
}

/// Опубликованная матрица sRGB → XYZ (IEC 61966-2-1) против выведенной из основных цветов.
#[test]
fn srgb_matrix_derived_from_primaries_matches_iec_61966_2_1() {
    let derived = Primaries::SRGB.rgb_to_xyz().unwrap();
    let published = Matrix3::from_rows([
        [0.4124, 0.3576, 0.1805],
        [0.2126, 0.7152, 0.0722],
        [0.0193, 0.1192, 0.9505],
    ]);
    assert_matrix_close(&derived, &published, 1e-4);
}

/// Display P3 → XYZ D65 (SMPTE EG 432-1; значения colour-science `RGB_COLOURSPACES['Display P3']`).
#[test]
fn display_p3_matrix_derived_from_primaries_matches_published() {
    let derived = Primaries::DISPLAY_P3.rgb_to_xyz().unwrap();
    let published = Matrix3::from_rows([
        [0.486570948648216, 0.2656676931690931, 0.1982172852343625],
        [0.2289745640697487, 0.6917385218365064, 0.079286914093745],
        [0.0, 0.04511338185890264, 1.0439443689009757],
    ]);
    assert_matrix_close(&derived, &published, 1e-9);
}

/// Белая точка гаммы — это RGB (1, 1, 1): определяющее свойство нормировки RP 177.
#[test]
fn rgb_one_one_one_is_the_gamut_white() {
    for primaries in [Primaries::SRGB, Primaries::DISPLAY_P3] {
        let white = primaries.rgb_to_xyz().unwrap().apply([1.0, 1.0, 1.0]);
        assert_vector_close(white, primaries.white.to_xyz(), 1e-12);
    }
}

#[test]
fn degenerate_primaries_are_an_error() {
    // Все три основных цвета совпадают — матрица вырождена.
    let same = Chromaticity { x: 0.3, y: 0.3 };
    let broken = Primaries {
        red: same,
        green: same,
        blue: same,
        white: Chromaticity::D65,
    };
    assert!(broken.rgb_to_xyz().is_err());
}

/// Брэдфорд D65 → D50 из colour-science (`matrix_chromatic_adaptation_VonKries`, 'Bradford'),
/// белые (0,3127, 0,3290) → (0,3457, 0,3585); обратное направление — обратная матрица.
#[test]
fn bradford_matches_colour_science_and_maps_white_to_white() {
    let d50_to_d65 = bradford_adaptation(Chromaticity::D50, Chromaticity::D65).unwrap();
    let expected = Matrix3::from_rows([
        [0.9554734214880751, -0.023098454948764575, 0.06325924320057064],
        [-0.02836970933386368, 1.009995398081304, 0.021041441191917306],
        [0.012314014864481975, -0.020507649298898943, 1.3303659262421237],
    ]);
    assert_matrix_close(&d50_to_d65, &expected, 1e-12);
    assert_vector_close(
        d50_to_d65.apply(Chromaticity::D50.to_xyz()),
        Chromaticity::D65.to_xyz(),
        1e-12,
    );

    let d65_to_d50 = bradford_adaptation(Chromaticity::D65, Chromaticity::D50).unwrap();
    assert_matrix_close(&d65_to_d50.mul(&d50_to_d65), &Matrix3::IDENTITY, 1e-12);
}

#[test]
fn bradford_to_same_white_is_identity() {
    let identity = bradford_adaptation(Chromaticity::D65, Chromaticity::D65).unwrap();
    assert_matrix_close(&identity, &Matrix3::IDENTITY, 1e-12);
}

#[test]
fn d50_white_maps_to_rgb_one_in_both_gamuts() {
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        let matrix = xyz_d50_to_scene_linear(gamut).unwrap();
        assert_vector_close(matrix.apply(Chromaticity::D50.to_xyz()), [1.0, 1.0, 1.0], 1e-9);
    }
}

/// Сравнение с опубликованной матрицей XYZ D50 → sRGB (Lindbloom, «sRGB D50»:
/// 3,1338561 …). Допуск 1e-3, а не 1e-9: Lindbloom берёт округлённые основные цвета и свою
/// белую D50 (0,34567, 0,35850), у DNG белая 0,3457/0,3585, поэтому числа различаются в 4-м знаке.
/// Эталон colour-science (опубликованная sRGB-матрица · Брэдфорд) — тот же порядок.
#[test]
fn srgb_d50_matrix_is_close_to_published_d50_srgb() {
    let ours = xyz_d50_to_scene_linear(DisplayGamut::Srgb).unwrap();
    let lindbloom = Matrix3::from_rows([
        [3.1338561, -1.6168667, -0.4906146],
        [-0.9787684, 1.9161415, 0.0334540],
        [0.0719453, -0.2289914, 1.4052427],
    ]);
    assert_matrix_close(&ours, &lindbloom, 1e-3);
    // colour-science: sRGB_XYZ_to_RGB · Bradford(D50 → D65), см. ref-скрипт в отчёте сессии.
    let colour = Matrix3::from_rows([
        [3.133777319050841, -1.6171926650971158, -0.49066745070876894],
        [-0.9784630672313814, 1.9160783932748635, 0.03338784058981373],
        [0.07202320399275142, -0.22900223045816834, 1.4054278698810454],
    ]);
    assert_matrix_close(&ours, &colour, 1e-3);
}

/// Display P3 D50 → RGB, colour-science: P3_XYZ_to_RGB · Bradford(D50 → D65).
#[test]
fn display_p3_d50_matrix_matches_colour_science() {
    let ours = xyz_d50_to_scene_linear(DisplayGamut::DisplayP3).unwrap();
    let colour = Matrix3::from_rows([
        [2.403934121855499, -0.990030442495594, -0.39761363181465637],
        [-0.8422700161454687, 1.7989580161067076, 0.016045624770904758],
        [0.04819381686413316, -0.09738519815446056, 1.2736713693321273],
    ]);
    assert_matrix_close(&ours, &colour, 1e-9);
}
