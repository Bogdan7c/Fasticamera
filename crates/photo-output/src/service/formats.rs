//! Запись форматов файлов за узким трейтом.
//!
//! Службе нужны три операции над байтами: DNG, блок Exif, вставка Exif в JPEG. Их реализуют модули
//! `dng`, `exif`, `jpeg_exif`; служба зависит от трейта, поэтому её тесты проверяют порядок и
//! учёт снимка, а не байты форматов (те проверяют тесты самих модулей).

use std::io::Write;

use crate::dng::{self, DngError, DngInputs};
use crate::exif::{self, ExifError, ExifInputs};
use crate::jpeg_exif::{self, JpegExifError};

/// Запись форматов файлов снимка.
pub trait StillFileFormats: Send + Sync {
    /// Пишет DNG потоком в `out`.
    fn write_dng(&self, inputs: &DngInputs<'_>, out: &mut dyn Write) -> Result<(), DngError>;

    /// Собирает блок Exif (TIFF без заголовка `Exif\0\0`).
    fn build_exif(&self, inputs: &ExifInputs<'_>) -> Result<Vec<u8>, ExifError>;

    /// Вставляет блок Exif в JPEG платформы.
    fn insert_jpeg_exif(&self, jpeg: &[u8], exif_tiff: &[u8]) -> Result<Vec<u8>, JpegExifError>;
}

/// Настоящие форматы: делегирует модулям `dng`, `exif`, `jpeg_exif`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhotoFileFormats;

impl StillFileFormats for PhotoFileFormats {
    fn write_dng(&self, inputs: &DngInputs<'_>, out: &mut dyn Write) -> Result<(), DngError> {
        dng::write_dng(inputs, out)
    }

    fn build_exif(&self, inputs: &ExifInputs<'_>) -> Result<Vec<u8>, ExifError> {
        exif::build_exif_tiff(inputs)
    }

    fn insert_jpeg_exif(&self, jpeg: &[u8], exif_tiff: &[u8]) -> Result<Vec<u8>, JpegExifError> {
        jpeg_exif::insert_exif_app1(jpeg, exif_tiff)
    }
}
