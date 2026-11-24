//! Байты картинки выхода работы стенда ([`OutputPixels`]) для сравнения «эталон и вариант» (сессия 16a, пакет IL).
//!
//! # Зачем (простыми словами)
//! Отпечаток отвечает «совпало бит в бит или нет». Для неточных оптимизаций (решение 213: не больше 1 младшего
//! разряда 10-битного выхода) нужно знать, насколько именно. Работа-вариант и работа-эталон гонятся в одном
//! бинаре на одной сцене; после прогона у каждой читаются байты содержимого картинки, а [`OutputPixels::diff_against`]
//! считает отчёт помощником [`compare_image_bytes`].
//!
//! Читается ТОЛЬКО содержимое (левый верхний угол ёмкости), строки плотные: хвост ёмкости в сравнение не попадает.

use ash::vk;

use super::digest::DigestError;
use super::image_diff::{ImageDiffError, ImageDiffReport, ImageFormat, compare_image_bytes};
use crate::storage_image::{StorageImage, StorageImageFormat, StorageImagePreviousUse};

/// Плотные байты картинки с форматом и размером.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPixels {
    /// Формат пикселя.
    pub format: ImageFormat,
    /// Ширина содержимого.
    pub width: u32,
    /// Высота содержимого.
    pub height: u32,
    /// Строки подряд без отступов.
    bytes: Vec<u8>,
}

/// Почему две картинки нельзя сравнить.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutputPixelsError {
    /// Разный размер или формат: это не пара «эталон и вариант».
    #[error(
        "картинки разные: {first_width}x{first_height} {first_format} против {second_width}x{second_height} {second_format}"
    )]
    ShapeMismatch {
        first_width: u32,
        first_height: u32,
        first_format: &'static str,
        second_width: u32,
        second_height: u32,
        second_format: &'static str,
    },
    /// Ошибка самого сравнения.
    #[error(transparent)]
    Diff(#[from] ImageDiffError),
}

impl OutputPixels {
    /// Картинка из плотных байт (для тестов и адаптеров, которые уже имеют байты).
    pub fn from_dense_bytes(format: ImageFormat, width: u32, height: u32, bytes: Vec<u8>) -> Option<Self> {
        let expected = width as usize * height as usize * format.bytes_per_pixel();
        (bytes.len() == expected).then_some(Self {
            format,
            width,
            height,
            bytes,
        })
    }

    /// Плотные байты.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Отчёт «насколько эта картинка отличается от эталона `reference`» (в кодах формата, см. `image_diff`).
    pub fn diff_against(&self, reference: &Self) -> Result<ImageDiffReport, OutputPixelsError> {
        if (self.width, self.height, self.format) != (reference.width, reference.height, reference.format) {
            return Err(OutputPixelsError::ShapeMismatch {
                first_width: reference.width,
                first_height: reference.height,
                first_format: reference.format.name(),
                second_width: self.width,
                second_height: self.height,
                second_format: self.format.name(),
            });
        }
        Ok(compare_image_bytes(&reference.bytes, &self.bytes, self.format)?)
    }
}

/// Формат сравнения для формата storage-картинки.
fn image_format_of(format: StorageImageFormat) -> ImageFormat {
    match format {
        StorageImageFormat::A2B10G10R10Unorm => ImageFormat::Rgb10A2,
        StorageImageFormat::Rgba16Float => ImageFormat::Rgba16F,
    }
}

/// Читает содержимое `content` storage-картинки блокирующе (звать после `wait_idle`, вне окна замера). Раскладка
/// картинки после чтения прежняя.
pub fn read_storage_image_pixels(
    image: &StorageImage,
    current_use: StorageImagePreviousUse,
    content: vk::Extent2D,
) -> Result<OutputPixels, DigestError> {
    let whole = crate::storage_image::readback::read_storage_image_bytes(image, current_use)?;
    let capacity = image.extent();
    let bytes_per_texel = image.format().bytes_per_texel() as usize;
    let row_pitch = capacity.width as usize * bytes_per_texel;
    let row_bytes = content.width as usize * bytes_per_texel;
    if content.width > capacity.width
        || content.height > capacity.height
        || whole.len() < row_pitch * capacity.height as usize
    {
        return Err(DigestError::ContentOutsideImage {
            content_width: content.width,
            content_height: content.height,
            capacity_width: capacity.width,
            capacity_height: capacity.height,
            read_bytes: whole.len(),
        });
    }
    let mut bytes = Vec::with_capacity(row_bytes * content.height as usize);
    for row in 0..content.height as usize {
        let start = row * row_pitch;
        bytes.extend_from_slice(&whole[start..start + row_bytes]);
    }
    Ok(OutputPixels {
        format: image_format_of(image.format()),
        width: content.width,
        height: content.height,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb10(red: u32, green: u32, blue: u32) -> [u8; 4] {
        (red | (green << 10) | (blue << 20) | (3 << 30)).to_le_bytes()
    }

    fn picture(pixels: &[[u8; 4]]) -> OutputPixels {
        OutputPixels::from_dense_bytes(ImageFormat::Rgb10A2, pixels.len() as u32, 1, pixels.concat()).unwrap()
    }

    #[test]
    fn diff_counts_pixels_that_differ_and_names_the_largest_difference() {
        let reference = picture(&[rgb10(100, 100, 100), rgb10(200, 200, 200), rgb10(300, 300, 300)]);
        let variant = picture(&[rgb10(100, 100, 100), rgb10(201, 200, 200), rgb10(300, 300, 303)]);
        let report = variant.diff_against(&reference).unwrap();
        assert_eq!(report.max_delta, [1, 0, 3]);
        assert_eq!((report.differing_at_least_1, report.differing_at_least_2), (2, 1));
        assert_eq!(report.pixel_count, 3);
    }

    #[test]
    fn identical_pictures_report_zero() {
        let reference = picture(&[rgb10(5, 6, 7), rgb10(8, 9, 10)]);
        assert!(reference.clone().diff_against(&reference).unwrap().is_identical());
    }

    #[test]
    fn pictures_of_another_shape_are_refused_with_a_typed_error() {
        let two = picture(&[rgb10(1, 1, 1), rgb10(2, 2, 2)]);
        let one = picture(&[rgb10(1, 1, 1)]);
        assert!(matches!(
            one.diff_against(&two),
            Err(OutputPixelsError::ShapeMismatch { .. })
        ));
    }

    #[test]
    fn bytes_that_do_not_fill_the_picture_are_refused() {
        assert!(OutputPixels::from_dense_bytes(ImageFormat::Rgb10A2, 2, 2, vec![0; 15]).is_none());
    }
}
