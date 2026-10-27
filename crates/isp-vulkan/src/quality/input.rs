//! Вход кадра стенда и его проверка против источника — чистая логика (тестируется без GPU).

use ash::vk;
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use isp_model::params::IspParams;

use super::error::QualityError;
use super::frame_params::SourceAddressing;

/// Поколение потока камеры: меняется — временной шумодав сбрасывает историю (другой поток = другая
/// сцена/экспозиция, смешивать нельзя). Render-loop берёт его из эпохи `raw-frame-stream`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamGeneration(pub u64);

/// Всё, что нужно на один кадр.
#[derive(Debug, Clone, Copy)]
pub struct VideoFrameInput<'a> {
    /// Кадр камеры, уже забранный для compute вызывающим (`record_acquire_for(.., Compute)`).
    pub source: &'a MosaicSource,
    /// Параметры обработки (`DemosaicMethod::Quality`).
    pub params: &'a IspParams,
    /// Полезная область кадра; её размер обязан совпасть с размером конвейера.
    pub valid_area: ValidArea,
    /// Поколение потока (для шумодава).
    pub stream: StreamGeneration,
    /// Монотонный номер кадра (метка для приёмника).
    pub sequence: u64,
    /// Время кадра, нс (время сенсора; для кодера — PTS).
    pub presentation_time_ns: u64,
}

/// Проверяет полезную область против размера конвейера и источника; возвращает адресацию для шейдера.
pub(crate) fn source_addressing(
    source: &MosaicSource,
    valid_area: ValidArea,
    extent: vk::Extent2D,
) -> Result<SourceAddressing, QualityError> {
    let invalid = |what: String| Err(QualityError::InvalidInput { what });
    if !valid_area.left.is_multiple_of(2) || !valid_area.top.is_multiple_of(2) {
        return invalid(format!(
            "полезная область {valid_area:?}: левый верхний угол должен быть чётным"
        ));
    }
    if (valid_area.width, valid_area.height) != (extent.width, extent.height) {
        return invalid(format!(
            "полезная область {}x{} не совпадает с размером конвейера {}x{}",
            valid_area.width, valid_area.height, extent.width, extent.height
        ));
    }
    let right = u64::from(valid_area.left) + u64::from(valid_area.width);
    let bottom = u64::from(valid_area.top) + u64::from(valid_area.height);
    match *source {
        MosaicSource::SampledR16 { width, height, .. } => {
            if right > u64::from(width) || bottom > u64::from(height) {
                return invalid(format!(
                    "полезная область {valid_area:?} выходит за кадр RAW16 {width}x{height}"
                ));
            }
            Ok(SourceAddressing {
                left: valid_area.left,
                top: valid_area.top,
                row_stride_bytes: 0,
            })
        }
        MosaicSource::PackedRaw10 {
            size_bytes,
            row_stride_bytes,
            width,
            height,
            ..
        } => {
            if right > u64::from(width) || bottom > u64::from(height) {
                return invalid(format!(
                    "полезная область {valid_area:?} выходит за кадр RAW10 {width}x{height}"
                ));
            }
            // Последний байт, который прочтёт шейдер: младшие биты группы последнего пикселя.
            let last_group_byte = (right - 1) / 4 * 5 + 4;
            if last_group_byte >= u64::from(row_stride_bytes) {
                return invalid(format!(
                    "шаг строки RAW10 {row_stride_bytes} байт меньше группы последнего пикселя {right}"
                ));
            }
            let last_byte = (bottom - 1) * u64::from(row_stride_bytes) + last_group_byte;
            // Шейдер читает 32-битными словами: слово с последним байтом должно лежать в буфере целиком.
            let last_word_end = (last_byte / 4 + 1) * 4;
            if last_word_end > size_bytes {
                return invalid(format!(
                    "буфер RAW10 {size_bytes} байт меньше, чем нужно полезной области ({last_word_end})"
                ));
            }
            Ok(SourceAddressing {
                left: valid_area.left,
                top: valid_area.top,
                row_stride_bytes,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpu_vulkan::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};

    fn raw16(width: u32, height: u32) -> MosaicSource {
        MosaicSource::SampledR16 {
            view: vk::ImageView::null(),
            image: vk::Image::null(),
            width,
            height,
            acquire: ImageAcquire::FromForeignQueue,
            release: ReleaseTarget::ForeignQueue,
        }
    }

    fn raw10(width: u32, height: u32, row_stride_bytes: u32, size_bytes: u64) -> MosaicSource {
        MosaicSource::PackedRaw10 {
            buffer: vk::Buffer::null(),
            size_bytes,
            row_stride_bytes,
            width,
            height,
            acquire: BufferAcquire::FromForeignQueue,
            release: ReleaseTarget::ForeignQueue,
        }
    }

    fn area(left: u32, top: u32, width: u32, height: u32) -> ValidArea {
        ValidArea {
            left,
            top,
            width,
            height,
        }
    }

    const EXTENT: vk::Extent2D = vk::Extent2D { width: 8, height: 4 };

    #[test]
    fn raw16_area_must_match_extent_and_fit_frame() {
        let addressing = source_addressing(&raw16(12, 6), area(2, 2, 8, 4), EXTENT).unwrap();
        assert_eq!(
            (addressing.left, addressing.top, addressing.row_stride_bytes),
            (2, 2, 0)
        );
        assert!(
            source_addressing(&raw16(9, 6), area(2, 2, 8, 4), EXTENT).is_err(),
            "за правым краем"
        );
        assert!(
            source_addressing(&raw16(12, 6), area(0, 0, 8, 6), EXTENT).is_err(),
            "не тот размер"
        );
        assert!(
            source_addressing(&raw16(12, 6), area(1, 0, 8, 4), EXTENT).is_err(),
            "нечётный угол"
        );
    }

    #[test]
    fn raw10_checks_row_stride_and_buffer_size_for_word_reads() {
        // 8 пикселей = 2 группы по 5 байт = 10 байт в строке; шаг 12, 4 строки = 48 байт.
        let addressing = source_addressing(&raw10(8, 4, 12, 48), area(0, 0, 8, 4), EXTENT).unwrap();
        assert_eq!(addressing.row_stride_bytes, 12);
        assert!(
            source_addressing(&raw10(8, 4, 9, 48), area(0, 0, 8, 4), EXTENT).is_err(),
            "шаг меньше 10 байт"
        );
        // Последний байт — 3·12 + 9 = 45; слово 44..48 должно поместиться.
        assert!(source_addressing(&raw10(8, 4, 12, 46), area(0, 0, 8, 4), EXTENT).is_err());
        assert!(source_addressing(&raw10(8, 4, 12, 48), area(0, 0, 8, 6), EXTENT).is_err());
    }
}
