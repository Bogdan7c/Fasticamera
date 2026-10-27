//! Геометрия в пикселях сенсора.

use serde::Serialize;

/// Размер в пикселях (ширина × высота).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

impl PixelSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Число пикселей (u64: 8192×6144 и больше не переполняются).
    pub const fn pixel_count(self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Прямоугольник в пикселях сенсора: левый верхний угол и размер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct PixelRect {
    pub left: u32,
    pub top: u32,
    pub size: PixelSize,
}

/// Ориентация сенсора относительно естественной (портретной) ориентации телефона
/// (`ACAMERA_SENSOR_ORIENTATION`): на сколько градусов по часовой стрелке надо повернуть кадр
/// сенсора, чтобы он встал прямо на экране в естественной ориентации.
///
/// Camera2 допускает только кратные 90°; другое значение — ошибка разбора, а не округление.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorOrientation {
    Degrees0,
    Degrees90,
    Degrees180,
    Degrees270,
}

impl SensorOrientation {
    /// Разбирает значение HAL; `None` — значение не кратно 90° или вне 0..360.
    pub const fn from_degrees(degrees: i32) -> Option<Self> {
        match degrees {
            0 => Some(Self::Degrees0),
            90 => Some(Self::Degrees90),
            180 => Some(Self::Degrees180),
            270 => Some(Self::Degrees270),
            _ => None,
        }
    }

    /// Число четвертей оборота по часовой стрелке (0–3).
    pub const fn quarter_turns_clockwise(self) -> u32 {
        match self {
            Self::Degrees0 => 0,
            Self::Degrees90 => 1,
            Self::Degrees180 => 2,
            Self::Degrees270 => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_count_of_full_50mp_frame_does_not_overflow_u32() {
        assert_eq!(PixelSize::new(8192, 6144).pixel_count(), 50_331_648);
        assert_eq!(PixelSize::new(u32::MAX, 2).pixel_count(), u64::from(u32::MAX) * 2);
    }

    #[test]
    fn sensor_orientation_accepts_only_quarter_turns() {
        assert_eq!(SensorOrientation::from_degrees(90), Some(SensorOrientation::Degrees90));
        assert_eq!(
            SensorOrientation::from_degrees(270).map(SensorOrientation::quarter_turns_clockwise),
            Some(3)
        );
        assert_eq!(SensorOrientation::from_degrees(45), None);
        assert_eq!(SensorOrientation::from_degrees(360), None);
        assert_eq!(SensorOrientation::from_degrees(-90), None);
    }
}
