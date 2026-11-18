//! Разбор результата запроса (`ACameraMetadata` из `onCaptureCompleted`) в [`CaptureResultMeta`].
//!
//! Тот же источник метаданных, что и у характеристик, поэтому разбор проверяется на ПК.
//! Ключ, которого HAL не отдал, — `None` (так задаёт контракт); ключ, который отдан, но
//! непригоден (не тот тип, отрицательная длительность), — тоже `None`, но отдельно
//! перечисляется в [`ParsedCaptureResult::unreadable_keys`], чтобы не путать с отсутствием.

use std::time::Duration;

use camera_contract::capabilities::FrameDuration;
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, ResultKey, SensorTimestamp};
use camera_contract::frame_passport::{CaptureColorHints, ColorCorrectionGains};
use camera_contract::three_a::RequestOrigin;
use serde::Serialize;

use crate::entry::{MetadataSource, Read, read_floats, read_int32, read_int64, read_rationals};
use crate::tags;
use crate::three_a_result::{ThreeAResultKey, parse_three_a_result};

/// Цветовые ключи результата (`CaptureColorHints`), по которым ведётся учёт непригодных записей.
/// Отдельно от [`ResultKey`]: тот принадлежит контракту движка, а цветовые подсказки идут только
/// в паспорт кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorHintKey {
    DynamicBlackLevel,
    DynamicWhiteLevel,
    ColorCorrectionGains,
    NeutralColorPoint,
}

/// Результат разбора: сводка и ключи, которые были, но не прочитались.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCaptureResult {
    pub meta: CaptureResultMeta,
    /// Ключи, присутствующие в результате, но непригодные (в порядке [`ResultKey`]).
    pub unreadable_keys: Vec<ResultKey>,
    /// Цветовые подсказки для паспорта кадра; ключ, которого HAL не отдал, — `None`.
    pub color: CaptureColorHints,
    /// Цветовые ключи, присутствующие в результате, но непригодные (в порядке [`ColorHintKey`]).
    pub unreadable_color_keys: Vec<ColorHintKey>,
    /// Ключи состояния 3A, присутствующие в результате, но непригодные (в порядке [`ThreeAResultKey`]).
    pub unreadable_three_a_keys: Vec<ThreeAResultKey>,
}

/// Время начала экспозиции кадра — по нему результат сопоставляется с номером кадра.
pub fn read_sensor_timestamp(source: &dyn MetadataSource) -> Option<SensorTimestamp> {
    match read_int64(source, tags::SENSOR_TIMESTAMP) {
        Ok(Some(nanos)) => Some(SensorTimestamp::from_nanos(nanos)),
        Ok(None) | Err(_) => None,
    }
}

/// Разбирает результат запроса. Номер кадра и происхождение запроса приходят снаружи (их знает
/// слой колбэков).
pub fn parse_capture_result(
    source: &dyn MetadataSource,
    frame_number: FrameNumber,
    origin: RequestOrigin,
) -> ParsedCaptureResult {
    let mut unreadable_keys = Vec::new();
    let mut take = |key: ResultKey, read: Read<i64>| -> Option<i64> {
        match read {
            Ok(value) => value,
            Err(_) => {
                unreadable_keys.push(key);
                None
            }
        }
    };
    let timestamp = take(ResultKey::SensorTimestamp, read_int64(source, tags::SENSOR_TIMESTAMP));
    let exposure = take(ResultKey::ExposureTime, read_int64(source, tags::SENSOR_EXPOSURE_TIME));
    let frame_duration = take(
        ResultKey::FrameDuration,
        read_int64(source, tags::SENSOR_FRAME_DURATION),
    );
    let skew = take(
        ResultKey::RollingShutterSkew,
        read_int64(source, tags::ROLLING_SHUTTER_SKEW),
    );
    let sensitivity = read_sensitivity(source, &mut unreadable_keys);

    let non_negative = |key: ResultKey, nanos: Option<i64>, unreadable: &mut Vec<ResultKey>| -> Option<u64> {
        let nanos = nanos?;
        match u64::try_from(nanos) {
            Ok(value) => Some(value),
            Err(_) => {
                unreadable.push(key);
                None
            }
        }
    };
    let exposure_time = non_negative(ResultKey::ExposureTime, exposure, &mut unreadable_keys).map(Duration::from_nanos);
    let reported_frame_duration =
        non_negative(ResultKey::FrameDuration, frame_duration, &mut unreadable_keys).map(FrameDuration::from_nanos);
    let rolling_shutter_skew =
        non_negative(ResultKey::RollingShutterSkew, skew, &mut unreadable_keys).map(Duration::from_nanos);

    unreadable_keys.sort_by_key(|key| result_key_order(*key));
    unreadable_keys.dedup();
    let (color, unreadable_color_keys) = parse_color_hints(source);
    let three_a = parse_three_a_result(source, origin);
    ParsedCaptureResult {
        meta: CaptureResultMeta {
            frame_number,
            sensor_timestamp: timestamp.map(SensorTimestamp::from_nanos),
            exposure_time,
            sensitivity_iso: sensitivity,
            reported_frame_duration,
            rolling_shutter_skew,
            three_a: three_a.state,
        },
        unreadable_keys,
        color,
        unreadable_color_keys,
        unreadable_three_a_keys: three_a.unreadable_keys,
    }
}

/// Значение цветового ключа: непригодная запись превращается в `None` и попадает в `unreadable`.
fn take_color_hint<T>(unreadable: &mut Vec<ColorHintKey>, key: ColorHintKey, read: Read<T>) -> Option<T> {
    read.unwrap_or_else(|_| {
        unreadable.push(key);
        None
    })
}

/// Разбирает цветовые подсказки. Отсутствующий ключ — `None` без заметки; ключ неверного типа,
/// короткий или с недопустимым числом — `None` **и** запись в списке непригодных.
fn parse_color_hints(source: &dyn MetadataSource) -> (CaptureColorHints, Vec<ColorHintKey>) {
    let mut unreadable = Vec::new();
    let dynamic_black_level = take_color_hint(
        &mut unreadable,
        ColorHintKey::DynamicBlackLevel,
        read_floats(source, tags::DYNAMIC_BLACK_LEVEL, 4),
    )
    .and_then(|values| <[f32; 4]>::try_from(&values[..4]).ok());
    let dynamic_white_level = take_color_hint(
        &mut unreadable,
        ColorHintKey::DynamicWhiteLevel,
        read_int32(source, tags::DYNAMIC_WHITE_LEVEL),
    );
    let color_gains = take_color_hint(
        &mut unreadable,
        ColorHintKey::ColorCorrectionGains,
        read_floats(source, tags::COLOR_CORRECTION_GAINS, 4),
    )
    .and_then(|values| <[f32; 4]>::try_from(&values[..4]).ok())
    .map(ColorCorrectionGains);
    let neutral_color_point = take_color_hint(
        &mut unreadable,
        ColorHintKey::NeutralColorPoint,
        read_rationals(source, tags::NEUTRAL_COLOR_POINT, 3),
    )
    .and_then(|values| <[_; 3]>::try_from(&values[..3]).ok());

    // Отрицательный белый уровень бессмыслен: это непригодная запись, а не «ноль».
    let dynamic_white_level = dynamic_white_level.and_then(|level| match u32::try_from(level) {
        Ok(level) => Some(level),
        Err(_) => {
            unreadable.push(ColorHintKey::DynamicWhiteLevel);
            None
        }
    });
    unreadable.sort_by_key(|key| *key as u8);
    unreadable.dedup();
    (
        CaptureColorHints {
            dynamic_black_level,
            dynamic_white_level,
            color_gains,
            neutral_color_point,
        },
        unreadable,
    )
}

fn read_sensitivity(source: &dyn MetadataSource, unreadable_keys: &mut Vec<ResultKey>) -> Option<u32> {
    let sensitivity: Read<i32> = read_int32(source, tags::SENSOR_SENSITIVITY);
    match sensitivity {
        Ok(None) => None,
        Ok(Some(value)) => match u32::try_from(value) {
            Ok(iso) => Some(iso),
            Err(_) => {
                unreadable_keys.push(ResultKey::Sensitivity);
                None
            }
        },
        Err(_) => {
            unreadable_keys.push(ResultKey::Sensitivity);
            None
        }
    }
}

/// Порядок ключей, как в объявлении [`ResultKey`] (для стабильного вывода).
fn result_key_order(key: ResultKey) -> u8 {
    match key {
        ResultKey::SensorTimestamp => 0,
        ResultKey::ExposureTime => 1,
        ResultKey::Sensitivity => 2,
        ResultKey::FrameDuration => 3,
        ResultKey::RollingShutterSkew => 4,
    }
}

#[cfg(test)]
#[path = "result_meta_tests.rs"]
mod tests;
