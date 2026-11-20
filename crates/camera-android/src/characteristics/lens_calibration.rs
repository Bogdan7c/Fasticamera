//! Геометрия объектива из характеристик камеры (сессия 13): калибровка (внутренние параметры,
//! поза, система координат) и наибольший цифровой зум. Вынесено из `characteristics`, чтобы
//! тот не раздувался; поведение разбора то же: нет ключа — `None` и заметка, а не ошибка камеры.

use camera_contract::geometry::PixelRect;
use camera_contract::lens_calibration::{LensCalibration, LensIntrinsics, LensPose, PoseReference};

use super::{CharacteristicsError, Notes, parse_array_rect};
use crate::entry::{EntryProblem, MetadataSource, read_byte, read_float, read_floats};
use crate::tags;

/// Геометрическая калибровка объектива. Все ключи необязательны: нет ключа или он непригоден —
/// это `None` в контракте и заметка в диагностике, а не ошибка камеры. Правдоподобие значений
/// (например, заглушка `c_x = c_y = 0` у фронтальной камеры) здесь **не** судим: поля отдаются
/// как пришли, решение принимает `LensCalibration::optical_axis`.
pub(super) fn parse_lens_calibration(
    source: &dyn MetadataSource,
    active_array: PixelRect,
    notes: &mut Notes,
) -> LensCalibration {
    LensCalibration {
        intrinsics: parse_intrinsics(source, notes),
        pose: parse_lens_pose(source, notes),
        pre_correction_active_array: parse_pre_correction_array(source, active_array, notes),
    }
}

/// `ACAMERA_LENS_INTRINSIC_CALIBRATION`: `[f_x, f_y, c_x, c_y, s]`. Короче пяти элементов —
/// ключ непригоден (как у любого другого массива с `min_len`); лишние элементы игнорируются.
fn parse_intrinsics(source: &dyn MetadataSource, notes: &mut Notes) -> Option<LensIntrinsics> {
    let tag = tags::LENS_INTRINSIC_CALIBRATION;
    let values = notes.optional(tag, read_floats(source, tag, 5))?;
    Some(LensIntrinsics {
        focal_length_x_px: values[0],
        focal_length_y_px: values[1],
        principal_point_x_px: values[2],
        principal_point_y_px: values[3],
        skew: values[4],
    })
}

/// Положение камеры: нужны **оба** ключа, поворот и сдвиг (половина позы бесполезна).
///
/// Ключ `LENS_POSE_REFERENCE` читается только при полной позе. Если его нет, берём
/// `PrimaryCamera`: NDK прямо называет это значением по умолчанию («default value for API levels
/// before Android P»), и это честнее, чем `Undefined` (тот означает «значения — умолчания по
/// направлению камеры», то есть заявляет, что поза ненастоящая). Подстановка пишется в заметки.
/// Непригодный ключ (не байт) честно превращается в `Undefined` + заметка `Malformed`.
fn parse_lens_pose(source: &dyn MetadataSource, notes: &mut Notes) -> Option<LensPose> {
    let rotation = notes.optional(
        tags::LENS_POSE_ROTATION,
        read_floats(source, tags::LENS_POSE_ROTATION, 4),
    );
    let translation = notes.optional(
        tags::LENS_POSE_TRANSLATION,
        read_floats(source, tags::LENS_POSE_TRANSLATION, 3),
    );
    let (rotation, translation) = (rotation?, translation?);
    let reference = match read_byte(source, tags::LENS_POSE_REFERENCE) {
        Ok(Some(code)) => pose_reference_from_code(code),
        Ok(None) => {
            notes.substituted(tags::LENS_POSE_REFERENCE, "PrimaryCamera (умолчание NDK)");
            PoseReference::PrimaryCamera
        }
        Err(problem) => {
            notes.malformed(tags::LENS_POSE_REFERENCE, problem);
            PoseReference::Undefined
        }
    };
    Some(LensPose {
        reference,
        rotation_xyzw: [rotation[0], rotation[1], rotation[2], rotation[3]],
        translation_m: [translation[0], translation[1], translation[2]],
    })
}

/// Код `ACAMERA_LENS_POSE_REFERENCE` в контракт; неизвестный код сохраняется как `Other`.
fn pose_reference_from_code(code: u8) -> PoseReference {
    match code {
        tags::LENS_POSE_REFERENCE_PRIMARY_CAMERA => PoseReference::PrimaryCamera,
        tags::LENS_POSE_REFERENCE_GYROSCOPE => PoseReference::Gyroscope,
        tags::LENS_POSE_REFERENCE_UNDEFINED => PoseReference::Undefined,
        tags::LENS_POSE_REFERENCE_AUTOMOTIVE => PoseReference::Automotive,
        other => PoseReference::Other(other),
    }
}

/// Система координат калибровки. Нет ключа или он непригоден — берём активную область (с
/// заметкой): калибровка задана в координатах `preCorrection`, а на OnePlus 11 обе области
/// совпадают (`[0, 0, 4096, 3072]`), так что подстановка не сдвигает ось.
fn parse_pre_correction_array(source: &dyn MetadataSource, active_array: PixelRect, notes: &mut Notes) -> PixelRect {
    let tag = tags::PRE_CORRECTION_ACTIVE_ARRAY_SIZE;
    match parse_array_rect(source, tag) {
        Ok(rect) => rect,
        Err(CharacteristicsError::Malformed { problem, .. }) => {
            notes.malformed(tag, problem);
            active_array
        }
        Err(_) => {
            notes.absent(tag);
            active_array
        }
    }
}

/// `ACAMERA_SCALER_AVAILABLE_MAX_DIGITAL_ZOOM`: число ≥ 1. Не число или меньше единицы —
/// непригодный ключ (`OutOfRange` в заметках), зум считается неизвестным.
pub(super) fn parse_max_digital_zoom(source: &dyn MetadataSource, notes: &mut Notes) -> Option<f32> {
    let tag = tags::MAX_DIGITAL_ZOOM;
    let zoom = notes.optional(tag, read_float(source, tag))?;
    if zoom.is_finite() && zoom >= 1.0 {
        Some(zoom)
    } else {
        notes.malformed(tag, EntryProblem::OutOfRange);
        None
    }
}
