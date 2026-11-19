//! Цвет и прозрачность списка рисования.
//!
//! Цвета UI лежат в ЗАКОДИРОВАННЫХ значениях экрана — тех же, что ISP пишет в кадр видоискателя
//! (кривая sRGB, первичные — дело экрана, Display P3 или sRGB). Смешивание, насыщенность и яркость
//! считаются прямо по ним, без линеаризации: именно так смешивает интерфейс iOS (решение главного
//! агента сессии 11; проверять глазами на 11a). Линейный свет здесь нигде не нужен.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Прозрачность вне диапазона 0..1 или не число.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("прозрачность должна быть числом от 0 до 1, получено {0}")]
pub struct OpacityError(pub f32);

/// Прозрачность элемента: 0 — не виден, 1 — полностью непрозрачен. Диапазон проверяется при создании.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct Opacity(f32);

impl Opacity {
    pub const OPAQUE: Opacity = Opacity(1.0);
    pub const TRANSPARENT: Opacity = Opacity(0.0);

    /// Создать; NaN и значения вне 0..1 — ошибка (молча не зажимаем).
    pub fn new(value: f32) -> Result<Self, OpacityError> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(OpacityError(value))
        }
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

impl TryFrom<f32> for Opacity {
    type Error = OpacityError;
    fn try_from(value: f32) -> Result<Self, OpacityError> {
        Self::new(value)
    }
}

impl From<Opacity> for f32 {
    fn from(opacity: Opacity) -> f32 {
        opacity.0
    }
}

/// Цвет с ПРЯМОЙ (не предумноженной) альфой; компоненты 0..1 в закодированных значениях экрана.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DisplayColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// Цвет с ПРЕДУМНОЖЕННОЙ альфой: `r,g,b` уже умножены на `a`. Так результат стекла уходит в кадр.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PremultipliedColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl DisplayColor {
    pub const TRANSPARENT: DisplayColor = DisplayColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: DisplayColor = DisplayColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: DisplayColor = DisplayColor {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Непрозрачный цвет.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Тот же цвет с другой альфой.
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Предумножить альфу.
    pub fn premultiplied(self) -> PremultipliedColor {
        PremultipliedColor {
            r: self.r * self.a,
            g: self.g * self.a,
            b: self.b * self.a,
            a: self.a,
        }
    }

    /// Покомпонентная линейная смесь `self·(1−t) + other·t` (включая альфу).
    pub fn lerp(self, other: DisplayColor, t: f32) -> DisplayColor {
        DisplayColor {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Все компоненты конечны и лежат в 0..1.
    pub fn is_valid(self) -> bool {
        [self.r, self.g, self.b, self.a]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
}

impl PremultipliedColor {
    pub const TRANSPARENT: PremultipliedColor = PremultipliedColor {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
}

/// Яркость по весам Rec.709 (`0,2126 R + 0,7152 G + 0,0722 B`), считаемая по ЗАКОДИРОВАННЫМ значениям.
///
/// Это не «физическая» яркость: настоящая считается по линейному свету. Для интерфейса нужна
/// «воспринимаемая светлота» в той же шкале, в которой UI смешивает цвета (так же поступает iOS),
/// поэтому формула применяется прямо к закодированным значениям. Альфа не учитывается.
pub fn luma_rec709(encoded: DisplayColor) -> f32 {
    0.2126 * encoded.r + 0.7152 * encoded.g + 0.0722 * encoded.b
}
