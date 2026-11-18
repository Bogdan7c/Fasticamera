//! Сверка белой точки с независимыми эталонами (фикстура `white-point-reference`):
//! colour-science 0.4.7 (Робертсон 1968, Ohno 2014) для «CCT + Duv ↔ цветность» и
//! colour-hdri 0.2.6 для «белая точка → нейтраль камеры» на калибровке OnePlus 11.
//!
//! Круг «туда и обратно» не доказывает правильность (согласованная ошибка его пройдёт),
//! поэтому здесь числа сравниваются с чужим кодом; каждый допуск объяснён у константы.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в интеграционных тестах падение и есть способ сообщить о провале"
)]

mod support;

use camera_contract::capabilities::{ColorCalibrationSet, Matrix3x3, Rational, ReferenceIlluminant};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, cct_duv_from_chromaticity, chromaticity_from_cct_duv};
use serde_json::Value;

/// Прямая задача: тот же алгоритм (таблица Робертсона, линейная интерполяция точки и вектора
/// изотермы по майредам, сдвиг на −Duv вдоль изотермы), оба считают в f64. Расхождение — лишь
/// порядок операций с плавающей точкой (измерено ≲ 1e-15); допуск 1e-9 по uv с запасом на печать
/// чисел в JSON и ловит любую ошибку знака, масштаба или индекса линии (они дают ≥ 1e-4).
const FORWARD_UV_TOLERANCE: f64 = 1e-9;

/// Температура обратной задачи: у нас — `Set_xy_coord` из Adobe SDK, у colour — тот же Робертсон
/// с той же таблицей; измеренное расхождение < 1e-9 К (порядок операций с плавающей точкой).
/// Допуск 1e-6 К: на 4 порядка меньше точности, нужной AWB, и ловит любую ошибку таблицы.
const INVERSE_KELVIN_TOLERANCE: f64 = 1e-6;

/// Duv обратной задачи: у colour и у нас это расстояние до точки кривой при тех же майредах;
/// измерено 5e-12. Допуск 1e-9 ловит ошибку знака (дала бы 2·|Duv| ~ 1e-2) и масштаба.
const INVERSE_DUV_TOLERANCE: f64 = 1e-9;

/// Ohno 2014 — другой метод (расстояние до кривой минимизируется точнее), только для сведения.
/// На сетке 2500–10 000 К, |Duv| ≤ 0,02 uv различаются не более чем на ~8e-5 (измерено 7,6e-5); допуск
/// 2e-4 фиксирует «тот же порядок величины», а не точное совпадение.
const OHNO_UV_SANITY_TOLERANCE: f64 = 2e-4;

/// Нейтраль камеры: те же матрицы, та же интерполяция по майредам, тот же Робертсон, нормировка
/// на G = 1; различие — только температура белой (SDK-стиль против Робертсона, ≲ 1e-5 К → ≲ 1e-9
/// в нейтрали) и квантование знаменателей 2^20 в `Rational` калибровки (≲ 5e-7 в элементах
/// матриц; измеренное расхождение 7,2e-7). Допуск 3e-6 покрывает оба источника, а ошибка интерполяции (калибровка 1 вместо
/// смеси) даёт ≥ 1e-2.
const NEUTRAL_TOLERANCE: f64 = 3e-6;

fn number(value: &Value, what: &str) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("в фикстуре {what} не число: {value}"))
}

fn pair(value: &Value, what: &str) -> [f64; 2] {
    [number(&value[0], what), number(&value[1], what)]
}

/// u, v по xy: CIE 1960 UCS.
fn ucs(xy: Chromaticity) -> [f64; 2] {
    let denominator = -2.0 * xy.x + 12.0 * xy.y + 3.0;
    [4.0 * xy.x / denominator, 6.0 * xy.y / denominator]
}

fn cases<'a>(asset: &'a support::TestAsset, key: &str) -> &'a Vec<Value> {
    asset
        .field(&format!("extra.{key}"))
        .as_array()
        .unwrap_or_else(|| panic!("extra.{key} не массив"))
}

#[test]
fn cct_duv_to_chromaticity_matches_colour_science_on_the_grid() {
    let asset = support::load_asset("white-point-reference");
    let forward = cases(&asset, "forward");
    assert_eq!(forward.len(), 27, "9 температур × 3 значения Duv");
    for case in forward {
        let (kelvin, duv) = (number(&case["cct"], "cct"), number(&case["duv"], "duv"));
        let xy = chromaticity_from_cct_duv(CctDuv {
            temperature: Kelvin(kelvin),
            duv: Duv(duv),
        })
        .unwrap();
        let uv = ucs(xy);
        let robertson = pair(&case["robertson_uv"], "robertson_uv");
        let robertson_xy = pair(&case["robertson_xy"], "robertson_xy");
        assert!(
            (uv[0] - robertson[0]).abs() < FORWARD_UV_TOLERANCE,
            "{kelvin} К, Duv {duv}: u {uv:?} против {robertson:?}"
        );
        assert!(
            (uv[1] - robertson[1]).abs() < FORWARD_UV_TOLERANCE,
            "{kelvin} К, Duv {duv}: v {uv:?} против {robertson:?}"
        );
        assert!(
            (xy.x - robertson_xy[0]).abs() < FORWARD_UV_TOLERANCE
                && (xy.y - robertson_xy[1]).abs() < FORWARD_UV_TOLERANCE
        );
        let ohno = pair(&case["ohno_uv"], "ohno_uv");
        assert!(
            (uv[0] - ohno[0]).abs() < OHNO_UV_SANITY_TOLERANCE && (uv[1] - ohno[1]).abs() < OHNO_UV_SANITY_TOLERANCE,
            "{kelvin} К, Duv {duv}: Ohno {ohno:?} против {uv:?}"
        );
    }
}

#[test]
fn chromaticity_to_cct_duv_matches_colour_science() {
    let asset = support::load_asset("white-point-reference");
    for case in cases(&asset, "inverse") {
        let label = case["label"].as_str().unwrap();
        let xy = pair(&case["xy"], "xy");
        let measured = cct_duv_from_chromaticity(Chromaticity { x: xy[0], y: xy[1] }).unwrap();
        let expected_kelvin = number(&case["robertson"]["cct"], "robertson.cct");
        let expected_duv = number(&case["robertson"]["duv"], "robertson.duv");
        assert!(
            (measured.temperature.0 - expected_kelvin).abs() < INVERSE_KELVIN_TOLERANCE,
            "{label}: {measured:?} против {expected_kelvin} К"
        );
        assert!(
            (measured.duv.0 - expected_duv).abs() < INVERSE_DUV_TOLERANCE,
            "{label}: {measured:?} против Duv {expected_duv}"
        );
    }
}

/// Калибровка из манифеста в форме, которую отдаёт HAL (рациональные числа со знаменателем 2^20).
fn profile_from_fixture(asset: &support::TestAsset) -> DngColorProfile {
    let matrix = |path: &str| {
        let rows = asset
            .field(path)
            .as_array()
            .unwrap_or_else(|| panic!("{path} не массив"));
        let flat: Vec<f64> = rows
            .iter()
            .flat_map(|row| row.as_array().unwrap().iter().map(|value| number(value, path)))
            .collect();
        rational_matrix(flat.try_into().unwrap_or_else(|_| panic!("{path}: нужно 9 чисел")))
    };
    let diagonal = |path: &str| {
        let values: Vec<f64> = asset
            .field(path)
            .as_array()
            .unwrap()
            .iter()
            .map(|v| number(v, path))
            .collect();
        rational_matrix([values[0], 0.0, 0.0, 0.0, values[1], 0.0, 0.0, 0.0, values[2]])
    };
    // ForwardMatrix в фикстуре не участвует (нейтраль от неё не зависит) — нужна лишь валидная.
    let forward = rational_matrix([
        0.4375, 0.3828125, 0.140625, 0.21875, 0.71875, 0.0625, 0.015625, 0.09375, 0.7109375,
    ]);
    // Порядок как у телефона: первая калибровка D65, вторая StdA.
    let sets = [
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::D65,
            color_matrix: matrix("extra.calibration.color_matrix_d65"),
            forward_matrix: forward,
            calibration_transform: diagonal("extra.calibration.camera_calibration_d65_diagonal"),
        },
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::StandardA,
            color_matrix: matrix("extra.calibration.color_matrix_standard_a"),
            forward_matrix: forward,
            calibration_transform: diagonal("extra.calibration.camera_calibration_standard_a_diagonal"),
        },
    ];
    DngColorProfile::from_calibration_sets(&sets).unwrap()
}

fn rational_matrix(values: [f64; 9]) -> Matrix3x3 {
    const DENOMINATOR: i32 = 1 << 20;
    Matrix3x3(values.map(|value| Rational {
        numerator: (value * f64::from(DENOMINATOR)).round() as i32,
        denominator: DENOMINATOR,
    }))
}

#[test]
fn neutral_for_white_matches_colour_hdri() {
    let asset = support::load_asset("white-point-reference");
    let profile = profile_from_fixture(&asset);
    let neutral_cases = cases(&asset, "neutral");
    assert_eq!(neutral_cases.len(), 8);
    for case in neutral_cases {
        let label = case["label"].as_str().unwrap();
        let xy = pair(&case["xy"], "xy");
        let expected: Vec<f64> = case["neutral"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| number(v, "neutral"))
            .collect();
        let neutral = profile
            .neutral_for_white(Chromaticity { x: xy[0], y: xy[1] })
            .unwrap()
            .values();
        for channel in 0..3 {
            assert!(
                (neutral[channel] - expected[channel]).abs() < NEUTRAL_TOLERANCE,
                "{label}: канал {channel}: {neutral:?} против {expected:?}"
            );
        }
    }
}
