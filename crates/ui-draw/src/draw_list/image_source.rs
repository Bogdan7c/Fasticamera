//! Откуда берутся пиксели элемента-картинки: готовая [`UiPicture`] в памяти или картинка, которую рендер
//! рисует на GPU сам на каждом кадре ([`LiveImage`]).
//!
//! # Договор «живых» картинок
//! Пиксели живой картинки НИКОГДА не лежат в обычной памяти: их считает GPU (например, приборы
//! цвета 14h: compute-шейдер накапливает счётчики и рисует картинку прибора). Список рисования хранит
//! только «какая именно» ([`LiveImage`]), а рендер на каждом кадре получает от владельца картинки готовое
//! изображение GPU. Новый источник = новый вариант [`LiveImage`] (плюс его размещение у владельца),
//! правок в соседних вариантах не нужно.
//!
//! # Кодировка
//! У картинки из памяти кодировка своя ([`PictureEncoding::Srgb`](crate::picture::PictureEncoding)), и её
//! переводят в цвет экрана. Живая картинка уже посчитана в цвете ЭКРАНА с ПРЯМОЙ (не предумноженной) альфой:
//! перевода нет. Различие выражено типом [`ImageColorSpace`], а не флагом.

use thiserror::Error;

use crate::picture::{PictureSize, UiPicture};

/// Картинка, которую рендер получает от владельца на каждом кадре прямо в видеопамяти.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveImage {
    /// Картинка цветового прибора (waveform, parade, вектороскоп; сессия 14h): цвет экрана, прямая
    /// альфа, фон прозрачный.
    ColorScope,
}

/// В каком цветовом договоре пиксели источника картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageColorSpace {
    /// sRGB-кодировка источника: рендер переводит в цвет экрана ([`srgb_picture_to_display`](crate::color::srgb_picture_to_display)).
    SrgbPicture,
    /// Уже цвет экрана (кодировка и основные цвета текущего экрана), альфа прямая: перевода нет.
    DisplayEncoded,
}

impl ImageColorSpace {
    /// Код для упаковки в экземпляр шейдера (значения совпадают с `COLOR_SPACE_*` в `ui_image_data.glsl`).
    pub const fn shader_code(self) -> u32 {
        match self {
            Self::SrgbPicture => 0,
            Self::DisplayEncoded => 1,
        }
    }
}

/// Источник пикселей элемента-картинки.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageSource {
    /// Готовые пиксели RGBA8 в обычной памяти (миниатюра снимка, 14g).
    Picture(UiPicture),
    /// Картинка с GPU, которую рендер подставит сам; если на кадре её нет — элемент не рисуется.
    Live(LiveImage),
}

impl ImageSource {
    /// Цветовой договор источника.
    pub const fn color_space(&self) -> ImageColorSpace {
        match self {
            Self::Picture(_) => ImageColorSpace::SrgbPicture,
            Self::Live(_) => ImageColorSpace::DisplayEncoded,
        }
    }

    /// Картинка из памяти, если источник — она.
    pub const fn as_picture(&self) -> Option<&UiPicture> {
        match self {
            Self::Picture(picture) => Some(picture),
            Self::Live(_) => None,
        }
    }

    /// Какая живая картинка нужна, если источник живой.
    pub const fn live_image(&self) -> Option<LiveImage> {
        match self {
            Self::Live(live) => Some(*live),
            Self::Picture(_) => None,
        }
    }
}

impl From<UiPicture> for ImageSource {
    fn from(picture: UiPicture) -> Self {
        Self::Picture(picture)
    }
}

impl From<LiveImage> for ImageSource {
    fn from(live: LiveImage) -> Self {
        Self::Live(live)
    }
}

/// Почему пиксели живой картинки нельзя принять.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LiveImageTexelsError {
    /// Ширина или высота равна нулю.
    #[error("пустой размер живой картинки {size:?}")]
    EmptySize { size: PictureSize },
    /// Число пикселей не равно `ширина × высота`.
    #[error("в буфере живой картинки {actual} пикселей, а размеру {size:?} нужно {expected}")]
    LengthMismatch {
        size: PictureSize,
        expected: u64,
        actual: u64,
    },
}

/// Пиксели живой картинки на стороне CPU — ТОЛЬКО для эталона и тестов (боевой путь их не читает:
/// картинка живёт на GPU). Цвет экрана, прямая альфа, значения 0..1, построчно.
#[derive(Debug, Clone, Copy)]
pub struct LiveImageTexels<'a> {
    size: PictureSize,
    rgba: &'a [[f32; 4]],
}

impl<'a> LiveImageTexels<'a> {
    /// Собирает вид на пиксели; длина обязана быть `ширина × высота`.
    pub fn new(size: PictureSize, rgba: &'a [[f32; 4]]) -> Result<Self, LiveImageTexelsError> {
        if size.width == 0 || size.height == 0 {
            return Err(LiveImageTexelsError::EmptySize { size });
        }
        let expected = size.pixel_count();
        let actual = u64::try_from(rgba.len()).unwrap_or(u64::MAX);
        if expected != actual {
            return Err(LiveImageTexelsError::LengthMismatch { size, expected, actual });
        }
        Ok(Self { size, rgba })
    }

    pub const fn size(&self) -> PictureSize {
        self.size
    }

    /// Пиксель `(x, y)`; вне картинки — `None`.
    pub fn texel(&self, x: u32, y: u32) -> Option<[f32; 4]> {
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        let index = usize::try_from(u64::from(y) * u64::from(self.size.width) + u64::from(x)).ok()?;
        self.rgba.get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "тесты")]

    use super::*;
    use crate::picture::PictureEncoding;

    #[test]
    fn live_source_is_display_encoded_and_picture_is_srgb() {
        let picture = UiPicture::from_rgba8(PictureSize::new(1, 1), PictureEncoding::Srgb, vec![0; 4]).unwrap();
        let from_picture = ImageSource::from(picture.clone());
        let from_live = ImageSource::from(LiveImage::ColorScope);
        assert_eq!(from_picture.color_space(), ImageColorSpace::SrgbPicture);
        assert_eq!(from_live.color_space(), ImageColorSpace::DisplayEncoded);
        assert_eq!(from_picture.as_picture(), Some(&picture));
        assert_eq!(
            (from_picture.live_image(), from_live.live_image()),
            (None, Some(LiveImage::ColorScope))
        );
        assert_eq!(from_live.as_picture(), None);
        assert_ne!(
            ImageColorSpace::SrgbPicture.shader_code(),
            ImageColorSpace::DisplayEncoded.shader_code()
        );
    }

    #[test]
    fn texels_reject_wrong_length_and_read_the_right_cell() {
        let cells = [[0.0; 4], [0.1; 4], [0.2; 4], [0.3; 4], [0.4; 4], [0.5; 4]];
        let size = PictureSize::new(3, 2);
        let texels = LiveImageTexels::new(size, &cells).unwrap();
        assert_eq!(texels.texel(2, 1), Some([0.5; 4]));
        assert_eq!(texels.texel(0, 1), Some([0.3; 4]));
        assert_eq!(texels.texel(3, 0), None);
        assert_eq!(
            LiveImageTexels::new(size, &cells[..5]).unwrap_err(),
            LiveImageTexelsError::LengthMismatch {
                size,
                expected: 6,
                actual: 5
            }
        );
        assert_eq!(
            LiveImageTexels::new(PictureSize::new(0, 2), &[]).unwrap_err(),
            LiveImageTexelsError::EmptySize {
                size: PictureSize::new(0, 2)
            }
        );
    }
}
