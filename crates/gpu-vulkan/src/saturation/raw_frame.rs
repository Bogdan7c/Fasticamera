//! [`HostRawFrame`] — RAW10-кадр на обычной памяти Vulkan для стенда насыщения (не зависит от `test-support`).
//!
//! Стенд насыщения — «стенд вне продукта» и обязан работать в release на телефоне, где настоящий кадр
//! камеры (`AHardwareBuffer`) для замера не нужен: достаточно буфера тех же размеров и раскладки.
//! Раскладка — как у буфера камеры: RAW10, четыре пикселя в пяти байтах, шаг строки в байтах.
//! Буфер host-visible (`HostBuffer`), владение «наше» (`AlreadyOwned` / `KeepOwned`): барьеров очереди FOREIGN нет.

use crate::compute::DescriptorResource;
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::hardware_buffer_cache::queue_handoff::{BufferAcquire, ReleaseTarget};
use crate::host_buffer::{HostBuffer, HostBufferUsage};
use crate::raw_mosaic_preview::MosaicSource;

/// Что не так с кадром (проверяется ДО обращения к GPU).
#[derive(Debug, thiserror::Error)]
pub enum RawFrameError {
    /// Нулевая ширина или высота.
    #[error("кадр RAW10 {width}x{height}: нулевая сторона")]
    ZeroSize {
        /// Ширина, пиксели.
        width: u32,
        /// Высота, пиксели.
        height: u32,
    },
    /// Шаг строки меньше упакованной строки (`width / 4 * 5` байт) или ширина не кратна 4.
    #[error(
        "шаг строки {row_stride_bytes} байт не вмещает строку RAW10 шириной {width} (нужно кратное 4 и ≥ width/4*5)"
    )]
    BadRowStride {
        /// Ширина, пиксели.
        width: u32,
        /// Шаг строки, байты.
        row_stride_bytes: u32,
    },
    /// Число байт не равно `высота × шаг строки`.
    #[error("{actual} байт для кадра {height} строк по {row_stride_bytes} байт (ожидалось {expected})")]
    ByteCountMismatch {
        /// Высота, пиксели.
        height: u32,
        /// Шаг строки, байты.
        row_stride_bytes: u32,
        /// Ожидаемое число байт.
        expected: u64,
        /// Фактическое число байт.
        actual: usize,
    },
    /// Ошибка GPU при создании буфера.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

/// Владелец буфера RAW10-кадра. Пока жив, [`Self::source`] пригоден для чтения compute-проходами.
pub struct HostRawFrame {
    /// Буфер: владелец памяти, на которую ссылается `source`. Поле ниже НЕ читается (держит память живой).
    _buffer: HostBuffer,
    source: MosaicSource,
}

impl HostRawFrame {
    /// Проверка размеров без GPU.
    pub fn check_layout(
        width: u32,
        height: u32,
        row_stride_bytes: u32,
        byte_count: usize,
    ) -> Result<(), RawFrameError> {
        if width == 0 || height == 0 {
            return Err(RawFrameError::ZeroSize { width, height });
        }
        if !width.is_multiple_of(4) || u64::from(row_stride_bytes) < u64::from(width / 4) * 5 {
            return Err(RawFrameError::BadRowStride {
                width,
                row_stride_bytes,
            });
        }
        let expected = u64::from(row_stride_bytes) * u64::from(height);
        if byte_count as u64 != expected {
            return Err(RawFrameError::ByteCountMismatch {
                height,
                row_stride_bytes,
                expected,
                actual: byte_count,
            });
        }
        Ok(())
    }

    /// Создаёт буфер и загружает упакованные байты RAW10 (`height` строк по `row_stride_bytes`).
    pub fn new(
        context: &GpuContext,
        width: u32,
        height: u32,
        row_stride_bytes: u32,
        bytes: &[u8],
    ) -> Result<Self, RawFrameError> {
        Self::check_layout(width, height, row_stride_bytes, bytes.len())?;
        let mut buffer = HostBuffer::new(context, bytes.len() as u64, HostBufferUsage::ReadOnlyStorage)?;
        buffer.write_bytes(0, bytes)?;
        let DescriptorResource::ReadOnlyBuffer {
            buffer: handle,
            range_bytes,
        } = buffer.binding()
        else {
            return Err(GpuError::InvalidState {
                what: "HostRawFrame: буфер не только-чтение (внутренняя ошибка)".to_owned(),
            }
            .into());
        };
        let source = MosaicSource::PackedRaw10 {
            buffer: handle,
            size_bytes: range_bytes,
            row_stride_bytes,
            width,
            height,
            acquire: BufferAcquire::AlreadyOwned,
            release: ReleaseTarget::KeepOwned,
        };
        Ok(Self {
            _buffer: buffer,
            source,
        })
    }

    /// Источник для ISP: буфер наш, барьера владения не нужно. Пригоден, пока жив `self`.
    pub fn source(&self) -> MosaicSource {
        self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_layouts_are_typed_errors() {
        assert!(matches!(
            HostRawFrame::check_layout(0, 4, 5, 20),
            Err(RawFrameError::ZeroSize { width: 0, height: 4 })
        ));
        assert!(matches!(
            HostRawFrame::check_layout(6, 4, 8, 32),
            Err(RawFrameError::BadRowStride { .. })
        ));
        assert!(matches!(
            HostRawFrame::check_layout(8, 4, 9, 36),
            Err(RawFrameError::BadRowStride { .. })
        ));
        assert!(matches!(
            HostRawFrame::check_layout(8, 4, 10, 39),
            Err(RawFrameError::ByteCountMismatch {
                expected: 40,
                actual: 39,
                ..
            })
        ));
        assert!(HostRawFrame::check_layout(8, 4, 10, 40).is_ok());
    }
}
