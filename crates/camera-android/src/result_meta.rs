//! Разбор результата запроса (`ACameraMetadata` из `onCaptureCompleted`) в [`CaptureResultMeta`].
//!
//! Тот же источник метаданных, что и у характеристик, поэтому разбор проверяется на ПК.
//! Ключ, которого HAL не отдал, — `None` (так задаёт контракт); ключ, который отдан, но
//! непригоден (не тот тип, отрицательная длительность), — тоже `None`, но отдельно
//! перечисляется в [`ParsedCaptureResult::unreadable_keys`], чтобы не путать с отсутствием.

use std::time::Duration;

use camera_contract::capabilities::FrameDuration;
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, ResultKey, SensorTimestamp};

use crate::entry::{MetadataSource, Read, read_int32, read_int64};
use crate::tags;

/// Результат разбора: сводка и ключи, которые были, но не прочитались.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCaptureResult {
    pub meta: CaptureResultMeta,
    /// Ключи, присутствующие в результате, но непригодные (в порядке [`ResultKey`]).
    pub unreadable_keys: Vec<ResultKey>,
}

/// Время начала экспозиции кадра — по нему результат сопоставляется с номером кадра.
pub fn read_sensor_timestamp(source: &dyn MetadataSource) -> Option<SensorTimestamp> {
    match read_int64(source, tags::SENSOR_TIMESTAMP) {
        Ok(Some(nanos)) => Some(SensorTimestamp::from_nanos(nanos)),
        Ok(None) | Err(_) => None,
    }
}

/// Разбирает результат запроса. Номер кадра приходит снаружи (его знает слой колбэков).
pub fn parse_capture_result(source: &dyn MetadataSource, frame_number: FrameNumber) -> ParsedCaptureResult {
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
    ParsedCaptureResult {
        meta: CaptureResultMeta {
            frame_number,
            sensor_timestamp: timestamp.map(SensorTimestamp::from_nanos),
            exposure_time,
            sensitivity_iso: sensitivity,
            reported_frame_duration,
            rolling_shutter_skew,
        },
        unreadable_keys,
    }
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
