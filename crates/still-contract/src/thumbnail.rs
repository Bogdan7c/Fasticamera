//! Миниатюра снимка для экрана камеры (сессия 14g, решения 154–156).
//!
//! Рендер, проявив снимок, уменьшает его на GPU (≈ 256 px по длинной стороне) и отдаёт маленькую копию
//! в ОБЫЧНОЙ памяти (решение 155): у пикселей миниатюры нет GPU-хозяина, поэтому копия переживает
//! сворачивание приложения, когда GPU снимков отпускается (решение 150).
//!
//! Миниатюра — НЕ итог снимка. Протокол «ровно один итог» (`develop::DevelopEvent`) она не трогает:
//! это отдельное событие, которое приходит раньше итога; снимок без миниатюры (сбой уменьшения)
//! остаётся удачным снимком.
//!
//! Договор о пикселях:
//! * RGBA8, построчно, без промежутков между строками, альфа = 255;
//! * кодировка — та же, что у основного снимка: кривая sRGB и основные цвета sRGB (охват снимка sRGB
//!   независимо от экрана, сессия 14);
//! * ориентация — «как покажет галерея»: поворот снимка (`StillFrameFacts::rotation`) уже применён к
//!   пикселям, зеркала у фронтальной камеры нет (файл тоже не зеркальный).

use std::fmt;

use camera_contract::geometry::PixelSize;

use crate::plan::StillId;
use crate::shot::PixelStorageSizeMismatch;

/// Байтов на пиксель RGBA8.
const BYTES_PER_PIXEL: u64 = 4;

/// Маленькая копия проявленного снимка (договор о пикселях — в доке модуля).
///
/// Поля закрыты: длина буфера проверяется один раз в [`StillThumbnail::new`], дальше инвариант
/// `rgba.len() == width × height × 4` держит тип.
#[derive(Clone, PartialEq, Eq)]
pub struct StillThumbnail {
    id: StillId,
    size: PixelSize,
    rgba: Vec<u8>,
}

impl StillThumbnail {
    /// Собирает миниатюру. Буфер обязан держать ровно `width × height × 4` байт; пустой размер
    /// (ширина или высота 0) — тоже несовпадение: рисовать нечего, а экрану нужен честный отказ.
    pub fn new(id: StillId, size: PixelSize, rgba: Vec<u8>) -> Result<Self, PixelStorageSizeMismatch> {
        let expected = size.pixel_count().saturating_mul(BYTES_PER_PIXEL);
        let actual = u64::try_from(rgba.len()).unwrap_or(u64::MAX);
        if expected == 0 || expected != actual {
            return Err(PixelStorageSizeMismatch { size, expected, actual });
        }
        Ok(Self { id, size, rgba })
    }

    /// Какого снимка миниатюра.
    pub fn id(&self) -> StillId {
        self.id
    }

    /// Размер в пикселях (уже повёрнутый: «как покажет галерея»).
    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Байты RGBA построчно.
    pub fn rgba_bytes(&self) -> &[u8] {
        &self.rgba
    }

    /// Забрать буфер без копии (приложение передаёт его экрану как картинку UI).
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
}

/// `Debug` без байтов: 0,2 МБ пикселей в логе никому не нужны.
impl fmt::Debug for StillThumbnail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StillThumbnail")
            .field("id", &self.id)
            .field("size", &self.size)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тесты")]

    use super::*;

    #[test]
    fn buffer_of_exact_size_is_accepted_and_given_back_without_copy() {
        let rgba = vec![7_u8; 3 * 2 * 4];
        let pointer = rgba.as_ptr();
        let thumbnail = StillThumbnail::new(StillId(5), PixelSize::new(3, 2), rgba).unwrap();
        assert_eq!(thumbnail.id(), StillId(5));
        assert_eq!(thumbnail.size(), PixelSize::new(3, 2));
        assert_eq!(thumbnail.rgba_bytes().len(), 24);
        // Буфер отдаётся тем же выделением памяти, без копии.
        let returned = thumbnail.into_rgba();
        assert_eq!(returned.as_ptr(), pointer);
    }

    #[test]
    fn wrong_length_and_empty_size_are_rejected_with_the_numbers() {
        let short = StillThumbnail::new(StillId(1), PixelSize::new(2, 2), vec![0; 15]).unwrap_err();
        assert_eq!((short.expected, short.actual), (16, 15));
        let empty = StillThumbnail::new(StillId(1), PixelSize::new(0, 4), Vec::new()).unwrap_err();
        assert_eq!((empty.expected, empty.actual), (0, 0));
    }

    #[test]
    fn debug_does_not_print_pixels() {
        let thumbnail = StillThumbnail::new(StillId(2), PixelSize::new(1, 1), vec![200, 201, 202, 255]).unwrap();
        let text = format!("{thumbnail:?}");
        assert!(text.contains("bytes: 4"), "{text}");
        assert!(!text.contains("201"), "{text}");
    }
}
