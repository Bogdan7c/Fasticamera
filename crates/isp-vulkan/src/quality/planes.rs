//! Плоскости f32 качественного демозаика: семь device-local буферов размера полезной области.
//!
//! # Почему отдельные буферы и почему семь
//! * Дескриптор storage-буфера видит не больше `maxStorageBufferRange` (128 МиБ на Adreno 740), а плоскость
//!   f32 кадра 4096×2304 — 36 МиБ: многоканальные данные в одном буфере не поместятся, поэтому одна
//!   плоскость = один буфер.
//! * Буферы переиспользуются по времени жизни данных (проходы идут строго по порядку, между ними —
//!   глобальный барьер). Без переиспользования понадобилось бы 17 плоскостей (~640 МиБ на 4K):
//!
//! | слот | проходы 0–2 | 3–4 | 5–7 |
//! |------|-------------|-----|-----|
//! | `Mosaic`           | мозаика (0→4)         |                   | уточнённый G (5→выход) |
//! | `ScratchHorizontal`| G_H (1→2)             | R в зелёных (3→4) | уточнённый R (6→7)     |
//! | `ScratchVertical`  | G_V (1→2)             | B в зелёных (3→4) | уточнённый B (6→7)     |
//! | `Green`            | выбранный G (2→5)     |                   |                        |
//! | `Direction`        | решение H/V (2→7)     |                   |                        |
//! | `Red`              | D_H (2а→2б)           | R (4→6)           | итоговый R (7→выход)   |
//! | `Blue`             | D_V (2а→2б)           | B (4→6)           | итоговый B (7→выход)   |
//!
//! Итог демозаика (линейный RGB камеры) — `Red`, `Mosaic`, `Blue` ([`QualityPlanes::linear_rgb`]).

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::DeviceBuffer;

use super::error::QualityError;

/// Слот плоскости (см. таблицу в документации модуля).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneSlot {
    Mosaic,
    ScratchHorizontal,
    ScratchVertical,
    Green,
    Direction,
    Red,
    Blue,
}

impl PlaneSlot {
    const COUNT: usize = 7;

    fn index(self) -> usize {
        match self {
            Self::Mosaic => 0,
            Self::ScratchHorizontal => 1,
            Self::ScratchVertical => 2,
            Self::Green => 3,
            Self::Direction => 4,
            Self::Red => 5,
            Self::Blue => 6,
        }
    }
}

/// Три плоскости линейного RGB (вход шумодава и выходного прохода).
#[derive(Debug, Clone, Copy)]
pub struct LinearRgbPlanes<'a> {
    /// Красный.
    pub red: &'a DeviceBuffer,
    /// Зелёный.
    pub green: &'a DeviceBuffer,
    /// Синий.
    pub blue: &'a DeviceBuffer,
}

/// Байт на отсчёт плоскости (f32).
const BYTES_PER_SAMPLE: u64 = 4;

/// Размер плоскости в байтах; проверяет, что полезная область чётная и не меньше 2×2.
pub(crate) fn plane_bytes(extent: vk::Extent2D) -> Result<u64, QualityError> {
    let vk::Extent2D { width, height } = extent;
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(QualityError::InvalidInput {
            what: format!("размер плоскостей {width}x{height}: нужен чётный и не меньше 2x2 (ячейка CFA)"),
        });
    }
    Ok(u64::from(width) * u64::from(height) * BYTES_PER_SAMPLE)
}

/// Семь плоскостей демозаика (владелец буферов).
pub struct QualityPlanes {
    buffers: Vec<DeviceBuffer>,
    extent: vk::Extent2D,
}

impl std::fmt::Debug for QualityPlanes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "QualityPlanes({}x{})", self.extent.width, self.extent.height)
    }
}

impl QualityPlanes {
    /// Создаёт плоскости размера `extent` (чётный, ≥ 2×2; одна плоскость ≤ `maxStorageBufferRange`).
    pub fn new(context: &GpuContext, extent: vk::Extent2D) -> Result<Self, QualityError> {
        let bytes = plane_bytes(extent)?;
        let buffers = (0..PlaneSlot::COUNT)
            .map(|_| DeviceBuffer::new(context, bytes))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { buffers, extent })
    }

    /// Размер плоскостей, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Плоскость мозаики (вход демозаика; тесты загружают её напрямую).
    pub fn mosaic(&self) -> &DeviceBuffer {
        self.plane(PlaneSlot::Mosaic)
    }

    /// Итог демозаика: линейный RGB камеры (действителен после прохода 7 и до следующего кадра).
    pub fn linear_rgb(&self) -> LinearRgbPlanes<'_> {
        LinearRgbPlanes {
            red: self.plane(PlaneSlot::Red),
            green: self.plane(PlaneSlot::Mosaic),
            blue: self.plane(PlaneSlot::Blue),
        }
    }

    pub(crate) fn plane(&self, slot: PlaneSlot) -> &DeviceBuffer {
        // Вектор создаётся ровно из COUNT буферов, индекс слота всегда в пределах.
        &self.buffers[slot.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_size_requires_even_extent_of_at_least_two() {
        assert_eq!(
            plane_bytes(vk::Extent2D {
                width: 4096,
                height: 2304
            })
            .unwrap(),
            37_748_736
        );
        for (width, height) in [(1, 2), (2, 1), (3, 4), (4, 5), (0, 0)] {
            assert!(plane_bytes(vk::Extent2D { width, height }).is_err(), "{width}x{height}");
        }
    }
}
