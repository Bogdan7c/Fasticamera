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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_count_of_full_50mp_frame_does_not_overflow_u32() {
        assert_eq!(PixelSize::new(8192, 6144).pixel_count(), 50_331_648);
        assert_eq!(PixelSize::new(u32::MAX, 2).pixel_count(), u64::from(u32::MAX) * 2);
    }
}
