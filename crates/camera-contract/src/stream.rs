//! Режим потока: какой RAW, какая вырезка сенсора, какая частота — и как частота достигнута.

use serde::Serialize;

use crate::capabilities::{FpsRange, FrameDuration};
use crate::geometry::PixelSize;
use crate::ids::CameraId;
use crate::raw_format::RawFormat;

/// Какую часть сенсора читать.
///
/// На основной камере OnePlus 11 оба варианта приходят в одном буфере 4096×3072:
/// 16:9@60 — это режим сенсора 4096×2304, данные в строках 0–2303, ниже нули (00 C2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadoutAspect {
    /// Полный кадр 4:3 — фото-видоискатель.
    FourByThree,
    /// Вырезка 16:9 — видео.
    SixteenByNine,
}

/// Целевая частота кадров потока, к/с.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TargetFrameRate(u32);

/// Частота вне разумных границ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("целевая частота кадров должна быть от {min} до {max} к/с, а не {requested}", min = TargetFrameRate::MIN, max = TargetFrameRate::MAX)]
pub struct InvalidFrameRate {
    pub requested: u32,
}

impl TargetFrameRate {
    pub const MIN: u32 = 1;
    /// Режимы сенсора OnePlus 11 доходят до 480 к/с (не RAW); выше — явная ошибка конфига.
    pub const MAX: u32 = 480;

    pub fn new(frames_per_second: u32) -> Result<Self, InvalidFrameRate> {
        if !(Self::MIN..=Self::MAX).contains(&frames_per_second) {
            return Err(InvalidFrameRate {
                requested: frames_per_second,
            });
        }
        Ok(Self(frames_per_second))
    }

    pub const fn frames_per_second(self) -> u32 {
        self.0
    }

    /// Длительность одного кадра на этой частоте (60 → 16 666 667 нс, с округлением).
    pub const fn frame_duration(self) -> FrameDuration {
        let fps = self.0 as u64;
        FrameDuration::from_nanos((1_000_000_000 + fps / 2) / fps)
    }
}

/// Режим потока, который просит продукт. Не зависит от камеры: размер буфера движок
/// выбирает сам по возможностям камеры (самый большой публичный RAW этого формата),
/// поэтому при `switch_to` режим переносится на другую камеру как есть.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct StreamMode {
    pub raw_format: RawFormat,
    pub readout: ReadoutAspect,
    pub frame_rate: TargetFrameRate,
}

/// Режим, разрешённый для конкретной камеры: всё, что нужно бэкенду для сессии.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ResolvedStream {
    pub camera: CameraId,
    pub raw_format: RawFormat,
    /// Размер RAW-буфера из карты потоков HAL (для 16:9 основной — всё равно 4096×3072).
    pub buffer_size: PixelSize,
    pub readout: ReadoutAspect,
    pub frame_rate: TargetFrameRate,
}

/// Каким рычагом бэкенд просил частоту. Имена вендорских тегов сюда не попадают
/// (инвариант 7 README): только смысл рычага.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FrameRateMechanism {
    /// Ничего не задавали: частоту выбирает HAL (обычно 30 к/с).
    HalDefault,
    /// Публичный `CONTROL_AE_TARGET_FPS_RANGE` (основная 16:9@60, ширик 4:3@60).
    PublicAeTargetRange { range: FpsRange },
    /// Вендорское переопределение длительности кадра сенсора (основная 4:3@60, решение 23).
    VendorSensorFrameDurationOverride { frame_duration: FrameDuration },
}

/// Поток, который бэкенд реально сконфигурировал.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfiguredStream {
    pub stream: ResolvedStream,
    pub frame_rate_mechanism: FrameRateMechanism,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_bounds_are_enforced() {
        assert_eq!(TargetFrameRate::new(0), Err(InvalidFrameRate { requested: 0 }));
        assert_eq!(TargetFrameRate::new(481), Err(InvalidFrameRate { requested: 481 }));
        assert_eq!(TargetFrameRate::new(60).map(TargetFrameRate::frames_per_second), Ok(60));
    }

    #[test]
    fn frame_duration_matches_the_camx_override_value_from_session_00() {
        // 00 C2: overrideSensorFrameDuration = 16666667 нс для 60 к/с.
        let sixty = TargetFrameRate::new(60).expect("60 к/с допустимы");
        assert_eq!(sixty.frame_duration().as_nanos(), 16_666_667);
        let thirty = TargetFrameRate::new(30).expect("30 к/с допустимы");
        assert_eq!(thirty.frame_duration().as_nanos(), 33_333_333);
    }
}
