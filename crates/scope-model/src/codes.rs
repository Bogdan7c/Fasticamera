//! Вход приборов: КОДЫ показанной картинки (10 бит на канал) и единственное правило квантования.
//!
//! Прибор меряет сигнал так, как его меряют видеоинструменты: целые коды выхода видоискателя
//! (кодировка экрана, OETF sRGB, основные цвета sRGB или Display P3). GPU читает ту же картинку и
//! округляет её по тому же правилу [`quantize_display_value`], поэтому коды, а значит и счётчики,
//! совпадают бит в бит.

/// Наибольший код канала: 10 бит.
pub const MAX_DISPLAY_CODE: u16 = 1023;

/// Число различных кодов канала (размер таблицы декодирования).
pub const DISPLAY_CODE_COUNT: usize = MAX_DISPLAY_CODE as usize + 1;

/// Наибольшее число пикселей картинки на входе приборов (8K — 33 млн). От него зависит граница
/// переполнения счётчиков: в одну ячейку за кадр попадает не больше одного отсчёта на отсчёт-источник,
/// значит счётчик `u32` не переполняется (`2^26 ≪ 2^32`).
pub const MAX_SHOWN_PIXELS: u64 = 1 << 26;

/// Наибольшая сторона картинки на входе. Вместе с наибольшим числом столбцов сетки (4096) даёт
/// `x · columns < 2^32`: GPU считает столбец в `uint` без переполнения.
pub const MAX_SHOWN_SIDE: u32 = 16_384;

/// Квантование значения экрана `0…1` в код `0…1023` — ЕДИНОЕ правило CPU и GPU.
///
/// `floor(clamp(v, 0, 1) · 1023 + 0,5)`; умножение и сложение — отдельные операции f32 (на GPU они
/// помечены `precise`, слияние в FMA запрещено), `floor` — точная операция. NaN даёт 0.
///
/// GLSL: `uint(floor(clamp(v, 0.0, 1.0) * 1023.0 + 0.5))` при `precise`.
pub fn quantize_display_value(value: f32) -> u16 {
    if value.is_nan() {
        return 0;
    }
    let scaled = value.clamp(0.0, 1.0) * 1023.0;
    let rounded = (scaled + 0.5).floor();
    // Значение уже в 0…1023, приведение не обрезает ничего.
    rounded as u16
}

/// Нужен ли разворот строк: картинка на экране зеркальна (фронтальная камера), приборы меряют
/// именно показанное.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalOrder {
    /// Столбцы как лежат в буфере.
    AsStored,
    /// Столбцы каждой строки в обратном порядке (картинка зеркальна).
    Mirrored,
}

/// Ошибка проверки картинки на входе приборов.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShownCodesError {
    /// Нулевая сторона.
    #[error("картинка приборов пуста: {width}×{height}")]
    Empty {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
    /// Сторона или число пикселей больше предела.
    #[error("картинка приборов слишком велика: {width}×{height}")]
    TooLarge {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
    /// Число пикселей не равно `ширина × высота`.
    #[error("в картинке {actual} пикселей, а по размеру {width}×{height} должно быть {expected}")]
    PixelCountMismatch {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
        /// Ожидаемое число пикселей.
        expected: usize,
        /// Фактическое число пикселей.
        actual: usize,
    },
    /// Код канала больше 1023.
    #[error("код {code} в пикселе {pixel_index} больше {MAX_DISPLAY_CODE}")]
    CodeOutOfRange {
        /// Индекс пикселя в порядке строк.
        pixel_index: usize,
        /// Недопустимый код.
        code: u16,
    },
}

/// Показанная картинка в кодах `0…1023`: построчно, сверху вниз, слева направо; уже зеркальная,
/// если на экране она зеркальна.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownCodes {
    width: u32,
    height: u32,
    pixels: Vec<[u16; 3]>,
}

impl ShownCodes {
    /// Проверенная картинка: размер в пределах, число пикселей сходится, все коды ≤ 1023.
    pub fn try_new(width: u32, height: u32, pixels: Vec<[u16; 3]>) -> Result<Self, ShownCodesError> {
        Self::check_size(width, height)?;
        let expected = width as usize * height as usize;
        if pixels.len() != expected {
            return Err(ShownCodesError::PixelCountMismatch {
                width,
                height,
                expected,
                actual: pixels.len(),
            });
        }
        for (pixel_index, pixel) in pixels.iter().enumerate() {
            if let Some(&code) = pixel.iter().find(|&&code| code > MAX_DISPLAY_CODE) {
                return Err(ShownCodesError::CodeOutOfRange { pixel_index, code });
            }
        }
        Ok(Self { width, height, pixels })
    }

    /// Картинка из значений экрана RGBA (чтение GPU-текстуры): альфа игнорируется, каждый канал
    /// проходит [`quantize_display_value`].
    pub fn from_display_rgba(
        width: u32,
        height: u32,
        values: &[[f32; 4]],
        order: HorizontalOrder,
    ) -> Result<Self, ShownCodesError> {
        let rgb: Vec<[f32; 3]> = values.iter().map(|value| [value[0], value[1], value[2]]).collect();
        Self::from_display_rgb(width, height, &rgb, order)
    }

    /// Картинка из значений экрана RGB: каждый канал проходит [`quantize_display_value`].
    pub fn from_display_rgb(
        width: u32,
        height: u32,
        values: &[[f32; 3]],
        order: HorizontalOrder,
    ) -> Result<Self, ShownCodesError> {
        Self::check_size(width, height)?;
        let expected = width as usize * height as usize;
        if values.len() != expected {
            return Err(ShownCodesError::PixelCountMismatch {
                width,
                height,
                expected,
                actual: values.len(),
            });
        }
        let quantize = |value: &[f32; 3]| value.map(quantize_display_value);
        let mut pixels = Vec::with_capacity(expected);
        for row in values.chunks_exact(width as usize) {
            match order {
                HorizontalOrder::AsStored => pixels.extend(row.iter().map(quantize)),
                HorizontalOrder::Mirrored => pixels.extend(row.iter().rev().map(quantize)),
            }
        }
        Ok(Self { width, height, pixels })
    }

    fn check_size(width: u32, height: u32) -> Result<(), ShownCodesError> {
        if width == 0 || height == 0 {
            return Err(ShownCodesError::Empty { width, height });
        }
        if width > MAX_SHOWN_SIDE || height > MAX_SHOWN_SIDE || u64::from(width) * u64::from(height) > MAX_SHOWN_PIXELS
        {
            return Err(ShownCodesError::TooLarge { width, height });
        }
        Ok(())
    }

    /// Ширина, пикселей.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Высота, пикселей.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Код пикселя, если он внутри картинки.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u16; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.pixels.get(y as usize * self.width as usize + x as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_rounds_half_up_and_clamps() {
        assert_eq!(quantize_display_value(0.0), 0);
        assert_eq!(quantize_display_value(1.0), 1023);
        assert_eq!(quantize_display_value(-3.0), 0);
        assert_eq!(quantize_display_value(7.0), 1023);
        assert_eq!(quantize_display_value(f32::NAN), 0);
        // 0,5 · 1023 = 511,5, плюс 0,5 даёт ровно 512: половина округляется вверх.
        assert_eq!(quantize_display_value(0.5), 512);
        assert_eq!(quantize_display_value(0.49 / 1023.0), 0);
        assert_eq!(quantize_display_value(0.51 / 1023.0), 1);
        assert_eq!(quantize_display_value(512.0 / 1023.0), 512);
    }

    #[test]
    fn mirrored_order_reverses_each_row_only() {
        let values = [[0.0; 3], [1.0; 3], [0.0; 3], [1.0; 3], [0.0; 3], [0.0; 3]];
        let stored = ShownCodes::from_display_rgb(3, 2, &values, HorizontalOrder::AsStored).unwrap();
        let mirrored = ShownCodes::from_display_rgb(3, 2, &values, HorizontalOrder::Mirrored).unwrap();
        assert_eq!(stored.pixel(1, 0), Some([1023; 3]));
        assert_eq!(mirrored.pixel(1, 0), Some([1023; 3]));
        assert_eq!(stored.pixel(0, 1), Some([1023; 3]));
        assert_eq!(mirrored.pixel(2, 1), Some([1023; 3]));
        assert_eq!(mirrored.pixel(0, 1), Some([0; 3]));
    }

    #[test]
    fn rgba_alpha_is_ignored() {
        let shown = ShownCodes::from_display_rgba(1, 1, &[[1.0, 0.0, 0.5, 0.0]], HorizontalOrder::AsStored).unwrap();
        assert_eq!(shown.pixel(0, 0), Some([1023, 0, 512]));
    }

    #[test]
    fn invalid_pictures_are_rejected_with_distinct_errors() {
        assert!(matches!(
            ShownCodes::try_new(0, 1, vec![]),
            Err(ShownCodesError::Empty { .. })
        ));
        assert!(matches!(
            ShownCodes::try_new(2, 1, vec![[0; 3]]),
            Err(ShownCodesError::PixelCountMismatch {
                expected: 2,
                actual: 1,
                ..
            })
        ));
        assert_eq!(
            ShownCodes::try_new(1, 1, vec![[0, 1024, 0]]),
            Err(ShownCodesError::CodeOutOfRange {
                pixel_index: 0,
                code: 1024
            })
        );
        assert!(matches!(
            ShownCodes::try_new(MAX_SHOWN_SIDE + 1, 1, vec![]),
            Err(ShownCodesError::TooLarge { .. })
        ));
    }

    #[test]
    fn pixel_outside_is_none() {
        let shown = ShownCodes::try_new(2, 1, vec![[1, 2, 3], [4, 5, 6]]).unwrap();
        assert_eq!(shown.pixel(1, 0), Some([4, 5, 6]));
        assert_eq!(shown.pixel(2, 0), None);
        assert_eq!(shown.pixel(0, 1), None);
    }
}
