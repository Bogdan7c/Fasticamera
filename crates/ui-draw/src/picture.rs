//! Картинка для списка рисования (сессия 14g): пиксели RGBA8 в обычной памяти за нейтральным хэндлом.
//!
//! Первый потребитель — миниатюра последнего снимка (решение 155): приложение получает маленькую копию
//! снимка (≈ 256 px) и отдаёт её экрану как [`UiPicture`]; экран кладёт её в `DrawItem::Image`, рендер
//! загружает пиксели в текстуру.
//!
//! Почему хэндл, а не `Vec<u8>` в элементе:
//! * список рисования клонируется и сравнивается каждый кадр — `Arc` делает это бесплатным;
//! * рендеру нужна идентичность картинки, чтобы загружать её в текстуру ОДИН раз, а не каждый кадр.
//!   Идентичность — [`PictureVersion`]: номер выдаёт только конструктор из счётчика процесса, поэтому
//!   две разные картинки никогда не получат один номер (иначе кэш рендера молча показал бы чужую текстуру).
//!
//! Кодировка источника — явная ([`PictureEncoding`]): цвета UI задаются в закодированных значениях экрана
//! (`color`), а у картинки свой цветовой договор; переводит в цвет экрана рендер (и его эталон на CPU).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

/// Байтов на пиксель RGBA8.
const BYTES_PER_PIXEL: u64 = 4;

/// Счётчик номеров картинок процесса. Начинается с 1: ноль не выдаётся никогда.
static NEXT_PICTURE_VERSION: AtomicU64 = AtomicU64::new(1);

/// В какой кодировке пиксели картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PictureEncoding {
    /// Кривая sRGB и основные цвета sRGB (BT.709), альфа прямая. Так закодирован снимок (охват снимка
    /// sRGB независимо от экрана, сессия 14) и обычные картинки из галереи.
    Srgb,
}

/// Размер картинки в пикселях источника.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PictureSize {
    pub width: u32,
    pub height: u32,
}

impl PictureSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Число пикселей (u64 — без переполнения).
    pub const fn pixel_count(self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Номер картинки: одинаковый номер = те же пиксели. Создать номер снаружи нельзя — только получить
/// у картинки ([`UiPicture::version`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PictureVersion(u64);

impl PictureVersion {
    /// Число для логов и диагностики.
    pub fn get(self) -> u64 {
        self.0
    }

    fn mint() -> Self {
        // Relaxed достаточно: нужна только уникальность номера, порядок между потоками не важен.
        Self(NEXT_PICTURE_VERSION.fetch_add(1, Ordering::Relaxed))
    }
}

/// Почему картинку нельзя собрать.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PictureError {
    /// Ширина или высота равна нулю: рисовать нечего.
    #[error("пустой размер картинки {size:?}")]
    EmptySize { size: PictureSize },
    /// Длина буфера не равна `ширина × высота × 4`.
    #[error("в буфере картинки {actual} байт, а размеру {size:?} нужно {expected}")]
    LengthMismatch {
        size: PictureSize,
        expected: u64,
        actual: u64,
    },
}

struct PictureInner {
    version: PictureVersion,
    size: PictureSize,
    encoding: PictureEncoding,
    /// RGBA8 построчно, без промежутков между строками; длина проверена конструктором.
    rgba: Vec<u8>,
}

/// Картинка: неизменяемые пиксели RGBA8 в обычной памяти за `Arc`.
///
/// Сравнение (`PartialEq`) — по номеру версии: один номер выдаётся ровно одной картинке, поэтому
/// «равны» значит «те же пиксели», а проверка не читает 0,2 МБ байтов каждый кадр.
#[derive(Clone)]
pub struct UiPicture(Arc<PictureInner>);

impl UiPicture {
    /// Собирает картинку из готового буфера (без копии) и выдаёт ей новый номер.
    pub fn from_rgba8(size: PictureSize, encoding: PictureEncoding, rgba: Vec<u8>) -> Result<Self, PictureError> {
        if size.width == 0 || size.height == 0 {
            return Err(PictureError::EmptySize { size });
        }
        let expected = size.pixel_count().saturating_mul(BYTES_PER_PIXEL);
        let actual = u64::try_from(rgba.len()).unwrap_or(u64::MAX);
        if expected != actual {
            return Err(PictureError::LengthMismatch { size, expected, actual });
        }
        Ok(Self(Arc::new(PictureInner {
            version: PictureVersion::mint(),
            size,
            encoding,
            rgba,
        })))
    }

    pub fn version(&self) -> PictureVersion {
        self.0.version
    }

    pub fn size(&self) -> PictureSize {
        self.0.size
    }

    pub fn encoding(&self) -> PictureEncoding {
        self.0.encoding
    }

    /// Байты RGBA8 построчно (длина = `ширина × высота × 4`).
    pub fn rgba8(&self) -> &[u8] {
        &self.0.rgba
    }

    /// Пиксель `(x, y)` источника; вне картинки — `None`.
    pub fn texel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let size = self.0.size;
        if x >= size.width || y >= size.height {
            return None;
        }
        let index = (u64::from(y) * u64::from(size.width) + u64::from(x)) * BYTES_PER_PIXEL;
        let start = usize::try_from(index).ok()?;
        let bytes = self.0.rgba.get(start..start + 4)?;
        Some([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

impl PartialEq for UiPicture {
    fn eq(&self, other: &Self) -> bool {
        self.0.version == other.0.version
    }
}

impl Eq for UiPicture {}

/// `Debug` без байтов: пиксели в логе бесполезны и огромны.
impl fmt::Debug for UiPicture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiPicture")
            .field("version", &self.0.version.0)
            .field("size", &self.0.size)
            .field("encoding", &self.0.encoding)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тесты")]

    use super::*;

    fn picture(width: u32, height: u32) -> UiPicture {
        let bytes = (0..width * height * 4).map(|value| (value % 251) as u8).collect();
        UiPicture::from_rgba8(PictureSize::new(width, height), PictureEncoding::Srgb, bytes).unwrap()
    }

    #[test]
    fn every_picture_gets_its_own_version_and_clones_share_it() {
        let first = picture(2, 2);
        let second = picture(2, 2);
        // Одинаковые байты — всё равно разные картинки: номер выдаётся при сборке.
        assert_ne!(first.version(), second.version());
        assert_ne!(first, second);
        let clone = first.clone();
        assert_eq!(clone, first);
        assert_eq!(clone.version(), first.version());
    }

    #[test]
    fn wrong_length_and_empty_size_are_rejected() {
        let short = UiPicture::from_rgba8(PictureSize::new(2, 2), PictureEncoding::Srgb, vec![0; 15]);
        assert_eq!(
            short.unwrap_err(),
            PictureError::LengthMismatch {
                size: PictureSize::new(2, 2),
                expected: 16,
                actual: 15
            }
        );
        let empty = UiPicture::from_rgba8(PictureSize::new(0, 3), PictureEncoding::Srgb, Vec::new());
        assert_eq!(
            empty.unwrap_err(),
            PictureError::EmptySize {
                size: PictureSize::new(0, 3)
            }
        );
    }

    #[test]
    fn texel_reads_the_right_row_and_column_and_refuses_outside() {
        let bytes: Vec<u8> = (0..3 * 2 * 4).map(|value| value as u8).collect();
        let picture = UiPicture::from_rgba8(PictureSize::new(3, 2), PictureEncoding::Srgb, bytes).unwrap();
        // Пиксель (2, 1) — последний: байты 20..24.
        assert_eq!(picture.texel(2, 1), Some([20, 21, 22, 23]));
        assert_eq!(picture.texel(1, 0), Some([4, 5, 6, 7]));
        assert_eq!(picture.texel(3, 0), None);
        assert_eq!(picture.texel(0, 2), None);
    }

    #[test]
    fn debug_prints_size_but_not_pixels() {
        let text = format!("{:?}", picture(1, 1));
        assert!(text.contains("width: 1"), "{text}");
        assert!(!text.contains("rgba"), "{text}");
    }
}
