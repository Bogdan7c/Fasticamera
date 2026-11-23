//! Размер картинки прибора на экране, в пикселях.

use serde::{Deserialize, Serialize};

/// Наибольшая сторона картинки прибора, пикселей. Это предел словаря (картинка прибора — часть
/// экрана телефона, 1080×2412), а не настройка бюджета: бюджет задаёт конфиг рендера.
pub const MAX_SCOPE_IMAGE_SIDE_PX: u32 = 4096;

/// Размер картинки прибора в пикселях экрана. Сетки счётчиков от него не зависят (решение 185):
/// размер меняется при повороте раздела, сетки — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawScopeImageSize", into = "RawScopeImageSize")]
pub struct ScopeImageSize {
    width: u32,
    height: u32,
}

/// Ошибка размера картинки прибора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScopeImageSizeError {
    /// Нулевая сторона.
    #[error("сторона картинки прибора равна нулю: {width}×{height}")]
    Empty {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
    /// Сторона больше предела словаря.
    #[error("сторона картинки прибора больше {max} пикселей: {width}×{height}")]
    TooLarge {
        /// Предел.
        max: u32,
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
}

impl ScopeImageSize {
    /// Проверенный размер: обе стороны в `1..=MAX_SCOPE_IMAGE_SIDE_PX`.
    pub fn try_new(width: u32, height: u32) -> Result<Self, ScopeImageSizeError> {
        if width == 0 || height == 0 {
            return Err(ScopeImageSizeError::Empty { width, height });
        }
        if width > MAX_SCOPE_IMAGE_SIDE_PX || height > MAX_SCOPE_IMAGE_SIDE_PX {
            return Err(ScopeImageSizeError::TooLarge {
                max: MAX_SCOPE_IMAGE_SIDE_PX,
                width,
                height,
            });
        }
        Ok(Self { width, height })
    }

    /// Ширина, пикселей.
    pub fn width(self) -> u32 {
        self.width
    }

    /// Высота, пикселей.
    pub fn height(self) -> u32 {
        self.height
    }
}

/// Сырая форма для serde: проверка — та же [`ScopeImageSize::try_new`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScopeImageSize {
    width: u32,
    height: u32,
}

impl TryFrom<RawScopeImageSize> for ScopeImageSize {
    type Error = ScopeImageSizeError;

    fn try_from(raw: RawScopeImageSize) -> Result<Self, Self::Error> {
        Self::try_new(raw.width, raw.height)
    }
}

impl From<ScopeImageSize> for RawScopeImageSize {
    fn from(size: ScopeImageSize) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_inside_the_limit_are_kept_exactly() {
        let size = ScopeImageSize::try_new(512, 288).unwrap();
        assert_eq!((size.width(), size.height()), (512, 288));
        let largest = ScopeImageSize::try_new(MAX_SCOPE_IMAGE_SIDE_PX, 1).unwrap();
        assert_eq!(largest.width(), MAX_SCOPE_IMAGE_SIDE_PX);
    }

    #[test]
    fn zero_side_is_rejected_as_empty() {
        assert_eq!(
            ScopeImageSize::try_new(0, 10),
            Err(ScopeImageSizeError::Empty { width: 0, height: 10 })
        );
        assert_eq!(
            ScopeImageSize::try_new(10, 0),
            Err(ScopeImageSizeError::Empty { width: 10, height: 0 })
        );
    }

    #[test]
    fn side_over_the_limit_is_rejected_as_too_large() {
        assert_eq!(
            ScopeImageSize::try_new(MAX_SCOPE_IMAGE_SIDE_PX + 1, 10),
            Err(ScopeImageSizeError::TooLarge {
                max: MAX_SCOPE_IMAGE_SIDE_PX,
                width: MAX_SCOPE_IMAGE_SIDE_PX + 1,
                height: 10
            })
        );
    }

    #[test]
    fn deserialization_goes_through_the_same_check() {
        let parsed: ScopeImageSize = serde_json::from_str(r#"{"width":300,"height":200}"#).unwrap();
        assert_eq!((parsed.width(), parsed.height()), (300, 200));
        assert!(serde_json::from_str::<ScopeImageSize>(r#"{"width":0,"height":200}"#).is_err());
        let round_trip = serde_json::to_string(&parsed).unwrap();
        assert_eq!(round_trip, r#"{"width":300,"height":200}"#);
    }
}
