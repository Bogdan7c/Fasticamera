//! Проверка и подготовка входа кодеров снимка до перехода через JNI (HEIF и JPEG).
//!
//! Нейтральная часть (тестируется на ПК). Платформа не должна получать заведомо неверные
//! параметры: исключение Kotlin из-за размера 0 или качества 300 — это уже «платформа отказала»,
//! а здесь такой вход честно называется `EncodeError::InvalidInput`.

use std::time::Duration;

use shell_contract::still_media::{EncodeError, HeifEncodeSettings};

/// Заголовок блока Exif для `HeifWriter.addExifData`: `"Exif\0\0"`, затем TIFF
/// (документация androidx `HeifWriter.addExifData`; JEITA CP-3451C, 4.5.2).
const EXIF_BLOCK_HEADER: &[u8; 6] = b"Exif\0\0";

/// Допустимые начала TIFF: порядок байт и число 42 (TIFF 6.0, раздел 2 «Image File Header»).
const TIFF_MAGIC_LITTLE_ENDIAN: [u8; 4] = [b'I', b'I', 42, 0];
const TIFF_MAGIC_BIG_ENDIAN: [u8; 4] = [b'M', b'M', 0, 42];

/// Коды результата `StillHeifSession.finish` — контракт с Kotlin (`StillHeifEncoder.kt`, `FINISH_*`).
pub const FINISH_DONE: i32 = 0;
/// Кодер не уложился в срок (Kotlin поймал `TimeoutException` от `HeifWriter.stop`).
pub const FINISH_TIMED_OUT: i32 = 1;

/// Параметры HEIF, проверенные и приведённые к типам JNI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeifRequest {
    pub width: i32,
    pub height: i32,
    pub rotation_degrees: i32,
    pub quality: i32,
    /// Блок Exif уже с заголовком `Exif\0\0` (если Exif есть).
    pub exif_block: Option<Vec<u8>>,
}

/// Проверяет настройки HEIF и готовит блок Exif для кодера.
pub fn prepare_heif_request(settings: &HeifEncodeSettings) -> Result<HeifRequest, EncodeError> {
    let width = positive_dimension("ширина", settings.width)?;
    let height = positive_dimension("высота", settings.height)?;
    if !matches!(settings.rotation_degrees, 0 | 90 | 180 | 270) {
        return Err(EncodeError::InvalidInput(format!(
            "поворот HEIF {}° не кратен 90 или вне 0–270",
            settings.rotation_degrees
        )));
    }
    if settings.quality > 100 {
        return Err(EncodeError::InvalidInput(format!(
            "качество HEIF {} вне 0–100",
            settings.quality
        )));
    }
    let exif_block = settings.exif_tiff.as_deref().map(exif_block_from_tiff).transpose()?;
    Ok(HeifRequest {
        width,
        height,
        // Значение проверено выше (0–270), преобразование без потерь.
        rotation_degrees: settings.rotation_degrees as i32,
        quality: i32::from(settings.quality),
        exif_block,
    })
}

/// TIFF (без заголовка) → блок для `HeifWriter.addExifData`. Пустой или не-TIFF вход отвергается:
/// кодер записал бы мусор в файл и молча.
pub fn exif_block_from_tiff(tiff: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let starts_like_tiff = tiff.starts_with(&TIFF_MAGIC_LITTLE_ENDIAN) || tiff.starts_with(&TIFF_MAGIC_BIG_ENDIAN);
    if !starts_like_tiff {
        return Err(EncodeError::InvalidInput(
            "Exif для HEIF должен начинаться с заголовка TIFF (II*\\0 или MM\\0*), без `Exif\\0\\0`".to_owned(),
        ));
    }
    let mut block = Vec::with_capacity(EXIF_BLOCK_HEADER.len() + tiff.len());
    block.extend_from_slice(EXIF_BLOCK_HEADER);
    block.extend_from_slice(tiff);
    Ok(block)
}

/// Вход JPEG: размеры и качество в типах JNI после проверки длины буфера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegRequest {
    pub width: i32,
    pub height: i32,
    pub quality: i32,
}

/// Проверяет вход `JpegCompressor::compress`: ненулевые размеры, качество 0–100 и длина буфера
/// ровно `width * height * 4` (RGBA8 построчно, без выравнивания).
pub fn prepare_jpeg_request(rgba_len: usize, width: u32, height: u32, quality: u8) -> Result<JpegRequest, EncodeError> {
    let width_jni = positive_dimension("ширина", width)?;
    let height_jni = positive_dimension("высота", height)?;
    if quality > 100 {
        return Err(EncodeError::InvalidInput(format!("качество JPEG {quality} вне 0–100")));
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| EncodeError::InvalidInput(format!("размер {width}×{height} переполняет usize")))?;
    if rgba_len != expected_len {
        return Err(EncodeError::InvalidInput(format!(
            "буфер RGBA {rgba_len} байт, а для {width}×{height} нужно {expected_len}"
        )));
    }
    Ok(JpegRequest {
        width: width_jni,
        height: height_jni,
        quality: i32::from(quality),
    })
}

/// Срок ожидания кодера для `HeifWriter.stop(timeoutMs)` в миллисекундах.
///
/// У `stop` значение 0 означает «ждать бесконечно» (исходники androidx `ResultWaiter.waitForResult`),
/// поэтому нулевой и субмиллисекундный срок поднимается до 1 мс: бесконечных ожиданий не бывает
/// (память `conventions`, раздел GPU).
pub fn stop_timeout_millis(timeout: Duration) -> i64 {
    i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX).max(1)
}

/// Код от `StillHeifSession.finish` → результат контракта.
pub fn heif_finish_result(code: i32, timeout: Duration) -> Result<(), EncodeError> {
    match code {
        FINISH_DONE => Ok(()),
        FINISH_TIMED_OUT => Err(EncodeError::TimedOut(timeout)),
        unknown => Err(EncodeError::Platform(format!(
            "StillHeifSession.finish вернул неизвестный код {unknown}"
        ))),
    }
}

fn positive_dimension(name: &str, value: u32) -> Result<i32, EncodeError> {
    match i32::try_from(value) {
        Ok(converted) if converted > 0 => Ok(converted),
        _ => Err(EncodeError::InvalidInput(format!("{name} {value} вне 1–{}", i32::MAX))),
    }
}

#[cfg(test)]
#[path = "still_encode_input_tests.rs"]
mod tests;
