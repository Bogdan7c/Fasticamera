use super::{CctDuv, Duv, TableEdge, WhitePointError, cct_duv_from_chromaticity, chromaticity_from_cct_duv};
use crate::color::chromaticity::Chromaticity;
use crate::color::temperature::{ISOTEMPERATURE_LINES, Kelvin, temperature_from_chromaticity, ucs_from_chromaticity};

fn white(kelvin: f64, duv: f64) -> CctDuv {
    CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(duv),
    }
}

fn xy_of(kelvin: f64, duv: f64) -> Chromaticity {
    chromaticity_from_cct_duv(white(kelvin, duv)).unwrap()
}

// --- chromaticity_from_cct_duv ---

/// Температура ровно на линии таблицы при Duv = 0 даёт саму точку кривой из таблицы:
/// ловит перепутанные u/v, не тот индекс линии и ошибки перевода uv → xy.
#[test]
fn table_points_are_returned_exactly_for_zero_duv() {
    for line in &ISOTEMPERATURE_LINES[1..30] {
        let xy = xy_of(1.0e6 / line.mired, 0.0);
        let (u, v) = ucs_from_chromaticity(xy).unwrap();
        assert!(
            (u - line.u).abs() < 1e-12 && (v - line.v).abs() < 1e-12,
            "{line:?}: ({u}, {v})"
        );
    }
}

/// Положительный Duv — выше кривой Планка, к зелёному: больше v и больше y; отрицательный — ниже.
#[test]
fn positive_duv_is_above_the_locus_toward_green() {
    let on_locus = xy_of(5000.0, 0.0);
    let green = xy_of(5000.0, 0.02);
    let magenta = xy_of(5000.0, -0.02);
    assert!(green.y > on_locus.y && on_locus.y > magenta.y);
    let v_of = |xy| ucs_from_chromaticity(xy).unwrap().1;
    assert!(v_of(green) > v_of(on_locus) && v_of(on_locus) > v_of(magenta));
    // Расстояние в UCS равно |Duv| (смещение идёт на единичный вектор).
    let (u0, v0) = ucs_from_chromaticity(on_locus).unwrap();
    let (u1, v1) = ucs_from_chromaticity(green).unwrap();
    assert!(((u1 - u0).hypot(v1 - v0) - 0.02).abs() < 1e-12);
}

/// Значение из colour-science 0.4.7: `CCT_to_uv_Robertson1968([6500, 0.02])` (прогон генератора фикстур).
#[test]
fn matches_known_colour_science_value() {
    let (u, v) = ucs_from_chromaticity(xy_of(6500.0, 0.02)).unwrap();
    let on_locus = ucs_from_chromaticity(xy_of(6500.0, 0.0)).unwrap();
    assert!(((u - on_locus.0).hypot(v - on_locus.1) - 0.02).abs() < 1e-12);
    // D65 лежит выше кривой Планка на ≈ +0,0033 (colour: 0.003256663).
    let d65 = cct_duv_from_chromaticity(Chromaticity::D65).unwrap();
    assert!((d65.temperature.0 - 6503.70718479529).abs() < 1e-5, "{d65:?}");
    assert!((d65.duv.0 - 0.003256663373801633).abs() < 1e-9, "{d65:?}");
}

#[test]
fn rejects_non_finite_and_out_of_range_input_instead_of_clamping() {
    for bad in [
        white(f64::NAN, 0.0),
        white(5000.0, f64::NAN),
        white(f64::INFINITY, 0.0),
        white(5000.0, f64::INFINITY),
    ] {
        assert_eq!(
            chromaticity_from_cct_duv(bad),
            Err(WhitePointError::NonFiniteInput),
            "{bad:?}"
        );
    }
    // Холоднее конца таблицы (1666,67 К), ноль и отрицательные температуры.
    for kelvin in [1600.0, 1000.0, 0.0, -3000.0] {
        assert_eq!(
            chromaticity_from_cct_duv(white(kelvin, 0.0)),
            Err(WhitePointError::TemperatureOutOfRange { kelvin })
        );
    }
    // Сама граница таблицы допустима.
    assert!(chromaticity_from_cct_duv(white(1.0e6 / 600.0, 0.0)).is_ok());
    // Экстремально большой Duv выводит из области допустимого света — вырожденная цветность.
    assert_eq!(
        chromaticity_from_cct_duv(white(5000.0, 50.0)),
        Err(WhitePointError::DegenerateChromaticity)
    );
}

// --- cct_duv_from_chromaticity ---

/// Температура — ровно та же, что у существующей `temperature_from_chromaticity`.
#[test]
fn temperature_agrees_with_robertson_function() {
    let points = [
        (0.3127, 0.3290),
        (0.44757, 0.40745),
        (0.25, 0.27),
        (0.2, 0.45),
        (0.363, 0.384),
    ];
    for (x, y) in points {
        let xy = Chromaticity { x, y };
        let measured = cct_duv_from_chromaticity(xy).unwrap();
        let expected = temperature_from_chromaticity(xy);
        assert!(
            (measured.temperature.0 - expected.0).abs() < 1e-9,
            "({x}, {y}): {measured:?} против {expected:?}"
        );
    }
}

/// Круг (T, Duv) → xy → (T, Duv). Робертсон интерполирует майреды линейно между линиями таблицы,
/// а кривая между ними не линейна, поэтому круг не идеален: на кривой ошибка ≲ 5e-6 от T,
/// вне кривой (изотерма из интерполированного вектора не совпадает с линией таблицы) — до 2e-4 от T.
/// Допуски 1e-5 и 2e-4 с запасом над измеренным; Duv воспроизводится до 1e-6.
#[test]
fn round_trip_recovers_temperature_and_duv() {
    for kelvin in [1800.0, 2500.0, 2856.0, 3706.0, 5000.0, 6500.0, 10_000.0, 20_000.0] {
        for duv in [-0.02, -0.005, 0.0, 0.005, 0.02] {
            let back = cct_duv_from_chromaticity(xy_of(kelvin, duv)).unwrap();
            let kelvin_tolerance = if duv == 0.0 { kelvin * 1e-5 } else { kelvin * 2e-4 };
            assert!(
                (back.temperature.0 - kelvin).abs() < kelvin_tolerance,
                "{kelvin} К, Duv {duv}: {back:?}"
            );
            assert!((back.duv.0 - duv).abs() < 1e-6, "{kelvin} К, Duv {duv}: {back:?}");
        }
    }
}

#[test]
fn duv_sign_follows_the_side_of_the_locus() {
    // Зеленоватый и пурпурный сдвиги от точки на кривой при 5000 К.
    let greenish = cct_duv_from_chromaticity(Chromaticity { x: 0.34, y: 0.38 }).unwrap();
    let magenta = cct_duv_from_chromaticity(Chromaticity { x: 0.35, y: 0.33 }).unwrap();
    assert!(greenish.duv.0 > 0.01, "{greenish:?}");
    assert!(magenta.duv.0 < -0.01, "{magenta:?}");
}

#[test]
fn chromaticity_outside_the_table_is_an_error_not_a_clamp() {
    assert_eq!(
        cct_duv_from_chromaticity(Chromaticity { x: 0.65, y: 0.30 }),
        Err(WhitePointError::ChromaticityOutOfTable {
            edge: TableEdge::RedderThanTable
        })
    );
    assert_eq!(
        cct_duv_from_chromaticity(Chromaticity { x: 0.2, y: 0.2 }),
        Err(WhitePointError::ChromaticityOutOfTable {
            edge: TableEdge::BluerThanTable
        })
    );
    assert_eq!(
        cct_duv_from_chromaticity(Chromaticity { x: f64::NAN, y: 0.3 }),
        Err(WhitePointError::NonFiniteInput)
    );
    // 1,5 − x + 6y ≤ 0: перевод в UCS не определён.
    assert_eq!(
        cct_duv_from_chromaticity(Chromaticity { x: 2.0, y: 0.0 }),
        Err(WhitePointError::DegenerateChromaticity)
    );
}
