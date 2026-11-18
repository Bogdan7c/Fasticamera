//! Метаданные результата запроса, которые нужны движку (темп потока) и проверкам.
//!
//! Полный паспорт RAW-кадра — [`crate::frame_passport::RawFramePassport`].

use std::time::Duration;

use serde::Serialize;

use crate::capabilities::FrameDuration;
use crate::three_a::ThreeAResultState;

/// Время начала экспозиции кадра от сенсора (`ACAMERA_SENSOR_TIMESTAMP`), наносекунды.
///
/// На OnePlus 11 источник меток — `REALTIME`, то есть шкала `CLOCK_BOOTTIME` (00 C8, V2):
/// та же, что у гироскопа. Отдельный тип, чтобы не спутать с `CLOCK_MONOTONIC` касаний.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SensorTimestamp(i64);

impl SensorTimestamp {
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// Сколько прошло от `earlier` до `self`; `None`, если метки не по порядку.
    pub fn duration_since(self, earlier: SensorTimestamp) -> Option<Duration> {
        let delta = self.0.checked_sub(earlier.0)?;
        u64::try_from(delta).ok().map(Duration::from_nanos)
    }
}

/// Номер кадра в последовательности запросов (`frameNumber` колбэка результата).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct FrameNumber(pub i64);

/// Ключ результата, наличие которого проверяем.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKey {
    SensorTimestamp,
    ExposureTime,
    Sensitivity,
    FrameDuration,
    RollingShutterSkew,
}

/// Сводка результата одного запроса. `None` в поле значит «HAL ключ не отдал».
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CaptureResultMeta {
    pub frame_number: FrameNumber,
    pub sensor_timestamp: Option<SensorTimestamp>,
    pub exposure_time: Option<Duration>,
    pub sensitivity_iso: Option<u32>,
    /// `ACAMERA_SENSOR_FRAME_DURATION` как его сообщил HAL. В режиме CamX 4:3@60 он **ложный**
    /// (33,3 мс при реальных 16,7 мс, 00 C2): темп считаем только по меткам сенсора.
    pub reported_frame_duration: Option<FrameDuration>,
    pub rolling_shutter_skew: Option<Duration>,
    /// Состояние автоматики и происхождение запроса (08): фокус, AE, объектив, мерцание.
    pub three_a: ThreeAResultState,
}

impl CaptureResultMeta {
    /// Какие из проверяемых ключей HAL не отдал (в порядке [`ResultKey`]).
    pub fn missing_keys(&self) -> Vec<ResultKey> {
        let presence = [
            (ResultKey::SensorTimestamp, self.sensor_timestamp.is_some()),
            (ResultKey::ExposureTime, self.exposure_time.is_some()),
            (ResultKey::Sensitivity, self.sensitivity_iso.is_some()),
            (ResultKey::FrameDuration, self.reported_frame_duration.is_some()),
            (ResultKey::RollingShutterSkew, self.rolling_shutter_skew.is_some()),
        ];
        presence
            .into_iter()
            .filter(|(_, present)| !present)
            .map(|(key, _)| key)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_since_rejects_out_of_order_timestamps() {
        let first = SensorTimestamp::from_nanos(1_000);
        let second = SensorTimestamp::from_nanos(17_667_667);
        assert_eq!(second.duration_since(first), Some(Duration::from_nanos(17_666_667)));
        assert_eq!(first.duration_since(second), None);
    }

    #[test]
    fn missing_keys_lists_exactly_the_absent_ones() {
        let meta = CaptureResultMeta {
            frame_number: FrameNumber(7),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(1)),
            exposure_time: None,
            sensitivity_iso: Some(400),
            reported_frame_duration: None,
            rolling_shutter_skew: Some(Duration::from_micros(13_660)),
            three_a: ThreeAResultState::unreported(crate::three_a::RequestOrigin::Repeating),
        };
        assert_eq!(
            meta.missing_keys(),
            vec![ResultKey::ExposureTime, ResultKey::FrameDuration]
        );
    }
}
