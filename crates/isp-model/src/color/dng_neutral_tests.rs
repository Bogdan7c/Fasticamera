//! Тесты `DngColorProfile::neutral_for_white` на калибровке основной камеры OnePlus 11
//! (`test_support`, значения как их отдаёт HAL).

use super::{ColorProfileError, DngColorProfile};
use crate::color::chromaticity::Chromaticity;
use crate::color::temperature::Kelvin;
use crate::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use crate::test_support::{lamp_neutral, main_camera_calibration};

fn main_profile() -> DngColorProfile {
    DngColorProfile::from_calibration_sets(&main_camera_calibration()).unwrap()
}

fn locus_white(kelvin: f64, duv: f64) -> Chromaticity {
    chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(duv),
    })
    .unwrap()
}

/// Нейтраль нормирована на G = 1, а теплее света — больше красного и меньше синего отклика.
#[test]
fn neutral_is_green_normalized_and_warmer_light_is_redder() {
    let profile = main_profile();
    let warm = profile.neutral_for_white(locus_white(2800.0, 0.0)).unwrap().values();
    let cool = profile.neutral_for_white(locus_white(6500.0, 0.0)).unwrap().values();
    assert_eq!(warm[1], 1.0);
    assert_eq!(cool[1], 1.0);
    assert!(warm[0] > cool[0], "R: {warm:?} против {cool:?}");
    assert!(warm[2] < cool[2], "B: {warm:?} против {cool:?}");
}

/// Круг «белая → нейтраль → белая»: итерация `white_for_neutral` обязана вернуть исходную белую.
/// Один круг ловит лишь несогласованность двух сторон (общая ошибка прошла бы) — независимую
/// проверку чисел даёт `tests/white_point_reference.rs`.
#[test]
fn white_for_neutral_inverts_neutral_for_white() {
    let profile = main_profile();
    for kelvin in [2300.0, 2850.0, 3706.0, 5000.0, 6500.0, 9000.0] {
        for duv in [-0.01, 0.0, 0.01] {
            let white = locus_white(kelvin, duv);
            let neutral = profile.neutral_for_white(white).unwrap();
            let estimate = profile.white_for_neutral(neutral).unwrap();
            assert!(
                (estimate.white.x - white.x).abs() < 1e-5 && (estimate.white.y - white.y).abs() < 1e-5,
                "{kelvin} К, Duv {duv}: {white:?} -> {:?}",
                estimate.white
            );
        }
    }
}

/// Обратное направление: кадровая нейтраль «лампа» → белая → нейтраль возвращает ту же нейтраль.
#[test]
fn neutral_for_white_inverts_white_for_neutral() {
    let profile = main_profile();
    let estimate = profile.white_for_neutral(lamp_neutral()).unwrap();
    let back = profile.neutral_for_white(estimate.white).unwrap().values();
    let original = lamp_neutral().values();
    for channel in 0..3 {
        assert!(
            (back[channel] - original[channel]).abs() < 1e-5,
            "{back:?} против {original:?}"
        );
    }
}

/// Здравый смысл против вендора: AWB телефона на этой камере показывал «3706 К, R 1,73, G 1,00,
/// B 2,28» (множители; нейтраль = 1/множитель при G = 1). Тинт вендора неизвестен, поэтому Duv = 0
/// и допуск намеренно широкий — 15 % на канал (измерено: R −10 %, B −9 %, наши множители слабее вендорских); тест ловит грубые ошибки (перепутанные каналы,
/// обратная матрица, кельвины вместо майред), а не уточняет цвет.
#[test]
fn neutral_on_the_locus_is_roughly_consistent_with_vendor_awb_gains() {
    const VENDOR_TEMPERATURE_KELVIN: f64 = 3706.0;
    const VENDOR_GAINS_RGB: [f64; 3] = [1.73, 1.00, 2.28];
    const LOOSE_RELATIVE_TOLERANCE: f64 = 0.15;

    let neutral = main_profile()
        .neutral_for_white(locus_white(VENDOR_TEMPERATURE_KELVIN, 0.0))
        .unwrap()
        .values();
    for (channel, vendor_gain) in VENDOR_GAINS_RGB.iter().enumerate() {
        let our_gain = 1.0 / neutral[channel];
        let relative = (our_gain - vendor_gain).abs() / vendor_gain;
        assert!(
            relative < LOOSE_RELATIVE_TOLERANCE,
            "канал {channel}: наш множитель {our_gain:.3}, вендорский {vendor_gain}, расхождение {:.1} %",
            relative * 100.0
        );
    }
}

#[test]
fn degenerate_whites_are_errors_not_garbage() {
    let profile = main_profile();
    for (x, y) in [
        (f64::NAN, 0.3),
        (0.3, f64::INFINITY),
        (0.3, 0.0),
        (0.3, -0.1),
        (2.0, 0.0),
    ] {
        assert_eq!(
            profile.neutral_for_white(Chromaticity { x, y }),
            Err(ColorProfileError::UndefinedWhitePoint),
            "({x}, {y})"
        );
    }
}

/// Холоднее самой тёплой калибровки (A, 2850 К) матрицы не экстраполируются: нейтраль
/// 2000 К считается по калибровке A, и она гладко продолжает значения у 2850 К.
#[test]
fn below_the_warmest_calibration_matrices_are_clamped() {
    let profile = main_profile();
    let at_2000 = profile.neutral_for_white(locus_white(2000.0, 0.0)).unwrap().values();
    let at_2850 = profile.neutral_for_white(locus_white(2850.0, 0.0)).unwrap().values();
    assert!(
        at_2000[0] > at_2850[0] && at_2000[2] < at_2850[2],
        "{at_2000:?} против {at_2850:?}"
    );
}
