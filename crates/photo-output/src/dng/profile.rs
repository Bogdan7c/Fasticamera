//! Цветовая калибровка DNG: матрицы и иллюминанты из паспорта потока (DNG 1.7.1.0, глава 6).

use camera_contract::capabilities::{ColorCalibrationSet, Matrix3x3};

use super::DngError;
use super::tags;
use crate::tiff::{Ifd, TiffValue};

/// Сколько наборов калибровки DNG 1.7 пишет в IFD0 тегами `…1` и `…2` (третий набор — `…3`, у HAL его нет).
const MAX_CALIBRATION_SETS: usize = 2;

/// Матрица 3×3 как SRATIONAL ×9 без потери точности (числа HAL — рациональные).
fn matrix_value(matrix: &Matrix3x3, name: &str) -> Result<TiffValue, DngError> {
    let mut pairs = Vec::with_capacity(9);
    for element in matrix.0 {
        if element.denominator == 0 {
            return Err(DngError::InvalidInput(format!("в матрице {name} нулевой знаменатель")));
        }
        pairs.push((element.numerator, element.denominator));
    }
    Ok(TiffValue::SRational(pairs))
}

/// Пишет в IFD0 теги калибровки: `ColorMatrix`, `CameraCalibration`, `ForwardMatrix`, `CalibrationIlluminant`
/// для каждого набора. Порядок наборов — как у HAL (иллюминант 1, затем 2).
pub fn fill_color_calibration(ifd: &mut Ifd, sets: &[ColorCalibrationSet]) -> Result<(), DngError> {
    if sets.is_empty() {
        return Err(DngError::InvalidInput(
            "в паспорте потока нет цветовой калибровки: DNG без ColorMatrix1 недопустим (спецификация DNG, ColorMatrix1)"
                .to_owned(),
        ));
    }
    if sets.len() > MAX_CALIBRATION_SETS {
        return Err(DngError::InvalidInput(format!(
            "наборов калибровки {}, DNG пишет не более {MAX_CALIBRATION_SETS}",
            sets.len()
        )));
    }
    let tag_sets = [
        (
            tags::COLOR_MATRIX_1,
            tags::CAMERA_CALIBRATION_1,
            tags::FORWARD_MATRIX_1,
            tags::CALIBRATION_ILLUMINANT_1,
        ),
        (
            tags::COLOR_MATRIX_2,
            tags::CAMERA_CALIBRATION_2,
            tags::FORWARD_MATRIX_2,
            tags::CALIBRATION_ILLUMINANT_2,
        ),
    ];
    for (set, (color_tag, calibration_tag, forward_tag, illuminant_tag)) in sets.iter().zip(tag_sets) {
        ifd.set(color_tag, matrix_value(&set.color_matrix, "ColorMatrix")?);
        ifd.set(
            calibration_tag,
            matrix_value(&set.calibration_transform, "CameraCalibration")?,
        );
        ifd.set(forward_tag, matrix_value(&set.forward_matrix, "ForwardMatrix")?);
        ifd.set(illuminant_tag, TiffValue::short(u16::from(set.illuminant.exif_code())));
    }
    Ok(())
}
