//! Геометрия кадра видоискателя: полезная область, кадрирование, поворот сенсора → размер выхода.
//!
//! Все допущения шейдера проверяются здесь, до записи команд: шейдер за границами не проверяет и
//! читает ровно ячейки кадрирования. Поэтому строки буфера ниже полезной области (16:9 основной
//! камеры: строки 2304–3071 — нули/мусор, 00 C2) не читаются никогда: кадрирование обязано лежать
//! внутри полезной области, а та — внутри буфера.

use ash::vk;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::viewfinder_crop::ViewfinderCrop;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;

use super::error::ViewfinderError;

/// Байт в группе RAW10: 4 пикселя по 10 бит.
const RAW10_BYTES_PER_GROUP: u64 = 5;
/// Пикселей в группе RAW10.
const RAW10_PIXELS_PER_GROUP: u64 = 4;
/// Шейдер читает буфер RAW10 словами по 4 байта.
const RAW10_WORD_BYTES: u64 = 4;

/// Под какой размер выделяется выходная картинка ISP.
///
/// Непрерывный зум меняет размер кадрирования на каждом кадре. Если бы картинка пересоздавалась под каждый
/// размер, колесо зума аллоцировало бы GPU-память каждый кадр. Поэтому картинка выделяется ПОД ЁМКОСТЬ
/// (кадрирование при зуме 1× того же соотношения сторон), а ISP пишет реальное содержимое в её левый верхний
/// прямоугольник; читатели (компоновщик, подложка стекла) берут только его (`gpu_vulkan::source_window`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAllocation {
    /// Картинка ровно под размер кадрирования (сценарии, тесты: размер меняется — картинка пересоздаётся).
    Exact,
    /// Картинка под кадрирование такого размера (пиксели буфера RAW, до деления на ячейки): оно не меньше
    /// любого кадрирования того же соотношения сторон при большем зуме. Пересоздание — только когда оно
    /// меняется (смена потока или соотношения сторон).
    ReserveForCrop(PixelSize),
}

/// Геометрия кадра от вызывающего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewfinderGeometry {
    /// Полезная область буфера (паспорт потока).
    pub valid_area: PixelRect,
    /// Разрешённое кадрирование ([`ViewfinderCrop::resolve`]): чётное, внутри полезной области.
    pub crop: PixelRect,
    /// Ориентация сенсора: на сколько повернуть кадр по часовой, чтобы он встал прямо.
    pub sensor_orientation: SensorOrientation,
    /// Под какой размер выделить выходную картинку.
    pub allocation: OutputAllocation,
}

impl ViewfinderGeometry {
    /// Геометрия с кадрированием, разрешённым из выбора экрана. Картинка выделяется под ёмкость: то же
    /// соотношение сторон без зума (центр не важен — размер не зависит от него).
    pub fn from_crop(
        valid_area: PixelRect,
        crop: &ViewfinderCrop,
        sensor_orientation: SensorOrientation,
    ) -> Result<Self, ViewfinderError> {
        let capacity = ViewfinderCrop {
            zoom: camera_contract::viewfinder_crop::ZoomFactor::NONE,
            ..*crop
        };
        Ok(Self {
            valid_area,
            crop: crop.resolve(valid_area)?,
            sensor_orientation,
            allocation: OutputAllocation::ReserveForCrop(capacity.resolve(valid_area)?.size),
        })
    }

    /// Размер прямого (upright) выхода: половина кадрирования (ячейка 2×2 → пиксель), стороны
    /// меняются местами при повороте на 90° и 270°.
    pub fn output_extent(&self) -> vk::Extent2D {
        self.upright_extent_of(self.crop.size)
    }

    /// Размер выходной картинки (ёмкость): содержимое всегда помещается в левый верхний угол.
    pub fn allocation_extent(&self) -> vk::Extent2D {
        match self.allocation {
            OutputAllocation::Exact => self.output_extent(),
            OutputAllocation::ReserveForCrop(size) => self.upright_extent_of(size),
        }
    }

    fn upright_extent_of(&self, crop_size: PixelSize) -> vk::Extent2D {
        let cells_width = crop_size.width / 2;
        let cells_height = crop_size.height / 2;
        if self.sensor_orientation.quarter_turns_clockwise() % 2 == 1 {
            vk::Extent2D {
                width: cells_height,
                height: cells_width,
            }
        } else {
            vk::Extent2D {
                width: cells_width,
                height: cells_height,
            }
        }
    }
}

/// Проверенная геометрия — то, что уходит в uniform-блок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CheckedGeometry {
    /// Полезная область в пикселях буфера (над ней лежит карта виньетирования).
    pub valid_area: PixelRect,
    /// Кадрирование в пикселях буфера.
    pub crop: PixelRect,
    /// Размер выхода (содержимого кадра).
    pub output: vk::Extent2D,
    /// Размер выходной картинки (ёмкость ≥ выхода по обеим осям).
    pub allocation: vk::Extent2D,
    /// Поворот сенсора, четверти по часовой (0..=3).
    pub quarter_turns: u32,
    /// Шаг строки RAW10 в байтах; для RAW16 — 0 (шейдер его не читает).
    pub raw10_row_stride_bytes: u32,
}

/// Проверяет геометрию против источника (см. модуль).
pub(crate) fn check_geometry(
    geometry: &ViewfinderGeometry,
    source: &MosaicSource,
) -> Result<CheckedGeometry, ViewfinderError> {
    let ViewfinderGeometry { valid_area, crop, .. } = *geometry;
    if !is_cell_aligned(valid_area) {
        return Err(ViewfinderError::not_cell_aligned("полезная область", valid_area));
    }
    if !is_cell_aligned(crop) {
        return Err(ViewfinderError::not_cell_aligned("кадрирование", crop));
    }
    if !contains(valid_area, crop) {
        return Err(ViewfinderError::geometry(format!(
            "кадрирование {crop:?} выходит за полезную область {valid_area:?}"
        )));
    }
    let (source_width, source_height, raw10_row_stride_bytes) = match *source {
        MosaicSource::SampledR16 { width, height, .. } => (width, height, 0),
        MosaicSource::PackedRaw10 {
            width,
            height,
            row_stride_bytes,
            size_bytes,
            ..
        } => {
            check_raw10_bytes(crop, row_stride_bytes, size_bytes)?;
            (width, height, row_stride_bytes)
        }
    };
    if right(valid_area) > u64::from(source_width) || bottom(valid_area) > u64::from(source_height) {
        return Err(ViewfinderError::geometry(format!(
            "полезная область {valid_area:?} выходит за буфер {source_width}x{source_height}"
        )));
    }
    let (output, allocation) = (geometry.output_extent(), geometry.allocation_extent());
    if allocation.width < output.width || allocation.height < output.height {
        return Err(ViewfinderError::geometry(format!(
            "ёмкость выхода {allocation:?} меньше выхода {output:?}: кадрирование {crop:?} больше зарезервированного"
        )));
    }
    Ok(CheckedGeometry {
        valid_area,
        crop,
        output,
        allocation,
        quarter_turns: geometry.sensor_orientation.quarter_turns_clockwise(),
        raw10_row_stride_bytes,
    })
}

/// Все байты, которые шейдер прочитает для кадрирования, лежат в буфере (словами по 4 байта).
fn check_raw10_bytes(crop: PixelRect, row_stride_bytes: u32, size_bytes: u64) -> Result<(), ViewfinderError> {
    let stride = u64::from(row_stride_bytes);
    // Байты строки до правого края кадрирования включительно (последняя группа целиком).
    let row_bytes = right(crop).div_ceil(RAW10_PIXELS_PER_GROUP) * RAW10_BYTES_PER_GROUP;
    let last_byte_end = (bottom(crop) - 1) * stride + row_bytes;
    if stride < row_bytes || last_byte_end.next_multiple_of(RAW10_WORD_BYTES) > size_bytes {
        return Err(ViewfinderError::geometry(format!(
            "RAW10: шаг строки {row_stride_bytes} или буфер {size_bytes} байт не вмещают кадрирование {crop:?} (нужно {last_byte_end})"
        )));
    }
    Ok(())
}

fn is_cell_aligned(rect: PixelRect) -> bool {
    let even = [rect.left, rect.top, rect.size.width, rect.size.height]
        .iter()
        .all(|value| value % 2 == 0);
    even && rect.size.width >= 2 && rect.size.height >= 2
}

fn right(rect: PixelRect) -> u64 {
    u64::from(rect.left) + u64::from(rect.size.width)
}

fn bottom(rect: PixelRect) -> u64 {
    u64::from(rect.top) + u64::from(rect.size.height)
}

fn contains(outer: PixelRect, inner: PixelRect) -> bool {
    inner.left >= outer.left && inner.top >= outer.top && right(inner) <= right(outer) && bottom(inner) <= bottom(outer)
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
