use camera_contract::capabilities::ReferenceIlluminant;

use super::{
    ISOTEMPERATURE_LINES, Kelvin, UnknownCalibrationIlluminant, calibration_illuminant_temperature,
    temperature_from_chromaticity,
};
use crate::color::chromaticity::Chromaticity;

fn cct(x: f64, y: f64) -> f64 {
    temperature_from_chromaticity(Chromaticity { x, y }).0
}

/// Эталон: colour-science 0.4.7, `uv_to_CCT_Robertson1968(xy_to_UCS_uv(xy))[0]`
/// (`~/.cache/fasticamera/isp-reference-venv/bin/python -I -W ignore`, скрипт gen.py сессии 06).
/// Наш метод повторяет Adobe SDK, colour-science — тот же Робертсон с той же таблицей,
/// поэтому совпадение до 1e-6 К (разница только в порядке операций с плавающей точкой).
#[test]
fn matches_colour_science_robertson_1968() {
    let cases = [
        ((0.3127, 0.3290), 6503.70718479529),
        ((0.44757, 0.40745), 2855.7553370077117),
        ((0.3457, 0.3585), 5000.706605070579),
        ((0.3333, 0.3333), 5455.552358984636),
        ((0.25, 0.27), 18073.468955382916),
        ((0.5, 0.42), 2288.2956902043215),
        ((0.2, 0.45), 9872.898599650454),
        ((0.55, 0.40), 1748.697167313504),
        ((0.363269853831, 0.383835531643), 4532.095144003621),
        ((0.3, 0.3), 7738.272612738264),
    ];
    for ((x, y), expected) in cases {
        let actual = cct(x, y);
        assert!(
            (actual - expected).abs() < 1e-5,
            "({x}, {y}): {actual} против {expected}"
        );
    }
}

/// Точка ровно на линии таблицы возвращает майреды этой линии: проверяет согласованность
/// таблицы с алгоритмом (перепутанные u/v или знак наклона сломают интерполяцию).
#[test]
fn point_on_a_table_line_gives_its_mired() {
    for line in &ISOTEMPERATURE_LINES[2..30] {
        // Обратный переход (u, v) → xy: x = 3u / (2u − 8v + 4), y = 2v / (2u − 8v + 4).
        let denominator = 2.0 * line.u - 8.0 * line.v + 4.0;
        let xy = Chromaticity {
            x: 3.0 * line.u / denominator,
            y: 2.0 * line.v / denominator,
        };
        let kelvin = temperature_from_chromaticity(xy);
        assert!((kelvin.mired() - line.mired).abs() < 1e-6, "{line:?}: {kelvin:?}");
    }
}

/// Края таблицы по правилам SDK, а не NaN и не паника.
#[test]
fn table_edges_are_clamped_like_the_sdk() {
    // Очень красный: прижимается к 600 майред = 1666,67 К.
    assert!((cct(0.65, 0.30) - 1.0e6 / 600.0).abs() < 1e-9);
    // Очень синий: первая линия 10 майред = 100 000 К (colour-science даёт то же для (0.2, 0.2)).
    assert!((cct(0.2, 0.2) - 100_000.0).abs() < 1e-6);
}

#[test]
fn temperature_is_monotonic_along_the_planckian_locus() {
    let temperatures: Vec<f64> = ISOTEMPERATURE_LINES[2..30]
        .iter()
        .map(|line| {
            let d = 2.0 * line.u - 8.0 * line.v + 4.0;
            cct(3.0 * line.u / d, 2.0 * line.v / d)
        })
        .collect();
    assert!(
        temperatures.windows(2).all(|pair| pair[0] > pair[1]),
        "{temperatures:?}"
    );
}

#[test]
fn broken_chromaticity_gives_nan() {
    assert!(cct(f64::NAN, 0.3).is_nan());
    assert!(cct(1.0, -1.0).is_nan());
}

#[test]
fn mired_is_reciprocal_megakelvin() {
    assert!((Kelvin(5000.0).mired() - 200.0).abs() < 1e-12);
    assert!((Kelvin(2850.0).mired() - 350.877192982456).abs() < 1e-9);
}

#[test]
fn table_is_ordered_by_mired_with_negative_slopes() {
    assert_eq!(ISOTEMPERATURE_LINES.len(), 31);
    assert!(
        ISOTEMPERATURE_LINES
            .windows(2)
            .all(|pair| pair[0].mired < pair[1].mired)
    );
    assert!(ISOTEMPERATURE_LINES.iter().all(|line| line.slope < 0.0));
}

#[test]
fn standard_illuminants_have_dng_sdk_temperatures() {
    let kelvin = |illuminant| calibration_illuminant_temperature(illuminant).unwrap().0;
    assert_eq!(kelvin(ReferenceIlluminant::StandardA), 2850.0);
    assert_eq!(kelvin(ReferenceIlluminant::D65), 6500.0);
    assert_eq!(kelvin(ReferenceIlluminant::D50), 5000.0);
    // Коды EXIF без именованных вариантов.
    assert_eq!(kelvin(ReferenceIlluminant::Other(3)), 2850.0);
    assert_eq!(kelvin(ReferenceIlluminant::Other(24)), 3200.0);
    assert_eq!(kelvin(ReferenceIlluminant::Other(20)), 5500.0);
    assert_eq!(kelvin(ReferenceIlluminant::Other(22)), 7500.0);
    assert_eq!(kelvin(ReferenceIlluminant::Other(12)), 6400.0);
    assert_eq!(kelvin(ReferenceIlluminant::Other(16)), 2925.0);
}

#[test]
fn every_exif_code_the_sdk_knows_is_covered_and_others_fail() {
    let known = [
        1u8, 2, 3, 4, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
    ];
    for code in 0..=u8::MAX {
        let result = calibration_illuminant_temperature(ReferenceIlluminant::from_exif_code(code));
        if known.contains(&code) {
            assert!(result.is_ok(), "код {code}");
        } else {
            assert_eq!(
                result,
                Err(UnknownCalibrationIlluminant { exif_code: code }),
                "код {code}"
            );
        }
    }
}
